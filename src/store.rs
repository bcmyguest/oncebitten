use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use jiff::tz::TimeZone;
use jiff::Timestamp;
use rusqlite::{
    params, Connection, OpenFlags, OptionalExtension, Transaction, TransactionBehavior,
};
use sha2::{Digest, Sha256};

use crate::config::{home_dir, windows_app_dir, Platform};
use crate::signal::{self, Signal, DEFAULT_CORRECTION_LEXICON, KIND_PROMPT, USER_CORRECTION};

const APP_DIR: &str = "oncebitten";
const DB_FILE: &str = "events.db";
pub const SUMMARY_LIMIT: usize = 2000;
const SCHEMA_VERSION: i64 = 4;
const PROMPT_DIGEST_PREFIX: &str = "sha256:";
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);
/// Two hook scopes deliver one prompt milliseconds apart; a person cannot submit the same prompt twice in one session this fast.
const PROMPT_REDELIVERY_WINDOW_MS: i64 = 2000;
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
CREATE TABLE IF NOT EXISTS decisions (
    cluster TEXT PRIMARY KEY,
    ts INTEGER NOT NULL,
    outcome TEXT NOT NULL,
    rule TEXT,
    reason TEXT,
    occurrences INTEGER NOT NULL,
    sessions INTEGER NOT NULL,
    first_seen INTEGER,
    last_seen INTEGER,
    class TEXT,
    source TEXT,
    tool TEXT,
    pattern TEXT
);
CREATE TABLE IF NOT EXISTS triage_runs (
    id INTEGER PRIMARY KEY,
    ts INTEGER NOT NULL,
    cluster TEXT NOT NULL,
    host TEXT,
    outcome TEXT NOT NULL
);
";
/// Columns `proposals deliver` fills. Databases created before delivery lack
/// them; `add_delivery_columns` adds each missing one on open.
const DELIVERY_COLUMNS: [&str; 3] = ["target", "commit_sha", "pr_url"];
/// Columns `proposals list` and `show` read the baseline hits from. Databases
/// created before they existed get each one on open, as `add_hits_columns`.
const HITS_COLUMNS: [(&str, &str); 3] = [
    ("hits_count", "INTEGER"),
    ("hits_files", "INTEGER"),
    ("hits_samples", "TEXT"),
];
/// Duplicate marks on `proposals`: the verdict's class, the record-time
/// possible-duplicate note, and the link a user confirmed with `reject`.
/// `add_mark_columns` adds each missing one on open.
const MARK_COLUMNS: [&str; 3] = ["mistake_class", "possible_duplicate", "duplicate_of"];
/// Indexes for expiry. Created by `Store::expire`, never by `open`, so the
/// gate's open builds no index on a large store.
const EXPIRY_INDEXES: &str = "
CREATE INDEX IF NOT EXISTS events_ts ON events (ts);
CREATE INDEX IF NOT EXISTS signals_event ON signals (event_id);
";
/// Event ids a waiting proposal names; expiry keeps them until the decision.
const WAITING_EVENTS: &str = "SELECT j.value FROM proposals p, json_each(p.occurrences) j \
     WHERE p.state = 'waiting'";
const PROPOSAL_COLUMNS: &str =
    "cluster, ts, repo, kind, state, rule, dir, proven, note, verdict, rationale, occurrences";
const DECISION_COLUMNS: &str = "cluster, ts, outcome, rule, reason, occurrences, sessions, \
     first_seen, last_seen, class, source, tool, pattern";

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

/// What a `triage_runs` row records. `Started` spends one run of the daily
/// budget; `SkippedBudget` marks a capture that spawned nothing; `SpawnFailed`
/// marks a run that never started and so spends nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TriageOutcome {
    Started,
    SkippedBudget,
    SpawnFailed,
}

impl TriageOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            TriageOutcome::Started => "started",
            TriageOutcome::SkippedBudget => "skipped: budget",
            TriageOutcome::SpawnFailed => "spawn-failed",
        }
    }
}

/// Prefix of the `triage_runs.outcome` a failed background run writes; the
/// error text follows it.
pub const TRIAGE_FAILED_PREFIX: &str = "failed: ";
/// Longest error text stored with a failed run, in characters.
const TRIAGE_ERROR_MAX_CHARS: usize = 300;

