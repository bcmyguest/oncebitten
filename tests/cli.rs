use std::fs;
use std::path::Path;
use std::process::Command;

use tempfile::TempDir;

fn run(cwd: &Path, home: &Path, args: &[&str], extra: &[(&str, &str)]) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_oncebitten"))
        .args(args)
        .current_dir(cwd)
        .env_clear()
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join("xdg"))
        .envs(extra.iter().copied())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn show_prints_merged_toml_with_origins() {
    let root = TempDir::new().unwrap();
    let repo = root.path().join("repo");
    fs::create_dir_all(repo.join(".git")).unwrap();
    fs::create_dir_all(repo.join(".oncebitten")).unwrap();
    fs::write(
        repo.join(".oncebitten/config.toml"),
        "[triage]\ndefault = \"pi:default\"\n",
    )
    .unwrap();

    let out = run(
        &repo,
        root.path(),
        &["config", "show", "--origin"],
        &[("ONCEBITTEN_SANDBOX_RUNTIME", "openshell")],
    );

    assert!(
        out.contains("default = \"pi:default\"  # origin: project"),
        "{out}"
    );
    assert!(
        out.contains("runtime = \"openshell\"  # origin: env"),
        "{out}"
    );
    assert!(out.contains("[triggers]"), "{out}");
    assert!(
        out.contains("gate = [\"pre-push\", \"pr-create\"]"),
        "{out}"
    );
}

#[test]
fn show_without_origin_has_no_annotations() {
    let root = TempDir::new().unwrap();
    let repo = root.path().join("repo");
    fs::create_dir_all(&repo).unwrap();

    let out = run(&repo, root.path(), &["config", "show"], &[]);

    assert!(!out.contains("# origin"), "{out}");
    assert!(out.contains("default = \"claude:haiku\""), "{out}");
}

#[test]
fn path_prints_user_and_project_locations() {
    let root = TempDir::new().unwrap();
    let repo = root.path().join("repo");
    fs::create_dir_all(repo.join(".git")).unwrap();

    let out = run(&repo, root.path(), &["config", "path"], &[]);

    let user = root
        .path()
        .join("xdg")
        .join("oncebitten")
        .join("config.toml");
    assert!(
        out.contains(&format!("user     {} (missing)", user.display())),
        "{out}"
    );
    assert!(out.contains("project  none"), "{out}");
}

#[cfg(unix)]
#[test]
fn show_survives_a_non_unicode_unrelated_variable() {
    use std::os::unix::ffi::OsStrExt;

    let root = TempDir::new().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_oncebitten"))
        .args(["config", "show"])
        .current_dir(root.path())
        .env_clear()
        .env("HOME", root.path())
        .env("UNRELATED", std::ffi::OsStr::from_bytes(&[0xff]))
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn show_warns_about_an_unknown_variable_on_stderr() {
    let root = TempDir::new().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_oncebitten"))
        .args(["config", "show"])
        .current_dir(root.path())
        .env_clear()
        .env("HOME", root.path())
        .env("ONCEBITTEN_TRIAGE_DEFUALT", "pi:default")
        .output()
        .unwrap();

    assert!(output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("warning"), "{stderr}");
    assert!(stderr.contains("ONCEBITTEN_TRIAGE_DEFUALT"), "{stderr}");
}
