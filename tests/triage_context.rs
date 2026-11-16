use std::cell::RefCell;
use std::path::Path;
use std::time::Duration;

use oncebitten::config::Host;
use oncebitten::triage::flow::run_answer_with_context;
use oncebitten::triage::host::{Captured, Invocation};
use oncebitten::triage::prompt::PromptInput;
use oncebitten::triage::run::{RunError, Runner};

#[derive(Default)]
struct Recording {
    seen: RefCell<Vec<Invocation>>,
}

impl Runner for Recording {
    fn run(&self, inv: &Invocation, _dir: &Path, _timeout: Duration) -> Result<Captured, RunError> {
        self.seen.borrow_mut().push(inv.clone());
        Ok(Captured {
            stdout: String::new(),
            answer: None,
        })
    }
}

#[test]
fn the_context_block_reaches_the_host_prompt() {
    let runner = Recording::default();
    let context = "rejected proposals:\n- rejected: duplicate of c0\n";
    let _ = run_answer_with_context(
        &runner,
        Host::Codex,
        "small",
        &PromptInput {
            class: "repeatable",
            tool: None,
            pattern: "unwrap".into(),
            sessions: 1,
            occurrences: Vec::new(),
        },
        context,
        Duration::from_secs(5),
    );
    let seen = runner.seen.borrow();
    let first = seen.first().expect("the host was called");
    let carried = first
        .stdin
        .as_deref()
        .is_some_and(|prompt| prompt.contains("duplicate of c0"));
    assert!(carried, "the prompt must carry the context block");
}
