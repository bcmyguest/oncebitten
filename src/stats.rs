use std::collections::BTreeMap;

use anyhow::Result;
use serde::Serialize;

use crate::evidence::{self, Cluster, CLASS_FAILURE};
use crate::signal::{REPEAT_FAILURE, REVERT, TEST_FAIL_THEN_PASS, TOOL_FAILURE, USER_CORRECTION};
use crate::store::{Decision, Kind, Rejection, SignalRow, State, Store};

pub const NO_DATA: &str = "no data: no evidence store yet; the first capture creates it";
const DAY_MS: i64 = 24 * 60 * 60 * 1000;
const NOT_RECORDED: &str = "n/a";
const UNKNOWN_CLASS: &str = "unknown";
pub const VERSION_DIFFERS: &str =
    "store version differs from this build; stats left the store unchanged";
/// Funnel rows: a mistake class and the signal kinds that feed it. Drafts whose
/// cluster no longer has evidence (expired by TTL) fall under `UNKNOWN_CLASS`.
const ROWS: [(&str, &[&str]); 5] = [
    (CLASS_FAILURE, &[TOOL_FAILURE, REPEAT_FAILURE]),
    (TEST_FAIL_THEN_PASS, &[TEST_FAIL_THEN_PASS]),
    (REVERT, &[REVERT]),
    (USER_CORRECTION, &[USER_CORRECTION]),
    (UNKNOWN_CLASS, &[]),
];

/// One funnel row. `triage_runs` is `None`: the funnel does not read the
/// `triage_runs` table yet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Row {
    pub kind: &'static str,
    pub signals_captured: usize,
    pub clusters_at_threshold: usize,
    pub triage_runs: Option<usize>,
    pub drafts: usize,
    pub proven: usize,
    pub accepted: usize,
    pub rejected: usize,
    pub waiting: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Report {
    pub since_days: Option<u32>,
    pub kinds: Vec<Row>,
}

impl Row {
    fn new(kind: &'static str) -> Self {
        Row {
            kind,
            signals_captured: 0,
            clusters_at_threshold: 0,
            triage_runs: None,
            drafts: 0,
            proven: 0,
            accepted: 0,
            rejected: 0,
            waiting: 0,
        }
    }
}

/// The report with every count at zero, for a store that does not exist yet.
pub fn empty(since_days: Option<u32>) -> Report {
    Report {
        since_days,
        kinds: ROWS.iter().map(|(class, _)| Row::new(class)).collect(),
    }
}

/// The funnel row index for a signal kind or a mistake class.
fn row_index(kind: &str) -> Option<usize> {
    ROWS.iter()
        .position(|(class, kinds)| *class == kind || kinds.contains(&kind))
}

/// A cluster reached the threshold when its sessions meet the source's minimum.
/// A decided cluster has lost its events, so its decision record holds the count.
fn reached(cluster: &Cluster) -> bool {
    let sessions = cluster
        .decision
        .as_ref()
        .map_or(cluster.sessions(), |decision| decision.sessions as usize);
    sessions >= cluster.source.min_sessions()
}

/// Counts the funnel per mistake class. `since_days` keeps records whose
/// timestamp falls within that many days before `now` (milliseconds).
/// Accepting or rejecting a cluster deletes its signals, so their count drops.
pub fn report(store: &Store, since_days: Option<u32>, now: i64) -> Result<Report> {
    let cutoff = since_days.map(|days| now.saturating_sub(i64::from(days).saturating_mul(DAY_MS)));
    let within = |ts: i64| cutoff.is_none_or(|cutoff| ts >= cutoff);

    let signals = store.signals()?;
    let rejections = store.rejections()?;
    let decisions = store.decisions()?;
    let classes: BTreeMap<String, &'static str> =
        evidence::cluster_decided(&signals, &rejections, &decisions)
            .into_iter()
            .map(|cluster| (cluster.id, cluster.class))
            .collect();

    let live_signals: Vec<SignalRow> = signals.into_iter().filter(|s| within(s.event.ts)).collect();
    let live_rejections: Vec<Rejection> = rejections.into_iter().filter(|r| within(r.ts)).collect();
    let live_decisions: Vec<Decision> = decisions.into_iter().filter(|d| within(d.ts)).collect();
    let clusters = evidence::cluster_decided(&live_signals, &live_rejections, &live_decisions);

    let mut report = empty(since_days);
    for signal in &live_signals {
        if let Some(index) = row_index(&signal.kind) {
            report.kinds[index].signals_captured += 1;
        }
    }
    for cluster in clusters.iter().filter(|c| reached(c)) {
        if let Some(index) = row_index(cluster.class) {
            report.kinds[index].clusters_at_threshold += 1;
        }
    }
    for proposal in store.proposals()? {
        if proposal.kind != Kind::Proposal || !within(proposal.ts) {
            continue;
        }
        let Some(index) = classes
            .get(&proposal.cluster)
            .and_then(|class| row_index(class))
            .or_else(|| row_index(UNKNOWN_CLASS))
        else {
            continue;
        };
        let row = &mut report.kinds[index];
        row.drafts += 1;
        if proposal.proven {
            row.proven += 1;
        }
        match proposal.state {
            State::Accepted => row.accepted += 1,
            State::Rejected => row.rejected += 1,
            State::Waiting => row.waiting += 1,
        }
    }
    Ok(report)
}

/// The report as plain text: one block per mistake class, `n/a` for a stage
/// the store does not record.
pub fn text(report: &Report) -> String {
    let window = match report.since_days {
        Some(days) => format!("window: last {days} days"),
        None => "window: all records".to_owned(),
    };
    let mut lines = vec![window];
    for row in &report.kinds {
        lines.push(String::new());
        lines.push(row.kind.to_owned());
        let stages = [
            ("signals captured", Some(row.signals_captured)),
            ("clusters at threshold", Some(row.clusters_at_threshold)),
            ("triage runs", row.triage_runs),
            ("rule-shaped drafts", Some(row.drafts)),
            ("proven", Some(row.proven)),
            ("accepted", Some(row.accepted)),
            ("rejected", Some(row.rejected)),
            ("waiting", Some(row.waiting)),
        ];
        for (label, count) in stages {
            let value = count.map_or(NOT_RECORDED.to_owned(), |count| count.to_string());
            lines.push(format!("  {label:<22}{value}"));
        }
    }
    lines.join("\n")
}
