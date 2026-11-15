use std::collections::BTreeMap;
use std::path::PathBuf;

use oncebitten::config::Platform;
use oncebitten::signal::Signal;
use oncebitten::store::{events_db_path_for, NewEvent, Store, SUMMARY_LIMIT};
use tempfile::TempDir;

fn vars(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

fn event(session: &str, summary: &str, ts: i64) -> NewEvent {
    NewEvent {
        ts,
        host: "claude".into(),
        session_id: session.into(),
        repo: Some("/repo".into()),
        kind: "tool-error".into(),
        tool: Some("Bash".into()),
        summary: summary.into(),
    }
}

#[test]
fn xdg_data_home_wins_when_absolute() {
    let path = events_db_path_for(
        &vars(&[("XDG_DATA_HOME", "/data"), ("HOME", "/home/u")]),
        Platform::Unix,
    )
    .unwrap();
    assert_eq!(path, PathBuf::from("/data/oncebitten/events.db"));
}

#[test]
fn relative_xdg_data_home_falls_back_to_local_share() {
    let path = events_db_path_for(
        &vars(&[("XDG_DATA_HOME", "relative"), ("HOME", "/home/u")]),
        Platform::Unix,
    )
    .unwrap();
    assert_eq!(
        path,
        PathBuf::from("/home/u/.local/share/oncebitten/events.db")
    );
}

#[test]
fn unix_without_absolute_home_is_an_error() {
    assert!(events_db_path_for(&vars(&[("HOME", "relative")]), Platform::Unix).is_err());
    assert!(events_db_path_for(&vars(&[]), Platform::Unix).is_err());
}

#[test]
fn windows_uses_local_appdata_and_ignores_home() {
    let path = events_db_path_for(
        &vars(&[
            ("LOCALAPPDATA", r"C:\Users\u\AppData\Local"),
            ("HOME", "/h"),
        ]),
        Platform::Windows,
    )
    .unwrap();
    assert_eq!(
        path,
        PathBuf::from(r"C:\Users\u\AppData\Local")
            .join("oncebitten")
            .join("events.db")
    );
}

#[test]
fn windows_without_absolute_local_appdata_is_an_error() {
    assert!(events_db_path_for(&vars(&[("LOCALAPPDATA", "rel")]), Platform::Windows).is_err());
}

fn raw(path: &std::path::Path) -> rusqlite::Connection {
    rusqlite::Connection::open(path).unwrap()
}

#[test]
fn open_creates_schema_and_wal_mode() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("nested").join("events.db");
    let _store = Store::open(&path).unwrap();
    let conn = raw(&path);
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    let mode: String = conn
        .query_row("PRAGMA journal_mode", [], |r| r.get(0))
        .unwrap();
    assert_eq!(version, 4);
    assert_eq!(mode, "wal");
}

#[test]
fn record_then_history_returns_the_event() {
    let store = Store::open_in_memory().unwrap();
    let id = store.record(&event("s1", "cargo test", 1), &[]).unwrap();
    assert!(id.is_some());
    let history = store.session_history("claude", "s1", 10).unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].summary, "cargo test");
    assert_eq!(history[0].repo.as_deref(), Some("/repo"));
}

#[test]
fn identical_event_is_deduplicated() {
    let store = Store::open_in_memory().unwrap();
    assert!(store.record(&event("s1", "x", 1), &[]).unwrap().is_some());
    assert!(store.record(&event("s1", "x", 1), &[]).unwrap().is_none());
    assert_eq!(store.session_history("claude", "s1", 10).unwrap().len(), 1);
}

#[test]
fn same_command_at_a_later_time_is_kept() {
    let store = Store::open_in_memory().unwrap();
    store.record(&event("s1", "x", 1), &[]).unwrap();
    assert!(store.record(&event("s1", "x", 2), &[]).unwrap().is_some());
    assert_eq!(store.session_history("claude", "s1", 10).unwrap().len(), 2);
}

#[test]
fn duplicate_event_does_not_store_its_signals() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("events.db");
    let store = Store::open(&path).unwrap();
    let signal = Signal {
        kind: "tool-failure",
        detail: "boom".into(),
    };
    store
        .record(&event("s1", "x", 1), std::slice::from_ref(&signal))
        .unwrap();
    store
        .record(&event("s1", "x", 1), std::slice::from_ref(&signal))
        .unwrap();
    let count: i64 = raw(&path)
        .query_row("SELECT COUNT(*) FROM signals", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 1);
}

#[test]
fn summary_is_truncated_to_the_limit() {
    let store = Store::open_in_memory().unwrap();
    let long = "é".repeat(SUMMARY_LIMIT + 50);
    store.record(&event("s1", &long, 1), &[]).unwrap();
    let history = store.session_history("claude", "s1", 10).unwrap();
    assert_eq!(history[0].summary.chars().count(), SUMMARY_LIMIT);
}

