use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use oncebitten::config::Platform;
use oncebitten::hooks::{
    install_exe, install_steer_exe, steer_command, steer_limits, steer_target, target_path,
    uninstall, uninstall_steer, Scope, Shell,
};
use oncebitten::host::{Host, ALL_HOSTS};
use serde_json::Value;
use tempfile::TempDir;

const EXE: &str = "/opt/ob tools/oncebitten";

fn capture_path(host: Host, dir: &TempDir) -> PathBuf {
    target_path(host, Scope::User, dir.path(), Path::new("/unused"))
}

fn steer_path(host: Host, dir: &TempDir) -> PathBuf {
    steer_target(
        host,
        Scope::User,
        dir.path(),
        Path::new("/unused"),
        &BTreeMap::new(),
    )
    .unwrap()
}

fn read(path: &Path) -> Value {
    serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
}

fn events_running(value: &Value, needle: &str) -> Vec<String> {
    let hooks = value["hooks"].as_object().unwrap();
    hooks
        .iter()
        .filter(|(_, entries)| entries.to_string().contains(needle))
        .map(|(event, _)| event.clone())
        .collect()
}

#[test]
fn steer_command_quotes_the_exe_per_shell() {
    let exe = Path::new(EXE);
    assert_eq!(
        steer_command(exe, Host::Claude, Shell::Posix),
        "'/opt/ob tools/oncebitten' steer --host claude"
    );
    assert_eq!(
        steer_command(exe, Host::Cursor, Shell::Cmd),
        "\"/opt/ob tools/oncebitten\" steer --host cursor"
    );
    assert_eq!(
        steer_command(exe, Host::Copilot, Shell::PowerShell),
        "& '/opt/ob tools/oncebitten' steer --host copilot"
    );
}

#[test]
fn json_hosts_get_pre_tool_steer_hooks() {
    let cases = [
        (
            Host::Claude,
            vec!["PreToolUse", "SessionStart", "Stop", "UserPromptSubmit"],
        ),
        (
            Host::Codex,
            vec!["PreToolUse", "SessionStart", "Stop", "UserPromptSubmit"],
        ),
        (Host::Cursor, vec!["preToolUse"]),
        (Host::Copilot, vec!["preToolUse"]),
    ];
    for (host, wanted) in cases {
        let dir = TempDir::new().unwrap();
        let path = steer_path(host, &dir);
        assert!(install_steer_exe(host, &path, Path::new(EXE), Platform::Unix).unwrap());
        let mut events = events_running(&read(&path), "steer --host");
        events.sort();
        assert_eq!(events, wanted, "{host:?}");
        assert!(!install_steer_exe(host, &path, Path::new(EXE), Platform::Unix).unwrap());
    }
}

#[test]
fn steer_and_capture_share_a_settings_file_without_clobbering() {
    for host in [Host::Claude, Host::Codex, Host::Cursor] {
        let dir = TempDir::new().unwrap();
        let path = capture_path(host, &dir);
        assert_eq!(steer_path(host, &dir), path, "{host:?}");
        install_exe(host, &path, Path::new(EXE), Platform::Unix).unwrap();
        install_steer_exe(host, &path, Path::new(EXE), Platform::Unix).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("capture --host"), "{host:?}");
        assert!(text.contains("steer --host"), "{host:?}");

        // re-running capture install leaves the steer entries alone
        assert!(!install_exe(host, &path, Path::new(EXE), Platform::Unix).unwrap());
        assert!(fs::read_to_string(&path).unwrap().contains("steer --host"));

        assert!(uninstall_steer(host, &path).unwrap(), "{host:?}");
        let text = fs::read_to_string(&path).unwrap();
        assert!(!text.contains("steer --host"), "{host:?}");
        assert!(text.contains("capture --host"), "{host:?}");
        assert!(uninstall(host, &path).unwrap(), "{host:?}");
        assert!(!uninstall_steer(host, &path).unwrap(), "{host:?}");
    }
}

