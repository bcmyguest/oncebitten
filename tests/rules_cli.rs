use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use tempfile::TempDir;

struct Fixture {
    root: TempDir,
}

impl Fixture {
    fn new() -> Self {
        let root = TempDir::new().unwrap();
        fs::write(root.path().join("empty.gitconfig"), "").unwrap();
        Fixture { root }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.root.path().join(name)
    }

    fn oncebitten(&self, cwd: &Path, args: &[&str]) -> Output {
        let path = std::env::var_os("PATH").unwrap_or_default();
        Command::new(env!("CARGO_BIN_EXE_oncebitten"))
            .args(args)
            .current_dir(cwd)
            .env_clear()
            .env("PATH", path)
            .env("HOME", self.path("home"))
            .env("XDG_CONFIG_HOME", self.path("xdg-config"))
            .env("XDG_DATA_HOME", self.path("data"))
            .env("GIT_CONFIG_GLOBAL", self.path("empty.gitconfig"))
            .output()
            .unwrap()
    }

    fn git(&self, dir: &Path, args: &[&str]) {
        let output = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .env("GIT_CONFIG_GLOBAL", self.path("empty.gitconfig"))
            .output()
            .unwrap();
        assert!(output.status.success(), "git {args:?}");
    }

    fn git_output(&self, dir: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .env("GIT_CONFIG_GLOBAL", self.path("empty.gitconfig"))
            .output()
            .unwrap();
        assert!(output.status.success(), "git {args:?}");
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    }

    fn make_repo(&self, dir: &Path) {
        fs::create_dir_all(dir).unwrap();
        self.git(dir, &["init", "-q"]);
        self.git(dir, &["config", "user.name", "Test"]);
        self.git(dir, &["config", "user.email", "test@example.invalid"]);
    }

    fn commit_rule(&self, dir: &Path, id: &str) {
        write_rule(dir, id, "block", "active");
        self.git(dir, &["add", "-A"]);
        self.git(dir, &["commit", "-q", "-m", id]);
    }

    fn url(&self, dir: &Path) -> String {
        let path = dir.display().to_string().replace('\\', "/");
        if path.starts_with('/') {
            format!("file://{path}")
        } else {
            format!("file:///{path}")
        }
    }
}

fn write_rule(parent: &Path, id: &str, mode: &str, status: &str) {
    let rule = parent.join(id);
    fs::create_dir_all(rule.join("fixtures")).unwrap();
    fs::write(
        rule.join("rule.toml"),
        format!(
            "id = \"{id}\"\ndescription = \"Rule {id}.\"\nmode = \"{mode}\"\n\
             status = \"{status}\"\nengine = \"ast-grep\"\n"
        ),
    )
    .unwrap();
    fs::write(rule.join("check.yml"), "id: x\n").unwrap();
    fs::write(rule.join("fixtures/bad.rs"), "x.unwrap()\n").unwrap();
    fs::write(rule.join("fixtures/fixed.rs"), "x?\n").unwrap();
}

struct Setup {
    fx: Fixture,
    team: PathBuf,
    project: PathBuf,
}

fn setup() -> Setup {
    let fx = Fixture::new();
    let team = fx.path("team-src");
    fx.make_repo(&team);
    fx.commit_rule(&team, "no-unwrap");
    let project = fx.path("project");
    fx.make_repo(&project);
    fs::create_dir_all(project.join(".oncebitten")).unwrap();
    fs::write(
        project.join(".oncebitten/config.toml"),
        format!(
            "[[rules.sources]]\nname = \"team\"\nurl = \"{}\"\n",
            fx.url(&team)
        ),
    )
    .unwrap();
    write_rule(
        &project.join(".oncebitten/rules"),
        "local-rule",
        "block",
        "active",
    );
    Setup { fx, team, project }
}

fn code(output: &Output) -> i32 {
    output.status.code().unwrap()
}

fn stdout(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).unwrap()
}

fn stderr(output: &Output) -> String {
    String::from_utf8(output.stderr.clone()).unwrap()
}

#[test]
fn list_prints_each_rule_with_its_origin() {
    let s = setup();
    assert_eq!(code(&s.fx.oncebitten(&s.project, &["rules", "sync"])), 0);

    let output = s.fx.oncebitten(&s.project, &["rules", "list"]);

    assert_eq!(code(&output), 0, "{}", stderr(&output));
    let out = stdout(&output);
    assert!(out.contains("local-rule  block  active  project"), "{out}");
    assert!(out.contains("team/no-unwrap  block  active  team"), "{out}");
}

