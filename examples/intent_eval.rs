//! Measures intent classes on the labelled fixtures under `tests/fixtures/intent/`.
//!
//! Usage: `cargo run --release --example intent_eval -- <f32_model_dir> <int8_model_dir>`
//!
//! Two classifiers per embedding model:
//! - `nn`: nearest shipped example, neutral below a threshold.
//! - `knn`: k nearest neighbours over shipped examples plus the `dev` rows (neutral included),
//!   dev scored leave-one-out.
//!
//! Every threshold and `k` is chosen on `dev` and reported on `test`. Two thresholds per run:
//! max dev macro-F1, and the lowest whose dev neutral false-positive rate is at most
//! `HOOK_MAX_NEUTRAL_FPR`. The regex lexicon has no threshold and reports both splits.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use anyhow::{bail, Context, Result};
use model2vec_rs::model::StaticModel;
use oncebitten::intent::{
    Bank, Embed, CORRECTION, FRUSTRATION, NEUTRAL, RULE_REQUEST, SHIPPED_EXAMPLES,
};
use oncebitten::signal::{matched_phrases, DEFAULT_CORRECTION_LEXICON};

const CLASSES: [&str; 4] = [RULE_REQUEST, CORRECTION, FRUSTRATION, NEUTRAL];
const FIXTURES: [&str; 2] = [
    "tests/fixtures/intent/labelled.tsv",
    "tests/fixtures/intent/labelled-extra.tsv",
];
const HOOK_MAX_NEUTRAL_FPR: f32 = 0.05;
const KNN_K: [usize; 5] = [1, 3, 5, 7, 9];

/// Predicted class and confidence for one fixture row.
type Scored = (&'static str, f32);

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

/// Reports `scored` at the max-F1 and the hook threshold, both chosen on dev.
fn report_thresholds(name: &str, rows: &[Row], scored: &[(&'static str, f32)]) {
    let (dev_scored, dev_gold) = select(rows, scored, "dev");
    let (test_scored, test_gold) = select(rows, scored, "test");
    let (f1_threshold, _) = best_threshold(&dev_scored, &dev_gold);
    let hook = hook_threshold(&dev_scored, &dev_gold);
    for (label, threshold) in [("max-F1", f1_threshold), ("hook", hook)] {
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
    let mut prototypes: Vec<(Option<usize>, &'static str, &[f32])> = bank
        .examples()
        .iter()
        .map(|e| (None, e.class, e.vector.as_slice()))
        .collect();
    for (index, (row, query)) in rows.iter().zip(queries).enumerate() {
        if row.split == "dev" {
            prototypes.push((Some(index), row.label, query.as_slice()));
        }
    }
    queries
        .iter()
        .enumerate()
        .map(|(index, query)| {
            let mut near: Vec<(&'static str, f32)> = prototypes
                .iter()
                .filter(|(row, _, _)| *row != Some(index))
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
        })
        .collect()
}

fn evaluate(name: &str, model: &StaticModel, rows: &[Row]) -> Result<Vec<Vec<f32>>> {
    let bank = Bank::from_toml(SHIPPED_EXAMPLES, model)?;
    let (queries, scored) = score_rows(model, &bank, rows);
    report_thresholds(&format!("{name} nn"), rows, &scored);

    let mut best: Option<(f32, usize, Vec<Scored>)> = None;
    for k in KNN_K {
        let knn = knn_scores(&bank, rows, &queries, k);
        let (dev_scored, dev_gold) = select(rows, &knn, "dev");
        let (_, f1) = best_threshold(&dev_scored, &dev_gold);
        println!("{name} knn k={k} dev max macro-F1 {f1:.3}");
        if best.as_ref().is_none_or(|(top, _, _)| f1 > *top) {
            best = Some((f1, k, knn));
        }
    }
    if let Some((_, k, knn)) = best {
        report_thresholds(&format!("{name} knn k={k}"), rows, &knn);
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
    let args: Vec<String> = std::env::args().collect();
    let [_, f32_dir, int8_dir] = &args[..] else {
        bail!("usage: intent_eval <f32_model_dir> <int8_model_dir>");
    };
    let mut rows = Vec::new();
    for fixture in FIXTURES {
        rows.extend(read_rows(Path::new(fixture))?);
    }
    println!(
        "rows: {} dev, {} test",
        rows.iter().filter(|r| r.split == "dev").count(),
        rows.iter().filter(|r| r.split == "test").count()
    );

    let f32_model = StaticModel::from_pretrained(f32_dir, None, None, None)?;
    let int8_model = StaticModel::from_pretrained(int8_dir, None, None, None)?;
    let memory_int8 = StaticModel::from_pretrained(f32_dir, None, None, None)?.quantize_int8();

    let f32_queries = evaluate("f32", &f32_model, &rows)?;
    let int8_queries = evaluate("int8-file", &int8_model, &rows)?;
    let memory_queries = evaluate("int8-memory", &memory_int8, &rows)?;
    println!(
        "mean cosine int8-file vs int8-memory query vectors: {:.4}",
        mean_cosine(&int8_queries, &memory_queries)
    );
    println!(
        "mean cosine f32 vs int8-file query vectors: {:.4}",
        mean_cosine(&f32_queries, &int8_queries)
    );

    let regex = regex_predictions(&rows);
    for split in ["dev", "test"] {
        let (pred, g) = select(&rows, &regex, split);
        print_report("regex", split, &g, &pred);
    }
    Ok(())
}
