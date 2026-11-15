mod common;

use std::cell::Cell;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use common::FakeForge;
use oncebitten::config::Inputs;
use oncebitten::delivery::{self, Proof, Target};
use oncebitten::evidence::{self, Cluster};
use oncebitten::forge;
use oncebitten::proof::{self, BackendKind, Replayed, Validity};
use oncebitten::proposals::{self, Recorded};
use oncebitten::replay::{LocalRunner, Verdict};
use oncebitten::rule::{self, Rule, Status};
use oncebitten::signal;
use oncebitten::store::{NewEvent, Store};
use oncebitten::triage::verdict::{self as triage_verdict, Answer};
use tempfile::TempDir;

const CHECKER: &str = env!("CARGO_BIN_EXE_oncebitten-checker");
const VERDICT: &str = r#""repeatable":true,"user_correction":false,"root_cause":"docs drift","proposed_fix_kind":"lint","generality":"project""#;
const RULE: &str = "docs-with-src";
const SOURCE: &str = "team";

fn git_in(dir: &Path, global: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", global)
        .output()
        .unwrap();
    assert!(output.status.success(), "git {args:?}: {output:?}");
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

/// A project repo with a bare `origin`, a bare rules remote cloned as the
/// writable source `team`, and isolated config and data directories.
struct World {
    root: TempDir,
    store: Store,
}

impl World {
    fn new(writable: bool) -> Self {
        let root = TempDir::new().unwrap();
        let path = root.path().to_path_buf();
        let path = path.as_path();
        let global = path.join("gitconfig");
        fs::write(
            &global,
            "[user]\n\tname = Test\n\temail = test@example.invalid\n[init]\n\tdefaultBranch = main\n[core]\n\tautocrlf = false\n",
        )
        .unwrap();
        let world = World {
            root,
            store: Store::open_in_memory().unwrap(),
        };
        // The project repo, with a bare origin it tracks.
        git_in(path, &global, &["init", "-q", "--bare", "origin.git"]);
        git_in(path, &global, &["init", "-q", "repo"]);
        let repo = world.repo();
        fs::write(repo.join("README.md"), "hi\n").unwrap();
        git_in(&repo, &global, &["add", "-A"]);
        git_in(&repo, &global, &["commit", "-q", "-m", "init"]);
        let origin = path.join("origin.git");
        git_in(
            &repo,
            &global,
            &["remote", "add", "origin", &origin.to_string_lossy()],
        );
        git_in(&repo, &global, &["push", "-q", "-u", "origin", "main"]);
        // The rules remote and its clone under the sources root.
        let seed = path.join("seed");
        git_in(path, &global, &["init", "-q", "seed"]);
        fs::write(seed.join("README.md"), "rules\n").unwrap();
        git_in(&seed, &global, &["add", "-A"]);
        git_in(&seed, &global, &["commit", "-q", "-m", "init"]);
        git_in(
            path,
            &global,
            &["clone", "-q", "--bare", "seed", "rules.git"],
        );
        let sources = path
            .join("home")
            .join("data")
            .join("oncebitten")
            .join("sources");
        fs::create_dir_all(&sources).unwrap();
        git_in(
            &sources,
            &global,
            &[
                "clone",
                "-q",
                &path.join("rules.git").to_string_lossy(),
                SOURCE,
            ],
        );
        let config = format!(
            "forge = \"github\"\n\n[[rules.sources]]\nname = \"{SOURCE}\"\nurl = \"{}\"\nwritable = {writable}\n",
            path.join("rules.git").to_string_lossy().replace('\\', "/")
        );
        fs::create_dir_all(repo.join(".oncebitten")).unwrap();
        fs::write(repo.join(".oncebitten").join("config.toml"), config).unwrap();
        world
    }

    fn repo(&self) -> PathBuf {
        self.root.path().join("repo")
    }

    fn global(&self) -> PathBuf {
        self.root.path().join("gitconfig")
    }

    fn clone_dir(&self) -> PathBuf {
        self.root
            .path()
            .join("home")
            .join("data")
            .join("oncebitten")
            .join("sources")
            .join(SOURCE)
    }

    fn git(&self, dir: &Path, args: &[&str]) -> String {
        git_in(dir, &self.global(), args)
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
            ("GIT_CONFIG_GLOBAL", self.global()),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v.to_string_lossy().into_owned()))
        .collect();
        Inputs {
            cwd: self.repo(),
            vars,
        }
    }

    fn rule_dir(&self) -> PathBuf {
        self.repo().join(".oncebitten").join("rules").join(RULE)
    }

    /// Queues a proven proposal for the docs rule.
    fn propose(&self) -> Cluster {
        let cluster = cluster(&self.store);
        let answer = answer_with(Some(draft_json()));
        let recorded =
            proposals::record(&self.store, &self.inputs(), &cluster, &answer, &proving, 10)
                .unwrap();
        assert_eq!(recorded, Recorded::Proposed { proven: true });
        cluster
    }

    fn deliver(
        &self,
        id: &str,
        to: Option<Target>,
        push: bool,
        prove: &dyn Fn(&Rule) -> Replayed,
        fake: &FakeForge,
    ) -> anyhow::Result<delivery::Delivered> {
        let program = fake.program().to_path_buf();
        let forge_for =
            move |kind: forge::Kind, dir: &Path| forge::backend_with(kind, program.clone(), dir);
        delivery::deliver(&self.store, &self.inputs(), id, to, push, prove, &forge_for)
    }
}

