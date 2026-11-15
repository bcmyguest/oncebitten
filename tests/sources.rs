use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use oncebitten::config::{Inputs, Platform};
use oncebitten::registry::Resolved;
use oncebitten::rules::{self, Failure};
use oncebitten::sources::sources_root;
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

    fn clone_dir(&self, name: &str) -> PathBuf {
        self.path("data/oncebitten/sources").join(name)
    }

    fn inputs(&self, project: &Path) -> Inputs {
        let mut vars = BTreeMap::new();
        vars.insert("HOME".to_owned(), self.path("home").display().to_string());
        vars.insert(
            "XDG_CONFIG_HOME".to_owned(),
            self.path("xdg-config").display().to_string(),
        );
        vars.insert(
            "XDG_DATA_HOME".to_owned(),
            self.path("data").display().to_string(),
        );
        vars.insert(
            "GIT_CONFIG_GLOBAL".to_owned(),
            self.path("empty.gitconfig").display().to_string(),
        );
        Inputs {
            cwd: project.to_path_buf(),
            vars,
        }
    }

    fn git(&self, dir: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .env("GIT_CONFIG_GLOBAL", self.path("empty.gitconfig"))
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    }

    fn make_repo(&self, dir: &Path) {
        fs::create_dir_all(dir).unwrap();
        self.git(dir, &["init", "-q"]);
        self.git(dir, &["config", "user.name", "Test"]);
        self.git(dir, &["config", "user.email", "test@example.invalid"]);
    }

    fn commit_rule(&self, dir: &Path, id: &str) {
        let rule = dir.join(id);
        fs::create_dir_all(rule.join("fixtures")).unwrap();
        fs::write(
            rule.join("rule.toml"),
            format!(
                "id = \"{id}\"\ndescription = \"Rule {id}.\"\nmode = \"block\"\n\
                 status = \"active\"\nengine = \"ast-grep\"\n"
            ),
        )
        .unwrap();
        fs::write(rule.join("check.yml"), "id: x\n").unwrap();
        fs::write(rule.join("fixtures/bad.rs"), "x.unwrap()\n").unwrap();
        fs::write(rule.join("fixtures/fixed.rs"), "x?\n").unwrap();
        self.git(dir, &["add", "-A"]);
        self.git(dir, &["commit", "-q", "-m", id]);
    }

    fn head(&self, dir: &Path) -> String {
        self.git(dir, &["rev-parse", "HEAD"]).trim().to_owned()
    }

    fn url(&self, dir: &Path) -> String {
        let path = dir.display().to_string().replace('\\', "/");
        if path.starts_with('/') {
            format!("file://{path}")
        } else {
            format!("file:///{path}")
        }
    }

    fn write_config(&self, project: &Path, name: &str, url: &str, rev: Option<&str>) {
        let dir = project.join(".oncebitten");
        fs::create_dir_all(&dir).unwrap();
        let rev = rev.map(|r| format!("rev = \"{r}\"\n")).unwrap_or_default();
        fs::write(
            dir.join("config.toml"),
            format!("[[rules.sources]]\nname = \"{name}\"\nurl = \"{url}\"\n{rev}"),
        )
        .unwrap();
    }
}

fn ids(resolved: &[Resolved]) -> Vec<&str> {
    resolved.iter().map(|rule| rule.id.as_str()).collect()
}

fn lock_path(project: &Path) -> PathBuf {
    project.join(".oncebitten/rules.lock")
}

fn project_with_team(fx: &Fixture, rev: Option<&str>) -> (PathBuf, PathBuf) {
    let team = fx.path("team-src");
    fx.make_repo(&team);
    fx.commit_rule(&team, "no-unwrap");
    let project = fx.path("project");
    fx.make_repo(&project);
    fx.write_config(&project, "team", &fx.url(&team), rev);
    (team, project)
}

#[test]
fn data_dir_uses_an_absolute_xdg_data_home() {
    let mut vars = BTreeMap::new();
    vars.insert("XDG_DATA_HOME".to_owned(), "/srv/data".to_owned());
    vars.insert("HOME".to_owned(), "/home/u".to_owned());

    let dir = sources_root(&vars, Platform::Unix).unwrap();

    assert_eq!(
        dir,
        Path::new("/srv/data").join("oncebitten").join("sources")
    );
}

#[test]
fn data_dir_falls_back_to_local_share_on_unix() {
    let mut vars = BTreeMap::new();
    vars.insert("XDG_DATA_HOME".to_owned(), "relative/data".to_owned());
    vars.insert("HOME".to_owned(), "/home/u".to_owned());

    let dir = sources_root(&vars, Platform::Unix).unwrap();

    assert_eq!(
        dir,
        Path::new("/home/u")
            .join(".local")
            .join("share")
            .join("oncebitten")
            .join("sources")
    );
}

#[test]
fn data_dir_uses_localappdata_on_windows() {
    let mut vars = BTreeMap::new();
    vars.insert(
        "LOCALAPPDATA".to_owned(),
        r"C:\Users\u\AppData\Local".to_owned(),
    );

    let dir = sources_root(&vars, Platform::Windows).unwrap();

    assert_eq!(
        dir,
        Path::new(r"C:\Users\u\AppData\Local")
            .join("oncebitten")
            .join("sources")
    );
}

