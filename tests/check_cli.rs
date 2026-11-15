use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};

use tempfile::TempDir;

struct Repo {
    root: TempDir,
}

impl Repo {
    fn new() -> Self {
        let root = TempDir::new().unwrap();
        fs::write(root.path().join("empty.gitconfig"), "").unwrap();
        let repo = Repo { root };
        fs::create_dir_all(repo.project()).unwrap();
        repo.git(&["init", "-q"]);
        repo.git(&["config", "user.name", "Test"]);
        repo.git(&["config", "user.email", "test@example.invalid"]);
        repo
    }

    fn project(&self) -> PathBuf {
        self.root.path().join("project")
    }

    fn git(&self, args: &[&str]) {
        let output = Command::new("git")
            .args(args)
            .current_dir(self.project())
            .env(
                "GIT_CONFIG_GLOBAL",
                self.root.path().join("empty.gitconfig"),
            )
            .output()
            .unwrap();
        assert!(output.status.success(), "git {args:?}: {output:?}");
    }

    fn write(&self, relative: &str, contents: &str) {
        let path = self.project().join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }

    fn commit_all(&self, message: &str) {
        self.git(&["add", "-A"]);
        self.git(&["commit", "-q", "-m", message]);
    }

    fn check(&self, args: &[&str]) -> Output {
        let path = std::env::var_os("PATH").unwrap_or_default();
        Command::new(env!("CARGO_BIN_EXE_oncebitten"))
            .arg("check")
            .args(args)
            .current_dir(self.project())
            .env_clear()
            .env("PATH", path)
            .env("HOME", self.root.path().join("home"))
            .env("XDG_CONFIG_HOME", self.root.path().join("xdg-config"))
            .env("XDG_DATA_HOME", self.root.path().join("data"))
            .env(
                "GIT_CONFIG_GLOBAL",
                self.root.path().join("empty.gitconfig"),
            )
            .output()
            .unwrap()
    }

    fn add_rule(&self, id: &str, body: &str) {
        let dir = format!(".oncebitten/rules/{id}");
        self.write(&format!("{dir}/rule.toml"), body);
        self.write(&format!("{dir}/fixtures/bad.diff"), "bad\n");
        self.write(&format!("{dir}/fixtures/fixed.diff"), "fixed\n");
    }
}

fn diagram_rule(status: &str) -> String {
    format!(
        "id = \"arch-diagram\"\ndescription = \"Diagram required.\"\nmode = \"block\"\n\
         status = \"{status}\"\nengine = \"diff\"\n\
         when_changed = [\"src/api/**\"]\nrequire_changed = [\"docs/diagrams/*.svg\"]\n"
    )
}

fn code(output: &Output) -> i32 {
    output.status.code().unwrap()
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn committed_repo_with_rule(status: &str) -> Repo {
    let repo = Repo::new();
    repo.write("src/api/users.rs", "fn a() {}\n");
    repo.write("docs/diagrams/api.svg", "<svg/>\n");
    repo.add_rule("arch-diagram", &diagram_rule(status));
    repo.commit_all("baseline");
    repo
}

#[test]
fn clean_when_no_trigger_path_changed() {
    let repo = committed_repo_with_rule("active");
    repo.write("README.md", "docs\n");

    let output = repo.check(&[]);

    assert_eq!(code(&output), 0, "{}{}", stdout(&output), stderr(&output));
    assert!(stdout(&output).is_empty());
}

#[test]
fn fired_exits_1_and_names_the_rule() {
    let repo = committed_repo_with_rule("active");
    repo.write("src/api/users.rs", "fn a() { 1; }\n");

    let output = repo.check(&[]);

    assert_eq!(code(&output), 1, "{}{}", stdout(&output), stderr(&output));
    assert!(
        stdout(&output).contains("arch-diagram"),
        "{}",
        stdout(&output)
    );
    assert!(
        stdout(&output).contains("src/api/users.rs"),
        "{}",
        stdout(&output)
    );
}

#[test]
fn requirement_satisfied_in_the_same_change_is_clean() {
    let repo = committed_repo_with_rule("active");
    repo.write("src/api/users.rs", "fn a() { 1; }\n");
    repo.write("docs/diagrams/api.svg", "<svg>new</svg>\n");

    let output = repo.check(&[]);

    assert_eq!(code(&output), 0, "{}{}", stdout(&output), stderr(&output));
}

#[test]
fn inactive_rules_are_not_run() {
    let repo = committed_repo_with_rule("proposed");
    repo.write("src/api/users.rs", "fn a() { 1; }\n");

    let output = repo.check(&[]);

    assert_eq!(code(&output), 0, "{}{}", stdout(&output), stderr(&output));
}

#[test]
fn could_not_run_wins_over_fired_and_exits_2() {
    let repo = committed_repo_with_rule("active");
    repo.add_rule(
        "missing-tool",
        "id = \"missing-tool\"\ndescription = \"Runs a missing binary.\"\nmode = \"block\"\n\
         status = \"active\"\nengine = \"native\"\n\
         command = [\"oncebitten-no-such-binary\"]\nfired_exit_codes = [1]\n",
    );
    repo.write("src/api/users.rs", "fn a() { 1; }\n");

    let output = repo.check(&[]);

    assert_eq!(code(&output), 2, "{}{}", stdout(&output), stderr(&output));
    assert!(
        stdout(&output).contains("arch-diagram"),
        "{}",
        stdout(&output)
    );
    assert!(
        stderr(&output).contains("missing-tool: could not run"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn invalid_base_revision_exits_2() {
    let repo = committed_repo_with_rule("active");

    let output = repo.check(&["--base", "no-such-revision"]);

    assert_eq!(code(&output), 2, "{}{}", stdout(&output), stderr(&output));
    assert!(stderr(&output).contains("error:"), "{}", stderr(&output));
}

#[test]
fn outside_a_git_repository_exits_2() {
    let root = TempDir::new().unwrap();
    let path = std::env::var_os("PATH").unwrap_or_default();
    let output = Command::new(env!("CARGO_BIN_EXE_oncebitten"))
        .arg("check")
        .current_dir(root.path())
        .env_clear()
        .env("PATH", path)
        .env("HOME", root.path().join("home"))
        .env("XDG_CONFIG_HOME", root.path().join("xdg-config"))
        .env("XDG_DATA_HOME", root.path().join("data"))
        .output()
        .unwrap();

    assert_eq!(code(&output), 2, "{}{}", stdout(&output), stderr(&output));
}