#[test]
fn list_shows_a_local_rule_with_origin_local() {
    let s = setup();
    assert_eq!(code(&s.fx.oncebitten(&s.project, &["rules", "sync"])), 0);
    write_rule(
        &s.project.join(".oncebitten/rules.local"),
        "mine",
        "block",
        "active",
    );

    let output = s.fx.oncebitten(&s.project, &["rules", "list"]);

    assert_eq!(code(&output), 0, "{}", stderr(&output));
    let out = stdout(&output);
    assert!(out.contains("local/mine  block  active  local"), "{out}");
}

#[test]
fn ci_never_reads_the_local_layer() {
    let s = setup();
    assert_eq!(code(&s.fx.oncebitten(&s.project, &["rules", "sync"])), 0);
    let broken = s.project.join(".oncebitten/rules.local/broken");
    fs::create_dir_all(&broken).unwrap();
    fs::write(broken.join("rule.toml"), "id = \"broken\"\n").unwrap();

    let list = s.fx.oncebitten(&s.project, &["rules", "list", "--ci"]);
    assert_eq!(code(&list), 0, "{}", stderr(&list));
    assert!(!stdout(&list).contains("local/"), "{}", stdout(&list));

    let validate = s.fx.oncebitten(&s.project, &["rules", "validate", "--ci"]);
    assert_eq!(code(&validate), 0, "{}", stderr(&validate));

    let local = s.fx.oncebitten(&s.project, &["rules", "validate"]);
    assert_eq!(code(&local), 1, "{}", stderr(&local));
}

#[test]
fn sync_writes_the_local_gitignore_once() {
    let s = setup();
    assert_eq!(code(&s.fx.oncebitten(&s.project, &["rules", "sync"])), 0);
    let gitignore = s.project.join(".oncebitten/.gitignore");
    assert_eq!(
        fs::read_to_string(&gitignore).unwrap(),
        "rules.local/\nbaseline.local.json\n"
    );

    assert_eq!(code(&s.fx.oncebitten(&s.project, &["rules", "sync"])), 0);
    assert_eq!(
        fs::read_to_string(&gitignore).unwrap(),
        "rules.local/\nbaseline.local.json\n"
    );
}

#[test]
fn sync_keeps_existing_local_gitignore_lines() {
    let s = setup();
    fs::write(
        s.project.join(".oncebitten/.gitignore"),
        "baseline.json\nno-newline-at-eof",
    )
    .unwrap();

    assert_eq!(code(&s.fx.oncebitten(&s.project, &["rules", "sync"])), 0);
    assert_eq!(code(&s.fx.oncebitten(&s.project, &["rules", "sync"])), 0);

    let text = fs::read_to_string(s.project.join(".oncebitten/.gitignore")).unwrap();
    assert_eq!(
        text,
        "baseline.json\nno-newline-at-eof\nrules.local/\nbaseline.local.json\n"
    );
}

#[test]
fn validate_names_the_broken_file_and_exits_1() {
    let s = setup();
    assert_eq!(code(&s.fx.oncebitten(&s.project, &["rules", "sync"])), 0);
    fs::write(
        s.project.join(".oncebitten/rules/local-rule/rule.toml"),
        "id = \"local-rule\"\n",
    )
    .unwrap();

    let output = s.fx.oncebitten(&s.project, &["rules", "validate"]);

    assert_eq!(code(&output), 1);
    assert!(
        stderr(&output).contains(
            &std::path::Path::new("local-rule")
                .join("rule.toml")
                .display()
                .to_string()
        ),
        "{}",
        stderr(&output)
    );
}

#[test]
fn validate_ci_passes_at_the_locked_commit() {
    let s = setup();
    assert_eq!(code(&s.fx.oncebitten(&s.project, &["rules", "sync"])), 0);

    let output = s.fx.oncebitten(&s.project, &["rules", "validate", "--ci"]);

    assert_eq!(code(&output), 0, "{}", stderr(&output));
}

#[test]
fn validate_ci_without_a_lockfile_exits_2() {
    let s = setup();

    let output = s.fx.oncebitten(&s.project, &["rules", "validate", "--ci"]);

    assert_eq!(code(&output), 2, "{}", stderr(&output));
}