#[test]
fn data_dir_without_an_absolute_base_is_an_error() {
    let mut vars = BTreeMap::new();
    vars.insert("HOME".to_owned(), "home/u".to_owned());

    let error = sources_root(&vars, Platform::Unix).unwrap_err();

    assert!(format!("{error:#}").contains("HOME"), "{error:#}");
}

#[test]
fn sync_clones_the_source_and_pins_its_head() {
    let fx = Fixture::new();
    let (team, project) = project_with_team(&fx, None);
    let inputs = fx.inputs(&project);

    rules::sync(&inputs).unwrap();

    let lock = fs::read_to_string(lock_path(&project)).unwrap();
    assert!(lock.contains("name = \"team\""), "{lock}");
    assert!(
        lock.contains(&format!("commit = \"{}\"", fx.head(&team))),
        "{lock}"
    );
    assert!(fx.clone_dir("team").join(".git").exists());
    assert_eq!(
        ids(&rules::load(&inputs, false).unwrap()),
        ["team/no-unwrap"]
    );
}

#[test]
fn sync_checks_out_the_rev_when_set() {
    let fx = Fixture::new();
    let (team, project) = project_with_team(&fx, Some("v1"));
    fx.git(&team, &["tag", "v1"]);
    fx.commit_rule(&team, "later");
    let inputs = fx.inputs(&project);

    rules::sync(&inputs).unwrap();

    let lock = fs::read_to_string(lock_path(&project)).unwrap();
    let tagged = fx.git(&team, &["rev-parse", "v1^{commit}"]);
    assert!(
        lock.contains(&format!("commit = \"{}\"", tagged.trim())),
        "{lock}"
    );
    assert_eq!(
        ids(&rules::load(&inputs, false).unwrap()),
        ["team/no-unwrap"]
    );
}

#[test]
fn sync_fetches_new_upstream_commits() {
    let fx = Fixture::new();
    let (team, project) = project_with_team(&fx, None);
    let inputs = fx.inputs(&project);
    rules::sync(&inputs).unwrap();
    fx.commit_rule(&team, "later");

    rules::sync(&inputs).unwrap();

    let lock = fs::read_to_string(lock_path(&project)).unwrap();
    assert!(
        lock.contains(&format!("commit = \"{}\"", fx.head(&team))),
        "{lock}"
    );
    assert_eq!(
        ids(&rules::load(&inputs, false).unwrap()),
        ["team/later", "team/no-unwrap"]
    );
}

#[test]
fn ci_accepts_a_clone_at_the_locked_commit() {
    let fx = Fixture::new();
    let (_, project) = project_with_team(&fx, None);
    let inputs = fx.inputs(&project);
    rules::sync(&inputs).unwrap();

    assert_eq!(
        ids(&rules::load(&inputs, true).unwrap()),
        ["team/no-unwrap"]
    );
}

#[test]
fn ci_without_a_lockfile_could_not_run() {
    let fx = Fixture::new();
    let (_, project) = project_with_team(&fx, None);
    let inputs = fx.inputs(&project);
    rules::sync(&inputs).unwrap();
    fs::remove_file(lock_path(&project)).unwrap();

    assert!(matches!(
        rules::load(&inputs, true),
        Err(Failure::CouldNotRun(_))
    ));
}

#[test]
fn ci_with_a_missing_lock_entry_could_not_run() {
    let fx = Fixture::new();
    let (_, project) = project_with_team(&fx, None);
    let inputs = fx.inputs(&project);
    rules::sync(&inputs).unwrap();
    fs::write(lock_path(&project), "").unwrap();

    assert!(matches!(
        rules::load(&inputs, true),
        Err(Failure::CouldNotRun(_))
    ));
}

#[test]
fn ci_with_a_changed_url_could_not_run() {
    let fx = Fixture::new();
    let (_, project) = project_with_team(&fx, None);
    let inputs = fx.inputs(&project);
    rules::sync(&inputs).unwrap();
    let other = fx.path("other-src");
    fx.make_repo(&other);
    fx.commit_rule(&other, "no-unwrap");
    fx.write_config(&project, "team", &fx.url(&other), None);

    assert!(matches!(
        rules::load(&inputs, true),
        Err(Failure::CouldNotRun(_))
    ));
}

#[test]
fn ci_never_fetches_a_clone_behind_the_lock() {
    let fx = Fixture::new();
    let (team, project) = project_with_team(&fx, None);
    let inputs = fx.inputs(&project);
    rules::sync(&inputs).unwrap();
    let locked = fx.head(&fx.clone_dir("team"));
    fx.commit_rule(&team, "later");

    assert_eq!(
        ids(&rules::load(&inputs, true).unwrap()),
        ["team/no-unwrap"]
    );
    assert_eq!(fx.head(&fx.clone_dir("team")), locked);
}

#[test]
fn local_load_reads_the_clone_without_fetching() {
    let fx = Fixture::new();
    let (team, project) = project_with_team(&fx, None);
    let inputs = fx.inputs(&project);
    rules::sync(&inputs).unwrap();
    fx.commit_rule(&team, "later");

    assert_eq!(
        ids(&rules::load(&inputs, false).unwrap()),
        ["team/no-unwrap"]
    );
}

#[test]
fn local_load_without_a_clone_could_not_run() {
    let fx = Fixture::new();
    let (_, project) = project_with_team(&fx, None);
    let inputs = fx.inputs(&project);

    assert!(matches!(
        rules::load(&inputs, false),
        Err(Failure::CouldNotRun(_))
    ));
}
