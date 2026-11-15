mod common;

use std::collections::BTreeMap;
use std::path::PathBuf;

use common::{both, code, stderr, stdout, Repo};
use oncebitten::config::Inputs;
use oncebitten::evidence;
use oncebitten::proof::Replayed;
use oncebitten::proposals::{self, Recorded};
use oncebitten::replay::Verdict;
use oncebitten::rule::Rule;
use oncebitten::signal;
use oncebitten::store::{NewEvent, Store};
use oncebitten::triage::verdict;

const ANSWER: &str = r#"{"repeatable":true,"user_correction":false,"root_cause":"docs drift","proposed_fix_kind":"lint","generality":"project","draft":{"id":"docs-with-src","description":"a change under src needs a docs change","mode":"block","engine":"diff","when_changed":["src/**"],"require_changed":["docs/**"],"bad":{"name":"bad.diff","content":"src/lib.rs\n"},"fixed":{"name":"fixed.diff","content":"src/lib.rs\ndocs/a.md\n"},"corpus":[{"name":"readme.diff","content":"README.md\n"}],"rationale":"agents forgot docs twice"}}"#;
const NOTICE: &str = "1 rule proposal waiting: oncebitten proposals list";

fn db(repo: &Repo) -> PathBuf {
    repo.root
        .path()
        .join("data")
        .join("oncebitten")
        .join("events.db")
}

fn inputs(repo: &Repo) -> Inputs {
    let vars: BTreeMap<String, String> = [
        ("HOME", repo.root.path().join("home")),
        ("XDG_CONFIG_HOME", repo.root.path().join("xdg-config")),
        ("XDG_DATA_HOME", repo.root.path().join("data")),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v.to_string_lossy().into_owned()))
    .collect();
    Inputs {
        cwd: repo.project(),
        vars,
    }
}

fn no_sandbox(_rule: &Rule) -> Replayed {
    Replayed {
        verdict: Verdict::CouldNotRun("openshell is not installed".into()),
        written: None,
    }
}

/// Queues one unproven proposal for the repo and returns its id.
fn propose(repo: &Repo) -> String {
    std::fs::create_dir_all(db(repo).parent().unwrap()).unwrap();
    let store = Store::open(&db(repo)).unwrap();
    for (session, ts) in [("s1", 1), ("s2", 2)] {
        let event = NewEvent {
            ts,
            host: "pi".into(),
            session_id: session.into(),
            repo: Some(repo.project().to_string_lossy().into_owned()),
            kind: signal::KIND_TOOL_ERROR.into(),
            tool: Some("Bash".into()),
            summary: "cargo doc".into(),
        };
        let history = store.session_history("pi", session, 500).unwrap();
        let signals = signal::derive(&event, &history);
        store.record(&event, &signals).unwrap();
    }
    let cluster = evidence::load(&store).unwrap().remove(0);
    let answer = verdict::parse_answer(ANSWER).unwrap();
    let recorded =
        proposals::record(&store, &inputs(repo), &cluster, &answer, &no_sandbox, 10).unwrap();
    assert_eq!(recorded, Recorded::Proposed { proven: false });
    cluster.id
}

fn committed_repo() -> Repo {
    let repo = Repo::new();
    repo.write("README.md", "hello\n");
    let sync = repo.run(&["rules", "sync"], &[]);
    assert_eq!(code(&sync), 0, "{}", both(&sync));
    repo.commit_all("base");
    repo
}

#[test]
fn list_and_show_describe_a_waiting_proposal() {
    let repo = committed_repo();
    let id = propose(&repo);

    let list = repo.run(&["proposals", "list"], &[]);
    assert_eq!(code(&list), 0, "{}", both(&list));
    let line = stdout(&list);
    assert!(line.contains(&id), "{line}");
    assert!(line.contains("docs-with-src"), "{line}");
    assert!(line.contains("unproven"), "{line}");

    let show = repo.run(&["proposals", "show", &id], &[]);
    assert_eq!(code(&show), 0, "{}", both(&show));
    let text = stdout(&show);
    assert!(text.contains("agents forgot docs twice"), "{text}");
    assert!(text.contains("openshell is not installed"), "{text}");
    assert!(text.contains("docs drift"), "{text}");
    assert!(text.contains("occurrences: 1, 2"), "{text}");
}

#[test]
fn accept_without_a_proof_exits_2_with_the_reason() {
    let repo = committed_repo();
    let id = propose(&repo);
    let accept = repo.run(&["proposals", "accept", &id], &[]);
    assert_eq!(code(&accept), 2, "{}", both(&accept));
    assert!(stderr(&accept).contains("no proof"), "{}", both(&accept));
    let unknown = repo.run(&["proposals", "accept", "nope"], &[]);
    assert_eq!(code(&unknown), 2, "{}", both(&unknown));
}

#[test]
fn reject_removes_the_proposal_from_the_list() {
    let repo = committed_repo();
    let id = propose(&repo);
    let reject = repo.run(&["proposals", "reject", &id, "--reason", "noise"], &[]);
    assert_eq!(code(&reject), 0, "{}", both(&reject));
    assert!(!repo.rule_dir("docs-with-src").exists());
    let list = repo.run(&["proposals", "list"], &[]);
    assert!(stdout(&list).contains("rejected"), "{}", both(&list));
}

#[test]
fn the_gate_prints_the_notice_and_keeps_its_exit_code() {
    let repo = committed_repo();
    let before_push = repo.run(&["gate", "pre-push"], &[]);
    let before_pr = repo.run(&["gate", "pr", "--base", "main"], &[]);
    assert!(!stderr(&before_push).contains(NOTICE));
    propose(&repo);
    let after_push = repo.run(&["gate", "pre-push"], &[]);
    let after_pr = repo.run(&["gate", "pr", "--base", "main"], &[]);
    assert_eq!(
        code(&after_push),
        code(&before_push),
        "{}",
        both(&after_push)
    );
    assert_eq!(code(&after_pr), code(&before_pr), "{}", both(&after_pr));
    assert!(
        stderr(&after_push).contains(NOTICE),
        "{}",
        both(&after_push)
    );
    assert!(stderr(&after_pr).contains(NOTICE), "{}", both(&after_pr));
}

#[test]
fn the_gate_creates_no_store_when_none_exists() {
    let repo = committed_repo();
    repo.run(&["gate", "pre-push"], &[]);
    assert!(!db(&repo).exists());
}
