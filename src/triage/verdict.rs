use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

pub const MAX_ROOT_CAUSE_CHARS: usize = 1000;

/// JSON Schema for the verdict. Hosts that enforce a schema (claude
/// `--json-schema`, codex `--output-schema`) receive this text. `parse`
/// enforces the same rules again on the output.
pub const SCHEMA: &str = r#"{"type":"object","additionalProperties":false,"required":["repeatable","user_correction","root_cause","proposed_fix_kind","generality"],"properties":{"repeatable":{"type":"boolean"},"user_correction":{"type":"boolean"},"root_cause":{"type":"string","minLength":1,"maxLength":1000},"proposed_fix_kind":{"type":"string","enum":["harness","agent-workflow","lint","test","test-practice","invariant","none"]},"generality":{"type":"string","enum":["general","project","one-off"]}}}"#;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FixKind {
    Harness,
    AgentWorkflow,
    Lint,
    Test,
    TestPractice,
    Invariant,
    #[serde(rename = "none")]
    NoFix,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Generality {
    General,
    Project,
    OneOff,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Verdict {
    pub repeatable: bool,
    pub user_correction: bool,
    pub root_cause: String,
    pub proposed_fix_kind: FixKind,
    pub generality: Generality,
}

/// Parses a host's answer. Any text that is not exactly one verdict object
/// is an error; the caller reports it as "could not run", never as a verdict.
pub fn parse(text: &str) -> Result<Verdict> {
    let verdict: Verdict =
        serde_json::from_str(text.trim()).context("verdict is not valid JSON")?;
    let root_cause = verdict.root_cause.trim();
    if root_cause.is_empty() {
        bail!("verdict root_cause is empty");
    }
    if root_cause.chars().count() > MAX_ROOT_CAUSE_CHARS {
        bail!("verdict root_cause exceeds {MAX_ROOT_CAUSE_CHARS} characters");
    }
    Ok(verdict)
}
