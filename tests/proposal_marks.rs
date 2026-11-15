use oncebitten::proposals::{self, Kind, State};
use oncebitten::store::{ProposalRow, Store};

fn row(cluster: &str) -> ProposalRow {
    ProposalRow {
        cluster: cluster.into(),
        ts: 1,
        repo: "/r".into(),
        kind: Kind::Proposal,
        state: State::Waiting,
        rule: None,
        dir: None,
        proven: false,
        note: None,
        verdict: "{}".into(),
        rationale: Some("never use unwrap in library code".into()),
        occurrences: vec![],
        target: None,
        commit: None,
        pr_url: None,
        hits: None,
    }
}

#[test]
fn marks_round_trip_through_the_store() {
    let store = Store::open_in_memory().unwrap();
    store.insert_proposal(&row("c1")).unwrap();
    store
        .set_proposal_marks(
            "c1",
            Some("unwrap-in-lib"),
            Some("possible duplicate of c0 (same mistake_class)"),
        )
        .unwrap();
    let marks = store.proposal_marks("c1").unwrap().unwrap();
    assert_eq!(marks.mistake_class.as_deref(), Some("unwrap-in-lib"));
    assert_eq!(
        marks.possible_duplicate.as_deref(),
        Some("possible duplicate of c0 (same mistake_class)")
    );
    assert_eq!(marks.duplicate_of, None);
}

#[test]
fn a_confirmed_duplicate_link_is_stored_and_needs_a_queued_row() {
    let store = Store::open_in_memory().unwrap();
    store.insert_proposal(&row("c1")).unwrap();
    store.set_duplicate_of("c1", "c0").unwrap();
    assert_eq!(
        store
            .proposal_marks("c1")
            .unwrap()
            .unwrap()
            .duplicate_of
            .as_deref(),
        Some("c0")
    );
    assert!(store.set_duplicate_of("missing", "c0").is_err());
}

#[test]
fn candidates_carry_class_and_rationale() {
    let store = Store::open_in_memory().unwrap();
    store.insert_proposal(&row("c1")).unwrap();
    store
        .set_proposal_marks("c1", Some("unwrap-in-lib"), None)
        .unwrap();
    let candidates = store.mark_candidates().unwrap();
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].cluster, "c1");
    assert_eq!(
        candidates[0].mistake_class.as_deref(),
        Some("unwrap-in-lib")
    );
    assert_eq!(
        candidates[0].rationale.as_deref(),
        Some("never use unwrap in library code")
    );
}

#[test]
fn a_reworded_proposal_matches_the_queued_one() {
    let store = Store::open_in_memory().unwrap();
    store.insert_proposal(&row("c0")).unwrap();
    let mark = proposals::possible_duplicate(
        &store,
        "c1",
        None,
        Some("stop using unwrap in library code"),
    )
    .unwrap()
    .unwrap();
    assert_eq!(mark.of, "c0");
    assert!(mark.reason.starts_with("similar text"), "{}", mark.reason);
}

#[test]
fn equal_mistake_class_matches_regardless_of_text() {
    let store = Store::open_in_memory().unwrap();
    store.insert_proposal(&row("c0")).unwrap();
    store
        .set_proposal_marks("c0", Some("unwrap-in-lib"), None)
        .unwrap();
    let mark = proposals::possible_duplicate(&store, "c1", Some("unwrap-in-lib"), Some("zzz"))
        .unwrap()
        .unwrap();
    assert_eq!(mark.reason, "same mistake_class");
}

#[test]
fn an_unrelated_proposal_is_not_marked() {
    let store = Store::open_in_memory().unwrap();
    store.insert_proposal(&row("c0")).unwrap();
    let mark = proposals::possible_duplicate(&store, "c1", None, Some("always pin the toolchain"))
        .unwrap();
    assert!(mark.is_none());
}

