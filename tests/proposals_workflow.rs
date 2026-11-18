use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use oncebitten::config::Inputs;
use oncebitten::evidence::{self, Cluster};
use oncebitten::proof::{self, BackendKind, Replayed, Validity};
use oncebitten::proposals::{self, Kind, Recorded, State};
use oncebitten::replay::{Backend, RawRun, ReplaySpec};
use oncebitten::rule::{self, Check, Rule};
use oncebitten::signal;
use oncebitten::steer::{Matcher, SteerEvent};
use oncebitten::store::{NewEvent, Store};
use oncebitten::triage::verdict::{self as triage_verdict, Answer};
use tempfile::TempDir;

const CHECKER: &str = env!("CARGO_BIN_EXE_oncebitten-checker");
const MATCHER: &str = env!("CARGO_BIN_EXE_oncebitten");
const HOST: &str = "claude";
const VERDICT: &str = r#""repeatable":true,"user_correction":true,"root_cause":"agent installed with pip instead of uv","proposed_fix_kind":"agent-workflow","generality":"project""#;

/// A sandbox backend that must never run: steer rules prove with the matcher.
struct NoSandbox;

impl Backend for NoSandbox {
    fn run(&self, _spec: &ReplaySpec, _files: &[PathBuf]) -> Result<Vec<RawRun>, String> {
        Err("openshell is not installed".into())
    }
}

/// A git repo (marked by `.git`) with an isolated user config and data root.
struct Sandbox {
    root: TempDir,
}

impl Sandbox {
    fn new() -> Self {
        let root = TempDir::new().unwrap();
        fs::create_dir_all(root.path().join("repo").join(".git")).unwrap();
        Sandbox { root }
    }

    fn repo(&self) -> PathBuf {
        self.root.path().join("repo")
    }

    fn inputs(&self) -> Inputs {
        let home = self.root.path().join("home");
        let vars: BTreeMap<String, String> = [
            ("HOME", home.clone()),
            ("USERPROFILE", home.clone()),
            ("XDG_CONFIG_HOME", home.join("config")),
            ("XDG_DATA_HOME", home.join("data")),
            ("APPDATA", home.join("appdata")),
            ("LOCALAPPDATA", home.join("localappdata")),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v.to_string_lossy().into_owned()))
        .collect();
        Inputs {
            cwd: self.repo(),
            vars,
        }
    }
}

fn record(store: &Store, repo: &Path, session: &str, kind: &str, tool: Option<&str>, text: &str) {
    let event = NewEvent {
        ts: 1,
        host: HOST.into(),
        session_id: session.into(),
        repo: Some(repo.display().to_string()),
        kind: kind.into(),
        tool: tool.map(str::to_string),
        summary: text.into(),
    };
    let history = store.session_history(HOST, session, 500).unwrap();
    let signals = signal::derive(&event, &history);
    store.record(&event, &signals).unwrap().unwrap();
}

/// Two sessions where the agent ran `commands`, then the user corrected it.
fn corrected(store: &Store, repo: &Path, commands: &[&str]) -> Cluster {
    for session in ["s1", "s2"] {
        record(store, repo, session, signal::KIND_PROMPT, None, "set it up");
        record(
            store,
            repo,
            session,
            signal::KIND_TOOL_OK,
            Some("Bash"),
            "cargo build",
        );
        for command in commands {
            record(
                store,
                repo,
                session,
                signal::KIND_TOOL_OK,
                Some("Bash"),
                command,
            );
        }
        record(
            store,
            repo,
            session,
            signal::KIND_PROMPT,
            None,
            "no, use uv instead",
        );
    }
    let clusters = evidence::load(store).unwrap();
    clusters
        .into_iter()
        .find(|cluster| cluster.class == signal::USER_CORRECTION)
        .expect("a user-correction cluster")
}

fn answer(bad_command: &str) -> Answer {
    let event = |command: &str| {
        serde_json::json!({"event": "tool", "tool": "Bash", "command": command}).to_string()
    };
    let draft = serde_json::json!({
        "id": "use-uv",
        "description": "install with uv, not pip",
        "mode": "steer",
        "engine": "steer",
        "bad": {"name": "bad.json", "content": event(bad_command)},
        "fixed": {"name": "fixed.json", "content": event("uv add requests")},
        "rationale": "This repo installs with uv; run uv add instead of pip install."
    });
    triage_verdict::parse_answer(&format!("{{{VERDICT},\"draft\":{draft}}}")).unwrap()
}

fn prove(rule: &Rule) -> Replayed {
    proof::replay_any(
        rule,
        Path::new(CHECKER),
        Path::new(MATCHER),
        &NoSandbox,
        BackendKind::OpenShell,
    )
}

