mod common;

use std::collections::BTreeMap;
use std::path::PathBuf;

use common::{both, code, stdout, Repo};
use oncebitten::config::Inputs;
use oncebitten::evidence;
use oncebitten::proof::Replayed;
use oncebitten::proposals::{self, Recorded};
use oncebitten::replay::Verdict;
use oncebitten::rule::Rule;
use oncebitten::signal;
use oncebitten::store::{NewEvent, Store};
use oncebitten::triage::verdict;

const ANSWER: &str = r#"{"repeatable":true,"user_correction":false,"root_cause":"utcnow drift","proposed_fix_kind":"lint","generality":"project","draft":{"id":"py-no-utcnow","description":"use datetime.now(timezone.utc)","mode":"block","engine":"ast-grep","language":"python","check":"id: py-no-utcnow\nlanguage: python\nrule:\n  pattern: datetime.utcnow()\nmessage: use datetime.now(timezone.utc)\nseverity: error\n","command":null,"fired_exit_codes":null,"fired_output_contains":null,"when_changed":null,"require_changed":null,"bad":{"name":"bad.py","content":"datetime.utcnow()\n"},"fixed":{"name":"fixed.py","content":"datetime.now(timezone.utc)\n"},"corpus":[],"rationale":"utcnow is deprecated"}}"#;

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

/// Queues the ast-grep proposal for the repo and returns its id.
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
            summary: "utcnow".into(),
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

fn repo_with_utcnow() -> Repo {
    let repo = Repo::new();
    repo.write("README.md", "hello\n");
    repo.write("src/clock.py", "import datetime\nnow = datetime.utcnow()\n");
    let sync = repo.run(&["rules", "sync"], &[]);
    assert_eq!(code(&sync), 0, "{}", both(&sync));
    repo.commit_all("base");
    repo
}

#[test]
fn list_and_show_report_the_existing_hits() {
    let repo = repo_with_utcnow();
    let id = propose(&repo);

    let list = repo.run(&["proposals", "list"], &[]);
    assert_eq!(code(&list), 0, "{}", both(&list));
    assert!(
        stdout(&list).contains("1 hit in 1 file"),
        "{}",
        stdout(&list)
    );

    let show = repo.run(&["proposals", "show", &id], &[]);
    assert_eq!(code(&show), 0, "{}", both(&show));
    let text = stdout(&show);
    assert!(text.contains("hits: 1 hit in 1 file"), "{text}");
    assert!(text.contains("src/clock.py:2: datetime.utcnow()"), "{text}");
}
