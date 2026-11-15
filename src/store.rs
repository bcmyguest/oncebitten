use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use rusqlite::{params, Connection, OpenFlags};
use sha2::{Digest, Sha256};

use crate::config::{home_dir, windows_app_dir, Platform};
use crate::signal::{self, Signal, DEFAULT_CORRECTION_LEXICON, KIND_PROMPT, USER_CORRECTION};

const APP_DIR: &str = "oncebitten";
const DB_FILE: &str = "events.db";
pub const SUMMARY_LIMIT: usize = 2000;
const SCHEMA_VERSION: i64 = 4;
const PROMPT_DIGEST_PREFIX: &str = "sha256:";
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
    payload_hash TEXT NOT NULL UNIQUE,
    git_head TEXT
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
const REJECTIONS: &str = "
CREATE TABLE IF NOT EXISTS rejections (
    cluster TEXT PRIMARY KEY,
    ts INTEGER NOT NULL,
    reason TEXT NOT NULL
);
";

/// The proposal queue and the triage locks. Created on every open with
/// IF NOT EXISTS, so they need no schema version bump.
const QUEUE: &str = "
CREATE TABLE IF NOT EXISTS proposals (
    cluster TEXT PRIMARY KEY,
    ts INTEGER NOT NULL,
    repo TEXT NOT NULL,
    kind TEXT NOT NULL,
    state TEXT NOT NULL,
    rule TEXT,
    dir TEXT,
    proven INTEGER NOT NULL,
    note TEXT,
    verdict TEXT NOT NULL,
    rationale TEXT,
    occurrences TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS triage_locks (
    cluster TEXT PRIMARY KEY,
    ts INTEGER NOT NULL
);
";
const PROPOSAL_COLUMNS: &str =
    "cluster, ts, repo, kind, state, rule, dir, proven, note, verdict, rationale, occurrences";

/// Provenance the host payload supplies beside the event itself.
/// `tool_use_id` is the host's id for one tool call; redelivery repeats it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Capture {
    pub git_head: Option<String>,
    pub tool_use_id: Option<String>,
}

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
    pub git_head: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignalRow {
    pub kind: String,
    pub detail: String,
    pub event: StoredEvent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rejection {
    pub cluster: String,
    pub ts: i64,
    pub reason: String,
}

/// What a queue row holds: a drafted rule, or a triage finding without one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Proposal,
    Finding,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Proposal => "proposal",
            Kind::Finding => "finding",
        }
    }

    fn parse(text: &str) -> Option<Kind> {
        match text {
            "proposal" => Some(Kind::Proposal),
            "finding" => Some(Kind::Finding),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Waiting,
    Accepted,
    Rejected,
}

impl State {
    pub fn as_str(self) -> &'static str {
        match self {
            State::Waiting => "waiting",
            State::Accepted => "accepted",
            State::Rejected => "rejected",
        }
    }

    fn parse(text: &str) -> Option<State> {
        match text {
            "waiting" => Some(State::Waiting),
            "accepted" => Some(State::Accepted),
            "rejected" => Some(State::Rejected),
            _ => None,
        }
    }
}

