use std::fs;
use std::path::Path;

use oncebitten::config::{SessionTrigger, Triggers};
use oncebitten::proposals;
use oncebitten::signal;
use oncebitten::store::{NewEvent, Store};
use tempfile::TempDir;

/// Records a failing `cargo doc` in two sessions, so the cluster crosses the
/// threshold on the second event. The repo is a `.git`-marked temp dir.
fn cluster_at_threshold(store: &Store, repo: &Path) -> i64 {
    fs::create_dir_all(repo.join(".git")).unwrap();
    let mut last = 0;
    for (session, ts) in [("s1", 1), ("s2", 2)] {
        let event = NewEvent {
            ts,
            host: "codex".into(),
            session_id: session.into(),
            repo: Some(repo.to_string_lossy().into_owned()),
            kind: signal::KIND_TOOL_ERROR.into(),
            tool: Some("Bash".into()),
            summary: "cargo doc".into(),
        };
        let history = store.session_history("codex", session, 500).unwrap();
        let signals = signal::derive(&event, &history);
        last = store.record(&event, &signals).unwrap().unwrap();
    }
    last
}

#[test]
fn a_repeat_failure_cluster_is_blocked_when_repeat_failure_is_off() {
    let dir = TempDir::new().unwrap();
    let store = Store::open_in_memory().unwrap();
    let event = cluster_at_threshold(&store, &dir.path().join("repo"));
    let triggers = Triggers {
        session: vec![SessionTrigger::UserCorrection],
        gate: vec![],
    };
    let due = proposals::due_gated(&store, event, &triggers).unwrap();
    assert!(due.is_empty());
}

#[test]
fn a_repeat_failure_cluster_starts_when_repeat_failure_is_on() {
    let dir = TempDir::new().unwrap();
    let store = Store::open_in_memory().unwrap();
    let event = cluster_at_threshold(&store, &dir.path().join("repo"));
    let triggers = Triggers {
        session: vec![SessionTrigger::RepeatFailure],
        gate: vec![],
    };
    let due = proposals::due_gated(&store, event, &triggers).unwrap();
    assert_eq!(due.len(), 1);
}

#[test]
fn a_skipped_cluster_is_picked_up_again_for_its_repo_only() {
    use oncebitten::store::TriageOutcome;
    let dir = TempDir::new().unwrap();
    let repo = dir.path().join("repo");
    let store = Store::open_in_memory().unwrap();
    cluster_at_threshold(&store, &repo);
    let cluster = oncebitten::evidence::load(&store)
        .unwrap()
        .into_iter()
        .next()
        .unwrap()
        .id;
    store
        .record_triage(&cluster, Some("codex"), 5, TriageOutcome::SkippedBudget)
        .unwrap();
    let picked = proposals::pickup(&store, &repo, &Triggers::default()).unwrap();
    assert_eq!(picked.len(), 1);
    assert_eq!(picked[0].cluster, cluster);

    let other = dir.path().join("other");
    assert!(proposals::pickup(&store, &other, &Triggers::default())
        .unwrap()
        .is_empty());
}

#[test]
fn a_skipped_cluster_is_not_picked_up_once_a_later_run_starts() {
    use oncebitten::store::TriageOutcome;
    let dir = TempDir::new().unwrap();
    let repo = dir.path().join("repo");
    let store = Store::open_in_memory().unwrap();
    cluster_at_threshold(&store, &repo);
    let cluster = oncebitten::evidence::load(&store)
        .unwrap()
        .into_iter()
        .next()
        .unwrap()
        .id;
    store
        .record_triage(&cluster, Some("codex"), 5, TriageOutcome::SkippedBudget)
        .unwrap();
    store
        .record_triage(&cluster, Some("codex"), 6, TriageOutcome::Started)
        .unwrap();
    assert!(proposals::pickup(&store, &repo, &Triggers::default())
        .unwrap()
        .is_empty());
}
