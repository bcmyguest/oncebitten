//! Measures intent classes on the labelled fixtures under `tests/fixtures/intent/`.
//!
//! Usage: `cargo run --release --features intent --example intent_eval -- <f32_model_dir> <int8_model_dir> [--json <path>]`
//!
//! Two classifiers per embedding model:
//! - `nn`: nearest shipped example, neutral below a threshold.
//! - `knn`: k nearest neighbours over shipped examples plus the `dev` rows (neutral included),
//!   dev scored leave-one-out.
//!
//! Every threshold and `k` is chosen on `dev` and reported on `test`. Two thresholds per run:
//! max dev macro-F1, and the lowest whose dev neutral false-positive rate is at most
//! `HOOK_MAX_NEUTRAL_FPR`. The regex lexicon has no threshold and reports both splits.
//!
//! `--json <path>` also writes every variant (regex; f32 and int8-file x nn and every kNN `k` x
//! three thresholds: max-F1, hook, and `capped-F1`, the dev max-F1 under the
//! FPR cap) with per-split metrics, confusion matrix, per-query latency (embedding plus
//! classification, `LATENCY_PASSES` timed passes over all rows after one warm-up pass), and
//! model size on disk. The text output stays the same.

use std::collections::BTreeMap;
use std::fs;
use std::hint::black_box;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{bail, Context, Result};
use clap::Parser;
use model2vec_rs::model::StaticModel;
use oncebitten::intent::{
    Bank, Embed, CORRECTION, FRUSTRATION, NEUTRAL, RULE_REQUEST, SHIPPED_EXAMPLES,
};
use oncebitten::signal::{matched_phrases, DEFAULT_CORRECTION_LEXICON};
use serde_json::{json, Value};

const CLASSES: [&str; 4] = [RULE_REQUEST, CORRECTION, FRUSTRATION, NEUTRAL];
const FIXTURES: [&str; 2] = [
    "tests/fixtures/intent/labelled.tsv",
    "tests/fixtures/intent/labelled-extra.tsv",
];
const HOOK_MAX_NEUTRAL_FPR: f32 = 0.05;
const KNN_K: [usize; 5] = [1, 3, 5, 7, 9];
const LATENCY_PASSES: usize = 5;

/// Predicted class and confidence for one fixture row.
type Scored = (&'static str, f32);

/// A kNN prototype: source row index (`None` for a shipped example), class, vector.
type Prototype<'a> = (Option<usize>, &'static str, &'a [f32]);

#[derive(Parser)]
struct Args {
    f32_dir: PathBuf,
    int8_dir: PathBuf,
    /// Write machine-readable results for every variant to this path.
    #[arg(long)]
    json: Option<PathBuf>,
}

/// Fixed facts about one evaluated variant, before thresholds.
struct Variant<'a> {
    quant: Option<&'a str>,
    model_dir: Option<&'a Path>,
    classifier: &'a str,
    k: Option<usize>,
    k_selected_on_dev: bool,
    latency_us: Vec<f64>,
}

struct Row {
    split: String,
    label: &'static str,
    text: String,
}

fn class_named(name: &str) -> Result<&'static str> {
    match CLASSES.iter().copied().find(|class| *class == name) {
        Some(class) => Ok(class),
        None => bail!("unknown class `{name}` in fixtures"),
    }
}

fn read_rows(path: &Path) -> Result<Vec<Row>> {
    let source = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let mut rows = Vec::new();
    for line in source
        .lines()
        .filter(|l| !l.starts_with('#') && !l.is_empty())
    {
        let [split, label, text] = line.splitn(3, '\t').collect::<Vec<_>>()[..] else {
            bail!("malformed fixture row: {line}");
        };
        rows.push(Row {
            split: split.to_string(),
            label: class_named(label)?,
            text: text.to_string(),
        });
    }
    Ok(rows)
}

#[derive(Default, Clone, Copy)]
struct Counts {
    tp: f32,
    fp: f32,
    fn_: f32,
}