/// A background triage run that failed, and the error it ended with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TriageFailure {
    pub cluster: String,
    pub ts: i64,
    pub host: Option<String>,
    pub error: String,
}

/// A `triage_runs` row, as read back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TriageRun {
    pub cluster: String,
    pub ts: i64,
    pub host: Option<String>,
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

/// How a proposal was decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Accepted,
    Rejected,
}

impl Outcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Outcome::Accepted => "accepted",
            Outcome::Rejected => "rejected",
        }
    }

    fn parse(text: &str) -> Option<Outcome> {
        match text {
            "accepted" => Some(Outcome::Accepted),
            "rejected" => Some(Outcome::Rejected),
            _ => None,
        }
    }
}

/// What stays of a cluster once a decision deletes its events: the counts,
/// the time span and enough of the cluster key to list it. `class`, `source`
/// and `pattern` are `None` when the events were gone before the decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decision {
    pub cluster: String,
    pub ts: i64,
    pub outcome: Outcome,
    pub rule: Option<String>,
    pub reason: Option<String>,
    pub occurrences: i64,
    pub sessions: i64,
    pub first_seen: Option<i64>,
    pub last_seen: Option<i64>,
    pub class: Option<String>,
    pub source: Option<String>,
    pub tool: Option<String>,
    pub pattern: Option<String>,
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
    /// Where `proposals deliver` last sent the rule: `pr` or `rules-repo`.
    pub target: Option<String>,
    /// The commit that delivery made.
    pub commit: Option<String>,
    pub pr_url: Option<String>,
    /// How often the rule fires on the repo at HEAD. `None` until measured.
    pub hits: Option<HitsRow>,
}

/// The duplicate marks a proposal row holds.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ProposalMarks {
    pub mistake_class: Option<String>,
    pub possible_duplicate: Option<String>,
    pub duplicate_of: Option<String>,
}

/// The fields of a queued row that the duplicate check reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarkCandidate {
    pub cluster: String,
    pub mistake_class: Option<String>,
    pub rationale: Option<String>,
}

/// What `proposals deliver` did, as the store keeps it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryRow {
    pub target: String,
    pub commit: String,
    pub pr_url: Option<String>,
}

