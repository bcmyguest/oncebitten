#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use tempfile::TempDir;

/// A git repository in a temp directory, with oncebitten run against temp
/// HOME, config and data directories.
pub struct Repo {
    pub root: TempDir,
}

impl Repo {
    pub fn new() -> Self {
        let root = TempDir::new().unwrap();
        fs::write(root.path().join("empty.gitconfig"), "").unwrap();
        let repo = Repo { root };
        fs::create_dir_all(repo.project()).unwrap();
        repo.git(&["init", "-q", "-b", "main"]);
        repo.git(&["config", "user.name", "Test"]);
        repo.git(&["config", "user.email", "test@example.invalid"]);
        repo.git(&["config", "core.autocrlf", "false"]);
        repo
    }

    pub fn project(&self) -> PathBuf {
        self.root.path().join("project")
    }

    pub fn git(&self, args: &[&str]) -> String {
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
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    }

    pub fn write(&self, relative: &str, contents: &str) {
        let path = self.project().join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }

    pub fn commit_all(&self, message: &str) -> String {
        self.git(&["add", "-A"]);
        self.git(&["commit", "-q", "-m", message]);
        self.git(&["rev-parse", "HEAD"])
    }

    pub fn rule_dir(&self, id: &str) -> PathBuf {
        self.project().join(".oncebitten").join("rules").join(id)
    }

    /// Copies a shipped example into the project rules with the given status.
    pub fn add_example(&self, id: &str, status: &str) -> PathBuf {
        let src = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("examples")
            .join(id);
        let dest = self.rule_dir(id);
        copy_dir(&src, &dest);
        let toml = fs::read_to_string(dest.join("rule.toml")).unwrap();
        fs::write(
            dest.join("rule.toml"),
            toml.replace("status = \"proposed\"", &format!("status = \"{status}\"")),
        )
        .unwrap();
        dest
    }

    pub fn add_rule(&self, id: &str, body: &str) {
        let dir = format!(".oncebitten/rules/{id}");
        self.write(&format!("{dir}/rule.toml"), body);
        self.write(&format!("{dir}/fixtures/bad.diff"), "bad\n");
        self.write(&format!("{dir}/fixtures/fixed.diff"), "fixed\n");
    }

    pub fn run(&self, args: &[&str], env: &[(&str, &str)]) -> Output {
        let path = std::env::var_os("PATH").unwrap_or_default();
        let mut command = Command::new(env!("CARGO_BIN_EXE_oncebitten"));
        command
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
            );
        for var in ["SYSTEMROOT", "TEMP", "TMP"] {
            if let Some(value) = std::env::var_os(var) {
                command.env(var, value);
            }
        }
        for (name, value) in env {
            command.env(name, value);
        }
        command.output().unwrap()
    }
}

pub fn copy_dir(src: &Path, dest: &Path) {
    fs::create_dir_all(dest).unwrap();
    for entry in fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let target = dest.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), &target).unwrap();
        }
    }
}

pub fn code(output: &Output) -> i32 {
    output.status.code().unwrap()
}

pub fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

pub fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

pub fn both(output: &Output) -> String {
    format!("{}{}", stdout(output), stderr(output))
}

/// A copy of the `fake-forge` binary installed under a forge CLI's name in its
/// own temp directory. Canned replies and the argv log live beside it.
pub struct FakeForge {
    dir: TempDir,
    program: PathBuf,
}

impl FakeForge {
    pub fn new(cli: &str) -> Self {
        let dir = TempDir::new().unwrap();
        let program = dir
            .path()
            .join(format!("{cli}{}", std::env::consts::EXE_SUFFIX));
        fs::copy(env!("CARGO_BIN_EXE_fake-forge"), &program).unwrap();
        FakeForge { dir, program }
    }

    pub fn program(&self) -> &Path {
        &self.program
    }

    /// Sets a canned file: `reply`, `status` or `stderr`, or `reply.N` for call N.
    pub fn set(&self, name: &str, contents: &str) {
        fs::write(self.dir.path().join(name), contents).unwrap();
    }

    /// Every call's argv, in order.
    pub fn calls(&self) -> Vec<Vec<String>> {
        fs::read_to_string(self.dir.path().join("calls.log"))
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }
}