#[test]
fn validate_ci_with_a_drifted_clone_exits_2_and_does_not_fetch() {
    let s = setup();
    assert_eq!(code(&s.fx.oncebitten(&s.project, &["rules", "sync"])), 0);
    let clone = s.fx.path("data/oncebitten/sources/team");
    s.fx.commit_rule(&s.team, "later");
    s.fx.git(&clone, &["fetch", "-q", "origin"]);
    s.fx.git(&clone, &["checkout", "-q", "--detach", "origin/HEAD"]);
    s.fx.commit_rule(&s.team, "newer");
    let before = s.fx.git_output(&clone, &["rev-parse", "origin/HEAD"]);

    let output = s.fx.oncebitten(&s.project, &["rules", "validate", "--ci"]);

    assert_eq!(code(&output), 2, "{}", stderr(&output));
    assert_eq!(
        s.fx.git_output(&clone, &["rev-parse", "origin/HEAD"]),
        before
    );
}

#[test]
fn sync_frozen_checks_out_the_locked_commit_and_exits_0() {
    let s = setup();
    assert_eq!(code(&s.fx.oncebitten(&s.project, &["rules", "sync"])), 0);
    let lock = fs::read_to_string(s.project.join(".oncebitten/rules.lock")).unwrap();
    s.fx.commit_rule(&s.team, "later");

    let output = s.fx.oncebitten(&s.project, &["rules", "sync", "--frozen"]);

    assert_eq!(code(&output), 0, "{}", stderr(&output));
    assert_eq!(
        fs::read_to_string(s.project.join(".oncebitten/rules.lock")).unwrap(),
        lock
    );
}

#[test]
fn sync_frozen_without_a_lockfile_exits_2() {
    let s = setup();

    let output = s.fx.oncebitten(&s.project, &["rules", "sync", "--frozen"]);

    assert_eq!(code(&output), 2, "{}", stderr(&output));
    assert!(
        stderr(&output).contains("rules.lock"),
        "{}",
        stderr(&output)
    );
    assert!(!s.project.join(".oncebitten/rules.lock").exists());
}

fn use_examples(s: &Setup, ids: &str) {
    fs::write(
        s.project.join(".oncebitten/config.toml"),
        format!(
            "[rules]\nuse_examples = {ids}\n\n[[rules.sources]]\nname = \"team\"\nurl = \"{}\"\n",
            s.fx.url(&s.team)
        ),
    )
    .unwrap();
}

#[test]
fn list_omits_examples_that_are_not_listed() {
    let s = setup();
    assert_eq!(code(&s.fx.oncebitten(&s.project, &["rules", "sync"])), 0);

    let output = s.fx.oncebitten(&s.project, &["rules", "list"]);

    assert_eq!(code(&output), 0, "{}", stderr(&output));
    assert!(
        !stdout(&output).contains("examples/"),
        "{}",
        stdout(&output)
    );
}

#[test]
fn list_shows_a_listed_example_as_active() {
    let s = setup();
    use_examples(&s, "[\"py-no-print\"]");
    assert_eq!(code(&s.fx.oncebitten(&s.project, &["rules", "sync"])), 0);

    let output = s.fx.oncebitten(&s.project, &["rules", "list"]);

    assert_eq!(code(&output), 0, "{}", stderr(&output));
    let out = stdout(&output);
    assert!(out.contains("examples/py-no-print"), "{out}");
    assert!(out.contains("  active  examples"), "{out}");
}

#[test]
fn unknown_listed_example_is_an_error() {
    let s = setup();
    use_examples(&s, "[\"no-such-example\"]");

    let output = s.fx.oncebitten(&s.project, &["rules", "list"]);

    assert_ne!(code(&output), 0, "{}", stdout(&output));
    assert!(
        stderr(&output).contains("names no shipped example"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn retire_a_source_rule_points_at_the_rules_repo() {
    let s = setup();
    assert_eq!(code(&s.fx.oncebitten(&s.project, &["rules", "sync"])), 0);

    let output = s.fx.oncebitten(
        &s.project,
        &["rules", "retire", "team/no-unwrap", "--reason", "r"],
    );

    assert_eq!(code(&output), 2, "{}", stderr(&output));
    let err = stderr(&output);
    assert!(err.contains("retire it in that rules repo"), "{err}");
    assert!(err.contains("overrides = \"team/no-unwrap\""), "{err}");
}

#[test]
fn rollback_a_source_rule_points_at_the_rules_repo() {
    let s = setup();
    assert_eq!(code(&s.fx.oncebitten(&s.project, &["rules", "sync"])), 0);

    let output =
        s.fx.oncebitten(&s.project, &["rules", "rollback", "team/no-unwrap"]);

    assert_eq!(code(&output), 2, "{}", stderr(&output));
    assert!(
        stderr(&output).contains("roll it back in that rules repo"),
        "{}",
        stderr(&output)
    );
}
