//! Measures intent classes on `tests/fixtures/intent/labelled.tsv`.
//!
//! Usage: `cargo run --release --example intent_eval -- <f32_model_dir> <int8_model_dir>`
//!
//! Each embedding model gets a threshold chosen on the `dev` split (max macro-F1) and
//! reported on the `test` split. The regex lexicon has no threshold and reports both splits.

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
const FIXTURE: &str = "tests/fixtures/intent/labelled.tsv";

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

fn print_report(name: &str, split: &str, gold: &[&'static str], pred: &[&'static str]) {
    println!(
        "{name} {split}: accuracy {:.3} macro-F1 {:.3}",
        accuracy(gold, pred),
        macro_f1(gold, pred)
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

fn best_threshold(scored: &[(&'static str, f32)], gold: &[&'static str]) -> f32 {
    let mut best = (f32::MIN, 0.0_f32);
    for step in 0..=90 {
        let threshold = step as f32 / 100.0;
        let f1 = macro_f1(gold, &predict(scored, threshold));
        if f1 > best.0 {
            best = (f1, threshold);
        }
    }
    best.1
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

fn evaluate(name: &str, model: &StaticModel, rows: &[Row]) -> Result<(Vec<Vec<f32>>, f32)> {
    let bank = Bank::from_toml(SHIPPED_EXAMPLES, model)?;
    let (queries, scored) = score_rows(model, &bank, rows);
    let (dev_scored, dev_gold) = select(rows, &scored, "dev");
    let threshold = best_threshold(&dev_scored, &dev_gold);
    println!("{name} threshold {threshold:.2} (chosen on dev)");
    print_report(name, "dev", &dev_gold, &predict(&dev_scored, threshold));
    let (test_scored, test_gold) = select(rows, &scored, "test");
    print_report(name, "test", &test_gold, &predict(&test_scored, threshold));
    Ok((queries, threshold))
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
    let rows = read_rows(Path::new(FIXTURE))?;
    println!(
        "rows: {} dev, {} test",
        rows.iter().filter(|r| r.split == "dev").count(),
        rows.iter().filter(|r| r.split == "test").count()
    );

    let f32_model = StaticModel::from_pretrained(f32_dir, None, None, None)?;
    let int8_model = StaticModel::from_pretrained(int8_dir, None, None, None)?;
    let memory_int8 = StaticModel::from_pretrained(f32_dir, None, None, None)?.quantize_int8();

    let (f32_queries, _) = evaluate("f32", &f32_model, &rows)?;
    let (int8_queries, _) = evaluate("int8-file", &int8_model, &rows)?;
    let (memory_queries, _) = evaluate("int8-memory", &memory_int8, &rows)?;
    println!(
        "mean cosine int8-file vs int8-memory query vectors: {:.4}",
        mean_cosine(&int8_queries, &memory_queries)
    );
    println!(
        "mean cosine f32 vs int8-file query vectors: {:.4}",
        mean_cosine(&f32_queries, &int8_queries)
    );

    let regex = regex_predictions(&rows);
    let gold: Vec<&'static str> = rows.iter().map(|r| r.label).collect();
    for split in ["dev", "test"] {
        let (pred, g) = select(&rows, &regex, split);
        print_report("regex", split, &g, &pred);
    }
    let _ = gold;
    Ok(())
}
