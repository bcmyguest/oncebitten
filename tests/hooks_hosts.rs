use std::fs;
use std::path::{Path, PathBuf};

use oncebitten::config::Platform;
use oncebitten::hooks::{install_exe, target_path, uninstall, Scope};
use oncebitten::host::{Host, ALL_HOSTS};
use serde_json::{json, Value};
use tempfile::TempDir;

const EXE: &str = "/opt/ob tools/oncebitten";
const WIN_EXE: &str = r"C:\Program Files\oncebitten\oncebitten.exe";

fn path_for(host: Host, dir: &TempDir) -> PathBuf {
    target_path(host, Scope::User, dir.path(), Path::new("/unused"))
}

fn read(path: &Path) -> Value {
    serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn user_and_project_paths_per_host() {
    let home = Path::new("/h");
    let project = Path::new("/p");
    let at = |host, scope| target_path(host, scope, home, project);
    assert_eq!(
        at(Host::Codex, Scope::User),
        Path::new("/h/.codex/hooks.json")
    );
    assert_eq!(
        at(Host::Cursor, Scope::Project),
        Path::new("/p/.cursor/hooks.json")
    );
    assert_eq!(
        at(Host::Copilot, Scope::User),
        Path::new("/h/.copilot/hooks/oncebitten.json")
    );
    assert_eq!(
        at(Host::Copilot, Scope::Project),
        Path::new("/p/.github/hooks/oncebitten.json")
    );
    assert_eq!(
        at(Host::OpenCode, Scope::User),
        Path::new("/h/.config/opencode/plugins/oncebitten.js")
    );
    assert_eq!(
        at(Host::OpenCode, Scope::Project),
        Path::new("/p/.opencode/plugins/oncebitten.js")
    );
    assert_eq!(
        at(Host::Pi, Scope::User),
        Path::new("/h/.pi/agent/extensions/oncebitten.js")
    );
    assert_eq!(
        at(Host::Pi, Scope::Project),
        Path::new("/p/.pi/extensions/oncebitten.js")
    );
}

#[test]
fn windows_home_paths_come_from_userprofile() {
    use oncebitten::config::home_dir;
    let vars = [("USERPROFILE".to_string(), r"C:\Users\b".to_string())]
        .into_iter()
        .collect();
    let home = home_dir(&vars, Platform::Windows).unwrap();
    let path = target_path(Host::Codex, Scope::User, &home, Path::new("."));
    assert!(path.starts_with(r"C:\Users\b"));
}

#[test]
fn every_host_install_embeds_the_absolute_exe_and_reinstall_is_idempotent() {
    for host in ALL_HOSTS {
        let dir = TempDir::new().unwrap();
        let path = path_for(host, &dir);
        assert!(
            install_exe(host, &path, Path::new(EXE), Platform::Unix).unwrap(),
            "{host:?}"
        );
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains(EXE), "{host:?} lacks the absolute path");
        assert!(
            text.contains(&format!("capture --host {}", host.name())),
            "{host:?}"
        );
        assert!(
            !install_exe(host, &path, Path::new(EXE), Platform::Unix).unwrap(),
            "{host:?}"
        );
        assert!(uninstall(host, &path).unwrap(), "{host:?}");
        assert!(!uninstall(host, &path).unwrap(), "{host:?}");
    }
}

#[test]
fn windows_commands_are_quoted_and_free_of_posix_defaults() {
    for host in ALL_HOSTS {
        let dir = TempDir::new().unwrap();
        let path = path_for(host, &dir);
        install_exe(host, &path, Path::new(WIN_EXE), Platform::Windows).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(!text.contains("${"), "{host:?}");
        if host == Host::Copilot {
            let value = read(&path);
            let entry = &value["hooks"]["userPromptSubmitted"][0];
            assert_eq!(
                entry["powershell"],
                r"& 'C:\Program Files\oncebitten\oncebitten.exe' capture --host copilot"
            );
        }
    }
    let dir = TempDir::new().unwrap();
    let path = path_for(Host::Cursor, &dir);
    install_exe(Host::Cursor, &path, Path::new(WIN_EXE), Platform::Windows).unwrap();
    assert_eq!(
        read(&path)["hooks"]["beforeSubmitPrompt"][0]["command"],
        r#""C:\Program Files\oncebitten\oncebitten.exe" capture --host cursor"#
    );
}

#[test]
fn codex_installs_prompt_hook_only_in_the_nested_layout() {
    let dir = TempDir::new().unwrap();
    let path = path_for(Host::Codex, &dir);
    install_exe(Host::Codex, &path, Path::new(EXE), Platform::Unix).unwrap();
    let value = read(&path);
    let events: Vec<&String> = value["hooks"].as_object().unwrap().keys().collect();
    assert_eq!(events, ["UserPromptSubmit"]);
    assert_eq!(
        value["hooks"]["UserPromptSubmit"][0]["hooks"][0]["command"],
        "'/opt/ob tools/oncebitten' capture --host codex"
    );
}

#[test]
fn cursor_install_keeps_foreign_hooks_and_version_and_uninstall_restores() {
    let dir = TempDir::new().unwrap();
    let path = path_for(Host::Cursor, &dir);
    let original = json!({"version": 1, "hooks": {"stop": [{"command": "./audit.sh"}]}});
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
        &path,
        serde_json::to_string_pretty(&original).unwrap() + "\n",
    )
    .unwrap();
    install_exe(Host::Cursor, &path, Path::new(EXE), Platform::Unix).unwrap();
    let value = read(&path);
    assert_eq!(value["hooks"]["stop"], original["hooks"]["stop"]);
    assert_eq!(
        value["hooks"]["postToolUseFailure"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert!(uninstall(Host::Cursor, &path).unwrap());
    assert_eq!(read(&path), original);
}

#[test]
fn moved_binary_rewrites_the_path_in_place() {
    for host in [Host::Claude, Host::Codex, Host::Cursor] {
        let dir = TempDir::new().unwrap();
        let path = path_for(host, &dir);
        install_exe(host, &path, Path::new("/old/oncebitten"), Platform::Unix).unwrap();
        assert!(install_exe(host, &path, Path::new("/new/oncebitten"), Platform::Unix).unwrap());
        let text = fs::read_to_string(&path).unwrap();
        assert!(
            !text.contains("/old/") && text.contains("/new/oncebitten"),
            "{host:?}"
        );
    }
}

#[test]
fn script_hosts_refuse_to_overwrite_or_delete_foreign_files() {
    for host in [Host::OpenCode, Host::Pi, Host::Copilot] {
        let dir = TempDir::new().unwrap();
        let path = path_for(host, &dir);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "{\"hooks\":{\"stop\":[{\"bash\":\"./mine.sh\"}]}}").unwrap();
        assert!(
            install_exe(host, &path, Path::new(EXE), Platform::Unix).is_err(),
            "{host:?}"
        );
        assert!(uninstall(host, &path).is_err(), "{host:?}");
        assert!(fs::read_to_string(&path).unwrap().contains("mine.sh"));
    }
}

#[test]
fn plugin_scripts_forward_to_the_installed_command_and_honour_triage() {
    for host in [Host::OpenCode, Host::Pi] {
        let dir = TempDir::new().unwrap();
        let path = path_for(host, &dir);
        install_exe(host, &path, Path::new(EXE), Platform::Unix).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("ONCEBITTEN_TRIAGE"));
        assert!(text.contains("oncebitten-managed"));
        assert!(!text.contains("__ONCEBITTEN_COMMAND__"));
    }
}
