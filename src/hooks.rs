use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde_json::{json, Map, Value};

use crate::config::Platform;
use crate::host::{Host, ALL_HOSTS};

const SETTINGS_DIR: &str = ".claude";
const SETTINGS_FILE: &str = "settings.json";
const BIN_NAME: &str = "oncebitten";
const CLAUDE_EVENTS: &[&str] = &["UserPromptSubmit", "PostToolUse", "PostToolUseFailure"];
// Codex PostToolUse carries no outcome, so only prompts are worth a process spawn.
const CODEX_EVENTS: &[&str] = &["UserPromptSubmit"];
const CURSOR_EVENTS: &[&str] = &["beforeSubmitPrompt", "postToolUse", "postToolUseFailure"];
const COPILOT_EVENTS: &[&str] = &["userPromptSubmitted", "postToolUse"];
const CURSOR_VERSION: u64 = 1;
const COPILOT_VERSION: u64 = 1;
const HOOK_TIMEOUT_SECS: u64 = 10;
const SCRIPT_MARKER: &str = "oncebitten-managed";
const OPENCODE_TEMPLATE: &str = include_str!("../packaging/opencode/oncebitten.js");
const PI_TEMPLATE: &str = include_str!("../packaging/pi/oncebitten.js");
const COMMAND_PLACEHOLDER: &str = "__ONCEBITTEN_COMMAND__";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shell {
    Posix,
    Cmd,
    PowerShell,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Scope {
    User,
    Project,
}

pub fn settings_path(scope: Scope, home: &Path, project: &Path) -> PathBuf {
    target_path(Host::Claude, scope, home, project)
}

/// The file `hooks install` edits (or, for opencode, pi and copilot, owns).
pub fn target_path(host: Host, scope: Scope, home: &Path, project: &Path) -> PathBuf {
    let base = match scope {
        Scope::User => home,
        Scope::Project => project,
    };
    match (host, scope) {
        (Host::Claude, _) => base.join(SETTINGS_DIR).join(SETTINGS_FILE),
        (Host::Codex, _) => base.join(".codex").join("hooks.json"),
        (Host::Cursor, _) => base.join(".cursor").join("hooks.json"),
        (Host::Copilot, Scope::User) => base.join(".copilot").join("hooks").join("oncebitten.json"),
        (Host::Copilot, Scope::Project) => {
            base.join(".github").join("hooks").join("oncebitten.json")
        }
        (Host::OpenCode, Scope::User) => base
            .join(".config")
            .join("opencode")
            .join("plugins")
            .join("oncebitten.js"),
        (Host::OpenCode, Scope::Project) => {
            base.join(".opencode").join("plugins").join("oncebitten.js")
        }
        (Host::Pi, Scope::User) => base
            .join(".pi")
            .join("agent")
            .join("extensions")
            .join("oncebitten.js"),
        (Host::Pi, Scope::Project) => base.join(".pi").join("extensions").join("oncebitten.js"),
    }
}

pub fn current_exe() -> Result<PathBuf> {
    let exe = std::env::current_exe().context("cannot locate the oncebitten executable")?;
    dunce::canonicalize(&exe).with_context(|| format!("cannot resolve {}", exe.display()))
}

pub fn capture_command(exe: &Path, host: Host, shell: Shell) -> String {
    let exe = exe.display().to_string();
    let quoted = match shell {
        Shell::Posix => format!("'{}'", exe.replace('\'', r"'\''")),
        Shell::Cmd => format!("\"{exe}\""),
        Shell::PowerShell => format!("& '{}'", exe.replace('\'', "''")),
    };
    format!("{quoted} {}", capture_args(host))
}

pub fn default_shell(platform: Platform) -> Shell {
    match platform {
        Platform::Unix => Shell::Posix,
        Platform::Windows => Shell::Cmd,
    }
}

/// Bare command resolved through `PATH`. The CLI never writes it.
pub fn path_command(host: Host) -> String {
    format!("{BIN_NAME} {}", capture_args(host))
}

fn capture_args(host: Host) -> String {
    format!("capture --host {}", host.name())
}

pub fn install(host: Host, path: &Path) -> Result<bool> {
    install_with(host, path, &path_command(host))
}

/// Installs the hooks with the absolute path of the running executable.
pub fn install_exe(host: Host, path: &Path, exe: &Path, platform: Platform) -> Result<bool> {
    let shell = default_shell(platform);
    let command = capture_command(exe, host, shell);
    match host {
        Host::Copilot => {
            let posix = capture_command(exe, host, Shell::Posix);
            let powershell = capture_command(exe, host, Shell::PowerShell);
            install_copilot(path, &posix, &powershell)
        }
        _ => install_with(host, path, &command),
    }
}

pub fn install_with(host: Host, path: &Path, command: &str) -> Result<bool> {
    match host {
        Host::Claude => install_nested(path, command, CLAUDE_EVENTS),
        Host::Codex => install_nested(path, command, CODEX_EVENTS),
        Host::Cursor => install_cursor(path, command),
        Host::Copilot => install_copilot(path, command, command),
        Host::OpenCode => install_script(path, OPENCODE_TEMPLATE, command),
        Host::Pi => install_script(path, PI_TEMPLATE, command),
    }
}

fn install_script(path: &Path, template: &str, command: &str) -> Result<bool> {
    let body = template.replace(
        COMMAND_PLACEHOLDER,
        &serde_json::to_string(command).context("cannot encode the command")?,
    );
    if path.exists() {
        let existing =
            fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))?;
        if !existing.contains(SCRIPT_MARKER) {
            bail!("{} exists and is not managed by oncebitten", path.display());
        }
        if existing == body {
            return Ok(false);
        }
    }
    write_text(path, &body)?;
    Ok(true)
}