fn fail(store: &Store, session: &str, command: &str, ts: i64) {
    let event = NewEvent {
        ts,
        host: "codex".into(),
        session_id: session.into(),
        repo: Some("/r".into()),
        kind: signal::KIND_TOOL_ERROR.into(),
        tool: Some("Bash".into()),
        summary: command.into(),
    };
    let history = store.session_history("codex", session, 500).unwrap();
    let signals = signal::derive(&event, &history);
    store.record(&event, &signals).unwrap().unwrap();
}

fn cluster(store: &Store) -> Cluster {
    fail(store, "s1", "cargo doc", 1);
    fail(store, "s2", "cargo doc", 2);
    evidence::load(store).unwrap().into_iter().next().unwrap()
}

fn draft_json() -> serde_json::Value {
    serde_json::json!({
        "id": RULE,
        "description": "a change under src needs a docs change",
        "mode": "block",
        "engine": "diff",
        "when_changed": ["src/**"],
        "require_changed": ["docs/**"],
        "bad": {"name": "bad.diff", "content": "src/lib.rs\n"},
        "fixed": {"name": "fixed.diff", "content": "src/lib.rs\ndocs/a.md\n"},
        "corpus": [{"name": "readme-only.diff", "content": "README.md\n"}],
        "rationale": "agents forgot docs twice"
    })
}

fn answer_with(draft: Option<serde_json::Value>) -> Answer {
    let text = match draft {
        Some(draft) => format!("{{{VERDICT},\"draft\":{draft}}}"),
        None => format!("{{{VERDICT}}}"),
    };
    triage_verdict::parse_answer(&text).unwrap()
}

fn proving(rule: &Rule) -> Replayed {
    proof::replay_rule(
        rule,
        Path::new(CHECKER),
        &LocalRunner,
        BackendKind::OpenShell,
    )
}

fn no_sandbox(_rule: &Rule) -> Replayed {
    Replayed {
        verdict: Verdict::CouldNotRun("openshell is not installed".into()),
        written: None,
    }
}

fn never(_rule: &Rule) -> Replayed {
    panic!("a valid proof must not be replayed")
}

const OPEN_PR: &str = r#"[{"number":7,"url":"https://github.com/o/r/pull/7"}]"#;

#[test]
fn an_open_pr_makes_pr_the_default_target_and_only_the_rule_dir_is_committed() {
    let world = World::new(true);
    let cluster = world.propose();
    let repo = world.repo();
    fs::write(repo.join("unrelated.txt"), "keep out\n").unwrap();
    world.git(&repo, &["add", "unrelated.txt"]);
    let fake = FakeForge::new("gh");
    fake.set("reply", OPEN_PR);

    let done = world
        .deliver(&cluster.id, None, false, &never, &fake)
        .unwrap();

    assert_eq!(done.target, Target::Pr);
    assert_eq!(done.proof, Proof::Proven);
    assert_eq!(
        done.pr_url.as_deref(),
        Some("https://github.com/o/r/pull/7")
    );
    let subject = world.git(&repo, &["log", "-1", "--format=%s"]);
    assert_eq!(subject, format!("chore(oncebitten): propose rule {RULE}"));
    let body = world.git(&repo, &["log", "-1", "--format=%b"]);
    assert!(body.contains(&format!("Cluster: {}", cluster.id)), "{body}");
    assert!(
        body.contains("Rationale: agents forgot docs twice"),
        "{body}"
    );
    assert!(body.contains("Proof: proven"), "{body}");
    let files = world.git(&repo, &["show", "--name-only", "--format=", "HEAD"]);
    assert!(
        files.contains(&format!(".oncebitten/rules/{RULE}/rule.toml")),
        "{files}"
    );
    assert!(!files.contains("unrelated.txt"), "{files}");
    // Delivery never activates the rule.
    let rule = rule::load_rule(&world.rule_dir()).unwrap();
    assert_eq!(rule.status, Status::Proposed);
    let row = world.store.proposal(&cluster.id).unwrap().unwrap();
    assert_eq!(row.target.as_deref(), Some("pr"));
    assert_eq!(row.commit.as_deref(), Some(done.commit.as_str()));
    assert_eq!(row.pr_url.as_deref(), Some("https://github.com/o/r/pull/7"));
}

