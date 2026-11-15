use std::collections::BTreeSet;

use anyhow::{bail, Result};

use crate::signal::{
    normalize_command, REPEAT_FAILURE, REVERT, TEST_FAIL_THEN_PASS, TOOL_FAILURE, USER_CORRECTION,
};
use crate::store::{hex_sha256, Decision, Outcome, Rejection, SignalRow, Store, StoredEvent};

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
    /// The accept or reject that deleted earlier occurrences, if any.
    pub decision: Option<Decision>,
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
        self.rejection.is_none()
            && self.decision.is_none()
            && self.sessions() >= self.source.min_sessions()
    }

    pub fn status(&self) -> &'static str {
        if self.rejection.is_some() {
            "rejected"
        } else if self.decision.is_some() {
            "accepted"
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
    cluster_decided(rows, rejections, &[])
}

/// `cluster`, plus each decided cluster. One whose events are all gone is
/// rebuilt from its decision record with no occurrences.
pub fn cluster_decided(
    rows: &[SignalRow],
    rejections: &[Rejection],
    decisions: &[Decision],
) -> Vec<Cluster> {
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
                    decision: decisions.iter().find(|d| d.cluster == id).cloned(),
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
    for decision in decisions {
        if clusters.iter().any(|c| c.id == decision.cluster) {
            continue;
        }
        if let Some(cluster) = from_decision(decision, rejections) {
            clusters.push(cluster);
        }
    }
    clusters.sort_by(|a, b| b.sessions().cmp(&a.sessions()).then(a.id.cmp(&b.id)));
    clusters
}

/// Clusters that event `event_id` brought to their threshold: candidates now,
/// below the threshold without that event's session. A session counts once,
/// so a second occurrence in a counted session reaches nothing.
pub fn reached_threshold(clusters: &[Cluster], event_id: i64) -> Vec<&Cluster> {
    clusters
        .iter()
        .filter(|cluster| cluster.is_candidate())
        .filter(|cluster| {
            let Some(trigger) = cluster.occurrences.iter().find(|o| o.event.id == event_id) else {
                return false;
            };
            let key = (&trigger.event.host, &trigger.event.session_id);
            let before = cluster
                .occurrences
                .iter()
                .filter(|o| o.event.id != event_id)
                .map(|o| (&o.event.host, &o.event.session_id))
                .collect::<BTreeSet<_>>();
            !before.contains(&key) && before.len() < cluster.source.min_sessions()
        })
        .collect()
}

pub fn load(store: &Store) -> Result<Vec<Cluster>> {
    Ok(cluster_decided(
        &store.signals()?,
        &store.rejections()?,
        &store.decisions()?,
    ))
}

/// Records the decision on cluster `id` and deletes its events. The counts
/// come from the live cluster; `fallback` is the proposal's occurrence count
/// for a cluster whose events were already gone.
pub fn decide(
    store: &Store,
    id: &str,
    outcome: Outcome,
    rule: Option<&str>,
    reason: Option<&str>,
    ts: i64,
    fallback: usize,
) -> Result<()> {
    let clusters = load(store)?;
    let live = clusters.iter().find(|c| c.id == id);
    let occurrences = live.map(|c| c.occurrences.as_slice()).unwrap_or(&[]);
    let seen = occurrences.iter().map(|o| o.event.ts);
    let decision = Decision {
        cluster: id.to_owned(),
        ts,
        outcome,
        rule: rule.map(str::to_owned),
        reason: reason.map(str::to_owned),
        occurrences: match live {
            Some(cluster) => cluster.occurrences.len(),
            None => fallback,
        } as i64,
        sessions: live.map_or(0, Cluster::sessions) as i64,
        first_seen: seen.clone().min(),
        last_seen: seen.max(),
        class: live.map(|c| c.class.to_owned()),
        source: live.map(|c| c.source.as_str().to_owned()),
        tool: live.and_then(|c| c.tool.clone()),
        pattern: live.map(|c| c.pattern.clone()),
    };
    let signals: Vec<(i64, Vec<String>)> = occurrences
        .iter()
        .map(|o| (o.event.id, o.signals.clone()))
        .collect();
    store.decide(&decision, &signals)
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

fn from_decision(decision: &Decision, rejections: &[Rejection]) -> Option<Cluster> {
    let class = [CLASS_FAILURE, TEST_FAIL_THEN_PASS, REVERT, USER_CORRECTION]
        .into_iter()
        .find(|class| Some(*class) == decision.class.as_deref())?;
    let source = match decision.source.as_deref()? {
        "mined" => Source::Mined,
        "stated" => Source::Stated,
        _ => return None,
    };
    Some(Cluster {
        id: decision.cluster.clone(),
        class,
        source,
        tool: decision.tool.clone(),
        pattern: decision.pattern.clone()?,
        occurrences: Vec::new(),
        rejection: rejections
            .iter()
            .find(|r| r.cluster == decision.cluster)
            .cloned(),
        decision: Some(decision.clone()),
    })
}

fn classify(row: &SignalRow) -> Option<(&'static str, Source, String)> {
    let command = || normalize_command(&row.event.summary);
    match row.kind.as_str() {
        TOOL_FAILURE | REPEAT_FAILURE => Some((CLASS_FAILURE, Source::Mined, command())),
        TEST_FAIL_THEN_PASS => Some((TEST_FAIL_THEN_PASS, Source::Mined, command())),
        REVERT => Some((REVERT, Source::Mined, command())),
        // The stored prompt text keeps distinct corrections apart; identical
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
