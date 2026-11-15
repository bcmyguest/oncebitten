use std::collections::BTreeMap;
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
const OPENCODE_STEER_TEMPLATE: &str = include_str!("../packaging/opencode/oncebitten-steer.js");
const PI_STEER_TEMPLATE: &str = include_str!("../packaging/pi/oncebitten-steer.js");
// Steer hooks run before a tool call or prompt. Cursor's beforeSubmitPrompt and
// Copilot's userPromptSubmitted cannot add context, so those hosts get only the
// pre-tool hook; see docs/steer.md.
const CLAUDE_STEER_EVENTS: &[&str] = &["PreToolUse", "UserPromptSubmit"];
const CODEX_STEER_EVENTS: &[&str] = &["PreToolUse", "UserPromptSubmit"];
const CURSOR_STEER_EVENTS: &[&str] = &["preToolUse"];
const COPILOT_STEER_EVENTS: &[&str] = &["preToolUse"];
const STEER_JSON_FILE: &str = "oncebitten-steer.json";
const STEER_SCRIPT_FILE: &str = "oncebitten-steer.js";
const COMMAND_PLACEHOLDER: &str = "__ONCEBITTEN_COMMAND__";
const PI_AGENT_DIR_VAR: &str = "PI_CODING_AGENT_DIR";

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

/// `target_path`, except that a user-scope pi install follows
/// `PI_CODING_AGENT_DIR` the way pi does: a non-empty value replaces
/// `~/.pi/agent`, with a leading `~` or `~/` expanded to `home`.
pub fn install_target(
    host: Host,
    scope: Scope,
    home: &Path,
    project: &Path,
    vars: &BTreeMap<String, String>,
) -> Result<PathBuf> {
    let agent_dir = vars.get(PI_AGENT_DIR_VAR).filter(|value| !value.is_empty());
    let (Host::Pi, Scope::User, Some(value)) = (host, scope, agent_dir) else {
        return Ok(target_path(host, scope, home, project));
    };
    let dir = match value.as_str() {
        "~" => home.to_path_buf(),
        value => match value.strip_prefix("~/") {
            Some(rest) => home.join(rest),
            None => PathBuf::from(value),
        },
    };
    if !dir.is_absolute() {
        bail!(
            "{PI_AGENT_DIR_VAR}=`{value}` is relative; pi resolves it against its own working directory, so set an absolute path"
        );
    }
    Ok(dir.join("extensions").join("oncebitten.js"))
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
    install_copilot_events(path, posix, powershell, COPILOT_EVENTS, is_capture_command)
}