#[test]
fn history_is_scoped_to_host_and_session() {
    let store = Store::open_in_memory().unwrap();
    store.record(&event("s1", "a", 1), &[]).unwrap();
    store.record(&event("s2", "b", 2), &[]).unwrap();
    let mut other_host = event("s1", "c", 3);
    other_host.host = "codex".into();
    store.record(&other_host, &[]).unwrap();
    let history = store.session_history("claude", "s1", 10).unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].summary, "a");
}

#[test]
fn history_returns_the_most_recent_events_oldest_first() {
    let store = Store::open_in_memory().unwrap();
    for ts in 1..=5 {
        store
            .record(&event("s1", &format!("e{ts}"), ts), &[])
            .unwrap();
    }
    let summaries: Vec<String> = store
        .session_history("claude", "s1", 2)
        .unwrap()
        .into_iter()
        .map(|e| e.summary)
        .collect();
    assert_eq!(summaries, vec!["e4", "e5"]);
}

#[test]
fn reopening_keeps_data() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("events.db");
    Store::open(&path)
        .unwrap()
        .record(&event("s1", "kept", 1), &[])
        .unwrap();
    let reopened = Store::open(&path).unwrap();
    assert_eq!(
        reopened.session_history("claude", "s1", 10).unwrap().len(),
        1
    );
}

#[test]
fn unknown_future_schema_version_is_an_error() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("events.db");
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.pragma_update(None, "user_version", 99).unwrap();
    }
    assert!(Store::open(&path).is_err());
}

fn prompt_event(session: &str, text: &str, ts: i64) -> NewEvent {
    NewEvent {
        kind: "prompt".into(),
        tool: None,
        ..event(session, text, ts)
    }
}

fn digest(text: &str) -> String {
    use sha2::{Digest, Sha256};
    let hex: String = Sha256::digest(text.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    format!("sha256:{hex}")
}

#[test]
fn prompt_text_is_stored_as_a_digest() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("events.db");
    let store = Store::open(&path).unwrap();
    let signal = Signal {
        kind: "user-correction",
        detail: "no,".into(),
    };
    store
        .record(&prompt_event("s", "No, my secret plan", 1), &[signal])
        .unwrap();
    let conn = raw(&path);
    let summary: String = conn
        .query_row("SELECT summary FROM events", [], |r| r.get(0))
        .unwrap();
    let detail: String = conn
        .query_row("SELECT detail FROM signals", [], |r| r.get(0))
        .unwrap();
    assert_eq!(summary, digest("No, my secret plan"));
    assert_eq!(detail, "no,");
}

#[test]
fn migration_blanks_prompt_text_from_schema_one() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("events.db");
    {
        let conn = raw(&path);
        conn.execute_batch(
            "CREATE TABLE events (id INTEGER PRIMARY KEY, ts INTEGER NOT NULL, host TEXT NOT NULL,
             session_id TEXT NOT NULL, repo TEXT, kind TEXT NOT NULL, tool TEXT,
             summary TEXT NOT NULL, payload_hash TEXT NOT NULL UNIQUE);
             CREATE TABLE signals (id INTEGER PRIMARY KEY, event_id INTEGER NOT NULL,
             ts INTEGER NOT NULL, host TEXT NOT NULL, session_id TEXT NOT NULL,
             kind TEXT NOT NULL, detail TEXT NOT NULL);
             INSERT INTO events VALUES (1, 1, 'claude', 's', NULL, 'prompt', NULL,
             'Please never touch my keys', 'h1');
             INSERT INTO events VALUES (2, 2, 'claude', 's', NULL, 'tool-ok', 'Bash',
             'cargo test', 'h2');
             INSERT INTO signals VALUES (1, 1, 1, 'claude', 's', 'user-correction',
             'Please never touch my keys');",
        )
        .unwrap();
        conn.pragma_update(None, "user_version", 1).unwrap();
    }
    let _store = Store::open(&path).unwrap();
    let conn = raw(&path);
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    let prompt: String = conn
        .query_row("SELECT summary FROM events WHERE id = 1", [], |r| r.get(0))
        .unwrap();
    let detail: String = conn
        .query_row("SELECT detail FROM signals WHERE id = 1", [], |r| r.get(0))
        .unwrap();
    let tool: String = conn
        .query_row("SELECT summary FROM events WHERE id = 2", [], |r| r.get(0))
        .unwrap();
    assert_eq!(version, 4);
    assert_eq!(prompt, digest("Please never touch my keys"));
    assert_eq!(detail, "never");
    assert_eq!(tool, "cargo test");
}

const SCHEMA_ONE: &str = "CREATE TABLE events (id INTEGER PRIMARY KEY, ts INTEGER NOT NULL,
    host TEXT NOT NULL, session_id TEXT NOT NULL, repo TEXT, kind TEXT NOT NULL, tool TEXT,
    summary TEXT NOT NULL, payload_hash TEXT NOT NULL UNIQUE);
    CREATE TABLE signals (id INTEGER PRIMARY KEY, event_id INTEGER NOT NULL,
    ts INTEGER NOT NULL, host TEXT NOT NULL, session_id TEXT NOT NULL,
    kind TEXT NOT NULL, detail TEXT NOT NULL);";

