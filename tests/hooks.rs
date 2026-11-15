use std::fs;
use std::path::{Path, PathBuf};

use oncebitten::hooks::{install, settings_path, uninstall, Scope};
use oncebitten::host::Host;
use serde_json::{json, Value};
use tempfile::TempDir;

const EVENTS: &[&str] = &["UserPromptSubmit", "PostToolUse", "PostToolUseFailure"];
const COMMAND: &str = "oncebitten capture --host claude";

fn settings(dir: &TempDir) -> PathBuf {
    dir.path().join(".claude").join("settings.json")
}

fn read(path: &Path) -> Value {
    serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
}

fn write(path: &Path, value: &Value) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, serde_json::to_string_pretty(value).unwrap()).unwrap();
}

fn count_ours(value: &Value, event: &str) -> usize {
    value["hooks"][event]
        .as_array()
        .map(|groups| {
            groups
                .iter()
                .flat_map(|group| group["hooks"].as_array().into_iter().flatten())
                .filter(|hook| hook["command"] == COMMAND)
                .count()
        })
        .unwrap_or(0)
}

fn existing() -> Value {
    json!({
        "model": "haiku",
        "permissions": {"allow": ["Bash(ls:*)"]},
        "hooks": {
            "PostToolUse": [{
                "matcher": "Read",
                "hooks": [{"type": "command", "command": "other-tool read"}]
            }],
            "SessionStart": [{
                "hooks": [{"type": "command", "command": "other-tool start"}]
            }]
        }
    })
}

#[test]
fn user_scope_points_at_home_claude_settings() {
    let home = TempDir::new().unwrap();
    let project = TempDir::new().unwrap();
    assert_eq!(
        settings_path(Scope::User, home.path(), project.path()),
        home.path().join(".claude").join("settings.json")
    );
}

#[test]
fn project_scope_points_at_project_claude_settings() {
    let home = TempDir::new().unwrap();
    let project = TempDir::new().unwrap();
    assert_eq!(
        settings_path(Scope::Project, home.path(), project.path()),
        project.path().join(".claude").join("settings.json")
    );
}

#[test]
fn install_creates_a_missing_file_with_three_events() {
    let dir = TempDir::new().unwrap();
    let path = settings(&dir);
    assert!(install(Host::Claude, &path).unwrap());
    let value = read(&path);
    for event in EVENTS {
        assert_eq!(count_ours(&value, event), 1, "{event}");
    }
}

#[test]
fn install_twice_adds_nothing() {
    let dir = TempDir::new().unwrap();
    let path = settings(&dir);
    write(&path, &existing());
    assert!(install(Host::Claude, &path).unwrap());
    let once = read(&path);
    assert!(!install(Host::Claude, &path).unwrap());
    assert_eq!(read(&path), once);
    for event in EVENTS {
        assert_eq!(count_ours(&once, event), 1, "{event}");
    }
}

#[test]
fn install_preserves_other_keys_and_hooks() {
    let dir = TempDir::new().unwrap();
    let path = settings(&dir);
    write(&path, &existing());
    install(Host::Claude, &path).unwrap();
    let value = read(&path);
    assert_eq!(value["model"], "haiku");
    assert_eq!(value["permissions"], existing()["permissions"]);
    assert_eq!(
        value["hooks"]["SessionStart"],
        existing()["hooks"]["SessionStart"]
    );
    assert_eq!(
        value["hooks"]["PostToolUse"][0],
        existing()["hooks"]["PostToolUse"][0]
    );
}

#[test]
fn uninstall_restores_the_original_file() {
    let dir = TempDir::new().unwrap();
    let path = settings(&dir);
    write(&path, &existing());
    install(Host::Claude, &path).unwrap();
    assert!(uninstall(Host::Claude, &path).unwrap());
    assert_eq!(read(&path), existing());
}

#[test]
fn uninstall_keeps_other_hooks_inside_a_shared_group() {
    let dir = TempDir::new().unwrap();
    let path = settings(&dir);
    let shared = json!({
        "hooks": {"PostToolUse": [{
            "hooks": [
                {"type": "command", "command": "other-tool read"},
                {"type": "command", "command": COMMAND}
            ]
        }]}
    });
    write(&path, &shared);
    assert!(uninstall(Host::Claude, &path).unwrap());
    let value = read(&path);
    assert_eq!(count_ours(&value, "PostToolUse"), 0);
    assert_eq!(
        value["hooks"]["PostToolUse"][0]["hooks"],
        json!([{"type": "command", "command": "other-tool read"}])
    );
}

