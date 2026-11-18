use std::fs;
use std::path::{Path, PathBuf};

use oncebitten::proof::{self, Replayed, Validity};
use oncebitten::proposals;
use oncebitten::replay::Verdict;
use oncebitten::rule::{self, Action, Check, Rule, Status};
use oncebitten::store::{Kind, ProposalRow, State, Store};
use tempfile::TempDir;

const MATCHER: &str = env!("CARGO_BIN_EXE_oncebitten");
const BAD: &str = r#"{"event": "tool", "tool": "Bash", "command": "sudo apt install jq"}"#;
const FIXED: &str = r#"{"event": "tool", "tool": "Bash", "command": "apt list jq"}"#;
const STEER_TOML: &str = "id = \"no-sudo\"\ndescription = \"Never run sudo.\"\nmode = \"steer\"\n\
status = \"proposed\"\nengine = \"steer\"\n\n[steer]\naction = \"inform\"\n\
message = \"Ask the user to run sudo.\"\n[steer.trigger]\ntool = \"Bash\"\n\
command = '^\\s*sudo\\b'\n";

/// Writes a proposed rule with `toml` and its fixtures, queues it as a
/// waiting proposal under the cluster `c1`, and returns the temp root, the
/// repo path and the store.
fn queued(toml: &str) -> (TempDir, PathBuf, Store) {
    let root = TempDir::new().unwrap();
    let repo = root.path().join("repo");
    let dir = repo.join(".oncebitten").join("rules").join("no-sudo");
    fs::create_dir_all(dir.join("fixtures")).unwrap();
    fs::write(dir.join("rule.toml"), toml).unwrap();
    fs::write(dir.join("fixtures").join("bad.json"), BAD).unwrap();
    fs::write(dir.join("fixtures").join("fixed.json"), FIXED).unwrap();
    let store = Store::open_in_memory().unwrap();
    store
        .insert_proposal(&ProposalRow {
            cluster: "c1".into(),
            ts: 1,
            repo: repo.display().to_string(),
            kind: Kind::Proposal,
            state: State::Waiting,
            rule: Some("no-sudo".into()),
            dir: Some(dir),
            proven: false,
            note: None,
            verdict: "{}".into(),
            rationale: None,
            occurrences: vec![],
            target: None,
            commit: None,
            pr_url: None,
            hits: None,
        })
        .unwrap();
    (root, repo, store)
}

fn prove(rule: &Rule) -> Replayed {
    proof::replay_steer(rule, Path::new(MATCHER))
}

fn rule_text(repo: &Path) -> String {
    fs::read_to_string(repo.join(".oncebitten/rules/no-sudo/rule.toml")).unwrap()
}

fn loaded(repo: &Path) -> Rule {
    rule::load_rule(&repo.join(".oncebitten/rules/no-sudo")).unwrap()
}

fn steer_action(rule: &Rule) -> Action {
    let Check::Steer(steer) = &rule.check else {
        panic!("not a steer rule")
    };
    steer.action
}

#[test]
fn plain_accept_keeps_a_steer_rule_at_inform() {
    let (_root, repo, store) = queued(STEER_TOML);
    assert_eq!(prove(&loaded(&repo)).verdict, Verdict::Proven);
    proposals::accept(&store, "c1").unwrap();
    let rule = loaded(&repo);
    assert_eq!(rule.status, Status::Active);
    assert_eq!(steer_action(&rule), Action::Inform);
}

#[test]
fn accept_deny_sets_action_deny_and_reproves_the_rule() {
    let (_root, repo, store) = queued(STEER_TOML);
    assert_eq!(prove(&loaded(&repo)).verdict, Verdict::Proven);
    proposals::accept_deny(&store, "c1", prove).unwrap();
    let rule = loaded(&repo);
    assert_eq!(rule.status, Status::Active);
    assert_eq!(steer_action(&rule), Action::Deny);
    assert_eq!(proof::validity(&rule), Validity::Valid);
    assert!(rule_text(&repo).contains("action = \"deny\""));
}

#[test]
fn accept_deny_inserts_the_action_when_the_table_has_none() {
    let toml = STEER_TOML.replace("action = \"inform\"\n", "");
    let (_root, repo, store) = queued(&toml);
    assert_eq!(prove(&loaded(&repo)).verdict, Verdict::Proven);
    proposals::accept_deny(&store, "c1", prove).unwrap();
    assert_eq!(steer_action(&loaded(&repo)), Action::Deny);
}

#[test]
fn accept_deny_refuses_a_rule_that_is_not_steer() {
    let diff = "id = \"no-sudo\"\ndescription = \"Docs follow source.\"\nmode = \"block\"\n\
status = \"proposed\"\nengine = \"diff\"\nwhen_changed = [\"src/**\"]\n\
require_changed = [\"docs/**\"]\n";
    let (_root, repo, store) = queued(diff);
    let error = proposals::accept_deny(&store, "c1", prove).unwrap_err();
    assert!(format!("{error:#}").contains("not a steer"), "{error:#}");
    assert_eq!(rule_text(&repo), diff);
    assert_eq!(store.proposal("c1").unwrap().unwrap().state, State::Waiting);
}

#[test]
fn accept_deny_refuses_a_path_trigger_and_keeps_the_file() {
    let toml = STEER_TOML.replace(
        "[steer.trigger]\ntool = \"Bash\"\ncommand = '^\\s*sudo\\b'\n",
        "[steer.trigger]\npaths = [\"src/**\"]\n",
    );
    let (_root, repo, store) = queued(&toml);
    let error = proposals::accept_deny(&store, "c1", prove).unwrap_err();
    assert!(format!("{error:#}").contains("tool trigger"), "{error:#}");
    assert_eq!(rule_text(&repo), toml);
    assert_eq!(store.proposal("c1").unwrap().unwrap().state, State::Waiting);
}

#[test]
fn accept_deny_refuses_a_prompt_steer_proposal_and_keeps_the_file() {
    let toml = STEER_TOML
        .replace(
            "[steer.trigger]\ntool = \"Bash\"\ncommand = '^\\s*sudo\\b'\n",
            "[steer.trigger]\nprompt = '(?i)review comments?'\n",
        )
        .replace(
            "action = \"inform\"\n",
            "action = \"inform\"\nat = \"stop\"\n",
        );
    let (_root, repo, store) = queued(&toml);
    let fixtures = repo.join(".oncebitten/rules/no-sudo/fixtures");
    fs::write(
        fixtures.join("bad.json"),
        r#"{"event": "prompt", "prompt": "address the review comments"}"#,
    )
    .unwrap();
    fs::write(
        fixtures.join("fixed.json"),
        r#"{"event": "prompt", "prompt": "run the tests"}"#,
    )
    .unwrap();
    assert_eq!(prove(&loaded(&repo)).verdict, Verdict::Proven);
    let before = rule_text(&repo);
    let error = proposals::accept_deny(&store, "c1", prove).unwrap_err();
    let text = format!("{error:#}");
    assert!(text.contains("prompt steer"), "{text}");
    assert!(text.contains("--deny needs a tool trigger"), "{text}");
    assert_eq!(rule_text(&repo), before);
    assert_eq!(store.proposal("c1").unwrap().unwrap().state, State::Waiting);
}
