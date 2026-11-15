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

const EXTRA_FIXTURE: &str = "tests/fixtures/intent/labelled-extra.tsv";

fn fixture_rows(path: &str) -> Vec<(String, String, String)> {
    let source = fs::read_to_string(path).expect("fixture readable");
    source
        .lines()
        .filter(|l| !l.starts_with('#') && !l.is_empty())
        .map(|line| {
            let fields: Vec<&str> = line.splitn(3, '\t').collect();
            assert_eq!(fields.len(), 3, "malformed row: {line}");
            assert!(matches!(fields[0], "dev" | "test"), "split in {line}");
            assert!(CLASSES.contains(&fields[1]), "class in {line}");
            (
                fields[0].to_string(),
                fields[1].to_string(),
                fields[2].to_string(),
            )
        })
        .collect()
}

fn all_rows() -> Vec<(String, String, String)> {
    let mut rows = fixture_rows(FIXTURE);
    rows.extend(fixture_rows(EXTRA_FIXTURE));
    rows
}

#[test]
fn combined_fixtures_are_stratified_by_class() {
    let mut counts: BTreeMap<(String, String), usize> = BTreeMap::new();
    for (split, class, _) in all_rows() {
        *counts.entry((class, split)).or_default() += 1;
    }
    for class in CLASSES {
        let dev = counts[&(class.to_string(), "dev".to_string())];
        let test = counts[&(class.to_string(), "test".to_string())];
        assert_eq!(dev, test, "{class} dev/test");
        let minimum = if class == "neutral" { 100 } else { 50 };
        assert!(dev + test >= minimum, "{class} has {} rows", dev + test);
    }
}

#[test]
fn combined_fixtures_have_no_duplicate_texts() {
    let mut texts: Vec<String> = all_rows().into_iter().map(|(_, _, text)| text).collect();
    let total = texts.len();
    texts.sort_unstable();
    texts.dedup();
    assert_eq!(texts.len(), total);
}

#[test]
fn combined_fixtures_are_disjoint_from_shipped_examples() {
    let shipped: BTreeMap<String, Vec<String>> =
        toml::from_str(oncebitten::intent::SHIPPED_EXAMPLES).expect("shipped examples parse");
    let shipped: Vec<String> = shipped.into_values().flatten().collect();
    for (_, _, text) in all_rows() {
        assert!(
            !shipped.contains(&text),
            "fixture row is a shipped example: {text}"
        );
    }
}