#[test]
fn uninstall_without_our_entries_changes_nothing() {
    let dir = TempDir::new().unwrap();
    let path = settings(&dir);
    write(&path, &existing());
    assert!(!uninstall(Host::Claude, &path).unwrap());
    assert_eq!(read(&path), existing());
}

#[test]
fn uninstall_on_a_missing_file_creates_nothing() {
    let dir = TempDir::new().unwrap();
    let path = settings(&dir);
    assert!(!uninstall(Host::Claude, &path).unwrap());
    assert!(!path.exists());
}

#[test]
fn invalid_json_is_an_error_and_is_left_alone() {
    let dir = TempDir::new().unwrap();
    let path = settings(&dir);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, "{ not json").unwrap();
    assert!(install(Host::Claude, &path).is_err());
    assert!(uninstall(Host::Claude, &path).is_err());
    assert_eq!(fs::read_to_string(&path).unwrap(), "{ not json");
}

#[test]
fn unverified_hosts_are_refused() {
    let dir = TempDir::new().unwrap();
    let path = settings(&dir);
    for host in [Host::Codex, Host::OpenCode, Host::Cursor, Host::Pi] {
        let err = install(host, &path).unwrap_err().to_string();
        assert!(err.contains("unsupported host"), "{host:?}: {err}");
        assert!(!path.exists());
    }
}

mod absolute_exe {
    use std::path::Path;

    use oncebitten::hooks::{self, Shell};
    use oncebitten::host::Host;

    #[test]
    fn current_exe_is_absolute() {
        assert!(hooks::current_exe().unwrap().is_absolute());
    }

    #[test]
    fn posix_command_quotes_the_absolute_path() {
        let exe = Path::new("/opt/my tools/oncebitten");
        let command = hooks::capture_command(exe, Host::Claude, Shell::Posix);
        assert_eq!(command, "'/opt/my tools/oncebitten' capture --host claude");
    }

    #[test]
    fn posix_command_escapes_single_quotes() {
        let command =
            hooks::capture_command(Path::new("/o'x/oncebitten"), Host::Claude, Shell::Posix);
        assert_eq!(command, r"'/o'\''x/oncebitten' capture --host claude");
    }

    #[test]
    fn cmd_command_double_quotes_a_windows_path() {
        let exe = Path::new(r"C:\Program Files\oncebitten\oncebitten.exe");
        let command = hooks::capture_command(exe, Host::Claude, Shell::Cmd);
        assert_eq!(
            command,
            r#""C:\Program Files\oncebitten\oncebitten.exe" capture --host claude"#
        );
        assert!(!command.contains("${"));
    }

    #[test]
    fn powershell_command_uses_the_call_operator() {
        let exe = Path::new(r"C:\Users\b\bin\oncebitten.exe");
        let command = hooks::capture_command(exe, Host::Claude, Shell::PowerShell);
        assert_eq!(
            command,
            r"& 'C:\Users\b\bin\oncebitten.exe' capture --host claude"
        );
    }

    #[test]
    fn install_with_writes_the_command_and_reinstall_is_idempotent() {
        let dir = std::env::temp_dir().join(format!("ob-abs-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        let exe = hooks::current_exe().unwrap();
        let command = hooks::capture_command(&exe, Host::Claude, Shell::Posix);
        assert!(hooks::install_with(Host::Claude, &path, &command).unwrap());
        let written = std::fs::read_to_string(&path).unwrap();
        assert!(written.contains(&exe.display().to_string()));
        assert!(!hooks::install_with(Host::Claude, &path, &command).unwrap());
        assert!(hooks::uninstall(Host::Claude, &path).unwrap());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn install_replaces_a_path_command_with_the_absolute_one() {
        let dir = std::env::temp_dir().join(format!("ob-mig-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        assert!(hooks::install(Host::Claude, &path).unwrap());
        let command =
            hooks::capture_command(Path::new("/usr/bin/oncebitten"), Host::Claude, Shell::Posix);
        assert!(hooks::install_with(Host::Claude, &path, &command).unwrap());
        let written = std::fs::read_to_string(&path).unwrap();
        assert_eq!(written.matches("/usr/bin/oncebitten").count(), 3);
        assert!(hooks::uninstall(Host::Claude, &path).unwrap());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