fn file_bytes(path: &std::path::Path) -> Vec<u8> {
    std::fs::read(path).unwrap_or_default()
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

#[test]
fn migration_leaves_no_prompt_text_in_the_file_or_wal() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("events.db");
    let secret = "UNIQUE-SECRET-PROMPT-TEXT never share this";
    {
        let conn = raw(&path);
        conn.pragma_update(None, "journal_mode", "WAL").unwrap();
        conn.execute_batch(SCHEMA_ONE).unwrap();
        for id in 1..=200 {
            conn.execute(
                "INSERT INTO events VALUES (?1, ?1, 'claude', 's', NULL, 'prompt', NULL, ?2, ?3)",
                rusqlite::params![id, format!("{secret} {id}"), format!("h{id}")],
            )
            .unwrap();
        }
        conn.pragma_update(None, "user_version", 1).unwrap();
    }
    let _store = Store::open(&path).unwrap();
    let wal = dir.path().join("events.db-wal");
    let needle = b"UNIQUE-SECRET-PROMPT-TEXT";
    assert!(
        !contains(&file_bytes(&path), needle),
        "text left in the db file"
    );
    assert!(!contains(&file_bytes(&wal), needle), "text left in the WAL");
}

#[test]
fn rerunning_the_migration_keeps_digested_rows() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("events.db");
    let digested = digest("Please never touch my keys");
    {
        let conn = raw(&path);
        conn.execute_batch(SCHEMA_ONE).unwrap();
        conn.execute(
            "INSERT INTO events VALUES (1, 1, 'claude', 's', NULL, 'prompt', NULL, ?1, 'h1')",
            [&digested],
        )
        .unwrap();
        conn.execute_batch(
            "INSERT INTO signals VALUES (1, 1, 1, 'claude', 's', 'user-correction', 'never');",
        )
        .unwrap();
        conn.pragma_update(None, "user_version", 1).unwrap();
    }
    let _store = Store::open(&path).unwrap();
    let conn = raw(&path);
    let prompt: String = conn
        .query_row("SELECT summary FROM events WHERE id = 1", [], |r| r.get(0))
        .unwrap();
    let detail: String = conn
        .query_row("SELECT detail FROM signals WHERE id = 1", [], |r| r.get(0))
        .unwrap();
    assert_eq!(prompt, digested);
    assert_eq!(detail, "never");
}

fn column_names(path: &std::path::Path, table: &str) -> Vec<String> {
    let conn = raw(path);
    let mut stmt = conn
        .prepare(&format!("PRAGMA table_info({table})"))
        .unwrap();
    stmt.query_map([], |row| row.get::<_, String>(1))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap()
}

#[test]
fn recorded_event_stores_its_git_head() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("events.db");
    let store = Store::open(&path).unwrap();
    store
        .record_with_head(&event("s1", "x", 1), &[], Some("abc123"))
        .unwrap();
    let head: Option<String> = raw(&path)
        .query_row("SELECT git_head FROM events", [], |r| r.get(0))
        .unwrap();
    assert_eq!(head.as_deref(), Some("abc123"));
}

#[test]
fn record_without_a_head_stores_null() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("events.db");
    let store = Store::open(&path).unwrap();
    store.record(&event("s1", "x", 1), &[]).unwrap();
    let head: Option<String> = raw(&path)
        .query_row("SELECT git_head FROM events", [], |r| r.get(0))
        .unwrap();
    assert_eq!(head, None);
}

#[test]
fn migration_from_schema_two_adds_rejections_and_keeps_rows() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("events.db");
    {
        let conn = raw(&path);
        conn.execute_batch(SCHEMA_ONE).unwrap();
        conn.execute_batch(
            "INSERT INTO events VALUES (1, 1, 'claude', 's', NULL, 'tool-ok', 'Bash', 'ls', 'h1');",
        )
        .unwrap();
        conn.pragma_update(None, "user_version", 2).unwrap();
    }
    let store = Store::open(&path).unwrap();
    store.reject("abc", 1, "one-off").unwrap();
    assert_eq!(store.rejections().unwrap()[0].reason, "one-off");
    assert_eq!(store.session_history("claude", "s", 10).unwrap().len(), 1);
    let version: i64 = raw(&path)
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(version, 4);
}

#[test]
fn schema_two_database_gains_git_head_column_on_open() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("events.db");
    {
        let conn = raw(&path);
        conn.execute_batch(SCHEMA_ONE).unwrap();
        conn.pragma_update(None, "user_version", 2).unwrap();
    }
    let store = Store::open(&path).unwrap();
    store
        .record_with_head(&event("s1", "x", 1), &[], Some("def456"))
        .unwrap();
    assert!(column_names(&path, "events").contains(&"git_head".to_string()));
    let version: i64 = raw(&path)
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(version, 4);
}

#[test]
fn reopening_does_not_fail_on_existing_git_head_column() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("events.db");
    drop(Store::open(&path).unwrap());
    assert!(Store::open(&path).is_ok());
}
