//! Checks the labelled intent fixture that `examples/intent_eval.rs` measures.

use std::collections::BTreeMap;
use std::fs;

const FIXTURE: &str = "tests/fixtures/intent/labelled.tsv";
const CLASSES: [&str; 4] = ["rule-request", "correction", "frustration", "neutral"];

#[test]
fn labelled_fixture_has_thirty_rows_per_class_split_evenly() {
    let source = fs::read_to_string(FIXTURE).expect("fixture readable");
    let mut counts: BTreeMap<(String, String), usize> = BTreeMap::new();
    for line in source
        .lines()
        .filter(|l| !l.starts_with('#') && !l.is_empty())
    {
        let fields: Vec<&str> = line.splitn(3, '\t').collect();
        assert_eq!(fields.len(), 3, "malformed row: {line}");
        assert!(matches!(fields[0], "dev" | "test"), "split in {line}");
        assert!(CLASSES.contains(&fields[1]), "class in {line}");
        *counts
            .entry((fields[1].to_string(), fields[0].to_string()))
            .or_default() += 1;
    }
    for class in CLASSES {
        for split in ["dev", "test"] {
            let key = (class.to_string(), split.to_string());
            assert_eq!(counts.get(&key).copied(), Some(15), "{class} {split}");
        }
    }
}

#[test]
fn labelled_fixture_has_no_duplicate_texts() {
    let source = fs::read_to_string(FIXTURE).expect("fixture readable");
    let mut texts: Vec<&str> = source
        .lines()
        .filter(|l| !l.starts_with('#') && !l.is_empty())
        .filter_map(|l| l.splitn(3, '\t').nth(2))
        .collect();
    let total = texts.len();
    texts.sort_unstable();
    texts.dedup();
    assert_eq!(texts.len(), total);
}
