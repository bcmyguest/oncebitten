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
use oncebitten::store::{HitsRow, NewEvent, Store};
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

    /// Where a drafted rule lands: the local layer.
    fn draft_dir(&self) -> PathBuf {
        self.repo()
            .join(".oncebitten")
            .join("rules.local")
            .join(RULE)
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
    let file = world.draft_dir().join("rule.toml");
    let text = fs::read_to_string(&file).unwrap();
    fs::write(
        &file,
        text.replace("a change under src", "any change under src"),
    )
    .unwrap();
    let rule = rule::load_rule(&world.draft_dir()).unwrap();
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
    let file = world.draft_dir().join("rule.toml");
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

#[test]
fn the_pr_body_states_the_existing_hits() {
    let world = World::new(true);
    let cluster = world.propose();
    world
        .store
        .set_proposal_hits(
            &cluster.id,
            &HitsRow {
                count: 2,
                files: 1,
                samples: vec!["src/lib.rs:3: x.unwrap()".to_owned()],
            },
        )
        .unwrap();
    let fake = FakeForge::new("gh");
    fake.set("reply", "https://github.com/o/rules/pull/4\n");

    world
        .deliver(&cluster.id, Some(Target::RulesRepo), true, &never, &fake)
        .unwrap();

    let calls = fake.calls();
    assert!(
        calls[0]
            .iter()
            .any(|arg| arg.contains("Hits: 2 hits in 1 file")),
        "{:?}",
        calls[0]
    );
    assert!(calls[0]
        .iter()
        .any(|arg| arg.contains("existing hits will be baselined")));
}

#[test]
fn a_rules_repo_pr_body_limits_the_baseline_to_the_proposals_repo() {
    let world = World::new(true);
    let cluster = world.propose();
    world
        .store
        .set_proposal_hits(
            &cluster.id,
            &HitsRow {
                count: 2,
                files: 1,
                samples: vec!["src/lib.rs:3: x.unwrap()".to_owned()],
            },
        )
        .unwrap();
    let fake = FakeForge::new("gh");
    fake.set("reply", "https://github.com/o/rules/pull/4\n");

    world
        .deliver(&cluster.id, Some(Target::RulesRepo), true, &never, &fake)
        .unwrap();

    let calls = fake.calls();
    let body = calls[0]
        .iter()
        .find(|arg| arg.contains("existing hits will be baselined"))
        .unwrap_or_else(|| panic!("{:?}", calls[0]));
    let line = body
        .lines()
        .find(|line| line.contains("existing hits will be baselined"))
        .unwrap();
    assert!(
        line.contains(&format!("proposals accept {}", cluster.id)),
        "{line}"
    );
    assert!(line.contains("other repos"), "{line}");
}

#[test]
fn deliver_pr_moves_a_local_rule_into_the_project_layer() {
    let world = World::new(true);
    let cluster = world.propose();
    assert!(world.draft_dir().join("rule.toml").exists());
    let fake = FakeForge::new("gh");

    world
        .deliver(&cluster.id, Some(Target::Pr), false, &never, &fake)
        .unwrap();

    assert!(!world.draft_dir().exists());
    let files = world.git(&world.repo(), &["show", "--name-only", "--format=", "HEAD"]);
    assert!(
        files.contains(&format!(".oncebitten/rules/{RULE}/rule.toml")),
        "{files}"
    );
    let row = world.store.proposal(&cluster.id).unwrap().unwrap();
    assert_eq!(
        fs::canonicalize(row.dir.as_ref().unwrap()).unwrap(),
        fs::canonicalize(world.rule_dir()).unwrap()
    );
    let listed = oncebitten::rules::load(&world.inputs(), false).unwrap();
    let entry = listed
        .iter()
        .find(|entry| entry.id == RULE)
        .unwrap_or_else(|| {
            panic!(
                "no project rule: {:?}",
                listed.iter().map(|e| &e.id).collect::<Vec<_>>()
            )
        });
    assert_eq!(entry.layer, oncebitten::registry::Layer::Project);
}

#[test]
fn deliver_to_rules_repo_copies_the_rule_and_keeps_the_local_copy() {
    let world = World::new(true);
    let cluster = world.propose();
    let fake = FakeForge::new("gh");

    world
        .deliver(&cluster.id, Some(Target::RulesRepo), false, &never, &fake)
        .unwrap();

    let files = world.git(
        &world.clone_dir(),
        &["show", "--name-only", "--format=", "HEAD"],
    );
    assert!(files.contains(&format!("{RULE}/rule.toml")), "{files}");
    assert!(world.draft_dir().join("rule.toml").exists());
    assert!(!world.rule_dir().exists());
    let row = world.store.proposal(&cluster.id).unwrap().unwrap();
    assert_eq!(
        fs::canonicalize(row.dir.as_ref().unwrap()).unwrap(),
        fs::canonicalize(world.draft_dir()).unwrap()
    );
}

#[cfg(unix)]
fn fail_commits(repo: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let hook = repo.join(".git").join("hooks").join("pre-commit");
    fs::create_dir_all(hook.parent().unwrap()).unwrap();
    fs::write(&hook, "#!/bin/sh\necho hook said no >&2\nexit 1\n").unwrap();
    fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
}

#[cfg(unix)]
#[test]
fn a_failed_commit_unstages_and_restores_the_local_rule() {
    let world = World::new(true);
    let cluster = world.propose();
    let repo = world.repo();
    fail_commits(&repo);
    let fake = FakeForge::new("gh");

    let error = world
        .deliver(&cluster.id, Some(Target::Pr), false, &never, &fake)
        .unwrap_err();

    assert!(format!("{error:#}").contains("hook said no"), "{error:#}");
    assert!(world.draft_dir().join("rule.toml").exists());
    assert!(!world.rule_dir().exists());
    let status = world.git(&repo, &["status", "--porcelain"]);
    let staged: Vec<&str> = status
        .lines()
        .filter(|line| !line.starts_with("??"))
        .collect();
    assert!(staged.is_empty(), "{status}");
    let row = world.store.proposal(&cluster.id).unwrap().unwrap();
    assert_eq!(
        fs::canonicalize(row.dir.as_ref().unwrap()).unwrap(),
        fs::canonicalize(world.draft_dir()).unwrap()
    );
}

#[test]
fn a_local_rule_whose_id_a_shared_rule_holds_is_not_delivered_to_the_pr() {
    let world = World::new(true);
    let cluster = world.propose();
    common::copy_dir(&world.draft_dir(), &world.clone_dir().join(RULE));
    let fake = FakeForge::new("gh");

    let error = world
        .deliver(&cluster.id, Some(Target::Pr), false, &never, &fake)
        .unwrap_err();

    assert!(
        format!("{error:#}").contains(&format!("{SOURCE}/{RULE}")),
        "{error:#}"
    );
    assert!(world.draft_dir().join("rule.toml").exists());
    assert!(!world.rule_dir().exists());
    assert_eq!(world.git(&world.repo(), &["log", "--format=%s"]), "init");
}

const LOCAL_HIT: &str =
    r#"{"entries":[{"rule":"local/docs-with-src","path":"src/a.rs","sha256":"abc"}]}"#;
const LOCAL_EXCEPTION: &str = "# keep\n[[exception]]\nrule = \"local/docs-with-src\"\npaths = [\"src/**\"]\nreason = \"legacy\"\nexpires = \"2999-01-01\"\n";

fn oncebitten_file(world: &World, name: &str) -> PathBuf {
    world.repo().join(".oncebitten").join(name)
}

#[test]
fn deliver_pr_moves_local_baseline_entries_and_exceptions_to_the_shared_files() {
    let world = World::new(true);
    let cluster = world.propose();
    fs::write(oncebitten_file(&world, "baseline.local.json"), LOCAL_HIT).unwrap();
    fs::write(oncebitten_file(&world, "exceptions.toml"), LOCAL_EXCEPTION).unwrap();
    let fake = FakeForge::new("gh");

    world
        .deliver(&cluster.id, Some(Target::Pr), false, &never, &fake)
        .unwrap();

    let shared = fs::read_to_string(oncebitten_file(&world, "baseline.json")).unwrap();
    assert!(shared.contains("\"rule\": \"docs-with-src\""), "{shared}");
    assert!(!shared.contains("local/"), "{shared}");
    let local = fs::read_to_string(oncebitten_file(&world, "baseline.local.json")).unwrap();
    assert!(!local.contains("docs-with-src"), "{local}");
    let exceptions = fs::read_to_string(oncebitten_file(&world, "exceptions.toml")).unwrap();
    assert!(
        exceptions.contains("rule = \"docs-with-src\""),
        "{exceptions}"
    );
    assert!(exceptions.contains("# keep"), "{exceptions}");
    let files = world.git(&world.repo(), &["show", "--name-only", "--format=", "HEAD"]);
    assert!(files.contains(".oncebitten/baseline.json"), "{files}");
    assert!(files.contains(".oncebitten/exceptions.toml"), "{files}");
    assert!(
        files.contains(&format!(".oncebitten/rules/{RULE}/rule.toml")),
        "{files}"
    );
}

#[test]
fn deliver_to_rules_repo_leaves_the_baseline_and_exception_files_alone() {
    let world = World::new(true);
    let cluster = world.propose();
    fs::write(oncebitten_file(&world, "baseline.local.json"), LOCAL_HIT).unwrap();
    fs::write(oncebitten_file(&world, "exceptions.toml"), LOCAL_EXCEPTION).unwrap();
    let fake = FakeForge::new("gh");

    world
        .deliver(&cluster.id, Some(Target::RulesRepo), false, &never, &fake)
        .unwrap();

    assert_eq!(
        fs::read_to_string(oncebitten_file(&world, "baseline.local.json")).unwrap(),
        LOCAL_HIT
    );
    assert_eq!(
        fs::read_to_string(oncebitten_file(&world, "exceptions.toml")).unwrap(),
        LOCAL_EXCEPTION
    );
    assert!(!oncebitten_file(&world, "baseline.json").exists());
}

#[cfg(unix)]
#[test]
fn a_failed_commit_restores_the_baseline_and_exception_files() {
    let world = World::new(true);
    let cluster = world.propose();
    fs::write(oncebitten_file(&world, "baseline.local.json"), LOCAL_HIT).unwrap();
    fs::write(oncebitten_file(&world, "exceptions.toml"), LOCAL_EXCEPTION).unwrap();
    fail_commits(&world.repo());
    let fake = FakeForge::new("gh");

    world
        .deliver(&cluster.id, Some(Target::Pr), false, &never, &fake)
        .unwrap_err();

    assert_eq!(
        fs::read_to_string(oncebitten_file(&world, "baseline.local.json")).unwrap(),
        LOCAL_HIT
    );
    assert_eq!(
        fs::read_to_string(oncebitten_file(&world, "exceptions.toml")).unwrap(),
        LOCAL_EXCEPTION
    );
    assert!(!oncebitten_file(&world, "baseline.json").exists());
    let status = world.git(&world.repo(), &["status", "--porcelain"]);
    assert!(
        status.lines().all(|line| line.starts_with("??")),
        "{status}"
    );
}

#[test]
fn the_pr_commit_carries_only_the_promoted_rules_records() {
    let world = World::new(true);
    let cluster = world.propose();
    let repo = world.repo();
    let committed = "[[exception]]\nrule = \"team/old\"\npaths = [\"**\"]\nreason = \"old\"\nexpires = \"2999-01-01\"\n";
    fs::write(oncebitten_file(&world, "exceptions.toml"), committed).unwrap();
    world.git(&repo, &["add", ".oncebitten/exceptions.toml"]);
    world.git(&repo, &["commit", "-q", "-m", "exceptions"]);
    let other = "\n[[exception]]\nrule = \"local/other\"\npaths = [\"**\"]\nreason = \"mine\"\nexpires = \"2999-01-01\"\n";
    let ours = LOCAL_EXCEPTION.replace("# keep\n", "\n");
    fs::write(
        oncebitten_file(&world, "exceptions.toml"),
        format!("{committed}{other}{ours}"),
    )
    .unwrap();
    fs::write(
        oncebitten_file(&world, "baseline.json"),
        r#"{"entries":[{"rule":"other","path":"src/x.rs","sha256":"def"}]}"#,
    )
    .unwrap();
    fs::write(oncebitten_file(&world, "baseline.local.json"), LOCAL_HIT).unwrap();
    let fake = FakeForge::new("gh");

    world
        .deliver(&cluster.id, Some(Target::Pr), false, &never, &fake)
        .unwrap();

    let head_baseline = world.git(&repo, &["show", "HEAD:.oncebitten/baseline.json"]);
    assert!(
        head_baseline.contains("\"docs-with-src\""),
        "{head_baseline}"
    );
    assert!(!head_baseline.contains("other"), "{head_baseline}");
    let head_exceptions = world.git(&repo, &["show", "HEAD:.oncebitten/exceptions.toml"]);
    assert!(head_exceptions.contains("team/old"), "{head_exceptions}");
    assert!(
        head_exceptions.contains("rule = \"docs-with-src\""),
        "{head_exceptions}"
    );
    assert!(!head_exceptions.contains("other"), "{head_exceptions}");
    let work_baseline = fs::read_to_string(oncebitten_file(&world, "baseline.json")).unwrap();
    assert!(
        work_baseline.contains("\"other\"") && work_baseline.contains("\"docs-with-src\""),
        "{work_baseline}"
    );
    let work_exceptions = fs::read_to_string(oncebitten_file(&world, "exceptions.toml")).unwrap();
    assert!(work_exceptions.contains("local/other"), "{work_exceptions}");
    assert!(
        work_exceptions.contains("rule = \"docs-with-src\""),
        "{work_exceptions}"
    );
    let status = world.git(&repo, &["status", "--porcelain"]);
    assert!(status.contains("M .oncebitten/exceptions.toml"), "{status}");
    assert!(status.contains("M .oncebitten/baseline.json"), "{status}");
}