#[test]
fn mark_lines_show_the_possible_and_confirmed_duplicate() {
    use oncebitten::store::ProposalMarks;
    let marks = ProposalMarks {
        mistake_class: None,
        possible_duplicate: Some("possible duplicate of c0 (same mistake_class)".into()),
        duplicate_of: Some("c0".into()),
    };
    assert_eq!(
        proposals::mark_lines(&marks),
        vec![
            "possible duplicate of c0 (same mistake_class)".to_string(),
            "confirmed duplicate of c0".to_string(),
        ]
    );
    assert!(proposals::mark_lines(&ProposalMarks::default()).is_empty());
}

#[test]
fn rejecting_as_a_duplicate_links_and_rejects() {
    let store = Store::open_in_memory().unwrap();
    store.insert_proposal(&row("c0")).unwrap();
    store.insert_proposal(&row("c1")).unwrap();
    proposals::reject_duplicate(&store, "c1", "c0", 2).unwrap();
    assert_eq!(
        store
            .proposal_marks("c1")
            .unwrap()
            .unwrap()
            .duplicate_of
            .as_deref(),
        Some("c0")
    );
    assert_eq!(
        store.proposal("c1").unwrap().unwrap().state,
        State::Rejected
    );
    assert!(proposals::reject_duplicate(&store, "c1", "missing", 3).is_err());
}

#[test]
fn triage_context_carries_past_rejection_reasons() {
    let store = Store::open_in_memory().unwrap();
    store.reject("c0", 1, "duplicate of unwrap-rule").unwrap();
    let cwd = tempfile::TempDir::new().unwrap();
    let inputs = oncebitten::config::Inputs {
        cwd: cwd.path().to_path_buf(),
        vars: std::collections::BTreeMap::new(),
    };
    let context = proposals::triage_context(&store, &inputs).unwrap();
    assert!(context.contains("rejected proposals:"), "{context}");
    assert!(context.contains("duplicate of unwrap-rule"), "{context}");
}

#[test]
fn reject_duplicate_refuses_the_proposals_own_id() {
    let store = Store::open_in_memory().unwrap();
    store.insert_proposal(&row("c1")).unwrap();
    let error = proposals::reject_duplicate(&store, "c1", "c1", 2).unwrap_err();
    assert!(error.to_string().contains("itself"), "{error:#}");
    assert_eq!(store.proposal("c1").unwrap().unwrap().state, State::Waiting);
    assert_eq!(
        store.proposal_marks("c1").unwrap().unwrap().duplicate_of,
        None
    );
}

/// Copies `examples/<id>` into `dest`, a proposed rule directory.
#[cfg(unix)]
fn copy_example(id: &str, dest: &std::path::Path) {
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples")
        .join(id);
    for entry in walk(&src) {
        let target = dest.join(entry.strip_prefix(&src).unwrap());
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::copy(&entry, &target).unwrap();
    }
}

#[cfg(unix)]
fn walk(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files.extend(walk(&path));
        } else {
            files.push(path);
        }
    }
    files
}

#[cfg(unix)]
#[test]
fn a_failed_duplicate_reject_writes_no_duplicate_link() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::TempDir::new().unwrap();
    let rules = root.path().join("rules");
    let dir = rules.join("py-utcnow");
    copy_example("py-utcnow", &dir);
    let store = Store::open_in_memory().unwrap();
    store.insert_proposal(&row("c0")).unwrap();
    let mut proposal = row("c1");
    proposal.dir = Some(dir.clone());
    store.insert_proposal(&proposal).unwrap();
    std::fs::set_permissions(&rules, std::fs::Permissions::from_mode(0o555)).unwrap();
    let result = proposals::reject_duplicate(&store, "c1", "c0", 2);
    std::fs::set_permissions(&rules, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(result.is_err(), "the rule dir was removable");
    assert_eq!(
        store.proposal_marks("c1").unwrap().unwrap().duplicate_of,
        None
    );
}
