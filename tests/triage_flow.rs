use std::cell::Cell;
use std::fs;
use std::path::Path;
use std::time::Duration;

use oncebitten::config::{Host, Triage, TriageTarget};
use oncebitten::triage::flow::{run, target};
use oncebitten::triage::host::{Captured, Invocation};
use oncebitten::triage::prompt::PromptInput;
use oncebitten::triage::run::{RunError, Runner};

const VERDICT: &str = r#"{"repeatable":true,"user_correction":false,"root_cause":"stale lockfile","proposed_fix_kind":"lint","generality":"project"}"#;

/// A host stand-in. Call 0 is the hooks-on control, call 1 the hooks-off triage
/// call. Each call creates the given number of `markers/marker-N` files, the
/// way a sentinel hook would, and returns `stdout`.
struct Fake {
    calls: Cell<usize>,
    control_fires: usize,
    triage_fires: usize,
    stdout: String,
    missing: bool,
}

impl Fake {
    fn new(control_fires: usize, triage_fires: usize, stdout: &str) -> Self {
        Fake {
            calls: Cell::new(0),
            control_fires,
            triage_fires,
            stdout: stdout.to_string(),
            missing: false,
        }
    }
}

impl Runner for Fake {
    fn run(&self, _inv: &Invocation, dir: &Path, _timeout: Duration) -> Result<Captured, RunError> {
        if self.missing {
            return Err(RunError::Missing("claude".into()));
        }
        let call = self.calls.get();
        self.calls.set(call + 1);
        let fires = if call == 0 {
            self.control_fires
        } else {
            self.triage_fires
        };
        let markers = dir.join("markers");
        fs::create_dir_all(&markers).unwrap();
        for index in 0..fires {
            fs::write(markers.join(format!("marker-{index}")), "").unwrap();
        }
        Ok(Captured {
            stdout: self.stdout.clone(),
            answer: Some(self.stdout.clone()),
        })
    }
}

fn input() -> PromptInput {
    PromptInput {
        class: "failure",
        tool: Some("Bash".into()),
        pattern: "cargo test".into(),
        sessions: 2,
        occurrences: Vec::new(),
    }
}

fn claude_stdout(verdict: &str) -> String {
    format!(r#"[{{"type":"result","is_error":false,"structured_output":{verdict}}}]"#)
}

fn opencode_stdout(verdict: &str) -> String {
    let event = serde_json::json!({"type": "text", "part": {"text": verdict}});
    format!("{event}\n")
}

#[test]
fn opencode_returns_the_verdict_without_a_proof() {
    let fake = Fake::new(0, 0, &opencode_stdout(VERDICT));
    let verdict = run(
        &fake,
        Host::Opencode,
        "plan",
        &input(),
        Duration::from_secs(5),
    )
    .unwrap();
    assert!(verdict.repeatable);
    assert_eq!(fake.calls.get(), 1);
}

#[test]
fn claude_returns_the_verdict_when_the_hooks_off_call_fires_nothing() {
    let fake = Fake::new(2, 0, &claude_stdout(VERDICT));
    let verdict = run(
        &fake,
        Host::Claude,
        "haiku",
        &input(),
        Duration::from_secs(5),
    )
    .unwrap();
    assert_eq!(verdict.root_cause, "stale lockfile");
    assert_eq!(fake.calls.get(), 2);
}

#[test]
fn claude_hooks_that_fire_in_the_triage_call_refuse_the_host() {
    let fake = Fake::new(2, 1, &claude_stdout(VERDICT));
    let err = run(
        &fake,
        Host::Claude,
        "haiku",
        &input(),
        Duration::from_secs(5),
    )
    .unwrap_err();
    assert!(err.to_string().contains("hooks-off proof failed"), "{err}");
}

#[test]
fn claude_silent_control_is_inconclusive_and_refused() {
    let fake = Fake::new(0, 0, &claude_stdout(VERDICT));
    let err = run(
        &fake,
        Host::Claude,
        "haiku",
        &input(),
        Duration::from_secs(5),
    )
    .unwrap_err();
    assert!(err.to_string().contains("inconclusive"), "{err}");
}

#[test]
fn codex_returns_the_verdict_from_its_answer_file_after_the_proof() {
    let fake = Fake::new(2, 0, VERDICT);
    let verdict = run(
        &fake,
        Host::Codex,
        "default",
        &input(),
        Duration::from_secs(5),
    )
    .unwrap();
    assert!(verdict.repeatable);
    assert_eq!(fake.calls.get(), 2);
}

#[test]
fn pi_is_unsupported_and_never_runs() {
    let fake = Fake::new(0, 0, "");
    let err = run(&fake, Host::Pi, "default", &input(), Duration::from_secs(5)).unwrap_err();
    assert!(err.to_string().contains("unsupported host `pi`"), "{err}");
    assert_eq!(fake.calls.get(), 0);
}

#[test]
fn output_without_a_verdict_is_could_not_run() {
    let fake = Fake::new(0, 0, "not json at all");
    let err = run(
        &fake,
        Host::Opencode,
        "plan",
        &input(),
        Duration::from_secs(5),
    )
    .unwrap_err();
    assert!(err.to_string().contains("could not run"), "{err}");
}

#[test]
fn a_missing_host_binary_is_an_error() {
    let mut fake = Fake::new(0, 0, "");
    fake.missing = true;
    let err = run(
        &fake,
        Host::Opencode,
        "plan",
        &input(),
        Duration::from_secs(5),
    )
    .unwrap_err();
    assert!(err.to_string().contains("not found"), "{err}");
}

fn target_of(spec: &str) -> TriageTarget {
    TriageTarget::try_from(spec.to_string()).unwrap()
}

#[test]
fn default_target_applies_to_its_own_host_only() {
    let triage = Triage {
        default: target_of("claude:haiku"),
        ..Triage::default()
    };
    assert_eq!(
        target(&triage, None).unwrap(),
        (Host::Claude, "haiku".into())
    );
    assert!(target(&triage, Some(Host::Opencode)).is_err());
}

#[test]
fn per_host_key_overrides_the_default_agent() {
    let triage = Triage {
        default: target_of("claude:haiku"),
        claude: Some(target_of("claude:sonnet")),
        ..Triage::default()
    };
    assert_eq!(
        target(&triage, None).unwrap(),
        (Host::Claude, "sonnet".into())
    );
}
