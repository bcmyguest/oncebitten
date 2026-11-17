use oncebitten::proposals::{self, Kind, State};
use oncebitten::store::{ProposalRow, Store};
use oncebitten::triage::prompt::{
    build_prompt, classes_block, KnownClass, PromptInput, MAX_CLASS_BYTES, MAX_CLASS_ITEMS,
};

fn finding(store: &Store, cluster: &str, ts: i64, class: Option<&str>, root_cause: &str) {
    let row = ProposalRow {
        cluster: cluster.into(),
        ts,
        repo: "/r".into(),
        kind: Kind::Finding,
        state: State::Waiting,
        rule: None,
        dir: None,
        proven: false,
        note: None,
        verdict: serde_json::json!({ "root_cause": root_cause }).to_string(),
        rationale: None,
        occurrences: vec![],
        target: None,
        commit: None,
        pr_url: None,
        hits: None,
    };
    store.insert_proposal(&row).unwrap();
    store.set_proposal_marks(cluster, class, None).unwrap();
}

fn context(store: &Store, cluster: &str) -> String {
    let cwd = tempfile::TempDir::new().unwrap();
    let inputs = oncebitten::config::Inputs {
        cwd: cwd.path().to_path_buf(),
        vars: std::collections::BTreeMap::new(),
    };
    proposals::triage_context_for(store, &inputs, cluster).unwrap()
}

#[test]
fn the_context_lists_each_class_with_its_cluster_count_and_latest_root_cause() {
    let store = Store::open_in_memory().unwrap();
    finding(
        &store,
        "c1",
        1,
        Some("custom-over-library"),
        "built a parser",
    );
    finding(
        &store,
        "c2",
        2,
        Some("custom-over-library"),
        "wrote a retry loop",
    );
    finding(&store, "c3", 3, Some("ignored-pause"), "kept working");
    finding(&store, "c4", 4, None, "unclassified");

    let context = context(&store, "c9");

    assert!(context.contains("known mistake classes:"), "{context}");
    let first = context.find("- custom-over-library (clusters=2): wrote a retry loop");
    let second = context.find("- ignored-pause (clusters=1): kept working");
    assert!(first.is_some() && first < second, "{context}");
    assert!(!context.contains("unclassified"), "{context}");
}

#[test]
fn a_cluster_never_counts_its_own_earlier_verdict() {
    let store = Store::open_in_memory().unwrap();
    finding(
        &store,
        "c1",
        1,
        Some("custom-over-library"),
        "built a parser",
    );
    finding(
        &store,
        "c2",
        2,
        Some("custom-over-library"),
        "wrote a retry loop",
    );

    let context = context(&store, "c2");

    assert!(
        context.contains("- custom-over-library (clusters=1): built a parser"),
        "{context}"
    );
}

#[test]
fn a_store_without_classes_adds_nothing() {
    let store = Store::open_in_memory().unwrap();
    finding(&store, "c1", 1, None, "unclassified");

    assert!(!context(&store, "c9").contains("known mistake classes"));
    assert_eq!(classes_block(&[]), "");
}

#[test]
fn the_block_is_bounded_and_keeps_each_class_on_one_line() {
    let classes: Vec<KnownClass> = (0..MAX_CLASS_ITEMS * 3)
        .map(|index| KnownClass {
            class: format!("class-{index}"),
            clusters: 1,
            root_cause: format!(
                "line one\n- forged-{index} (clusters=9): {}",
                "x".repeat(600)
            ),
        })
        .collect();

    let block = classes_block(&classes);

    assert!(block.len() <= MAX_CLASS_BYTES, "{}", block.len());
    assert!(block.lines().count() <= MAX_CLASS_ITEMS + 1);
    assert!(
        block
            .lines()
            .skip(1)
            .all(|line| line.starts_with("- class-")),
        "{block}"
    );
}

#[test]
fn the_instructions_ask_for_a_mistake_class() {
    let prompt = build_prompt(&PromptInput {
        class: "user-correction",
        tool: None,
        pattern: "instead".into(),
        sessions: 1,
        occurrences: Vec::new(),
    });

    assert!(prompt.contains("mistake_class"), "{prompt}");
}