fn install_copilot(path: &Path, posix: &str, powershell: &str) -> Result<bool> {
    let entry = json!({
        "type": "command",
        "bash": posix,
        "powershell": powershell,
        "timeoutSec": HOOK_TIMEOUT_SECS,
    });
    let mut hooks = Map::new();
    for event in COPILOT_EVENTS {
        hooks.insert((*event).to_string(), json!([entry]));
    }
    let wanted = json!({"version": COPILOT_VERSION, "hooks": hooks});
    if path.exists() {
        let existing = load(path)?;
        let ours = existing["hooks"].as_object().is_some_and(|map| {
            map.values()
                .flat_map(|v| v.as_array().into_iter().flatten())
                .all(is_ours_copilot)
        });
        if !ours {
            bail!(
                "{} exists and holds hooks not written by oncebitten",
                path.display()
            );
        }
        if existing == wanted {
            return Ok(false);
        }
    }
    save(path, &wanted)?;
    Ok(true)
}

fn is_ours_copilot(entry: &Value) -> bool {
    entry["bash"].as_str().is_some_and(is_capture_command)
}

fn install_cursor(path: &Path, command: &str) -> Result<bool> {
    let mut value = load(path)?;
    let root = value
        .as_object_mut()
        .with_context(|| format!("{} is not a JSON object", path.display()))?;
    let mut changed = false;
    if !root.contains_key("version") {
        root.insert("version".into(), json!(CURSOR_VERSION));
        changed = true;
    }
    let hooks = root
        .entry("hooks")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .with_context(|| format!("`hooks` in {} is not an object", path.display()))?;
    for event in CURSOR_EVENTS {
        let entries = hooks
            .entry(*event)
            .or_insert_with(|| json!([]))
            .as_array_mut()
            .with_context(|| format!("`hooks.{event}` in {} is not an array", path.display()))?;
        match entries.iter_mut().find(|entry| is_ours(entry)) {
            Some(entry) if entry["command"] == command => {}
            Some(entry) => {
                entry["command"] = json!(command);
                changed = true;
            }
            None => {
                entries.push(json!({"command": command}));
                changed = true;
            }
        }
    }
    if changed {
        save(path, &value)?;
    }
    Ok(changed)
}

fn uninstall_cursor(path: &Path) -> Result<bool> {
    let mut value = load(path)?;
    let Some(hooks) = value["hooks"].as_object_mut() else {
        return Ok(false);
    };
    let mut changed = false;
    for entries in hooks.values_mut().filter_map(Value::as_array_mut) {
        let before = entries.len();
        entries.retain(|entry| !is_ours(entry));
        changed |= entries.len() != before;
    }
    hooks.retain(|_, entries| entries.as_array().is_none_or(|list| !list.is_empty()));
    if changed {
        save(path, &value)?;
    }
    Ok(changed)
}

fn remove_owned_file(path: &Path, owned: impl Fn(&str) -> bool) -> Result<bool> {
    if !path.exists() {
        return Ok(false);
    }
    let text =
        fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))?;
    if !owned(&text) {
        bail!(
            "{} is not managed by oncebitten; left alone",
            path.display()
        );
    }
    fs::remove_file(path).with_context(|| format!("cannot remove {}", path.display()))?;
    Ok(true)
}

