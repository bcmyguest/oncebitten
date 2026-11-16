use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::triage::draft::Draft;

pub const MAX_ROOT_CAUSE_CHARS: usize = 1000;
pub const MAX_MISTAKE_CLASS_CHARS: usize = 64;

/// JSON Schema for the verdict and its optional rule draft. Hosts that enforce a schema (claude
/// `--json-schema`, codex `--output-schema`) receive this text. `parse`
/// enforces the same rules again on the output.
pub const SCHEMA: &str = r#"{"type":"object","additionalProperties":false,"required":["repeatable","user_correction","root_cause","proposed_fix_kind","generality"],"properties":{"repeatable":{"type":"boolean"},"user_correction":{"type":"boolean"},"root_cause":{"type":"string","minLength":1,"maxLength":1000},"proposed_fix_kind":{"type":"string","enum":["harness","agent-workflow","lint","test","test-practice","invariant","none"]},"generality":{"type":"string","enum":["general","project","one-off"]},"mistake_class":{"type":["string","null"],"pattern":"^[a-z0-9]+(-[a-z0-9]+)*$","maxLength":64},"draft":{"anyOf":[{"type":"object","additionalProperties":false,"required":["id","description","mode","engine","language","check","command","fired_exit_codes","fired_output_contains","when_changed","require_changed","bad","fixed","corpus","rationale"],"properties":{"id":{"type":"string","pattern":"^[a-z0-9]+(-[a-z0-9]+)*$","maxLength":64},"description":{"type":"string","minLength":1,"maxLength":1000},"mode":{"type":"string","enum":["block","steer"]},"engine":{"type":"string","enum":["ast-grep","diff","native"]},"language":{"type":["string","null"]},"check":{"type":["string","null"]},"command":{"type":["array","null"],"items":{"type":"string"}},"fired_exit_codes":{"type":["array","null"],"items":{"type":"integer"}},"fired_output_contains":{"type":["string","null"]},"when_changed":{"type":["array","null"],"items":{"type":"string"}},"require_changed":{"type":["array","null"],"items":{"type":"string"}},"bad":{"type":"object","additionalProperties":false,"required":["name","content"],"properties":{"name":{"type":"string","minLength":1,"maxLength":100},"content":{"type":"string","maxLength":65536}}},"fixed":{"type":"object","additionalProperties":false,"required":["name","content"],"properties":{"name":{"type":"string","minLength":1,"maxLength":100},"content":{"type":"string","maxLength":65536}}},"corpus":{"type":["array","null"],"items":{"type":"object","additionalProperties":false,"required":["name","content"],"properties":{"name":{"type":"string","minLength":1,"maxLength":100},"content":{"type":"string","maxLength":65536}}}},"rationale":{"type":"string","minLength":1,"maxLength":1000}}},{"type":"null"}]}}}"#;

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
    /// Slug naming the mistake, shared by reworded proposals of one mistake.
    #[serde(default)]
    pub mistake_class: Option<String>,
}

/// A host's whole answer: the verdict plus the rule it drafted, if any.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answer {
    pub verdict: Verdict,
    pub draft: Option<Draft>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawAnswer {
    repeatable: bool,
    user_correction: bool,
    root_cause: String,
    proposed_fix_kind: FixKind,
    generality: Generality,
    #[serde(default)]
    mistake_class: Option<String>,
    #[serde(default)]
    draft: Option<Draft>,
}

impl Verdict {
    /// A verdict is rule-shaped when it names a repeatable mistake with a fix
    /// that outlives one session. Only a rule-shaped verdict keeps its draft.
    pub fn is_rule_shaped(&self) -> bool {
        self.repeatable
            && self.proposed_fix_kind != FixKind::NoFix
            && self.generality != Generality::OneOff
    }
}

/// Parses a host's answer. Any text that is not exactly one verdict object
/// is an error; the caller reports it as "could not run", never as a verdict.
pub fn parse(text: &str) -> Result<Verdict> {
    parse_answer(text).map(|answer| answer.verdict)
}

/// Parses the verdict and validates its draft. An invalid draft rejects the
/// whole answer. A draft beside a verdict that is not rule-shaped is dropped.
pub fn parse_answer(text: &str) -> Result<Answer> {
    let raw: RawAnswer = serde_json::from_str(text.trim()).context("verdict is not valid JSON")?;
    let verdict = Verdict {
        repeatable: raw.repeatable,
        user_correction: raw.user_correction,
        root_cause: raw.root_cause,
        proposed_fix_kind: raw.proposed_fix_kind,
        generality: raw.generality,
        mistake_class: mistake_class(raw.mistake_class)?,
    };
    let root_cause = verdict.root_cause.trim();
    if root_cause.is_empty() {
        bail!("verdict root_cause is empty");
    }
    if root_cause.chars().count() > MAX_ROOT_CAUSE_CHARS {
        bail!("verdict root_cause exceeds {MAX_ROOT_CAUSE_CHARS} characters");
    }
    let draft = match raw.draft {
        Some(draft) if verdict.is_rule_shaped() => {
            draft.validate().context("verdict draft is invalid")?;
            Some(draft)
        }
        _ => None,
    };
    Ok(Answer { verdict, draft })
}

/// Empty and absent classes mean no class. Any other value must be a lowercase
/// slug of at most `MAX_MISTAKE_CLASS_CHARS` characters.
fn mistake_class(raw: Option<String>) -> Result<Option<String>> {
    let Some(class) = raw.map(|text| text.trim().to_string()) else {
        return Ok(None);
    };
    if class.is_empty() {
        return Ok(None);
    }
    let slug_ok = class.split('-').all(|part| {
        !part.is_empty()
            && part
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
    });
    if !slug_ok || class.len() > MAX_MISTAKE_CLASS_CHARS {
        bail!("verdict mistake_class must be a lowercase slug of at most {MAX_MISTAKE_CLASS_CHARS} characters");
    }
    Ok(Some(class))
}