fn install_copilot_events(
    path: &Path,
    posix: &str,
    powershell: &str,
    events: &[&str],
    owned: fn(&str) -> bool,
) -> Result<bool> {
    let entry = json!({
        "type": "command",
        "bash": posix,
        "powershell": powershell,
        "timeoutSec": HOOK_TIMEOUT_SECS,
    });
    let mut hooks = Map::new();
    for event in events {
        hooks.insert((*event).to_string(), json!([entry]));
    }
    let wanted = json!({"version": COPILOT_VERSION, "hooks": hooks});
    if path.exists() {
        let existing = load(path)?;
        let ours = existing["hooks"].as_object().is_some_and(|map| {
            map.values()
                .flat_map(|v| v.as_array().into_iter().flatten())
                .all(|entry| entry["bash"].as_str().is_some_and(owned))
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
    install_cursor_events(path, command, CURSOR_EVENTS, is_ours)
}

fn install_cursor_events(
    path: &Path,
    command: &str,
    events: &[&str],
    mine: fn(&Value) -> bool,
) -> Result<bool> {
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
    for event in events {
        let entries = hooks
            .entry(*event)
            .or_insert_with(|| json!([]))
            .as_array_mut()
            .with_context(|| format!("`hooks.{event}` in {} is not an array", path.display()))?;
        match entries.iter_mut().find(|entry| mine(entry)) {
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
    uninstall_cursor_with(path, is_ours)
}

fn uninstall_cursor_with(path: &Path, mine: fn(&Value) -> bool) -> Result<bool> {
    let mut value = load(path)?;
    let Some(hooks) = value["hooks"].as_object_mut() else {
        return Ok(false);
    };
    let mut changed = false;
    for entries in hooks.values_mut().filter_map(Value::as_array_mut) {
        let before = entries.len();
        entries.retain(|entry| !mine(entry));
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
    install_nested_with(path, command, events, is_ours)
}

fn install_nested_with(
    path: &Path,
    command: &str,
    events: &[&str],
    mine: fn(&Value) -> bool,
) -> Result<bool> {
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
        if groups.iter().any(|group| holds(group, mine)) {
            changed |= refresh_ours(groups, command, mine);
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

/// Whether `path` holds oncebitten's capture hook for `host`. Reads only.
pub fn is_installed(host: Host, path: &Path) -> Result<bool> {
    if !path.exists() {
        return Ok(false);
    }
    if matches!(host, Host::OpenCode | Host::Pi) {
        let text =
            fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))?;
        return Ok(text.contains(SCRIPT_MARKER));
    }
    let value = load(path)?;
    let mut entries = value["hooks"]
        .as_object()
        .into_iter()
        .flat_map(|map| map.values())
        .filter_map(Value::as_array)
        .flatten();
    Ok(match host {
        Host::Claude | Host::Codex => entries.any(is_ours_group),
        Host::Cursor => entries.any(is_ours),
        Host::Copilot => entries.any(is_ours_copilot),
        Host::OpenCode | Host::Pi => unreachable!("handled above"),
    })
}

fn uninstall_nested(path: &Path) -> Result<bool> {
    uninstall_nested_with(path, is_ours)
}

fn uninstall_nested_with(path: &Path, mine: fn(&Value) -> bool) -> Result<bool> {
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
    let changed = remove_ours(hooks, mine);
    if changed && had_hooks && hooks.is_empty() {
        root.remove("hooks");
    }
    if changed {
        save(path, &value)?;
    }
    Ok(changed)
}

fn holds(group: &Value, mine: fn(&Value) -> bool) -> bool {
    group["hooks"]
        .as_array()
        .is_some_and(|hooks| hooks.iter().any(mine))
}

fn refresh_ours(groups: &mut [Value], command: &str, mine: fn(&Value) -> bool) -> bool {
    let mut changed = false;
    for hook in groups
        .iter_mut()
        .filter_map(|group| group["hooks"].as_array_mut())
        .flatten()
        .filter(|hook| mine(hook))
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

fn remove_ours(hooks: &mut Map<String, Value>, mine: fn(&Value) -> bool) -> bool {
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
            entries.retain(|hook| !mine(hook));
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

/// The file the steer hook lives in. Claude, Codex and Cursor share the capture
/// hooks' settings file; Copilot, opencode and pi get a second owned file beside
/// the capture one.
pub fn steer_target(
    host: Host,
    scope: Scope,
    home: &Path,
    project: &Path,
    vars: &BTreeMap<String, String>,
) -> Result<PathBuf> {
    let path = install_target(host, scope, home, project, vars)?;
    Ok(match host {
        Host::Claude | Host::Codex | Host::Cursor => path,
        Host::Copilot => path.with_file_name(STEER_JSON_FILE),
        Host::OpenCode | Host::Pi => path.with_file_name(STEER_SCRIPT_FILE),
    })
}

/// What the host's hooks cannot do with a steer rule, for `hooks install` to
/// print. `None` when the host supports deny and inform on tools and prompts.
pub fn steer_limits(host: Host) -> Option<&'static str> {
    match host {
        Host::Claude | Host::Codex | Host::OpenCode => None,
        Host::Cursor => Some(
            "cursor hooks cannot add context before a tool call or prompt: steer rules with action = \"inform\" do nothing; deny works on tool calls",
        ),
        Host::Copilot => Some(
            "copilot file hooks cannot add context before a tool call and drop userPromptSubmitted output: steer rules with action = \"inform\" do nothing; deny works on tool calls",
        ),
        Host::Pi => Some(
            "pi cannot add context to a pending tool call: inform on a tool call arrives after the turn's tool calls run",
        ),
    }
}

pub fn steer_command(exe: &Path, host: Host, shell: Shell) -> String {
    let capture = capture_command(exe, host, shell);
    let head = capture
        .strip_suffix(&capture_args(host))
        .unwrap_or(&capture);
    format!("{head}{}", steer_args(host))
}

fn steer_args(host: Host) -> String {
    format!("steer --host {}", host.name())
}

/// Installs the steer hook with the absolute path of the running executable.
pub fn install_steer_exe(host: Host, path: &Path, exe: &Path, platform: Platform) -> Result<bool> {
    match host {
        Host::Copilot => install_copilot_events(
            path,
            &steer_command(exe, host, Shell::Posix),
            &steer_command(exe, host, Shell::PowerShell),
            COPILOT_STEER_EVENTS,
            is_steer_command,
        ),
        _ => install_steer_with(
            host,
            path,
            &steer_command(exe, host, default_shell(platform)),
        ),
    }
}

pub fn install_steer_with(host: Host, path: &Path, command: &str) -> Result<bool> {
    match host {
        Host::Claude => install_nested_with(path, command, CLAUDE_STEER_EVENTS, is_steer),
        Host::Codex => install_nested_with(path, command, CODEX_STEER_EVENTS, is_steer),
        Host::Cursor => install_cursor_events(path, command, CURSOR_STEER_EVENTS, is_steer),
        Host::Copilot => install_copilot_events(
            path,
            command,
            command,
            COPILOT_STEER_EVENTS,
            is_steer_command,
        ),
        Host::OpenCode => install_script(path, OPENCODE_STEER_TEMPLATE, command),
        Host::Pi => install_script(path, PI_STEER_TEMPLATE, command),
    }
}

pub fn uninstall_steer(host: Host, path: &Path) -> Result<bool> {
    match host {
        Host::Claude | Host::Codex => uninstall_nested_with(path, is_steer),
        Host::Cursor => {
            if path.exists() {
                uninstall_cursor_with(path, is_steer)
            } else {
                Ok(false)
            }
        }
        Host::Copilot => remove_owned_file(path, |text| {
            serde_json::from_str::<Value>(text).is_ok_and(|value| {
                value["hooks"].as_object().is_some_and(|map| {
                    map.values()
                        .flat_map(|v| v.as_array().into_iter().flatten())
                        .all(|entry| entry["bash"].as_str().is_some_and(is_steer_command))
                })
            })
        }),
        Host::OpenCode | Host::Pi => remove_owned_file(path, |text| text.contains(SCRIPT_MARKER)),
    }
}

fn is_steer(hook: &Value) -> bool {
    hook["command"].as_str().is_some_and(is_steer_command)
}

fn is_steer_command(command: &str) -> bool {
    ALL_HOSTS.into_iter().any(|host| {
        command
            .strip_suffix(&steer_args(host))
            .is_some_and(|head| !head.trim().is_empty())
    })
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