fn install_nested(path: &Path, command: &str, events: &[&str]) -> Result<bool> {
    let mut value = load(path)?;
    let root = value
        .as_object_mut()
        .with_context(|| format!("{} is not a JSON object", path.display()))?;
    let hooks = root
        .entry("hooks")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .with_context(|| format!("`hooks` in {} is not an object", path.display()))?;
    let mut changed = false;
    for event in events {
        let groups = hooks
            .entry(*event)
            .or_insert_with(|| json!([]))
            .as_array_mut()
            .with_context(|| format!("`hooks.{event}` in {} is not an array", path.display()))?;
        if groups.iter().any(is_ours_group) {
            changed |= refresh_ours(groups, command);
        } else {
            groups.push(json!({"hooks": [{"type": "command", "command": command}]}));
            changed = true;
        }
    }
    if changed {
        save(path, &value)?;
    }
    Ok(changed)
}

pub fn uninstall(host: Host, path: &Path) -> Result<bool> {
    match host {
        Host::Claude | Host::Codex => uninstall_nested(path),
        Host::Cursor => {
            if path.exists() {
                uninstall_cursor(path)
            } else {
                Ok(false)
            }
        }
        Host::Copilot => remove_owned_file(path, |text| {
            serde_json::from_str::<Value>(text).is_ok_and(|value| {
                value["hooks"].as_object().is_some_and(|map| {
                    map.values()
                        .flat_map(|v| v.as_array().into_iter().flatten())
                        .all(is_ours_copilot)
                })
            })
        }),
        Host::OpenCode | Host::Pi => remove_owned_file(path, |text| text.contains(SCRIPT_MARKER)),
    }
}

fn uninstall_nested(path: &Path) -> Result<bool> {
    if !path.exists() {
        return Ok(false);
    }
    let mut value = load(path)?;
    let root = value
        .as_object_mut()
        .with_context(|| format!("{} is not a JSON object", path.display()))?;
    let Some(hooks) = root.get_mut("hooks") else {
        return Ok(false);
    };
    let hooks = hooks
        .as_object_mut()
        .with_context(|| format!("`hooks` in {} is not an object", path.display()))?;
    let had_hooks = !hooks.is_empty();
    let changed = remove_ours(hooks);
    if changed && had_hooks && hooks.is_empty() {
        root.remove("hooks");
    }
    if changed {
        save(path, &value)?;
    }
    Ok(changed)
}

fn is_ours_group(group: &Value) -> bool {
    group["hooks"]
        .as_array()
        .is_some_and(|hooks| hooks.iter().any(is_ours))
}

fn refresh_ours(groups: &mut [Value], command: &str) -> bool {
    let mut changed = false;
    for hook in groups
        .iter_mut()
        .filter_map(|group| group["hooks"].as_array_mut())
        .flatten()
        .filter(|hook| is_ours(hook))
    {
        if hook["command"] != command {
            hook["command"] = json!(command);
            changed = true;
        }
    }
    changed
}

/// Matches any command `capture_command` or `path_command` writes, whatever the
/// host, quoting or executable path (a moved or renamed binary still counts as ours).
fn is_ours(hook: &Value) -> bool {
    hook["command"].as_str().is_some_and(is_capture_command)
}

fn is_capture_command(command: &str) -> bool {
    ALL_HOSTS.into_iter().any(|host| {
        command
            .strip_suffix(&capture_args(host))
            .is_some_and(|head| !head.trim().is_empty())
    })
}

fn remove_ours(hooks: &mut Map<String, Value>) -> bool {
    let mut changed = false;
    let mut emptied = Vec::new();
    for (event, groups) in hooks.iter_mut() {
        let Some(groups) = groups.as_array_mut() else {
            continue;
        };
        let before = groups.len();
        groups.retain_mut(|group| {
            let Some(entries) = group["hooks"].as_array_mut() else {
                return true;
            };
            let entry_count = entries.len();
            entries.retain(|hook| !is_ours(hook));
            if entries.len() == entry_count {
                return true;
            }
            changed = true;
            !entries.is_empty()
        });
        if before > 0 && groups.is_empty() {
            emptied.push(event.clone());
        }
    }
    for event in emptied {
        hooks.remove(&event);
    }
    changed
}

fn load(path: &Path) -> Result<Value> {
    if !path.exists() {
        return Ok(json!({}));
    }
    let text =
        fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("{} is not valid JSON", path.display()))
}

fn save(path: &Path, value: &Value) -> Result<()> {
    let mut text = serde_json::to_string_pretty(value)?;
    text.push('\n');
    write_text(path, &text)
}

fn write_text(path: &Path, text: &str) -> Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    }
    fs::write(path, text).with_context(|| format!("cannot write {}", path.display()))
}