#[test]
fn owned_file_hosts_get_a_second_file() {
    for host in [Host::Copilot, Host::OpenCode, Host::Pi] {
        let dir = TempDir::new().unwrap();
        let capture = capture_path(host, &dir);
        let steer = steer_path(host, &dir);
        assert_ne!(steer, capture, "{host:?}");
        assert_eq!(steer.parent(), capture.parent(), "{host:?}");
        install_exe(host, &capture, Path::new(EXE), Platform::Unix).unwrap();
        install_steer_exe(host, &steer, Path::new(EXE), Platform::Unix).unwrap();
        assert!(fs::read_to_string(&steer).unwrap().contains("steer --host"));
        assert!(!fs::read_to_string(&capture)
            .unwrap()
            .contains("steer --host"));
        assert!(uninstall_steer(host, &steer).unwrap(), "{host:?}");
        assert!(!steer.exists(), "{host:?}");
        assert!(capture.exists(), "{host:?}");
        assert!(!uninstall_steer(host, &steer).unwrap(), "{host:?}");
    }
}

#[test]
fn steer_install_refuses_foreign_owned_files() {
    for host in [Host::Copilot, Host::OpenCode, Host::Pi] {
        let dir = TempDir::new().unwrap();
        let path = steer_path(host, &dir);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "{\"hooks\":{\"stop\":[{\"bash\":\"./mine.sh\"}]}}").unwrap();
        assert!(install_steer_exe(host, &path, Path::new(EXE), Platform::Unix).is_err());
        assert!(uninstall_steer(host, &path).is_err(), "{host:?}");
        assert!(fs::read_to_string(&path).unwrap().contains("mine.sh"));
    }
}

#[test]
fn steer_plugin_scripts_call_steer_and_honour_triage() {
    for host in [Host::OpenCode, Host::Pi] {
        let dir = TempDir::new().unwrap();
        let path = steer_path(host, &dir);
        install_steer_exe(host, &path, Path::new(EXE), Platform::Unix).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("ONCEBITTEN_TRIAGE"));
        assert!(text.contains("oncebitten-managed"));
        assert!(text.contains(&format!("steer --host {}", host.name())));
        assert!(!text.contains("__ONCEBITTEN_COMMAND__"));
    }
}

#[test]
fn windows_commands_carry_no_posix_syntax() {
    let exe = Path::new(r"C:\Program Files\oncebitten\oncebitten.exe");
    for host in ALL_HOSTS {
        let dir = TempDir::new().unwrap();
        let path = steer_path(host, &dir);
        install_steer_exe(host, &path, exe, Platform::Windows).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(!text.contains("${"), "{host:?}");
        assert!(text.contains("oncebitten.exe"), "{host:?}");
    }
}

#[test]
fn hosts_that_cannot_inform_say_so() {
    assert_eq!(steer_limits(Host::Claude), None);
    assert_eq!(steer_limits(Host::Codex), None);
    for host in [Host::Cursor, Host::Copilot] {
        assert!(steer_limits(host).unwrap().contains("inform"), "{host:?}");
    }
}

#[test]
fn claude_and_codex_steer_also_runs_on_stop_and_session_start() {
    for host in [Host::Claude, Host::Codex] {
        let dir = TempDir::new().unwrap();
        let path = steer_path(host, &dir);
        assert!(install_steer_exe(host, &path, Path::new(EXE), Platform::Unix).unwrap());
        let mut events = events_running(&read(&path), "steer --host");
        events.sort();
        assert_eq!(
            events,
            ["PreToolUse", "SessionStart", "Stop", "UserPromptSubmit"],
            "{host:?}"
        );
        assert!(!install_steer_exe(host, &path, Path::new(EXE), Platform::Unix).unwrap());
        assert!(uninstall_steer(host, &path).unwrap(), "{host:?}");
        assert!(!fs::read_to_string(&path).unwrap().contains("steer --host"));
    }
}

#[test]
fn hosts_without_a_stop_hook_say_so() {
    for host in [Host::Cursor, Host::Copilot, Host::OpenCode, Host::Pi] {
        assert!(
            steer_limits(host).unwrap().contains("at = \"stop\""),
            "{host:?}"
        );
    }
}