#[test]
fn no_open_pr_makes_rules_repo_the_default_target() {
    let world = World::new(true);
    let cluster = world.propose();
    let fake = FakeForge::new("gh");
    fake.set("reply", "[]");

    let done = world
        .deliver(&cluster.id, None, false, &never, &fake)
        .unwrap();

    let branch = format!("oncebitten/propose-{RULE}");
    assert_eq!(done.target, Target::RulesRepo);
    assert_eq!(done.branch.as_deref(), Some(branch.as_str()));
    assert!(done.push_hint.as_deref().unwrap().contains(&branch));
    let clone = world.clone_dir();
    assert_eq!(
        world.git(&clone, &["rev-parse", "--abbrev-ref", "HEAD"]),
        branch
    );
    assert_eq!(world.git(&clone, &["rev-parse", "HEAD"]), done.commit);
    let files = world.git(&clone, &["show", "--name-only", "--format=", "HEAD"]);
    assert!(files.contains(&format!("{RULE}/rule.toml")), "{files}");
    // The project repo got no commit.
    assert_eq!(world.git(&world.repo(), &["log", "--format=%s"]), "init");
    let row = world.store.proposal(&cluster.id).unwrap().unwrap();
    assert_eq!(row.target.as_deref(), Some("rules-repo"));
}

#[test]
fn a_forge_failure_falls_back_to_a_writable_source_and_errors_without_one() {
    let fake = FakeForge::new("gh");
    fake.set("status", "1");
    fake.set("stderr", "gh: not logged in\n");

    let world = World::new(true);
    let cluster = world.propose();
    let done = world
        .deliver(&cluster.id, None, false, &never, &fake)
        .unwrap();
    assert_eq!(done.target, Target::RulesRepo);

    let world = World::new(false);
    let cluster = world.propose();
    let error = world
        .deliver(&cluster.id, None, false, &never, &fake)
        .unwrap_err();
    assert!(format!("{error:#}").contains("not logged in"), "{error:#}");
    assert_eq!(world.git(&world.repo(), &["log", "--format=%s"]), "init");
}

#[test]
fn rules_repo_without_a_writable_source_names_the_key() {
    let world = World::new(false);
    let cluster = world.propose();
    let fake = FakeForge::new("gh");

    let error = world
        .deliver(&cluster.id, Some(Target::RulesRepo), false, &never, &fake)
        .unwrap_err();

    assert!(
        format!("{error:#}").contains("rules.sources[].writable"),
        "{error:#}"
    );
}

#[test]
fn findings_and_rejected_proposals_are_refused() {
    let world = World::new(true);
    let cluster = cluster(&world.store);
    let recorded = proposals::record(
        &world.store,
        &world.inputs(),
        &cluster,
        &answer_with(None),
        &proving,
        10,
    )
    .unwrap();
    assert_eq!(recorded, Recorded::Finding);
    let fake = FakeForge::new("gh");
    let error = world
        .deliver(&cluster.id, Some(Target::Pr), false, &never, &fake)
        .unwrap_err();
    assert!(format!("{error:#}").contains("finding"), "{error:#}");

    let world = World::new(true);
    let cluster = world.propose();
    proposals::reject(&world.store, &cluster.id, "not wanted", 20).unwrap();
    let error = world
        .deliver(&cluster.id, Some(Target::Pr), false, &never, &fake)
        .unwrap_err();
    assert!(format!("{error:#}").contains("rejected"), "{error:#}");
}

#[test]
fn a_stale_proof_is_replayed_and_an_unproven_rule_is_still_delivered() {
    let world = World::new(true);
    let cluster = world.propose();
    let file = world.rule_dir().join("rule.toml");
    let text = fs::read_to_string(&file).unwrap();
    fs::write(
        &file,
        text.replace("a change under src", "any change under src"),
    )
    .unwrap();
    let rule = rule::load_rule(&world.rule_dir()).unwrap();
    assert!(matches!(proof::validity(&rule), Validity::Stale(_)));
    let calls = Cell::new(0);
    let counting = |rule: &Rule| {
        calls.set(calls.get() + 1);
        no_sandbox(rule)
    };
    let fake = FakeForge::new("gh");

    let done = world
        .deliver(&cluster.id, Some(Target::Pr), false, &counting, &fake)
        .unwrap();

    assert_eq!(calls.get(), 1);
    let Proof::Unproven(reason) = &done.proof else {
        panic!("expected unproven, got {:?}", done.proof)
    };
    assert!(reason.contains("openshell is not installed"), "{reason}");
    let body = world.git(&world.repo(), &["log", "-1", "--format=%b"]);
    assert!(body.contains("unproven: replay could not run"), "{body}");
    let row = world.store.proposal(&cluster.id).unwrap().unwrap();
    assert!(!row.proven);
    assert_eq!(
        rule::load_rule(&world.rule_dir()).unwrap().status,
        Status::Proposed
    );
}