/// One queued triage result. `cluster` is the evidence cluster that produced
/// it and doubles as the proposal id; `occurrences` lists its event ids.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProposalRow {
    pub cluster: String,
    pub ts: i64,
    pub repo: String,
    pub kind: Kind,
    pub state: State,
    pub rule: Option<String>,
    pub dir: Option<PathBuf>,
    pub proven: bool,
    pub note: Option<String>,
    pub verdict: String,
    pub rationale: Option<String>,
    pub occurrences: Vec<i64>,
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
        conn.execute_batch(QUEUE)?;
        Ok(Store { conn })
    }

    pub fn record(&self, event: &NewEvent, signals: &[Signal]) -> Result<Option<i64>> {
        self.record_captured(event, signals, &Capture::default())
    }

    /// Stores the event and its signals. A duplicate event (same dedupe key,
    /// see `dedupe_key`) stores nothing and returns `None`.
    pub fn record_captured(
        &self,
        event: &NewEvent,
        signals: &[Signal],
        capture: &Capture,
    ) -> Result<Option<i64>> {
        let event = NewEvent {
            summary: stored_summary(event),
            ..event.clone()
        };
        let tx = self.conn.unchecked_transaction()?;
        let inserted = tx.execute(
            "INSERT OR IGNORE INTO events \
             (ts, host, session_id, repo, kind, tool, summary, payload_hash, git_head) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                event.ts,
                event.host,
                event.session_id,
                event.repo,
                event.kind,
                event.tool,
                event.summary,
                dedupe_key(&event, capture.tool_use_id.as_deref()),
                capture.git_head
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
            "SELECT id, ts, host, session_id, repo, kind, tool, summary, git_head FROM \
             (SELECT * FROM events WHERE host = ?1 AND session_id = ?2 ORDER BY id DESC LIMIT ?3) \
             ORDER BY id",
        )?;
        let rows = stmt.query_map(params![host, session_id, limit as i64], |row| {
            stored_event(row, 0)
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn signals(&self) -> Result<Vec<SignalRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT s.kind, s.detail, e.id, e.ts, e.host, e.session_id, e.repo, e.kind, \
             e.tool, e.summary, e.git_head FROM signals s JOIN events e ON e.id = s.event_id \
             ORDER BY e.id, s.id",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(SignalRow {
                kind: row.get(0)?,
                detail: row.get(1)?,
                event: stored_event(row, 2)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn reject(&self, cluster: &str, ts: i64, reason: &str) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO rejections (cluster, ts, reason) VALUES (?1, ?2, ?3)",
            params![cluster, ts, reason],
        )?;
        Ok(())
    }

    pub fn rejections(&self) -> Result<Vec<Rejection>> {
        let mut stmt = self
            .conn
            .prepare("SELECT cluster, ts, reason FROM rejections ORDER BY cluster")?;
        let rows = stmt.query_map([], |row| {
            Ok(Rejection {
                cluster: row.get(0)?,
                ts: row.get(1)?,
                reason: row.get(2)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Queues a triage result. Returns false when the cluster already has one.
    pub fn insert_proposal(&self, row: &ProposalRow) -> Result<bool> {
        let inserted = self.conn.execute(
            &format!(
                "INSERT OR IGNORE INTO proposals ({PROPOSAL_COLUMNS}) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)"
            ),
            params![
                row.cluster,
                row.ts,
                row.repo,
                row.kind.as_str(),
                row.state.as_str(),
                row.rule,
                row.dir
                    .as_ref()
                    .map(|dir| dir.to_string_lossy().into_owned()),
                row.proven,
                row.note,
                row.verdict,
                row.rationale,
                serde_json::to_string(&row.occurrences)?,
            ],
        )?;
        Ok(inserted == 1)
    }

    pub fn proposal(&self, cluster: &str) -> Result<Option<ProposalRow>> {
        Ok(self
            .query_proposals("WHERE cluster = ?1", &[&cluster])?
            .into_iter()
            .next())
    }

    /// Every queued row, oldest first.
    pub fn proposals(&self) -> Result<Vec<ProposalRow>> {
        self.query_proposals("", &[])
    }

    pub fn set_proposal_state(&self, cluster: &str, state: State) -> Result<()> {
        let changed = self.conn.execute(
            "UPDATE proposals SET state = ?2 WHERE cluster = ?1",
            params![cluster, state.as_str()],
        )?;
        if changed == 0 {
            bail!("no proposal `{cluster}`");
        }
        Ok(())
    }

    /// Takes the triage lock for `cluster`. A lock older than `stale_ms` is
    /// taken over: its holder crashed or outlived any triage timeout.
    pub fn try_lock_triage(&self, cluster: &str, now: i64, stale_ms: i64) -> Result<bool> {
        let tx = self.conn.unchecked_transaction()?;
        let taken = tx.execute(
            "INSERT OR IGNORE INTO triage_locks (cluster, ts) VALUES (?1, ?2)",
            params![cluster, now],
        )? == 1
            || tx.execute(
                "UPDATE triage_locks SET ts = ?2 WHERE cluster = ?1 AND ts <= ?3",
                params![cluster, now, now - stale_ms],
            )? == 1;
        tx.commit()?;
        Ok(taken)
    }

    pub fn unlock_triage(&self, cluster: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM triage_locks WHERE cluster = ?1", [cluster])?;
        Ok(())
    }

    fn query_proposals(
        &self,
        filter: &str,
        args: &[&dyn rusqlite::ToSql],
    ) -> Result<Vec<ProposalRow>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {PROPOSAL_COLUMNS} FROM proposals {filter} ORDER BY ts, cluster"
        ))?;
        let rows = stmt.query_map(args, |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, Option<String>>(6)?,
                row.get::<_, bool>(7)?,
                row.get::<_, Option<String>>(8)?,
                row.get::<_, String>(9)?,
                row.get::<_, Option<String>>(10)?,
                row.get::<_, String>(11)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (cluster, ts, repo, kind, state, rule, dir, proven, note, verdict, rationale, occ) =
                row?;
            out.push(ProposalRow {
                kind: Kind::parse(&kind)
                    .with_context(|| format!("proposal `{cluster}` has unknown kind `{kind}`"))?,
                state: State::parse(&state)
                    .with_context(|| format!("proposal `{cluster}` has unknown state `{state}`"))?,
                occurrences: serde_json::from_str(&occ)
                    .with_context(|| format!("proposal `{cluster}` has unreadable occurrences"))?,
                cluster,
                ts,
                repo,
                rule,
                dir: dir.map(PathBuf::from),
                proven,
                note,
                verdict,
                rationale,
            });
        }
        Ok(out)
    }
}

/// The store at `path` as doctor sees it, read without migrating.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Probe {
    Missing,
    Current,
    /// An older schema; the next capture migrates it.
    Older(i64),
    /// A schema this build does not know: a newer oncebitten wrote it.
    Newer(i64),
}

/// Opens the store read-only and reads its schema version. Never creates or
/// migrates it.
pub fn probe(path: &Path) -> Result<Probe> {
    if !path.exists() {
        return Ok(Probe::Missing);
    }
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .with_context(|| format!("cannot open {}", path.display()))?;
    conn.busy_timeout(BUSY_TIMEOUT)?;
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .with_context(|| format!("cannot read {}", path.display()))?;
    Ok(match version {
        SCHEMA_VERSION => Probe::Current,
        older if older < SCHEMA_VERSION => Probe::Older(older),
        newer => Probe::Newer(newer),
    })
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

fn stored_event(row: &rusqlite::Row, start: usize) -> rusqlite::Result<StoredEvent> {
    Ok(StoredEvent {
        id: row.get(start)?,
        ts: row.get(start + 1)?,
        host: row.get(start + 2)?,
        session_id: row.get(start + 3)?,
        repo: row.get(start + 4)?,
        kind: row.get(start + 5)?,
        tool: row.get(start + 6)?,
        summary: row.get(start + 7)?,
        git_head: row.get(start + 8)?,
    })
}

/// Adds `events.git_head` to schema 1 to 3 databases. Skips the `ALTER` when
/// the column exists, so a retried migration does not fail.
fn add_git_head_column(conn: &Connection) -> Result<()> {
    let mut stmt = conn.prepare("PRAGMA table_info(events)")?;
    let columns: Vec<String> = stmt
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<_>>()?;
    if !columns.iter().any(|name| name == "git_head") {
        conn.execute_batch("ALTER TABLE events ADD COLUMN git_head TEXT")?;
    }
    Ok(())
}

fn migrate(conn: &Connection) -> Result<()> {
    let version: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    match version {
        0 => {
            conn.execute_batch(SCHEMA)?;
            conn.execute_batch(REJECTIONS)?;
            conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
            Ok(())
        }
        1 => {
            redact_prompts(conn)?;
            purge_freed_pages(conn)?;
            conn.execute_batch(REJECTIONS)?;
            add_git_head_column(conn)?;
            // Set last: if the purge fails, the next open redoes both steps,
            // and `redact_prompts` skips rows it already digested.
            conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
            Ok(())
        }
        2 => {
            conn.execute_batch(REJECTIONS)?;
            add_git_head_column(conn)?;
            conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
            Ok(())
        }
        3 => {
            add_git_head_column(conn)?;
            conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
            Ok(())
        }
        SCHEMA_VERSION => Ok(()),
        other => bail!(
            "events database schema version {other} is newer than supported version {SCHEMA_VERSION}"
        ),
    }
}

/// Prompts keep a SHA-256 digest, never their text. Tool summaries keep their
/// command or path, cut to `SUMMARY_LIMIT`.
fn stored_summary(event: &NewEvent) -> String {
    if event.kind == KIND_PROMPT {
        prompt_digest(&event.summary)
    } else {
        truncate_chars(&event.summary, SUMMARY_LIMIT)
    }
}

fn prompt_digest(prompt: &str) -> String {
    format!("{PROMPT_DIGEST_PREFIX}{}", hex_sha256(prompt.as_bytes()))
}

pub(crate) fn hex_sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Schema 1 stored prompt text in `events.summary` and in the detail of
/// `user-correction` signals. This replaces both: the summary becomes a digest,
/// and the detail becomes the lexicon phrases the default list matches.
fn redact_prompts(conn: &Connection) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    let prompts: Vec<(i64, String)> = {
        let mut stmt = tx.prepare("SELECT id, summary FROM events WHERE kind = ?1")?;
        let rows = stmt.query_map([KIND_PROMPT], |row| Ok((row.get(0)?, row.get(1)?)))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    for (id, prompt) in prompts {
        if prompt.starts_with(PROMPT_DIGEST_PREFIX) {
            continue;
        }
        let phrases = signal::matched_phrases(&prompt, DEFAULT_CORRECTION_LEXICON);
        tx.execute(
            "UPDATE events SET summary = ?1 WHERE id = ?2",
            params![prompt_digest(&prompt), id],
        )?;
        tx.execute(
            "UPDATE signals SET detail = ?1 WHERE event_id = ?2 AND kind = ?3",
            params![phrases.join(", "), id, USER_CORRECTION],
        )?;
    }
    tx.commit()?;
    Ok(())
}

/// After `redact_prompts`, the old prompt text still sits in SQLite's freed
/// pages and in the WAL. VACUUM rewrites the file without them, and the
/// TRUNCATE checkpoint empties the WAL.
fn purge_freed_pages(conn: &Connection) -> Result<()> {
    conn.execute_batch("VACUUM")
        .context("cannot VACUUM the events database after redacting prompts")?;
    conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
        row.get::<_, i64>(0)
    })
    .context("cannot checkpoint the events database WAL")
    .and_then(|busy| match busy {
        0 => Ok(()),
        _ => bail!("cannot truncate the events database WAL: another connection holds it"),
    })
}

/// The dedupe key, stored in `events.payload_hash` under a UNIQUE constraint.
///
/// - A host tool-use id keys the event by (host, session, kind, tool use id).
///   A redelivered hook carries the same id, whatever its capture time.
/// - Otherwise the key hashes the whole event: ts, host, session, repo, kind,
///   tool and summary. Two prompts or commands at different times differ by ts.
///
/// `git_head` is not part of either key.
fn dedupe_key(event: &NewEvent, tool_use_id: Option<&str>) -> String {
    match tool_use_id {
        Some(id) => {
            let canonical = serde_json::json!({
                "host": event.host,
                "session_id": event.session_id,
                "kind": event.kind,
                "tool_use_id": id,
            })
            .to_string();
            format!("tool-use:{}", hex_sha256(canonical.as_bytes()))
        }
        None => payload_hash(event),
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
    hex_sha256(canonical.as_bytes())
}

fn truncate_chars(text: &str, limit: usize) -> String {
    text.chars().take(limit).collect()
}