fn tally(gold: &[&'static str], pred: &[&'static str]) -> BTreeMap<&'static str, Counts> {
    let mut table: BTreeMap<&'static str, Counts> =
        CLASSES.iter().map(|c| (*c, Counts::default())).collect();
    for (g, p) in gold.iter().zip(pred) {
        if g == p {
            table.entry(g).or_default().tp += 1.0;
        } else {
            table.entry(p).or_default().fp += 1.0;
            table.entry(g).or_default().fn_ += 1.0;
        }
    }
    table
}

fn precision_recall_f1(c: Counts) -> (f32, f32, f32) {
    let p = if c.tp + c.fp > 0.0 {
        c.tp / (c.tp + c.fp)
    } else {
        0.0
    };
    let r = if c.tp + c.fn_ > 0.0 {
        c.tp / (c.tp + c.fn_)
    } else {
        0.0
    };
    let f1 = if p + r > 0.0 {
        2.0 * p * r / (p + r)
    } else {
        0.0
    };
    (p, r, f1)
}

fn macro_f1(gold: &[&'static str], pred: &[&'static str]) -> f32 {
    let table = tally(gold, pred);
    let sum: f32 = CLASSES
        .iter()
        .map(|c| precision_recall_f1(table[c]).2)
        .sum();
    sum / CLASSES.len() as f32
}

fn accuracy(gold: &[&str], pred: &[&str]) -> f32 {
    let hits = gold.iter().zip(pred).filter(|(g, p)| g == p).count();
    hits as f32 / gold.len().max(1) as f32
}

/// Share of gold-neutral rows given a non-neutral class.
fn neutral_fpr(gold: &[&str], pred: &[&str]) -> f32 {
    let neutral: Vec<_> = gold
        .iter()
        .zip(pred)
        .filter(|(g, _)| **g == NEUTRAL)
        .collect();
    let flagged = neutral.iter().filter(|(_, p)| **p != NEUTRAL).count();
    flagged as f32 / neutral.len().max(1) as f32
}

fn print_report(name: &str, split: &str, gold: &[&'static str], pred: &[&'static str]) {
    println!(
        "{name} {split}: accuracy {:.3} macro-F1 {:.3} neutral-FPR {:.3}",
        accuracy(gold, pred),
        macro_f1(gold, pred),
        neutral_fpr(gold, pred)
    );
    let table = tally(gold, pred);
    for class in CLASSES {
        let (p, r, f1) = precision_recall_f1(table[class]);
        println!("  {class:<13} precision {p:.3} recall {r:.3} f1 {f1:.3}");
    }
}