#[test]
fn a_stale_proof_replayed_to_proven_is_delivered_as_proven() {
    let world = World::new(true);
    let cluster = world.propose();
    let file = world.rule_dir().join("rule.toml");
    let text = fs::read_to_string(&file).unwrap();
    fs::write(
        &file,
        text.replace("a change under src", "any change under src"),
    )
    .unwrap();
    let fake = FakeForge::new("gh");

    let done = world
        .deliver(&cluster.id, Some(Target::Pr), false, &proving, &fake)
        .unwrap();

    assert_eq!(done.proof, Proof::Proven);
    assert!(world.store.proposal(&cluster.id).unwrap().unwrap().proven);
}

#[test]
fn delivering_twice_to_the_same_branch_has_nothing_to_commit() {
    let world = World::new(true);
    let cluster = world.propose();
    let fake = FakeForge::new("gh");
    world
        .deliver(&cluster.id, Some(Target::Pr), false, &never, &fake)
        .unwrap();

    let error = world
        .deliver(&cluster.id, Some(Target::Pr), false, &never, &fake)
        .unwrap_err();

    assert!(
        format!("{error:#}").contains("nothing to commit"),
        "{error:#}"
    );
}

#[test]
fn push_sends_a_pr_commit_upstream() {
    let world = World::new(true);
    let cluster = world.propose();
    let fake = FakeForge::new("gh");

    let done = world
        .deliver(&cluster.id, Some(Target::Pr), true, &never, &fake)
        .unwrap();

    assert!(done.pushed);
    let origin = world.root.path().join("origin.git");
    assert_eq!(world.git(&origin, &["rev-parse", "main"]), done.commit);
}

#[test]
fn push_sends_a_rules_repo_branch_and_opens_a_pr() {
    let world = World::new(true);
    let cluster = world.propose();
    let fake = FakeForge::new("gh");
    fake.set("reply", "https://github.com/o/rules/pull/3\n");

    let done = world
        .deliver(&cluster.id, Some(Target::RulesRepo), true, &never, &fake)
        .unwrap();

    let branch = format!("oncebitten/propose-{RULE}");
    let remote = world.root.path().join("rules.git");
    assert_eq!(world.git(&remote, &["rev-parse", &branch]), done.commit);
    assert_eq!(
        done.pr_url.as_deref(),
        Some("https://github.com/o/rules/pull/3")
    );
    let calls = fake.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0][..2], ["pr", "create"]);
    assert!(calls[0].contains(&branch));
    assert!(calls[0].iter().any(|arg| arg == "main"), "{:?}", calls[0]);
    assert!(calls[0].iter().any(|arg| arg.contains("Proof: proven")));
    let row = world.store.proposal(&cluster.id).unwrap().unwrap();
    assert_eq!(
        row.pr_url.as_deref(),
        Some("https://github.com/o/rules/pull/3")
    );
}

#[test]
fn an_old_proposals_table_gains_the_delivery_columns() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("events.db");
    {
        let store = Store::open(&path).unwrap();
        drop(store);
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch(
            "DROP TABLE proposals;
             CREATE TABLE proposals (cluster TEXT PRIMARY KEY, ts INTEGER NOT NULL, repo TEXT NOT NULL, kind TEXT NOT NULL, state TEXT NOT NULL, rule TEXT, dir TEXT, proven INTEGER NOT NULL, note TEXT, verdict TEXT NOT NULL, rationale TEXT, occurrences TEXT NOT NULL);
             INSERT INTO proposals VALUES ('c1', 1, '/r', 'finding', 'waiting', NULL, NULL, 0, NULL, '{}', NULL, '[]');",
        )
        .unwrap();
    }
    let store = Store::open(&path).unwrap();
    let row = store.proposal("c1").unwrap().unwrap();
    assert_eq!((row.target, row.commit, row.pr_url), (None, None, None));
    Store::open(&path).unwrap();
}

#[test]
fn cli_deliver_of_an_unknown_proposal_exits_2() {
    let repo = common::Repo::new();
    let output = repo.run(&["proposals", "deliver", "no-such-cluster"], &[]);
    assert_eq!(common::code(&output), 2, "{}", common::both(&output));
    assert!(
        common::stderr(&output).contains("no proposal"),
        "{}",
        common::both(&output)
    );
}