fn written_rule(sandbox: &Sandbox) -> Rule {
    rule::load_rule(
        &sandbox
            .repo()
            .join(".oncebitten")
            .join("rules")
            .join("use-uv"),
    )
    .unwrap()
}

#[test]
fn a_corrected_command_becomes_a_steer_proposal_proven_without_a_sandbox() {
    let sandbox = Sandbox::new();
    let store = Store::open_in_memory().unwrap();
    let cluster = corrected(&store, &sandbox.repo(), &["pip install requests"]);

    let recorded = proposals::record(
        &store,
        &sandbox.inputs(),
        &cluster,
        &answer("pip install flask"),
        &prove,
        10,
    )
    .unwrap();

    assert_eq!(recorded, Recorded::Proposed { proven: true });
    let rule = written_rule(&sandbox);
    assert_eq!(proof::validity(&rule), Validity::Valid);
    let Check::Steer(steer) = &rule.check else {
        panic!("expected a steer rule, got {:?}", rule.check);
    };
    let bad = fs::read_to_string(rule.file.with_file_name("fixtures").join("bad.json")).unwrap();
    assert!(bad.contains("pip install requests"), "{bad}");
    let fires = |command: &str| {
        Matcher::new(steer).unwrap().matches(&SteerEvent::Tool {
            tool: "Bash".into(),
            command: Some(command.into()),
            paths: Vec::new(),
        })
    };
    assert!(fires("pip install numpy"));
    assert!(!fires("cargo build"));
}

#[test]
fn the_draft_picks_the_latest_turn_command_its_bad_fixture_matches() {
    let sandbox = Sandbox::new();
    let store = Store::open_in_memory().unwrap();
    let cluster = corrected(
        &store,
        &sandbox.repo(),
        &["pip install requests", "git status --short"],
    );

    proposals::record(
        &store,
        &sandbox.inputs(),
        &cluster,
        &answer("pip install flask"),
        &prove,
        10,
    )
    .unwrap();

    let rule = written_rule(&sandbox);
    let bad = fs::read_to_string(rule.file.with_file_name("fixtures").join("bad.json")).unwrap();
    assert!(bad.contains("pip install requests"), "{bad}");
}

#[test]
fn a_draft_naming_no_command_of_the_corrected_turns_is_not_written() {
    let sandbox = Sandbox::new();
    let store = Store::open_in_memory().unwrap();
    let cluster = corrected(&store, &sandbox.repo(), &["pip install requests"]);

    let error = proposals::record(
        &store,
        &sandbox.inputs(),
        &cluster,
        &answer("npm install left-pad"),
        &prove,
        10,
    )
    .unwrap_err();

    assert!(
        format!("{error:#}").contains("recorded command in a corrected turn"),
        "{error:#}"
    );
    assert!(!sandbox.repo().join(".oncebitten").exists());
}

#[test]
fn a_block_rule_still_needs_the_sandbox() {
    let sandbox = Sandbox::new();
    let store = Store::open_in_memory().unwrap();
    let cluster = corrected(&store, &sandbox.repo(), &["pip install requests"]);
    let draft = serde_json::json!({
        "id": "docs-with-src",
        "description": "a change under src needs a docs change",
        "mode": "block",
        "engine": "diff",
        "when_changed": ["src/**"],
        "require_changed": ["docs/**"],
        "bad": {"name": "bad.diff", "content": "src/lib.rs\n"},
        "fixed": {"name": "fixed.diff", "content": "src/lib.rs\ndocs/a.md\n"},
        "corpus": [{"name": "readme-only.diff", "content": "README.md\n"}],
        "rationale": "agents forgot docs twice"
    });
    let answer = triage_verdict::parse_answer(&format!("{{{VERDICT},\"draft\":{draft}}}")).unwrap();

    let recorded =
        proposals::record(&store, &sandbox.inputs(), &cluster, &answer, &prove, 10).unwrap();

    assert_eq!(recorded, Recorded::Proposed { proven: false });
}

fn prompt_answer() -> Answer {
    let event = |prompt: &str| serde_json::json!({"event": "prompt", "prompt": prompt}).to_string();
    let draft = serde_json::json!({
        "id": "use-uv",
        "description": "check every review comment before finishing",
        "mode": "steer",
        "engine": "steer",
        "prompt": "(?i)review comments?|feedback",
        "bad": {"name": "bad.json", "content": event("address the review comments on my PR")},
        "fixed": {"name": "fixed.json", "content": event("review this function for bugs")},
        "corpus": [{"name": "tests.json", "content": event("run the tests")}],
        "rationale": "Before you finish, check that every review comment is addressed."
    });
    triage_verdict::parse_answer(&format!("{{{VERDICT},\"draft\":{draft}}}")).unwrap()
}

