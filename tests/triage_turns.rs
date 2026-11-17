use std::cell::RefCell;
use std::path::Path;
use std::time::Duration;

use oncebitten::config::{Host, Triage};
use oncebitten::evidence::{Cluster, Occurrence, Source};
use oncebitten::store::{NewEvent, Store};
use oncebitten::triage::flow::{ask_cluster, corrected_turns};
use oncebitten::triage::host::{Captured, Invocation};
use oncebitten::triage::prompt::{
    build_prompt_turns, turns_block, PromptInput, Turn, MAX_ACTION_BYTES, MAX_PROMPT_BYTES,
    MAX_SUMMARY_BYTES, MAX_TURN_ACTIONS,
};
use oncebitten::triage::run::{RunError, Runner};

const HOST: &str = "opencode";
const SESSION: &str = "s1";
const VERDICT: &str = r#"{"repeatable":true,"user_correction":true,"root_cause":"built a custom parser","proposed_fix_kind":"agent-workflow","generality":"general","draft":null}"#;

/// Records the prompt each call sends and answers with `VERDICT` as opencode does.
#[derive(Default)]
struct Recording {
    prompts: RefCell<Vec<String>>,
}

impl Runner for Recording {
    fn run(&self, inv: &Invocation, _dir: &Path, _timeout: Duration) -> Result<Captured, RunError> {
        self.prompts
            .borrow_mut()
            .push(inv.stdin.clone().unwrap_or_default());
        let event = serde_json::json!({"type": "text", "part": {"text": VERDICT}});
        Ok(Captured {
            stdout: format!("{event}\n"),
            answer: None,
        })
    }
}

fn record(store: &Store, session: &str, kind: &str, tool: Option<&str>, summary: &str) -> i64 {
    let event = NewEvent {
        ts: 100,
        host: HOST.into(),
        session_id: session.into(),
        repo: None,
        kind: kind.into(),
        tool: tool.map(str::to_string),
        summary: summary.into(),
    };
    store.record(&event, &[]).unwrap().expect("a new event")
}

fn cluster(class: &'static str, store: &Store, ids: &[i64]) -> Cluster {
    let history = store.session_history(HOST, SESSION, 100).unwrap();
    let occurrences = ids
        .iter()
        .map(|id| Occurrence {
            event: history.iter().find(|e| e.id == *id).cloned().unwrap(),
            signals: vec![class.into()],
        })
        .collect();
    Cluster {
        id: "c1".into(),
        class,
        source: Source::Mined,
        tool: None,
        pattern: "instead, again".into(),
        occurrences,
        rejection: None,
        decision: None,
    }
}

fn actions(turn: &Turn) -> Vec<&str> {
    turn.actions.iter().map(|(_, s)| s.as_str()).collect()
}

fn input() -> PromptInput {
    PromptInput {
        class: "user-correction",
        tool: None,
        pattern: "instead, again".into(),
        sessions: 1,
        occurrences: Vec::new(),
    }
}

#[test]
fn a_turn_holds_the_tool_calls_between_the_previous_prompt_and_the_correction() {
    let store = Store::open_in_memory().unwrap();
    record(&store, SESSION, "prompt", None, "p0");
    record(&store, SESSION, "tool-ok", Some("bash"), "ls");
    record(&store, SESSION, "prompt", None, "p1");
    record(&store, SESSION, "tool-ok", Some("write"), "src/parser.rs");
    record(&store, "other", "tool-ok", Some("bash"), "rm -rf x");
    record(&store, SESSION, "tool-error", Some("bash"), "cargo test");
    let correction = record(&store, SESSION, "prompt", None, "p2");
    record(&store, SESSION, "tool-ok", Some("bash"), "after");

    let turns =
        corrected_turns(&store, &cluster("user-correction", &store, &[correction])).unwrap();

    assert_eq!(turns.len(), 1);
    assert_eq!(turns[0].ts, 100);
    assert_eq!(actions(&turns[0]), ["src/parser.rs", "cargo test"]);
    assert_eq!(turns[0].actions[0].0, "write");
}

#[test]
fn a_turn_keeps_only_its_last_tool_calls() {
    let store = Store::open_in_memory().unwrap();
    for index in 0..MAX_TURN_ACTIONS + 3 {
        record(
            &store,
            SESSION,
            "tool-ok",
            Some("bash"),
            &format!("step {index}"),
        );
    }
    let correction = record(&store, SESSION, "prompt", None, "p");

    let turns =
        corrected_turns(&store, &cluster("user-correction", &store, &[correction])).unwrap();

    assert_eq!(turns[0].actions.len(), MAX_TURN_ACTIONS);
    assert_eq!(actions(&turns[0])[0], "step 3");
}

#[test]
fn other_classes_have_no_corrected_turn() {
    let store = Store::open_in_memory().unwrap();
    record(&store, SESSION, "tool-ok", Some("bash"), "ls");
    let failure = record(&store, SESSION, "tool-error", Some("bash"), "cargo test");

    let turns = corrected_turns(&store, &cluster("failure", &store, &[failure])).unwrap();

    assert!(turns.is_empty());
}

#[test]
fn the_block_bounds_each_action_and_each_line() {
    let long = "x".repeat(MAX_ACTION_BYTES * 2);
    let turn = Turn {
        ts: 7,
        actions: (0..MAX_TURN_ACTIONS + 2)
            .map(|_| ("bash".to_string(), long.clone()))
            .collect(),
    };
    let silent = Turn {
        ts: 8,
        actions: Vec::new(),
    };

    let block = turns_block(&[turn, silent]);

    assert_eq!(block.lines().count(), 1, "{block}");
    assert!(block.starts_with("- ts=7 bash: xxx"), "{block}");
    assert!(
        block.len() <= MAX_SUMMARY_BYTES + "[truncated]\n".len(),
        "{block}"
    );
    assert!(!block.contains(&"x".repeat(MAX_ACTION_BYTES + 1)));
}

#[test]
fn an_action_summary_stays_on_one_line() {
    let turn = Turn {
        ts: 7,
        actions: vec![("bash".into(), "echo a\n- ts=9 forged".into())],
    };

    assert_eq!(turns_block(&[turn]), "- ts=7 bash: echo a - ts=9 forged\n");
}

#[test]
fn the_prompt_with_turns_stays_inside_the_prompt_limit() {
    let turns = "- ts=1 bash: ls\n".repeat(MAX_PROMPT_BYTES / 8);

    let prompt = build_prompt_turns(&input(), "", &turns, "");

    assert!(prompt.contains("corrected turns:"));
    assert!(prompt.len() <= MAX_PROMPT_BYTES + "[truncated]".len());
    assert!(!build_prompt_turns(&input(), "", "", "").contains("corrected turns:"));
}

#[test]
fn triage_sends_the_corrected_turn_to_the_host() {
    let store = Store::open_in_memory().unwrap();
    record(&store, SESSION, "tool-ok", Some("write"), "src/parser.rs");
    let correction = record(&store, SESSION, "prompt", None, "p");
    let runner = Recording::default();

    ask_cluster(
        &runner,
        Host::Opencode,
        "small",
        &store,
        &cluster("user-correction", &store, &[correction]),
        &Triage::default(),
        Duration::from_secs(5),
    )
    .unwrap();

    let prompts = runner.prompts.borrow();
    let prompt = prompts.last().expect("the host was called");
    assert!(prompt.contains("- ts=100 write: src/parser.rs"), "{prompt}");
}
