use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, bail, Context, Result};

use crate::config::{Host, Triage};
use crate::evidence::Cluster;
use crate::triage::host::{control_invocation, invocation, verdict_from};
use crate::triage::prompt::{build_prompt, Occurrence, PromptInput};
use crate::triage::proof::{prove, requires_proof, workdir_sentinels};
use crate::triage::run::Runner;
use crate::triage::verdict::Verdict;

pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);

/// The host and agent a triage call uses. `host` picks the host; without it
/// the default target's host. A per-host key wins, else `triage.default` applies
/// when it names that host.
pub fn target(triage: &Triage, host: Option<Host>) -> Result<(Host, String)> {
    let host = host.unwrap_or(triage.default.host);
    let per_host = match host {
        Host::Claude => &triage.claude,
        Host::Codex => &triage.codex,
        Host::Pi => &triage.pi,
        Host::Opencode => &triage.opencode,
    };
    if let Some(target) = per_host {
        return Ok((host, target.agent.clone()));
    }
    if triage.default.host == host {
        return Ok((host, triage.default.agent.clone()));
    }
    bail!(
        "no triage target for host `{}`: set triage.{} or a triage.default for it",
        host.as_str(),
        host.as_str()
    )
}

/// The prompt's view of one evidence cluster.
pub fn prompt_input(cluster: &Cluster) -> PromptInput {
    PromptInput {
        class: cluster.class,
        tool: cluster.tool.clone(),
        pattern: cluster.pattern.clone(),
        sessions: cluster.sessions(),
        occurrences: cluster
            .occurrences
            .iter()
            .map(|o| Occurrence {
                host: o.event.host.clone(),
                ts: o.event.ts,
                signals: o.signals.clone(),
                summary: o.event.summary.clone(),
            })
            .collect(),
    }
}

/// A fresh empty directory that is removed when dropped.
struct Workdir(PathBuf);

impl Workdir {
    fn create() -> Result<Self> {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .context("system clock is before the unix epoch")?
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("oncebitten-triage-{}-{nanos}", std::process::id()));
        fs::create_dir(&path)
            .with_context(|| format!("cannot create triage workdir {}", path.display()))?;
        Ok(Workdir(path))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Workdir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// One triage call end to end: prompt, hooks-off proof where the host needs
/// it, the call, and the verdict. Every error means "could not run" or a
/// refused host; the caller exits 2.
pub fn run(
    runner: &dyn Runner,
    host: Host,
    agent: &str,
    input: &PromptInput,
    timeout: Duration,
) -> Result<Verdict> {
    let workdir = Workdir::create()?;
    let dir = workdir.path();
    let prompt = build_prompt(input);
    let triage = invocation(host, agent, &prompt, dir)?;
    let captured = if requires_proof(host) {
        let sentinels = workdir_sentinels(host, dir, &dir.join("markers"));
        let control = control_invocation(host, agent, &prompt, dir)?;
        prove(runner, &control, &triage, dir, timeout, &sentinels).map_err(anyhow::Error::new)?
    } else {
        runner
            .run(&triage, dir, timeout)
            .map_err(|err| anyhow!("{err}"))?
    };
    verdict_from(&triage, &captured).context("could not run: host output is not a verdict")
}