fn no_draft() -> Answer {
    triage_verdict::parse_answer(&format!("{{{VERDICT}}}")).unwrap()
}

#[test]
fn a_prompt_steer_draft_is_queued_proven_with_the_models_fixtures() {
    let sandbox = Sandbox::new();
    let store = Store::open_in_memory().unwrap();
    let cluster = corrected(&store, &sandbox.repo(), &[]);

    let recorded = proposals::record(
        &store,
        &sandbox.inputs(),
        &cluster,
        &prompt_answer(),
        &prove,
        10,
    )
    .unwrap();

    assert_eq!(recorded, Recorded::Proposed { proven: true });
    let rule = written_rule(&sandbox);
    assert_eq!(proof::validity(&rule), Validity::Valid);
    let bad = fs::read_to_string(rule.file.with_file_name("fixtures").join("bad.json")).unwrap();
    assert!(bad.contains("address the review comments"), "{bad}");
    let row = store.proposal(&cluster.id).unwrap().unwrap();
    assert_eq!(row.kind, Kind::Proposal);
    assert!(row.proven);
}

#[test]
fn a_manual_triage_upgrades_a_waiting_finding_to_a_proposal() {
    let sandbox = Sandbox::new();
    let store = Store::open_in_memory().unwrap();
    let cluster = corrected(&store, &sandbox.repo(), &[]);
    let inputs = sandbox.inputs();
    let first = proposals::record(&store, &inputs, &cluster, &no_draft(), &prove, 10).unwrap();
    assert_eq!(first, Recorded::Finding);

    let upgraded =
        proposals::record_upgrading(&store, &inputs, &cluster, &prompt_answer(), &prove, 20)
            .unwrap();

    assert_eq!(upgraded, Recorded::Proposed { proven: true });
    let rows = store.proposals().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].kind, Kind::Proposal);
    assert_eq!(rows[0].state, State::Waiting);
    assert_eq!(rows[0].rule.as_deref(), Some("use-uv"));
    assert_eq!(rows[0].ts, 20);
    assert_eq!(proof::validity(&written_rule(&sandbox)), Validity::Valid);
}

#[test]
fn a_manual_triage_without_a_draft_keeps_the_finding() {
    let sandbox = Sandbox::new();
    let store = Store::open_in_memory().unwrap();
    let cluster = corrected(&store, &sandbox.repo(), &[]);
    let inputs = sandbox.inputs();
    proposals::record(&store, &inputs, &cluster, &no_draft(), &prove, 10).unwrap();

    let kept =
        proposals::record_upgrading(&store, &inputs, &cluster, &no_draft(), &prove, 20).unwrap();

    assert_eq!(kept, Recorded::FindingKept);
    let row = store.proposal(&cluster.id).unwrap().unwrap();
    assert_eq!(row.kind, Kind::Finding);
    assert_eq!(row.ts, 10);
    assert!(!sandbox.repo().join(".oncebitten").exists());
}

#[test]
fn a_background_answer_never_replaces_a_finding() {
    let sandbox = Sandbox::new();
    let store = Store::open_in_memory().unwrap();
    let cluster = corrected(&store, &sandbox.repo(), &[]);
    let inputs = sandbox.inputs();
    proposals::record(&store, &inputs, &cluster, &no_draft(), &prove, 10).unwrap();

    let again = proposals::record(&store, &inputs, &cluster, &prompt_answer(), &prove, 20).unwrap();

    assert_eq!(again, Recorded::AlreadyQueued);
    assert_eq!(
        store.proposal(&cluster.id).unwrap().unwrap().kind,
        Kind::Finding
    );
    assert!(!sandbox.repo().join(".oncebitten").exists());
}

#[test]
fn a_manual_triage_leaves_a_queued_proposal_alone() {
    let sandbox = Sandbox::new();
    let store = Store::open_in_memory().unwrap();
    let cluster = corrected(&store, &sandbox.repo(), &[]);
    let inputs = sandbox.inputs();
    proposals::record(&store, &inputs, &cluster, &prompt_answer(), &prove, 10).unwrap();

    let again =
        proposals::record_upgrading(&store, &inputs, &cluster, &prompt_answer(), &prove, 20)
            .unwrap();

    assert_eq!(again, Recorded::AlreadyQueued);
    assert_eq!(store.proposal(&cluster.id).unwrap().unwrap().ts, 10);
}
