use std::collections::BTreeSet;

use anyhow::{bail, Result};

use crate::signal::{
    normalize_command, REPEAT_FAILURE, REVERT, TEST_FAIL_THEN_PASS, TOOL_FAILURE, USER_CORRECTION,
};
use crate::store::{hex_sha256, Rejection, SignalRow, Store, StoredEvent};

pub const MINED_MIN_SESSIONS: usize = 2;
pub const STATED_MIN_SESSIONS: usize = 1;
pub const CLASS_FAILURE: &str = "failure";
const ID_LEN: usize = 12;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Mined,
    Stated,
}

impl Source {
    pub fn as_str(self) -> &'static str {
        match self {
            Source::Mined => "mined",
            Source::Stated => "stated",
        }
    }

    pub fn min_sessions(self) -> usize {
        match self {
            Source::Mined => MINED_MIN_SESSIONS,
            Source::Stated => STATED_MIN_SESSIONS,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Occurrence {
    pub event: StoredEvent,
    pub signals: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cluster {
    pub id: String,
    pub class: &'static str,
    pub source: Source,
    pub tool: Option<String>,
    pub pattern: String,
    pub occurrences: Vec<Occurrence>,
    pub rejection: Option<Rejection>,
}

impl Cluster {
    /// Occurrences in one agent session are correlated, so a session counts once.
    pub fn sessions(&self) -> usize {
        self.occurrences
            .iter()
            .map(|o| (&o.event.host, &o.event.session_id))
            .collect::<BTreeSet<_>>()
            .len()
    }

    pub fn repos(&self) -> Vec<&str> {
        self.occurrences
            .iter()
            .filter_map(|o| o.event.repo.as_deref())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    pub fn is_candidate(&self) -> bool {
        self.rejection.is_none() && self.sessions() >= self.source.min_sessions()
    }

    pub fn status(&self) -> &'static str {
        if self.rejection.is_some() {
            "rejected"
        } else if self.is_candidate() {
            "candidate"
        } else {
            "below-threshold"
        }
    }
}

/// Groups signals into mistake classes. Signals on one event form one
/// occurrence; clusters are ordered by independent sessions, most first.
pub fn cluster(rows: &[SignalRow], rejections: &[Rejection]) -> Vec<Cluster> {
    let mut clusters: Vec<Cluster> = Vec::new();
    for row in rows {
        let Some((class, source, discriminator)) = classify(row) else {
            continue;
        };
        let tool = row.event.tool.clone();
        let key = format!(
            "{class}\0{}\0{discriminator}",
            tool.as_deref().unwrap_or("")
        );
        let id = hex_sha256(key.as_bytes())[..ID_LEN].to_string();
        let index = match clusters.iter().position(|c| c.id == id) {
            Some(index) => index,
            None => {
                clusters.push(Cluster {
                    rejection: rejections.iter().find(|r| r.cluster == id).cloned(),
                    id,
                    class,
                    source,
                    tool,
                    pattern: pattern(row),
                    occurrences: Vec::new(),
                });
                clusters.len() - 1
            }
        };
        let occurrences = &mut clusters[index].occurrences;
        match occurrences.last_mut() {
            Some(last) if last.event.id == row.event.id => {
                if !last.signals.contains(&row.kind) {
                    last.signals.push(row.kind.clone());
                }
            }
            _ => occurrences.push(Occurrence {
                event: row.event.clone(),
                signals: vec![row.kind.clone()],
            }),
        }
    }
    clusters.sort_by(|a, b| b.sessions().cmp(&a.sessions()).then(a.id.cmp(&b.id)));
    clusters
}

pub fn load(store: &Store) -> Result<Vec<Cluster>> {
    Ok(cluster(&store.signals()?, &store.rejections()?))
}

pub fn find<'a>(clusters: &'a [Cluster], id: &str) -> Result<&'a Cluster> {
    match clusters.iter().find(|c| c.id == id) {
        Some(found) => Ok(found),
        None => bail!("no evidence cluster `{id}`; see oncebitten evidence list --all"),
    }
}

/// Rejected clusters stay rejected as new occurrences arrive, so triage does
/// not propose the same one-off again.
pub fn reject(store: &Store, id: &str, ts: i64, reason: &str) -> Result<()> {
    if reason.trim().is_empty() {
        bail!("a rejection needs a non-empty reason");
    }
    find(&load(store)?, id)?;
    store.reject(id, ts, reason.trim())
}

fn classify(row: &SignalRow) -> Option<(&'static str, Source, String)> {
    let command = || normalize_command(&row.event.summary);
    match row.kind.as_str() {
        TOOL_FAILURE | REPEAT_FAILURE => Some((CLASS_FAILURE, Source::Mined, command())),
        TEST_FAIL_THEN_PASS => Some((TEST_FAIL_THEN_PASS, Source::Mined, command())),
        REVERT => Some((REVERT, Source::Mined, command())),
        // The prompt digest keeps distinct corrections apart; identical
        // prompts in different sessions still cluster.
        USER_CORRECTION => Some((USER_CORRECTION, Source::Stated, row.event.summary.clone())),
        _ => None,
    }
}

fn pattern(row: &SignalRow) -> String {
    match row.kind.as_str() {
        USER_CORRECTION => row.detail.clone(),
        _ => normalize_command(&row.event.summary),
    }
}
