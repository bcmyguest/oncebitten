use std::path::Path;
use std::process::{Command, Output};

use oncebitten::evidence::{self, Source};
use oncebitten::signal;
use oncebitten::store::{NewEvent, Store};
use tempfile::TempDir;

const BIN: &str = env!("CARGO_BIN_EXE_oncebitten");

fn tool_event(
    host: &str,
    session: &str,
    repo: &str,
    kind: &str,
    command: &str,
    ts: i64,
) -> NewEvent {
    NewEvent {
        ts,
        host: host.into(),
        session_id: session.into(),
        repo: Some(repo.into()),
        kind: kind.into(),
        tool: Some("Bash".into()),
        summary: command.into(),
    }
}

fn prompt(session: &str, text: &str, ts: i64) -> NewEvent {
    NewEvent {
        ts,
        host: "claude".into(),
        session_id: session.into(),
        repo: None,
        kind: signal::KIND_PROMPT.into(),
        tool: None,
        summary: text.into(),
    }
}

fn capture(store: &Store, event: NewEvent) {
    let history = store
        .session_history(&event.host, &event.session_id, 500)
        .unwrap();
    let signals = signal::derive(&event, &history);
    store.record(&event, &signals).unwrap();
}

fn fail(store: &Store, session: &str, repo: &str, command: &str, ts: i64) {
    capture(
        store,
        tool_event(
            "claude",
            session,
            repo,
            signal::KIND_TOOL_ERROR,
            command,
            ts,
        ),
    );
}

#[test]
fn failures_with_the_same_normalized_command_share_a_cluster() {
    let store = Store::open_in_memory().unwrap();
    fail(&store, "s1", "/a", "cargo build --target-dir /tmp/one", 1);
    fail(&store, "s2", "/b", "cargo  build --target-dir /tmp/two", 2);
    let clusters = evidence::load(&store).unwrap();
    assert_eq!(clusters.len(), 1);
    let cluster = &clusters[0];
    assert_eq!(cluster.class, evidence::CLASS_FAILURE);
    assert_eq!(cluster.source, Source::Mined);
    assert_eq!(cluster.pattern, "cargo build --target-dir <path>");
    assert_eq!(cluster.sessions(), 2);
    assert_eq!(cluster.repos(), vec!["/a", "/b"]);
    assert!(cluster.is_candidate());
}

#[test]
fn repeats_in_one_session_count_as_one_independent_occurrence() {
    let store = Store::open_in_memory().unwrap();
    fail(&store, "s1", "/a", "make lint", 1);
    fail(&store, "s1", "/a", "make lint", 2);
    fail(&store, "s1", "/a", "make lint", 3);
    let clusters = evidence::load(&store).unwrap();
    assert_eq!(clusters.len(), 1);
    assert_eq!(clusters[0].occurrences.len(), 3);
    assert_eq!(clusters[0].sessions(), 1);
    assert!(!clusters[0].is_candidate());
    assert_eq!(clusters[0].status(), "below-threshold");
}

#[test]
fn signals_on_one_event_form_one_occurrence() {
    let store = Store::open_in_memory().unwrap();
    fail(&store, "s1", "/a", "make lint", 1);
    fail(&store, "s1", "/a", "make lint", 2);
    let clusters = evidence::load(&store).unwrap();
    let second = &clusters[0].occurrences[1];
    assert_eq!(
        second.signals,
        vec![
            signal::TOOL_FAILURE.to_string(),
            signal::REPEAT_FAILURE.to_string()
        ]
    );
}

#[test]
fn the_same_session_id_on_two_hosts_is_two_sessions() {
    let store = Store::open_in_memory().unwrap();
    fail(&store, "s1", "/a", "make lint", 1);
    capture(
        &store,
        tool_event(
            "cursor",
            "s1",
            "/a",
            signal::KIND_TOOL_ERROR,
            "make lint",
            2,
        ),
    );
    assert_eq!(evidence::load(&store).unwrap()[0].sessions(), 2);
}

#[test]
fn different_commands_and_classes_stay_apart() {
    let store = Store::open_in_memory().unwrap();
    fail(&store, "s1", "/a", "make lint", 1);
    fail(&store, "s1", "/a", "make test", 2);
    capture(
        &store,
        tool_event(
            "claude",
            "s1",
            "/a",
            signal::KIND_TOOL_OK,
            "git revert abcdef1",
            3,
        ),
    );
    let mut classes: Vec<_> = evidence::load(&store)
        .unwrap()
        .iter()
        .map(|c| (c.class, c.pattern.clone()))
        .collect();
    classes.sort();
    assert_eq!(
        classes,
        vec![
            ("failure", "make lint".to_string()),
            ("failure", "make test".to_string()),
            ("revert", "git revert <hex>".to_string()),
        ]
    );
}