/// Class of each row: the scored class when its score reaches `threshold`, else neutral.
fn predict(scored: &[(&'static str, f32)], threshold: f32) -> Vec<&'static str> {
    scored
        .iter()
        .map(|(class, score)| if *score >= threshold { *class } else { NEUTRAL })
        .collect()
}

fn thresholds() -> impl Iterator<Item = f32> {
    (0..=100).map(|step| step as f32 / 100.0)
}

/// Threshold with the highest macro-F1 and that F1.
fn best_threshold(scored: &[(&'static str, f32)], gold: &[&'static str]) -> (f32, f32) {
    let mut best = (f32::MIN, 0.0_f32);
    for threshold in thresholds() {
        let f1 = macro_f1(gold, &predict(scored, threshold));
        if f1 > best.0 {
            best = (f1, threshold);
        }
    }
    (best.1, best.0)
}

/// Lowest threshold whose neutral false-positive rate is at most `HOOK_MAX_NEUTRAL_FPR`.
fn hook_threshold(scored: &[(&'static str, f32)], gold: &[&'static str]) -> f32 {
    thresholds()
        .find(|t| neutral_fpr(gold, &predict(scored, *t)) <= HOOK_MAX_NEUTRAL_FPR)
        .unwrap_or(1.0)
}

/// The max-F1 and the hook threshold, both chosen on dev.
fn dev_thresholds(rows: &[Row], scored: &[Scored]) -> [(&'static str, f32); 2] {
    let (dev_scored, dev_gold) = select(rows, scored, "dev");
    let (f1_threshold, _) = best_threshold(&dev_scored, &dev_gold);
    let hook = hook_threshold(&dev_scored, &dev_gold);
    [("max-F1", f1_threshold), ("hook", hook)]
}

/// Threshold with the highest macro-F1 among those whose neutral false-positive rate is at
/// most `HOOK_MAX_NEUTRAL_FPR`.
fn capped_threshold(scored: &[Scored], gold: &[&'static str]) -> f32 {
    let mut best = (f32::MIN, 1.0_f32);
    for threshold in thresholds() {
        let pred = predict(scored, threshold);
        if neutral_fpr(gold, &pred) <= HOOK_MAX_NEUTRAL_FPR {
            let f1 = macro_f1(gold, &pred);
            if f1 > best.0 {
                best = (f1, threshold);
            }
        }
    }
    best.1
}

/// The `dev_thresholds` plus `capped-F1`, the dev max-F1 under the neutral FPR cap.
fn json_thresholds(rows: &[Row], scored: &[Scored]) -> [(&'static str, f32); 3] {
    let [max_f1, hook] = dev_thresholds(rows, scored);
    let (dev_scored, dev_gold) = select(rows, scored, "dev");
    [
        max_f1,
        hook,
        ("capped-F1", capped_threshold(&dev_scored, &dev_gold)),
    ]
}

/// Two-decimal value without f32-to-f64 noise in the JSON.
fn rounded(value: f32) -> f64 {
    (f64::from(value) * 100.0).round() / 100.0
}

/// Reports `scored` at the max-F1 and the hook threshold, both chosen on dev.
fn report_thresholds(name: &str, rows: &[Row], scored: &[(&'static str, f32)]) {
    let (dev_scored, dev_gold) = select(rows, scored, "dev");
    let (test_scored, test_gold) = select(rows, scored, "test");
    for (label, threshold) in dev_thresholds(rows, scored) {
        let run = format!("{name} {label} t={threshold:.2}");
        print_report(&run, "dev", &dev_gold, &predict(&dev_scored, threshold));
        print_report(&run, "test", &test_gold, &predict(&test_scored, threshold));
    }
}

/// Best-matching example class and cosine score for every row.
fn score_rows(
    model: &StaticModel,
    bank: &Bank,
    rows: &[Row],
) -> (Vec<Vec<f32>>, Vec<(&'static str, f32)>) {
    let mut queries = Vec::with_capacity(rows.len());
    let mut scored = Vec::with_capacity(rows.len());
    for row in rows {
        let query = model.embed(&row.text);
        scored.push(match bank.best_match(&query) {
            Some(found) => (bank.examples()[found.example].class, found.score),
            None => (NEUTRAL, 0.0),
        });
        queries.push(query);
    }
    (queries, scored)
}

fn select<T: Copy>(rows: &[Row], items: &[T], split: &str) -> (Vec<T>, Vec<&'static str>) {
    let mut picked = Vec::new();
    let mut gold = Vec::new();
    for (row, item) in rows.iter().zip(items) {
        if row.split == split {
            picked.push(*item);
            gold.push(row.label);
        }
    }
    (picked, gold)
}

/// kNN class and confidence for every row. Prototypes are the shipped examples plus the dev
/// rows; a dev row never votes for itself. Votes sum cosine scores per class; confidence is
/// the winning class's best neighbour score.
fn knn_scores(
    bank: &Bank,
    rows: &[Row],
    queries: &[Vec<f32>],
    k: usize,
) -> Vec<(&'static str, f32)> {
    let prototypes = knn_prototypes(bank, rows, queries);
    queries
        .iter()
        .enumerate()
        .map(|(index, query)| knn_vote(&prototypes, query, Some(index), k))
        .collect()
}

/// Shipped examples plus the dev rows.
fn knn_prototypes<'a>(bank: &'a Bank, rows: &[Row], queries: &'a [Vec<f32>]) -> Vec<Prototype<'a>> {
    let mut prototypes: Vec<Prototype<'a>> = bank
        .examples()
        .iter()
        .map(|e| (None, e.class, e.vector.as_slice()))
        .collect();
    for (index, (row, query)) in rows.iter().zip(queries).enumerate() {
        if row.split == "dev" {
            prototypes.push((Some(index), row.label, query.as_slice()));
        }
    }
    prototypes
}

/// kNN class and confidence for one query, skipping the prototype from row `skip`.
fn knn_vote(prototypes: &[Prototype], query: &[f32], skip: Option<usize>, k: usize) -> Scored {
    let mut near: Vec<(&'static str, f32)> = prototypes
        .iter()
        .filter(|(row, _, _)| skip.is_none() || *row != skip)
        .map(|(_, class, vector)| (*class, cosine(query, vector)))
        .collect();
    near.sort_by(|a, b| b.1.total_cmp(&a.1));
    near.truncate(k);
    let mut votes: BTreeMap<&'static str, (f32, f32)> = BTreeMap::new();
    for (class, score) in near {
        let entry = votes.entry(class).or_insert((0.0, f32::MIN));
        entry.0 += score;
        entry.1 = entry.1.max(score);
    }
    votes
        .into_iter()
        .max_by(|a, b| a.1 .0.total_cmp(&b.1 .0))
        .map_or((NEUTRAL, 0.0), |(class, (_, best))| (class, best))
}

/// Microseconds per call of `classify` on each row text: one warm-up pass, then
/// `LATENCY_PASSES` timed passes.
fn time_per_row<T>(rows: &[Row], mut classify: impl FnMut(&str) -> T) -> Vec<f64> {
    for row in rows {
        black_box(classify(black_box(&row.text)));
    }
    let mut micros = Vec::with_capacity(rows.len() * LATENCY_PASSES);
    for _ in 0..LATENCY_PASSES {
        for row in rows {
            let start = Instant::now();
            black_box(classify(black_box(&row.text)));
            micros.push(start.elapsed().as_secs_f64() * 1e6);
        }
    }
    micros
}

/// Nearest-rank percentile of `values`, `q` in 0..=100.
fn percentile(values: &[f64], q: f64) -> f64 {
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    if sorted.is_empty() {
        return 0.0;
    }
    let rank = ((q / 100.0) * sorted.len() as f64).ceil() as usize;
    sorted[rank.clamp(1, sorted.len()) - 1]
}

/// Bytes of `model.safetensors` and of all files directly in `dir`.
fn model_bytes(dir: &Path) -> Result<(u64, u64)> {
    let mut weights = 0;
    let mut total = 0;
    for entry in fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
        let entry = entry?;
        let meta = entry.metadata()?;
        if meta.is_file() {
            total += meta.len();
            if entry.file_name() == "model.safetensors" {
                weights = meta.len();
            }
        }
    }
    Ok((weights, total))
}

/// Metrics and confusion matrix (rows gold, columns predicted, both in `CLASSES` order).
fn split_json(gold: &[&'static str], pred: &[&'static str]) -> Value {
    let table = tally(gold, pred);
    let per_class: serde_json::Map<String, Value> = CLASSES
        .iter()
        .map(|class| {
            let (p, r, f1) = precision_recall_f1(table[class]);
            let support = gold.iter().filter(|g| *g == class).count();
            let value = json!({"precision": p, "recall": r, "f1": f1, "support": support});
            (class.to_string(), value)
        })
        .collect();
    let index = |class: &str| CLASSES.iter().position(|c| *c == class).unwrap_or(0);
    let mut matrix = [[0_usize; CLASSES.len()]; CLASSES.len()];
    for (g, p) in gold.iter().zip(pred) {
        matrix[index(g)][index(p)] += 1;
    }
    json!({
        "rows": gold.len(),
        "accuracy": accuracy(gold, pred),
        "macro_f1": macro_f1(gold, pred),
        "neutral_fpr": neutral_fpr(gold, pred),
        "per_class": per_class,
        "confusion": {"labels": CLASSES, "matrix": matrix},
    })
}

/// One JSON record per threshold for `variant`; `thresholds` empty means no threshold.
fn variant_records(
    variant: &Variant,
    rows: &[Row],
    scored: &[Scored],
    thresholds: &[(&'static str, f32)],
) -> Result<Vec<Value>> {
    let (weights, dir_total) = match variant.model_dir {
        Some(dir) => model_bytes(dir)?,
        None => (0, 0),
    };
    let modes: Vec<(&str, Option<f32>)> = if thresholds.is_empty() {
        vec![("none", None)]
    } else {
        thresholds.iter().map(|(m, t)| (*m, Some(*t))).collect()
    };
    let mut records = Vec::new();
    for (mode, threshold) in modes {
        let pred = match threshold {
            Some(t) => predict(scored, t),
            None => scored.iter().map(|(class, _)| *class).collect(),
        };
        let mut splits = serde_json::Map::new();
        for split in ["dev", "test"] {
            let (p, g) = select(rows, &pred, split);
            splits.insert(split.to_string(), split_json(&g, &p));
        }
        let k_part = variant.k.map_or(String::new(), |k| format!(" k={k}"));
        let name = format!(
            "{} {}{k_part} {mode}",
            variant.quant.unwrap_or("-"),
            variant.classifier
        );
        records.push(json!({
            "name": name,
            "quant": variant.quant,
            "model_dir": variant.model_dir.map(|d| d.display().to_string()),
            "classifier": variant.classifier,
            "k": variant.k,
            "k_selected_on_dev": variant.k_selected_on_dev,
            "threshold_mode": mode,
            "threshold": threshold.map(rounded),
            "model_safetensors_bytes": weights,
            "model_dir_bytes": dir_total,
            "latency_us": {
                "samples": variant.latency_us.len(),
                "p50": percentile(&variant.latency_us, 50.0),
                "p95": percentile(&variant.latency_us, 95.0),
                "mean": variant.latency_us.iter().sum::<f64>()
                    / variant.latency_us.len().max(1) as f64,
            },
            "splits": splits,
        }));
    }
    Ok(records)
}

/// Prints the report for `model` and, when `quant` is set, appends its JSON records to `out`.
fn evaluate(
    name: &str,
    quant: Option<&str>,
    model_dir: &Path,
    model: &StaticModel,
    rows: &[Row],
    out: &mut Vec<Value>,
) -> Result<Vec<Vec<f32>>> {
    let bank = Bank::from_toml(SHIPPED_EXAMPLES, model)?;
    let (queries, scored) = score_rows(model, &bank, rows);
    report_thresholds(&format!("{name} nn"), rows, &scored);

    let mut knn_runs = Vec::new();
    let mut best: Option<(f32, usize)> = None;
    for k in KNN_K {
        let knn = knn_scores(&bank, rows, &queries, k);
        let (dev_scored, dev_gold) = select(rows, &knn, "dev");
        let (_, f1) = best_threshold(&dev_scored, &dev_gold);
        println!("{name} knn k={k} dev max macro-F1 {f1:.3}");
        if best.is_none_or(|(top, _)| f1 > top) {
            best = Some((f1, k));
        }
        knn_runs.push((k, knn));
    }
    let best_k = best.map(|(_, k)| k);
    if let Some((k, knn)) = knn_runs.iter().find(|(k, _)| Some(*k) == best_k) {
        report_thresholds(&format!("{name} knn k={k}"), rows, knn);
    }

    if quant.is_some() {
        let nn = Variant {
            quant,
            model_dir: Some(model_dir),
            classifier: "nn",
            k: None,
            k_selected_on_dev: false,
            latency_us: time_per_row(rows, |text| bank.best_match(&model.embed(text))),
        };
        out.extend(variant_records(
            &nn,
            rows,
            &scored,
            &json_thresholds(rows, &scored),
        )?);
        let prototypes = knn_prototypes(&bank, rows, &queries);
        for (k, knn) in &knn_runs {
            let variant = Variant {
                quant,
                model_dir: Some(model_dir),
                classifier: "knn",
                k: Some(*k),
                k_selected_on_dev: Some(*k) == best_k,
                latency_us: time_per_row(rows, |text| {
                    knn_vote(&prototypes, &model.embed(text), None, *k)
                }),
            };
            out.extend(variant_records(
                &variant,
                rows,
                knn,
                &json_thresholds(rows, knn),
            )?);
        }
    }
    Ok(queries)
}

fn regex_predictions(rows: &[Row]) -> Vec<&'static str> {
    rows.iter()
        .map(|row| {
            if matched_phrases(&row.text, DEFAULT_CORRECTION_LEXICON).is_empty() {
                NEUTRAL
            } else {
                CORRECTION
            }
        })
        .collect()
}

fn mean_cosine(a: &[Vec<f32>], b: &[Vec<f32>]) -> f32 {
    let total: f32 = a.iter().zip(b).map(|(x, y)| cosine(x, y)).sum();
    total / a.len().max(1) as f32
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if na == 0.0 || nb == 0.0 {
        0.0
    } else {
        dot / (na * nb)
    }
}

fn main() -> Result<()> {
    let args = Args::parse();
    let (f32_dir, int8_dir) = (args.f32_dir.as_path(), args.int8_dir.as_path());
    let mut rows = Vec::new();
    let mut fixtures = Vec::new();
    for fixture in FIXTURES {
        let read = read_rows(Path::new(fixture))?;
        fixtures.push(json!({"path": fixture, "rows": read.len()}));
        rows.extend(read);
    }
    println!(
        "rows: {} dev, {} test",
        rows.iter().filter(|r| r.split == "dev").count(),
        rows.iter().filter(|r| r.split == "test").count()
    );

    let f32_model = StaticModel::from_pretrained(f32_dir, None, None, None)?;
    let int8_model = StaticModel::from_pretrained(int8_dir, None, None, None)?;
    let memory_int8 = StaticModel::from_pretrained(f32_dir, None, None, None)?.quantize_int8();

    let mut variants = Vec::new();
    let f32_queries = evaluate(
        "f32",
        Some("f32"),
        f32_dir,
        &f32_model,
        &rows,
        &mut variants,
    )?;
    let int8_queries = evaluate(
        "int8-file",
        Some("int8"),
        int8_dir,
        &int8_model,
        &rows,
        &mut variants,
    )?;
    let memory_queries = evaluate(
        "int8-memory",
        None,
        f32_dir,
        &memory_int8,
        &rows,
        &mut variants,
    )?;
    let memory_cosine = mean_cosine(&int8_queries, &memory_queries);
    let f32_cosine = mean_cosine(&f32_queries, &int8_queries);
    println!("mean cosine int8-file vs int8-memory query vectors: {memory_cosine:.4}");
    println!("mean cosine f32 vs int8-file query vectors: {f32_cosine:.4}");

    let regex = regex_predictions(&rows);
    for split in ["dev", "test"] {
        let (pred, g) = select(&rows, &regex, split);
        print_report("regex", split, &g, &pred);
    }

    if let Some(path) = args.json {
        let regex_scored: Vec<Scored> = regex.iter().map(|class| (*class, 1.0)).collect();
        let lexicon = Variant {
            quant: None,
            model_dir: None,
            classifier: "regex",
            k: None,
            k_selected_on_dev: false,
            latency_us: time_per_row(&rows, |text| {
                matched_phrases(text, DEFAULT_CORRECTION_LEXICON)
            }),
        };
        variants.extend(variant_records(&lexicon, &rows, &regex_scored, &[])?);
        let mut counts = serde_json::Map::new();
        for split in ["dev", "test"] {
            let per_class: serde_json::Map<String, Value> = CLASSES
                .iter()
                .map(|class| {
                    let n = rows
                        .iter()
                        .filter(|r| r.split == split && r.label == *class)
                        .count();
                    (class.to_string(), json!(n))
                })
                .collect();
            counts.insert(split.to_string(), Value::Object(per_class));
        }
        let report = json!({
            "fixtures": fixtures,
            "rows": counts,
            "hook_max_neutral_fpr": rounded(HOOK_MAX_NEUTRAL_FPR),
            "knn_k": KNN_K,
            "latency_passes": LATENCY_PASSES,
            "mean_cosine_int8_file_vs_memory": memory_cosine,
            "mean_cosine_f32_vs_int8_file": f32_cosine,
            "variants": variants,
        });
        fs::write(&path, serde_json::to_string_pretty(&report)? + "\n")
            .with_context(|| format!("writing {}", path.display()))?;
    }
    Ok(())
}
