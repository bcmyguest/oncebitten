use std::io::Write;
use std::path::Path;
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

use oncebitten::capture::{run, run_with};
use tempfile::TempDir;

const BIN: &str = env!("CARGO_BIN_EXE_oncebitten");

fn prompt_payload(session: &str, prompt: &str) -> String {
    format!(
        r#"{{"session_id":"{session}","cwd":"/nowhere","hook_event_name":"UserPromptSubmit","prompt":"{prompt}"}}"#
    )
}

fn count(db: &Path, table: &str) -> i64 {
    rusqlite::Connection::open(db)
        .unwrap()
        .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
        .unwrap()
}

fn spawn_capture(home: &Path, host: &str, triage: Option<&str>) -> Child {
    let mut cmd = Command::new(BIN);
    cmd.args(["capture", "--host", host])
        .env("HOME", home)
        .env("XDG_DATA_HOME", home.join("data"))
        .env_remove("ONCEBITTEN_TRIAGE")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(value) = triage {
        cmd.env("ONCEBITTEN_TRIAGE", value);
    }
    cmd.spawn().unwrap()
}

fn capture(home: &Path, host: &str, payload: &str, triage: Option<&str>) -> Output {
    let mut child = spawn_capture(home, host, triage);
    child
        .stdin
        .take()
        .unwrap()
        .write_all(payload.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

fn events_db(home: &Path) -> std::path::PathBuf {
    home.join("data").join("oncebitten").join("events.db")
}

#[test]
fn run_records_the_event_and_its_signals() {
    let dir = TempDir::new().unwrap();
    let db = dir.path().join("events.db");
    run("claude", &prompt_payload("s1", "No, stop"), &db, 5).unwrap();
    assert_eq!(count(&db, "events"), 1);
    assert_eq!(count(&db, "signals"), 1);
}

#[test]
fn run_with_an_unsupported_host_records_nothing() {
    let dir = TempDir::new().unwrap();
    let db = dir.path().join("events.db");
    assert!(run("codex", "{}", &db, 5).is_err());
    assert!(!db.exists());
}

#[test]
fn capture_records_into_the_user_data_dir_and_prints_nothing() {
    let home = TempDir::new().unwrap();
    let out = capture(home.path(), "claude", &prompt_payload("s1", "fine"), None);
    assert!(out.status.success());
    assert!(out.stdout.is_empty());
    assert!(out.stderr.is_empty());
    assert_eq!(count(&events_db(home.path()), "events"), 1);
}

#[test]
fn triage_switch_exits_zero_and_records_nothing_for_every_host() {
    use oncebitten::host::ALL_HOSTS;
    for host in ALL_HOSTS {
        let name = host.name();
        let home = TempDir::new().unwrap();
        let payload = prompt_payload("s1", "No, stop");
        let out = capture(home.path(), name, &payload, Some("1"));
        assert!(out.status.success(), "{name}: {out:?}");
        assert!(!events_db(home.path()).exists(), "{name} recorded an event");
    }
}

#[test]
fn the_same_payload_records_for_claude_without_the_switch() {
    let home = TempDir::new().unwrap();
    let out = capture(
        home.path(),
        "claude",
        &prompt_payload("s1", "No, stop"),
        None,
    );
    assert!(out.status.success());
    assert!(events_db(home.path()).exists());
}

#[test]
fn triage_switch_skips_capture_entirely() {
    let home = TempDir::new().unwrap();
    let out = capture(home.path(), "claude", &prompt_payload("s1", "x"), Some("1"));
    assert!(out.status.success());
    assert!(!events_db(home.path()).exists());
}

#[test]
fn errors_exit_zero_with_one_stderr_line() {
    let home = TempDir::new().unwrap();
    let out = capture(home.path(), "codex", "{}", None);
    assert!(out.status.success());
    assert!(out.stdout.is_empty());
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert_eq!(stderr.lines().count(), 1);
    assert!(stderr.contains("no session id"));
}

#[test]
fn malformed_stdin_exits_zero() {
    let home = TempDir::new().unwrap();
    let out = capture(home.path(), "claude", "not json", None);
    assert!(out.status.success());
    assert!(out.stdout.is_empty());
}

/// Baseline for `one_hundred_captures_stay_near_process_spawn_cost`: the same
/// binary doing nothing, so machine load and slow spawns hit both sides alike.
fn timed_noop() -> Duration {
    let start = Instant::now();
    let out = Command::new(BIN)
        .arg("--version")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(out.status.success());
    start.elapsed()
}

fn median(mut samples: Vec<Duration>) -> Duration {
    samples.sort();
    samples[samples.len() / 2]
}

#[test]
fn one_hundred_captures_stay_near_process_spawn_cost() {
    let home = TempDir::new().unwrap();
    let mut captures = Vec::new();
    let mut baselines = Vec::new();
    for i in 0..100 {
        baselines.push(timed_noop());
        let payload = prompt_payload("s1", &format!("prompt {i}"));
        let start = Instant::now();
        let out = capture(home.path(), "claude", &payload, None);
        captures.push(start.elapsed());
        assert!(out.status.success());
    }
    assert_eq!(count(&events_db(home.path()), "events"), 100);
    let (capture_median, baseline_median) = (median(captures), median(baselines));
    eprintln!("median capture {capture_median:?}, median no-op spawn {baseline_median:?}");
    let limit = baseline_median * 3 + Duration::from_millis(20);
    assert!(
        capture_median < limit,
        "median capture {capture_median:?} exceeds {limit:?} (no-op spawn {baseline_median:?})"
    );
}

#[test]
fn run_with_uses_the_given_lexicon() {
    let dir = TempDir::new().unwrap();
    let db = dir.path().join("events.db");
    run_with(
        "claude",
        &prompt_payload("s1", "use tabs"),
        &db,
        5,
        &["tabs"],
    )
    .unwrap();
    assert_eq!(count(&db, "signals"), 1);
    let none: [&str; 0] = [];
    run_with("claude", &prompt_payload("s2", "use tabs"), &db, 6, &none).unwrap();
    assert_eq!(count(&db, "signals"), 1);
}

#[test]
fn stored_correction_keeps_its_text_and_the_signal_names_only_the_phrase() {
    let dir = TempDir::new().unwrap();
    let db = dir.path().join("events.db");
    run(
        "claude",
        &prompt_payload("s1", "No, my private note"),
        &db,
        5,
    )
    .unwrap();
    let conn = rusqlite::Connection::open(&db).unwrap();
    let summary: String = conn
        .query_row("SELECT summary FROM events", [], |r| r.get(0))
        .unwrap();
    let detail: String = conn
        .query_row("SELECT detail FROM signals", [], |r| r.get(0))
        .unwrap();
    assert_eq!(summary, "No, my private note");
    assert_eq!(detail, "no,");
}

fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?} failed");
}

