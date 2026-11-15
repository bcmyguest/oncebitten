use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use rusqlite::{params, Connection};
use sha2::{Digest, Sha256};

use crate::config::{home_dir, windows_app_dir, Platform};
use crate::signal::Signal;

const APP_DIR: &str = "oncebitten";
const DB_FILE: &str = "events.db";
pub const SUMMARY_LIMIT: usize = 2000;
const SCHEMA_VERSION: i64 = 1;
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);
const SCHEMA: &str = "
CREATE TABLE events (
    id INTEGER PRIMARY KEY,
    ts INTEGER NOT NULL,
    host TEXT NOT NULL,
    session_id TEXT NOT NULL,
    repo TEXT,
    kind TEXT NOT NULL,
    tool TEXT,
    summary TEXT NOT NULL,
    payload_hash TEXT NOT NULL UNIQUE
);
CREATE INDEX events_session ON events (host, session_id, id);
CREATE TABLE signals (
    id INTEGER PRIMARY KEY,
    event_id INTEGER NOT NULL REFERENCES events (id),
    ts INTEGER NOT NULL,
    host TEXT NOT NULL,
    session_id TEXT NOT NULL,
    kind TEXT NOT NULL,
    detail TEXT NOT NULL
);
";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewEvent {
    pub ts: i64,
    pub host: String,
    pub session_id: String,
    pub repo: Option<String>,
    pub kind: String,
    pub tool: Option<String>,
    pub summary: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredEvent {
    pub id: i64,
    pub ts: i64,
    pub host: String,
    pub session_id: String,
    pub repo: Option<String>,
    pub kind: String,
    pub tool: Option<String>,
    pub summary: String,
}

pub struct Store {
    conn: Connection,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
        }
        let conn =
            Connection::open(path).with_context(|| format!("cannot open {}", path.display()))?;
        Self::setup(conn)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::setup(Connection::open_in_memory()?)
    }

    fn setup(conn: Connection) -> Result<Self> {
        conn.busy_timeout(BUSY_TIMEOUT)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        migrate(&conn)?;
        Ok(Store { conn })
    }

    pub fn record(&self, event: &NewEvent, signals: &[Signal]) -> Result<Option<i64>> {
        let event = NewEvent {
            summary: truncate_chars(&event.summary, SUMMARY_LIMIT),
            ..event.clone()
        };
        let tx = self.conn.unchecked_transaction()?;
        let inserted = tx.execute(
            "INSERT OR IGNORE INTO events \
             (ts, host, session_id, repo, kind, tool, summary, payload_hash) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                event.ts,
                event.host,
                event.session_id,
                event.repo,
                event.kind,
                event.tool,
                event.summary,
                payload_hash(&event)
            ],
        )?;
        if inserted == 0 {
            return Ok(None);
        }
        let id = tx.last_insert_rowid();
        for signal in signals {
            tx.execute(
                "INSERT INTO signals (event_id, ts, host, session_id, kind, detail) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    id,
                    event.ts,
                    event.host,
                    event.session_id,
                    signal.kind,
                    signal.detail
                ],
            )?;
        }
        tx.commit()?;
        Ok(Some(id))
    }

    pub fn session_history(
        &self,
        host: &str,
        session_id: &str,
        limit: usize,
    ) -> Result<Vec<StoredEvent>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, ts, host, session_id, repo, kind, tool, summary FROM \
             (SELECT * FROM events WHERE host = ?1 AND session_id = ?2 ORDER BY id DESC LIMIT ?3) \
             ORDER BY id",
        )?;
        let rows = stmt.query_map(params![host, session_id, limit as i64], |row| {
            Ok(StoredEvent {
                id: row.get(0)?,
                ts: row.get(1)?,
                host: row.get(2)?,
                session_id: row.get(3)?,
                repo: row.get(4)?,
                kind: row.get(5)?,
                tool: row.get(6)?,
                summary: row.get(7)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }
}

pub fn events_db_path_for(vars: &BTreeMap<String, String>, platform: Platform) -> Result<PathBuf> {
    let base = match vars
        .get("XDG_DATA_HOME")
        .filter(|dir| platform.is_absolute(dir))
    {
        Some(dir) => PathBuf::from(dir),
        None => match platform {
            Platform::Windows => windows_app_dir(vars, "LOCALAPPDATA", "Local").context(
                "none of XDG_DATA_HOME, LOCALAPPDATA or a home directory is an absolute path",
            )?,
            Platform::Unix => home_dir(vars, platform)
                .context("neither XDG_DATA_HOME nor HOME is an absolute path")?
                .join(".local")
                .join("share"),
        },
    };
    Ok(base.join(APP_DIR).join(DB_FILE))
}

pub fn events_db_path() -> Result<PathBuf> {
    let vars = [
        "XDG_DATA_HOME",
        "HOME",
        "LOCALAPPDATA",
        "USERPROFILE",
        "HOMEDRIVE",
        "HOMEPATH",
    ]
    .into_iter()
    .filter_map(|key| {
        std::env::var(key)
            .ok()
            .map(|value| (key.to_string(), value))
    })
    .collect();
    events_db_path_for(&vars, Platform::current())
}

fn migrate(conn: &Connection) -> Result<()> {
    let version: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    match version {
        0 => {
            conn.execute_batch(SCHEMA)?;
            conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
            Ok(())
        }
        SCHEMA_VERSION => Ok(()),
        other => bail!(
            "events database schema version {other} is newer than supported version {SCHEMA_VERSION}"
        ),
    }
}

fn payload_hash(event: &NewEvent) -> String {
    let canonical = serde_json::json!({
        "ts": event.ts,
        "host": event.host,
        "session_id": event.session_id,
        "repo": event.repo,
        "kind": event.kind,
        "tool": event.tool,
        "summary": event.summary,
    })
    .to_string();
    Sha256::digest(canonical.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn truncate_chars(text: &str, limit: usize) -> String {
    text.chars().take(limit).collect()
}
