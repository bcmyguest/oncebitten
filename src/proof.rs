//! Replay proofs. A rule directory that passed its replay (in the OpenShell
//! sandbox, or with the matcher for a steer rule) carries `proof.json`, which
//! names the directory by a SHA-256 over every other file in it. The proof
//! stands while that hash matches.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::registry::{Layer, Resolved};
use crate::replay::{replay_dir, Backend, Verdict};
use crate::rule::{Check, Rule, Status};

pub const PROOF_FILE: &str = "proof.json";
const RULE_FILE: &str = "rule.toml";
pub const OPENSHELL: &str = "openshell";
pub const LOCAL: &str = "local";
/// Steer rules replay with the in-process matcher: it reads only the fixtures,
/// so it needs no sandbox.
pub const MATCHER: &str = "matcher";
pub const PROVEN: &str = "proven";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Proof {
    /// The `id` in `rule.toml`, without a source prefix, so the proof survives
    /// a move between the project and a rules repo.
    pub rule: String,
    pub dir_sha256: String,
    /// The oncebitten version that ran the replay; the checker builds from the same crate.
    pub checker_version: String,
    pub checker_sha256: String,
    pub backend: String,
    /// Seconds since the Unix epoch.
    pub timestamp: u64,
    pub result: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Validity {
    Valid,
    Missing,
    Stale(String),
    NotProven(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendKind {
    OpenShell,
    Local,
}

impl BackendKind {
    pub fn as_str(self) -> &'static str {
        match self {
            BackendKind::OpenShell => OPENSHELL,
            BackendKind::Local => LOCAL,
        }
    }
}

/// What one replay produced: the verdict and the proof file, if one was written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Replayed {
    pub verdict: Verdict,
    pub written: Option<PathBuf>,
}

pub fn rule_dir(rule: &Rule) -> &Path {
    rule.file.parent().unwrap_or_else(|| Path::new("."))
}

pub fn proof_path(dir: &Path) -> PathBuf {
    dir.join(PROOF_FILE)
}

/// SHA-256 over every file under `dir` except the top-level `proof.json`, in
/// sorted order of their `/`-separated relative paths. Each file adds its
/// path, a NUL, its length as a little-endian u64, then its bytes with CRLF
/// read as LF, so a Windows checkout hashes like a Unix one. The top-level
/// `rule.toml` hashes without its `status` key, so approving a proposed
/// rule keeps its proof valid (see `status_free_bytes`).
pub fn dir_hash(dir: &Path) -> Result<String> {
    let mut files = Vec::new();
    collect(dir, dir, &mut files)?;
    files.sort();
    let mut hasher = Sha256::new();
    for (relative, path) in &files {
        let bytes = if relative == RULE_FILE {
            status_free_bytes(path)?
        } else {
            let bytes =
                std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
            lf_only(&bytes)
        };
        hasher.update(relative.as_bytes());
        hasher.update([0]);
        hasher.update((bytes.len() as u64).to_le_bytes());
        hasher.update(&bytes);
    }
    Ok(hex(&hasher.finalize()))
}

/// The `rule.toml` bytes that enter the hash: the parsed table without
/// `status`, serialised canonically. Comments and layout do not reach the hash.
fn status_free_bytes(path: &Path) -> Result<Vec<u8>> {
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let text = String::from_utf8(lf_only(&bytes))
        .with_context(|| format!("{} is not UTF-8", path.display()))?;
    let mut table: toml::Table =
        toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
    table.remove("status");
    Ok(toml::to_string(&table)?.into_bytes())
}

pub fn file_sha256(path: &Path) -> Result<String> {
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    Ok(hex(&Sha256::digest(&bytes)))
}

pub fn text_sha256(text: &str) -> String {
    hex(&Sha256::digest(lf_only(text.as_bytes())))
}

pub fn read(dir: &Path) -> Result<Option<Proof>> {
    let path = proof_path(dir);
    if !path.exists() {
        return Ok(None);
    }
    let text =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let proof =
        serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
    Ok(Some(proof))
}

pub fn write(dir: &Path, proof: &Proof) -> Result<PathBuf> {
    let path = proof_path(dir);
    let mut text = serde_json::to_string_pretty(proof)?;
    text.push('\n');
    std::fs::write(&path, text).with_context(|| format!("writing {}", path.display()))?;
    Ok(path)
}

/// Whether the rule directory carries a proof that still stands.
pub fn validity(rule: &Rule) -> Validity {
    validity_with(rule, None)
}

/// The checker binary this process can compare a proof against: the
/// `oncebitten-checker` beside the running binary. `None` for a steer rule,
/// whose proof the gate re-checks by replaying, and when that file is absent,
/// as in CI without the musl checker.
pub fn local_checker(rule: &Rule) -> Option<PathBuf> {
    if matches!(rule.check, Check::Steer(_)) {
        return None;
    }
    let exe = std::env::current_exe().ok()?;
    Some(exe.with_file_name(format!(
        "oncebitten-checker{}",
        std::env::consts::EXE_SUFFIX
    )))
    .filter(|path| path.is_file())
}

/// Like [`validity`], and with a `checker` the proof must record that
/// checker's hash.
pub fn validity_with(rule: &Rule, checker: Option<&Path>) -> Validity {
    let dir = rule_dir(rule);
    let proof = match read(dir) {
        Ok(Some(proof)) => proof,
        Ok(None) => return Validity::Missing,
        Err(error) => return Validity::NotProven(format!("unreadable: {error:#}")),
    };
    if proof.result != PROVEN {
        return Validity::NotProven(format!("result is `{}`", proof.result));
    }
    let backend = match rule.check {
        Check::Steer(_) => MATCHER,
        _ => OPENSHELL,
    };
    if proof.backend != backend {
        return Validity::NotProven(format!(
            "backend is `{}`; only {backend} replays count",
            proof.backend
        ));
    }
    if proof.rule != rule.id {
        return Validity::Stale(format!("proof names rule `{}`", proof.rule));
    }
    match dir_hash(dir) {
        Ok(hash) if hash == proof.dir_sha256 => {}
        Ok(_) => return Validity::Stale("the rule directory changed since the replay".to_owned()),
        Err(error) => return Validity::NotProven(format!("hashing the rule directory: {error:#}")),
    }
    let Some(checker) = checker.filter(|_| !proof.checker_sha256.is_empty()) else {
        return Validity::Valid;
    };
    match file_sha256(checker) {
        Ok(hash) if hash == proof.checker_sha256 => Validity::Valid,
        Ok(_) => Validity::Stale(format!(
            "the replay used another checker than {}",
            checker.display()
        )),
        Err(error) => Validity::NotProven(format!("hashing the checker: {error:#}")),
    }
}

/// The project and local rules `replay --stale` runs: active or proposed,
/// needing a proof, whose proof is missing, stale or not accepted.
pub fn needing_replay(rules: &[Resolved]) -> Vec<&Resolved> {
    rules
        .iter()
        .filter(|resolved| {
            matches!(resolved.layer, Layer::Project | Layer::Local)
                && matches!(resolved.rule.status, Status::Active | Status::Proposed)
                && !matches!(resolved.rule.check, Check::Native { .. })
                && validity(&resolved.rule) != Validity::Valid
        })
        .collect()
}

/// Replays an ast-grep or diff rule. A proven verdict from the OpenShell backend writes
/// `proof.json`; a local run only reports. A rejected rule loses its proof.
pub fn replay_rule(
    rule: &Rule,
    checker: &Path,
    backend: &dyn Backend,
    kind: BackendKind,
) -> Replayed {
    let not_run = |reason: String| Replayed {
        verdict: Verdict::CouldNotRun(reason),
        written: None,
    };
    if let Check::Steer(_) = rule.check {
        return not_run("steer rules replay with replay_steer, not a sandbox".to_owned());
    }
    if matches!(rule.check, Check::Native { .. }) {
        return not_run(
            "replay supports ast-grep and diff rules; native-linter rules are exempt until replay can run their toolchain"
                .to_owned(),
        );
    }
    let dir = rule_dir(rule);
    let verdict = replay_dir(dir, checker, backend);
    match (&verdict, kind) {
        (Verdict::Proven, BackendKind::OpenShell) => match record(rule, dir, checker) {
            Ok(path) => Replayed {
                verdict,
                written: Some(path),
            },
            Err(error) => not_run(format!("writing the proof: {error:#}")),
        },
        (Verdict::Rejected(_), _) => match remove(dir) {
            Ok(()) => Replayed {
                verdict,
                written: None,
            },
            Err(error) => not_run(format!("removing the old proof: {error:#}")),
        },
        _ => Replayed {
            verdict,
            written: None,
        },
    }
}

/// Replays any rule the way its engine proves: a steer rule with the
/// in-process matcher held by `matcher`, every other rule on `backend`.
pub fn replay_any(
    rule: &Rule,
    checker: &Path,
    matcher: &Path,
    backend: &dyn Backend,
    kind: BackendKind,
) -> Replayed {
    if matches!(rule.check, Check::Steer(_)) {
        return replay_steer(rule, matcher);
    }
    replay_rule(rule, checker, backend, kind)
}

/// Replays a steer rule with the matcher (see [`crate::steer::replay`]). A
/// proven verdict writes `proof.json` naming `matcher`, the binary that holds
/// the matcher; a rejected rule loses its proof.
pub fn replay_steer(rule: &Rule, matcher: &Path) -> Replayed {
    let Check::Steer(steer) = &rule.check else {
        return Replayed {
            verdict: Verdict::CouldNotRun(format!("{} is not a steer rule", rule.id)),
            written: None,
        };
    };
    let dir = rule_dir(rule);
    let verdict = crate::steer::replay(dir, steer);
    let written = match &verdict {
        Verdict::Proven => record_with(rule, dir, matcher, MATCHER).map(Some),
        Verdict::Rejected(_) => remove(dir).map(|()| None),
        Verdict::CouldNotRun(_) => Ok(None),
    };
    match written {
        Ok(written) => Replayed { verdict, written },
        Err(error) => Replayed {
            verdict: Verdict::CouldNotRun(format!("updating the proof: {error:#}")),
            written: None,
        },
    }
}

fn record(rule: &Rule, dir: &Path, checker: &Path) -> Result<PathBuf> {
    record_with(rule, dir, checker, OPENSHELL)
}

fn record_with(rule: &Rule, dir: &Path, checker: &Path, backend: &str) -> Result<PathBuf> {
    let proof = Proof {
        rule: rule.id.clone(),
        dir_sha256: dir_hash(dir)?,
        checker_version: env!("CARGO_PKG_VERSION").to_owned(),
        checker_sha256: file_sha256(checker)?,
        backend: backend.to_owned(),
        timestamp: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_secs())
            .unwrap_or_default(),
        result: PROVEN.to_owned(),
    };
    write(dir, &proof)
}

fn remove(dir: &Path) -> Result<()> {
    let path = proof_path(dir);
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("removing {}", path.display())),
    }
}

fn collect(root: &Path, dir: &Path, out: &mut Vec<(String, PathBuf)>) -> Result<()> {
    for entry in std::fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
        let entry = entry.with_context(|| format!("reading {}", dir.display()))?;
        let path = entry.path();
        let kind = entry
            .file_type()
            .with_context(|| format!("reading {}", path.display()))?;
        if kind.is_symlink() {
            bail!("{}: rule directories may not hold symlinks", path.display());
        }
        if kind.is_dir() {
            collect(root, &path, out)?;
            continue;
        }
        let relative = path
            .strip_prefix(root)
            .with_context(|| format!("{} is outside {}", path.display(), root.display()))?
            .components()
            .map(|part| part.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/");
        if relative != PROOF_FILE {
            out.push((relative, path));
        }
    }
    Ok(())
}

fn lf_only(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut iter = bytes.iter().peekable();
    while let Some(&byte) = iter.next() {
        if byte == b'\r' && iter.peek() == Some(&&b'\n') {
            continue;
        }
        out.push(byte);
    }
    out
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