#[test]
fn capture_records_the_repo_head_for_an_event_in_a_git_repo() {
    let repo = TempDir::new().unwrap();
    git(repo.path(), &["init", "-q"]);
    git(
        repo.path(),
        &[
            "-c",
            "user.name=test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "init",
        ],
    );
    let head = Command::new("git")
        .arg("-C")
        .arg(repo.path())
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap();
    let expected = String::from_utf8(head.stdout).unwrap().trim().to_string();
    let data = TempDir::new().unwrap();
    let db = data.path().join("events.db");
    let cwd = repo.path().display().to_string().replace('\\', "\\\\");
    let payload = format!(
        r#"{{"session_id":"s1","cwd":"{cwd}","hook_event_name":"UserPromptSubmit","prompt":"hello"}}"#
    );
    run("claude", &payload, &db, 1).unwrap();
    let stored: Option<String> = rusqlite::Connection::open(&db)
        .unwrap()
        .query_row("SELECT git_head FROM events", [], |r| r.get(0))
        .unwrap();
    assert_eq!(stored, Some(expected));
}

#[test]
fn capture_outside_a_repo_stores_no_head() {
    let data = TempDir::new().unwrap();
    let db = data.path().join("events.db");
    run("claude", &prompt_payload("s1", "hello"), &db, 1).unwrap();
    let stored: Option<String> = rusqlite::Connection::open(&db)
        .unwrap()
        .query_row("SELECT git_head FROM events", [], |r| r.get(0))
        .unwrap();
    assert_eq!(stored, None);
}

#[test]
fn hook_redelivery_of_one_tool_use_is_stored_once() {
    let data = TempDir::new().unwrap();
    let db = data.path().join("events.db");
    let payload = r#"{"session_id":"s1","cwd":"/nowhere","hook_event_name":"PostToolUse","tool_name":"Bash","tool_input":{"command":"cargo test"},"tool_response":{"stdout":"ok"},"tool_use_id":"toolu_9"}"#;
    run("claude", payload, &db, 1).unwrap();
    run("claude", payload, &db, 2).unwrap();
    assert_eq!(count(&db, "events"), 1);
}

#[test]
fn two_hook_scopes_delivering_one_prompt_store_it_once() {
    let home = TempDir::new().unwrap();
    let payload = prompt_payload("s1", "No, stop");
    let mut children: Vec<Child> = (0..2)
        .map(|_| spawn_capture(home.path(), "claude", None))
        .collect();
    for child in &mut children {
        child
            .stdin
            .take()
            .unwrap()
            .write_all(payload.as_bytes())
            .unwrap();
    }
    for child in children {
        let out = child.wait_with_output().unwrap();
        assert!(out.status.success(), "{out:?}");
    }
    assert_eq!(count(&events_db(home.path()), "events"), 1);
}