#[test]
fn a_stated_correction_is_a_candidate_from_one_session() {
    let store = Store::open_in_memory().unwrap();
    capture(&store, prompt("s1", "Never push to main", 1));
    capture(&store, prompt("s1", "never use unwrap", 2));
    let clusters = evidence::load(&store).unwrap();
    assert_eq!(clusters.len(), 2);
    for cluster in &clusters {
        assert_eq!(cluster.source, Source::Stated);
        assert!(cluster.is_candidate());
        assert!(!cluster.pattern.contains("push"));
    }
}

#[test]
fn rejection_is_remembered_as_new_occurrences_arrive() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("events.db");
    let store = Store::open(&path).unwrap();
    fail(&store, "s1", "/a", "make lint", 1);
    fail(&store, "s2", "/a", "make lint", 2);
    let id = evidence::load(&store).unwrap()[0].id.clone();
    evidence::reject(&store, &id, 10, "  flaky linter  ").unwrap();
    drop(store);
    let store = Store::open(&path).unwrap();
    fail(&store, "s3", "/a", "make lint", 3);
    let cluster = evidence::load(&store).unwrap().remove(0);
    assert_eq!(cluster.id, id);
    assert_eq!(cluster.sessions(), 3);
    assert_eq!(cluster.status(), "rejected");
    assert_eq!(cluster.rejection.unwrap().reason, "flaky linter");
}

#[test]
fn rejecting_needs_a_known_cluster_and_a_reason() {
    let store = Store::open_in_memory().unwrap();
    fail(&store, "s1", "/a", "make lint", 1);
    let id = evidence::load(&store).unwrap()[0].id.clone();
    assert!(evidence::reject(&store, "000000000000", 1, "no").is_err());
    assert!(evidence::reject(&store, &id, 1, " ").is_err());
    assert!(store.rejections().unwrap().is_empty());
}

#[test]
fn cluster_ids_are_stable_across_stores() {
    let first = Store::open_in_memory().unwrap();
    let second = Store::open_in_memory().unwrap();
    fail(&first, "s1", "/a", "make lint", 1);
    fail(&second, "other", "/b", "make  lint", 9);
    assert_eq!(
        evidence::load(&first).unwrap()[0].id,
        evidence::load(&second).unwrap()[0].id
    );
}

fn run(home: &Path, args: &[&str]) -> Output {
    Command::new(BIN)
        .args(args)
        .current_dir(home)
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("XDG_DATA_HOME", home.join("data"))
        .output()
        .unwrap()
}

fn stdout(out: &Output) -> String {
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout.clone()).unwrap()
}

#[test]
fn evidence_cli_lists_shows_and_rejects() {
    let home = TempDir::new().unwrap();
    {
        let store = Store::open(&home.path().join("data/oncebitten/events.db")).unwrap();
        fail(&store, "s1", "/a", "make lint", 1);
        fail(&store, "s2", "/b", "make lint", 2);
        fail(&store, "s3", "/b", "make docs", 3);
    }
    let listed = stdout(&run(home.path(), &["evidence", "list"]));
    let lines: Vec<&str> = listed.lines().collect();
    assert_eq!(lines.len(), 1, "{listed}");
    assert!(lines[0].contains("failure  candidate  sessions=2  occurrences=2  Bash  make lint"));
    let id = lines[0].split_whitespace().next().unwrap().to_string();

    let shown = stdout(&run(home.path(), &["evidence", "show", &id]));
    assert!(shown.contains("repos: /a, /b"));
    assert!(shown.contains("1  claude  s1  /a  event 1  tool-failure  make lint"));
    assert!(shown.contains("head -"));

    stdout(&run(
        home.path(),
        &["evidence", "reject", &id, "--reason", "one-off"],
    ));
    assert_eq!(stdout(&run(home.path(), &["evidence", "list"])), "");
    let all = stdout(&run(home.path(), &["evidence", "list", "--all"]));
    assert_eq!(all.lines().count(), 2);
    assert!(all.contains(&format!("{id}  failure  rejected")));
    assert!(all.contains("below-threshold  sessions=1  occurrences=1  Bash  make docs"));

    let missing = run(home.path(), &["evidence", "show", "nope"]);
    assert!(!missing.status.success());
}
