use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde_json::{json, Map, Value};

use crate::host::Host;

const SETTINGS_DIR: &str = ".claude";
const SETTINGS_FILE: &str = "settings.json";
const COMMAND: &str = "oncebitten capture --host claude";
const HOOK_EVENTS: &[&str] = &["UserPromptSubmit", "PostToolUse", "PostToolUseFailure"];

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

pub fn install(host: Host, path: &Path) -> Result<bool> {
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
        if !groups.iter().any(is_ours_group) {
            groups.push(json!({"hooks": [{"type": "command", "command": COMMAND}]}));
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

fn is_ours(hook: &Value) -> bool {
    hook["command"] == COMMAND
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
