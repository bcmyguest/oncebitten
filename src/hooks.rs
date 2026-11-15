use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde_json::{json, Map, Value};

use crate::config::Platform;
use crate::host::Host;

const SETTINGS_DIR: &str = ".claude";
const SETTINGS_FILE: &str = "settings.json";
const BIN_NAME: &str = "oncebitten";
const HOOK_EVENTS: &[&str] = &["UserPromptSubmit", "PostToolUse", "PostToolUseFailure"];

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
    let base = match scope {
        Scope::User => home,
        Scope::Project => project,
    };
    base.join(SETTINGS_DIR).join(SETTINGS_FILE)
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

pub fn install_with(host: Host, path: &Path, command: &str) -> Result<bool> {
    ensure_claude(host)?;
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
    for event in HOOK_EVENTS {
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
    ensure_claude(host)?;
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

fn ensure_claude(host: Host) -> Result<()> {
    if host != Host::Claude {
        bail!(
            "unsupported host `{}` for hooks: its hook format is not verified",
            host.name()
        );
    }
    Ok(())
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
/// quoting or executable path (a moved or renamed binary still counts as ours).
fn is_ours(hook: &Value) -> bool {
    hook["command"].as_str().is_some_and(|command| {
        command
            .strip_suffix(&capture_args(Host::Claude))
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
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    }
    let mut text = serde_json::to_string_pretty(value)?;
    text.push('\n');
    fs::write(path, text).with_context(|| format!("cannot write {}", path.display()))
}