/// How often a proposed rule fires on the repo at HEAD: `count` hits in
/// `files` files, with up to three `path:line: text` samples.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HitsRow {
    pub count: i64,
    pub files: i64,
    pub samples: Vec<String>,
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

    /// Opens an existing store for reads. Runs no migration and no DDL; the
    /// caller checks the schema version first with `probe`.
    pub fn open_read_only(path: &Path) -> Result<Self> {
        let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .with_context(|| format!("cannot open {}", path.display()))?;
        conn.busy_timeout(BUSY_TIMEOUT)?;
        Ok(Store { conn })
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::setup(Connection::open_in_memory()?)
    }

    fn setup(conn: Connection) -> Result<Self> {
        conn.busy_timeout(BUSY_TIMEOUT)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        migrate(&conn)?;
        conn.execute_batch(QUEUE)?;
        add_delivery_columns(&conn)?;
        add_hits_columns(&conn)?;
        add_mark_columns(&conn)?;
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
            summary: stored_summary(event, signals),
            ..event.clone()
        };
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        if event.kind == KIND_PROMPT
            && capture.tool_use_id.is_none()
            && is_prompt_redelivery(&tx, &event)?
        {
            return Ok(None);
        }
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

    /// The paths the session's edit tools named, as the hook payloads gave them.
    /// A patch tool's summary lists one path per line.
    pub fn session_edit_paths(&self, host: &str, session_id: &str) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT tool, summary FROM events \
             WHERE host = ?1 AND session_id = ?2 AND tool IS NOT NULL",
        )?;
        let rows = stmt.query_map(params![host, session_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut paths = Vec::new();
        for row in rows {
            let (tool, summary) = row?;
            if crate::host::is_edit_tool(&tool) {
                paths.push(summary);
            } else if crate::host::is_patch_tool(&tool) {
                paths.extend(
                    summary
                        .lines()
                        .filter(|line| *line != tool)
                        .map(str::to_string),
                );
            }
        }
        Ok(paths)
    }

    /// Successful shell calls recorded in `repo`, newest first, without the
    /// command `exclude`. A steer draft takes its corpus from these.
    pub fn successful_commands(
        &self,
        repo: &str,
        exclude: &str,
        limit: usize,
    ) -> Result<Vec<StoredEvent>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, ts, host, session_id, repo, kind, tool, summary, git_head FROM events \
             WHERE repo IS NOT NULL AND kind = ?1 AND summary != ?2 ORDER BY id DESC",
        )?;
        let rows = stmt.query_map(params![signal::KIND_TOOL_OK, exclude], |row| {
            stored_event(row, 0)
        })?;
        let wanted = normalised_repo(repo);
        let mut same: HashMap<String, bool> = HashMap::new();
        let mut found = Vec::new();
        for row in rows {
            let event = row?;
            let Some(event_repo) = event.repo.as_deref() else {
                continue;
            };
            let in_repo = *same
                .entry(event_repo.to_owned())
                .or_insert_with(|| normalised_repo(event_repo) == wanted);
            if in_repo
                && event
                    .tool
                    .as_deref()
                    .is_some_and(crate::host::is_command_tool)
            {
                found.push(event);
                if found.len() == limit {
                    break;
                }
            }
        }
        Ok(found)
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

    /// Records `decision` and deletes the evidence behind it. `occurrences`
    /// pairs each event id with the signal kinds the cluster holds on it;
    /// those signals go, and so does any event left without a signal. A
    /// cluster decided again adds the new counts to its record.
    pub fn decide(&self, decision: &Decision, occurrences: &[(i64, Vec<String>)]) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        for (event, kinds) in occurrences {
            for kind in kinds {
                tx.execute(
                    "DELETE FROM signals WHERE event_id = ?1 AND kind = ?2",
                    params![event, kind],
                )?;
            }
            tx.execute(
                "DELETE FROM events WHERE id = ?1 \
                 AND NOT EXISTS (SELECT 1 FROM signals WHERE event_id = ?1)",
                [event],
            )?;
        }
        tx.execute(
            &format!(
                "INSERT INTO decisions ({DECISION_COLUMNS}) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13) \
                 ON CONFLICT (cluster) DO UPDATE SET ts = excluded.ts, \
                 outcome = excluded.outcome, rule = coalesce(excluded.rule, rule), \
                 reason = excluded.reason, occurrences = occurrences + excluded.occurrences, \
                 sessions = sessions + excluded.sessions, \
                 first_seen = coalesce(min(first_seen, excluded.first_seen), first_seen, \
                 excluded.first_seen), \
                 last_seen = coalesce(max(last_seen, excluded.last_seen), last_seen, \
                 excluded.last_seen), \
                 class = coalesce(class, excluded.class), \
                 source = coalesce(source, excluded.source), \
                 tool = coalesce(tool, excluded.tool), \
                 pattern = coalesce(pattern, excluded.pattern)"
            ),
            params![
                decision.cluster,
                decision.ts,
                decision.outcome.as_str(),
                decision.rule,
                decision.reason,
                decision.occurrences,
                decision.sessions,
                decision.first_seen,
                decision.last_seen,
                decision.class,
                decision.source,
                decision.tool,
                decision.pattern,
            ],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn decision(&self, cluster: &str) -> Result<Option<Decision>> {
        Ok(self
            .query_decisions("WHERE cluster = ?1", &[&cluster])?
            .into_iter()
            .next())
    }

    /// Every decision, by cluster id.
    pub fn decisions(&self) -> Result<Vec<Decision>> {
        self.query_decisions("", &[])
    }

    /// Deletes events captured before `before` (ms) and their signals, except
    /// those a waiting proposal names. Returns the number of events deleted.
    pub fn expire(&self, before: i64) -> Result<usize> {
        self.conn.execute_batch(EXPIRY_INDEXES)?;
        let tx = self.conn.unchecked_transaction()?;
        let expired =
            format!("SELECT id FROM events WHERE ts < ?1 AND id NOT IN ({WAITING_EVENTS})");
        tx.execute(
            &format!("DELETE FROM signals WHERE event_id IN ({expired})"),
            [before],
        )?;
        let deleted = tx.execute(
            &format!("DELETE FROM events WHERE id IN ({expired})"),
            [before],
        )?;
        tx.commit()?;
        Ok(deleted)
    }

    /// Queues a triage result. Returns false when the cluster already has one.
    pub fn insert_proposal(&self, row: &ProposalRow) -> Result<bool> {
        let inserted = self.conn.execute(
            &format!(
                "INSERT OR IGNORE INTO proposals ({PROPOSAL_COLUMNS}, hits_count, hits_files, hits_samples) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)"
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
                row.hits.as_ref().map(|hits| hits.count),
                row.hits.as_ref().map(|hits| hits.files),
                row.hits
                    .as_ref()
                    .map(|hits| serde_json::to_string(&hits.samples))
                    .transpose()?,
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

    /// Stores the class and the record-time mark. `None` clears a field.
    pub fn set_proposal_marks(
        &self,
        cluster: &str,
        mistake_class: Option<&str>,
        possible_duplicate: Option<&str>,
    ) -> Result<()> {
        let changed = self.conn.execute(
            "UPDATE proposals SET mistake_class = ?2, possible_duplicate = ?3 WHERE cluster = ?1",
            params![cluster, mistake_class, possible_duplicate],
        )?;
        if changed == 0 {
            bail!("no proposal `{cluster}`");
        }
        Ok(())
    }

    /// Records the duplicate link a user confirmed.
    pub fn set_duplicate_of(&self, cluster: &str, other: &str) -> Result<()> {
        let changed = self.conn.execute(
            "UPDATE proposals SET duplicate_of = ?2 WHERE cluster = ?1",
            params![cluster, other],
        )?;
        if changed == 0 {
            bail!("no proposal `{cluster}`");
        }
        Ok(())
    }

    pub fn proposal_marks(&self, cluster: &str) -> Result<Option<ProposalMarks>> {
        let mut stmt = self.conn.prepare(
            "SELECT mistake_class, possible_duplicate, duplicate_of FROM proposals WHERE cluster = ?1",
        )?;
        let mut rows = stmt.query_map(params![cluster], |row| {
            Ok(ProposalMarks {
                mistake_class: row.get(0)?,
                possible_duplicate: row.get(1)?,
                duplicate_of: row.get(2)?,
            })
        })?;
        Ok(rows.next().transpose()?)
    }

    /// Every queued row's class and rationale, oldest first. The duplicate
    /// check compares a new proposal against these.
    pub fn mark_candidates(&self) -> Result<Vec<MarkCandidate>> {
        let mut stmt = self.conn.prepare(
            "SELECT cluster, mistake_class, rationale FROM proposals ORDER BY ts, cluster",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(MarkCandidate {
                cluster: row.get(0)?,
                mistake_class: row.get(1)?,
                rationale: row.get(2)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
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

    /// Records the proof state a delivery replay left.
    pub fn set_proposal_proof(
        &self,
        cluster: &str,
        proven: bool,
        note: Option<&str>,
    ) -> Result<()> {
        let changed = self.conn.execute(
            "UPDATE proposals SET proven = ?2, note = ?3 WHERE cluster = ?1",
            params![cluster, proven, note],
        )?;
        if changed == 0 {
            bail!("no proposal `{cluster}`");
        }
        Ok(())
    }

    /// Records how often the proposed rule fires on the repo at HEAD.
    pub fn set_proposal_hits(&self, cluster: &str, hits: &HitsRow) -> Result<()> {
        let samples = serde_json::to_string(&hits.samples)?;
        let changed = self.conn.execute(
            "UPDATE proposals SET hits_count = ?2, hits_files = ?3, hits_samples = ?4 WHERE cluster = ?1",
            params![cluster, hits.count, hits.files, samples],
        )?;
        if changed == 0 {
            bail!("no proposal `{cluster}`");
        }
        Ok(())
    }

    /// Records where `proposals deliver` sent the rule.
    pub fn set_proposal_delivery(&self, cluster: &str, delivery: &DeliveryRow) -> Result<()> {
        let changed = self.conn.execute(
            "UPDATE proposals SET target = ?2, commit_sha = ?3, pr_url = ?4 WHERE cluster = ?1",
            params![cluster, delivery.target, delivery.commit, delivery.pr_url],
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

    pub fn record_triage(
        &self,
        cluster: &str,
        host: Option<&str>,
        ts: i64,
        outcome: TriageOutcome,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO triage_runs (ts, cluster, host, outcome) VALUES (?1, ?2, ?3, ?4)",
            params![ts, cluster, host, outcome.as_str()],
        )?;
        Ok(())
    }

    /// Records that a triage run of `cluster` failed with `error`, on one line
    /// cut to [`TRIAGE_ERROR_MAX_CHARS`]. This is a row of its own: the run's
    /// `started` row stays, so the failure still spends the daily budget.
    pub fn record_triage_failure(
        &self,
        cluster: &str,
        host: Option<&str>,
        ts: i64,
        error: &str,
    ) -> Result<()> {
        let line = error.split_whitespace().collect::<Vec<_>>().join(" ");
        let text: String = line.chars().take(TRIAGE_ERROR_MAX_CHARS).collect();
        self.conn.execute(
            "INSERT INTO triage_runs (ts, cluster, host, outcome) VALUES (?1, ?2, ?3, ?4)",
            params![ts, cluster, host, format!("{TRIAGE_FAILED_PREFIX}{text}")],
        )?;
        Ok(())
    }

    /// Clusters whose latest `triage_runs` row is a failure and that have no
    /// queued proposal, oldest first. Any later run of the cluster drops it.
    pub fn triage_failures(&self) -> Result<Vec<TriageFailure>> {
        let mut stmt = self.conn.prepare(
            "SELECT t.cluster, t.ts, t.host, t.outcome FROM triage_runs t \
             WHERE substr(t.outcome, 1, ?1) = ?2 AND t.id = (\
               SELECT MAX(id) FROM triage_runs WHERE cluster = t.cluster) \
             AND NOT EXISTS (SELECT 1 FROM proposals p WHERE p.cluster = t.cluster) \
             ORDER BY t.ts, t.id",
        )?;
        let prefix = TRIAGE_FAILED_PREFIX.len();
        let rows = stmt.query_map(params![prefix as i64, TRIAGE_FAILED_PREFIX], |row| {
            let outcome: String = row.get(3)?;
            Ok(TriageFailure {
                cluster: row.get(0)?,
                ts: row.get(1)?,
                host: row.get(2)?,
                error: outcome[prefix..].to_string(),
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Records a `Started` run for `cluster` when fewer than `max` runs started at
    /// or after `since` (ms), counting and inserting in one IMMEDIATE transaction
    /// so concurrent captures cannot overspend. Returns the row id, or `None`
    /// when the budget is spent.
    pub fn claim_triage_run(
        &self,
        cluster: &str,
        host: Option<&str>,
        ts: i64,
        since: i64,
        max: u32,
    ) -> Result<Option<i64>> {
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        let spent: i64 = tx.query_row(
            "SELECT COUNT(*) FROM triage_runs WHERE outcome = ?1 AND ts >= ?2",
            params![TriageOutcome::Started.as_str(), since],
            |row| row.get(0),
        )?;
        if !within_budget(spent, max) {
            return Ok(None);
        }
        tx.execute(
            "INSERT INTO triage_runs (ts, cluster, host, outcome) VALUES (?1, ?2, ?3, ?4)",
            params![ts, cluster, host, TriageOutcome::Started.as_str()],
        )?;
        let id = tx.last_insert_rowid();
        tx.commit()?;
        Ok(Some(id))
    }

    /// Rewrites the outcome of the `triage_runs` row `id`.
    pub fn set_triage_outcome(&self, id: i64, outcome: TriageOutcome) -> Result<()> {
        self.conn.execute(
            "UPDATE triage_runs SET outcome = ?2 WHERE id = ?1",
            params![id, outcome.as_str()],
        )?;
        Ok(())
    }

    /// Triage runs started at or after `since` (ms). Skipped rows do not count.
    pub fn triage_runs_since(&self, since: i64) -> Result<i64> {
        Ok(self.conn.query_row(
            "SELECT COUNT(*) FROM triage_runs WHERE outcome = ?1 AND ts >= ?2",
            params![TriageOutcome::Started.as_str(), since],
            |row| row.get(0),
        )?)
    }

    /// The repo an event was captured in, or `None` for an unknown id or an
    /// event captured outside a repo.
    pub fn event_repo(&self, id: i64) -> Result<Option<String>> {
        let repo = self
            .conn
            .query_row(
                "SELECT repo FROM events WHERE id = ?1",
                params![id],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()?;
        Ok(repo.flatten())
    }

    /// Clusters whose latest `triage_runs` row is `skipped: budget`, oldest
    /// first. Rows are filtered on their latest outcome, so a later run of the
    /// same cluster drops it from this list.
    pub fn budget_skipped(&self) -> Result<Vec<TriageRun>> {
        let mut stmt = self.conn.prepare(
            "SELECT t.cluster, t.ts, t.host FROM triage_runs t \
             WHERE t.outcome = ?1 AND t.id = (\
               SELECT MAX(id) FROM triage_runs WHERE cluster = t.cluster) \
             ORDER BY t.ts, t.id",
        )?;
        let rows = stmt.query_map(params![TriageOutcome::SkippedBudget.as_str()], |row| {
            Ok(TriageRun {
                cluster: row.get(0)?,
                ts: row.get(1)?,
                host: row.get(2)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    fn query_decisions(
        &self,
        filter: &str,
        args: &[&dyn rusqlite::ToSql],
    ) -> Result<Vec<Decision>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {DECISION_COLUMNS} FROM decisions {filter} ORDER BY cluster"
        ))?;
        let rows = stmt.query_map(args, |row| {
            Ok((
                row.get::<_, String>(2)?,
                Decision {
                    cluster: row.get(0)?,
                    ts: row.get(1)?,
                    outcome: Outcome::Accepted,
                    rule: row.get(3)?,
                    reason: row.get(4)?,
                    occurrences: row.get(5)?,
                    sessions: row.get(6)?,
                    first_seen: row.get(7)?,
                    last_seen: row.get(8)?,
                    class: row.get(9)?,
                    source: row.get(10)?,
                    tool: row.get(11)?,
                    pattern: row.get(12)?,
                },
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (outcome, mut decision) = row?;
            decision.outcome = Outcome::parse(&outcome).with_context(|| {
                format!(
                    "decision `{}` has unknown outcome `{outcome}`",
                    decision.cluster
                )
            })?;
            out.push(decision);
        }
        Ok(out)
    }

    fn query_proposals(
        &self,
        filter: &str,
        args: &[&dyn rusqlite::ToSql],
    ) -> Result<Vec<ProposalRow>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {PROPOSAL_COLUMNS}, target, commit_sha, pr_url, hits_count, hits_files, hits_samples FROM proposals {filter} ORDER BY ts, cluster"
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
                row.get::<_, Option<String>>(12)?,
                row.get::<_, Option<String>>(13)?,
                row.get::<_, Option<String>>(14)?,
                row.get::<_, Option<i64>>(15)?,
                row.get::<_, Option<i64>>(16)?,
                row.get::<_, Option<String>>(17)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (
                cluster,
                ts,
                repo,
                kind,
                state,
                rule,
                dir,
                proven,
                note,
                verdict,
                rationale,
                occ,
                target,
                commit,
                pr_url,
                hits_count,
                hits_files,
                hits_samples,
            ) = row?;
            let hits = match (hits_count, hits_samples) {
                (Some(count), Some(samples)) => Some(HitsRow {
                    count,
                    files: hits_files.unwrap_or(0),
                    samples: serde_json::from_str(&samples)
                        .with_context(|| format!("proposal `{cluster}` has unreadable hits"))?,
                }),
                _ => None,
            };
            out.push(ProposalRow {
                kind: Kind::parse(&kind)
                    .with_context(|| format!("proposal `{cluster}` has unknown kind `{kind}`"))?,
                state: State::parse(&state)
                    .with_context(|| format!("proposal `{cluster}` has unknown state `{state}`"))?,
                occurrences: serde_json::from_str(&occ)
                    .with_context(|| format!("proposal `{cluster}` has unreadable occurrences"))?,
                hits,
                cluster,
                ts,
                repo,
                rule,
                dir: dir.map(PathBuf::from),
                proven,
                note,
                verdict,
                rationale,
                target,
                commit,
                pr_url,
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

/// The start of the calendar day holding `now_ms`, in `zone`, as unix ms.
pub fn day_start_ms(now_ms: i64, zone: &TimeZone) -> Result<i64> {
    let now = Timestamp::from_millisecond(now_ms).context("timestamp out of range")?;
    let date = now.to_zoned(zone.clone()).date();
    let midnight = date.at(0, 0, 0, 0).to_zoned(zone.clone())?;
    Ok(midnight.timestamp().as_millisecond())
}

/// Whether `spent` triage runs leave room under the daily `max`.
pub fn within_budget(spent: i64, max: u32) -> bool {
    spent < i64::from(max)
}

/// `day_start_ms` in the system's local zone.
pub fn local_day_start_ms(now_ms: i64) -> Result<i64> {
    day_start_ms(now_ms, &TimeZone::system())
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

/// `repo` in one comparable form: canonical when the path exists, which
/// resolves symlinks such as macOS `/var` -> `/private/var`, and without the
/// Windows verbatim `\\?\` prefix either way.
fn normalised_repo(repo: &str) -> PathBuf {
    let path = Path::new(repo);
    match dunce::canonicalize(path) {
        Ok(canonical) => canonical,
        Err(_) => PathBuf::from(strip_verbatim(repo)),
    }
}

/// Drops a Windows verbatim prefix: `\\?\UNC\server` -> `\\server` and
/// `\\?\C:\dir` -> `C:\dir`.
fn strip_verbatim(repo: &str) -> String {
    if let Some(rest) = repo.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else {
        repo.strip_prefix(r"\\?\").unwrap_or(repo).to_owned()
    }
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

fn add_mark_columns(conn: &Connection) -> Result<()> {
    let mut stmt = conn.prepare("PRAGMA table_info(proposals)")?;
    let columns: Vec<String> = stmt
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<_>>()?;
    for name in MARK_COLUMNS {
        if !columns.iter().any(|column| column == name) {
            conn.execute_batch(&format!("ALTER TABLE proposals ADD COLUMN {name} TEXT"))?;
        }
    }
    Ok(())
}

fn add_delivery_columns(conn: &Connection) -> Result<()> {
    let mut stmt = conn.prepare("PRAGMA table_info(proposals)")?;
    let columns: Vec<String> = stmt
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<_>>()?;
    for name in DELIVERY_COLUMNS {
        if !columns.iter().any(|column| column == name) {
            conn.execute_batch(&format!("ALTER TABLE proposals ADD COLUMN {name} TEXT"))?;
        }
    }
    Ok(())
}

fn add_hits_columns(conn: &Connection) -> Result<()> {
    let mut stmt = conn.prepare("PRAGMA table_info(proposals)")?;
    let columns: Vec<String> = stmt
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<_>>()?;
    for (name, kind) in HITS_COLUMNS {
        if !columns.iter().any(|column| column == name) {
            conn.execute_batch(&format!("ALTER TABLE proposals ADD COLUMN {name} {kind}"))?;
        }
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

/// A prompt with a `user-correction` signal keeps its text, cut to
/// `SUMMARY_LIMIT`, because triage drafts the rule from what the user said.
/// Any other prompt keeps only a SHA-256 digest. Tool summaries keep their
/// command or path, cut to `SUMMARY_LIMIT`.
fn stored_summary(event: &NewEvent, signals: &[Signal]) -> String {
    let correction = signals.iter().any(|s| s.kind == USER_CORRECTION);
    if event.kind == KIND_PROMPT && !correction {
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

fn is_prompt_redelivery(conn: &Connection, event: &NewEvent) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM events \
         WHERE host = ?1 AND session_id = ?2 AND kind = ?3 AND summary = ?4 \
         AND ts BETWEEN ?5 AND ?6)",
        params![
            event.host,
            event.session_id,
            event.kind,
            event.summary,
            event.ts - PROMPT_REDELIVERY_WINDOW_MS,
            event.ts + PROMPT_REDELIVERY_WINDOW_MS
        ],
        |row| row.get(0),
    )?)
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
