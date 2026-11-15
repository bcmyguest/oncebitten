use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fmt;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::rule::is_kebab_case;

const PROJECT_DIR: &str = ".oncebitten";
const CONFIG_FILE: &str = "config.toml";
const GIT_MARKER: &str = ".git";
const ENV_PREFIX: &str = "ONCEBITTEN_";
const PATH_VARS: &[&str] = &["HOME", "APPDATA", "XDG_CONFIG_HOME"];
const ENV_VARS: &[(&str, &str, bool)] = &[
    ("ONCEBITTEN_TRIAGE_DEFAULT", "triage.default", false),
    ("ONCEBITTEN_TRIAGE_CLAUDE", "triage.claude", false),
    ("ONCEBITTEN_TRIAGE_CODEX", "triage.codex", false),
    ("ONCEBITTEN_TRIAGE_PI", "triage.pi", false),
    ("ONCEBITTEN_TRIAGE_OPENCODE", "triage.opencode", false),
    ("ONCEBITTEN_TRIGGERS_SESSION", "triggers.session", true),
    ("ONCEBITTEN_TRIGGERS_GATE", "triggers.gate", true),
    ("ONCEBITTEN_SANDBOX_RUNTIME", "sandbox.runtime", false),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Host {
    Claude,
    Codex,
    Pi,
    Opencode,
}

impl Host {
    fn as_str(self) -> &'static str {
        match self {
            Host::Claude => "claude",
            Host::Codex => "codex",
            Host::Pi => "pi",
            Host::Opencode => "opencode",
        }
    }

    fn parse(name: &str) -> Option<Host> {
        match name {
            "claude" => Some(Host::Claude),
            "codex" => Some(Host::Codex),
            "pi" => Some(Host::Pi),
            "opencode" => Some(Host::Opencode),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct TriageTarget {
    pub host: Host,
    pub agent: String,
}

impl TryFrom<String> for TriageTarget {
    type Error = anyhow::Error;

    fn try_from(value: String) -> Result<Self> {
        let Some((host, agent)) = value.split_once(':') else {
            bail!("triage target `{value}` must be <host>:<agent>");
        };
        let Some(host) = Host::parse(host) else {
            bail!("triage target `{value}` names an unknown host; expected claude, codex, pi or opencode");
        };
        if agent.is_empty() {
            bail!("triage target `{value}` has an empty agent");
        }
        Ok(TriageTarget {
            host,
            agent: agent.to_owned(),
        })
    }
}

impl From<TriageTarget> for String {
    fn from(target: TriageTarget) -> String {
        format!("{}:{}", target.host.as_str(), target.agent)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Triage {
    pub default: TriageTarget,
    pub claude: Option<TriageTarget>,
    pub codex: Option<TriageTarget>,
    pub pi: Option<TriageTarget>,
    pub opencode: Option<TriageTarget>,
}

impl Default for Triage {
    fn default() -> Self {
        Triage {
            default: TriageTarget {
                host: Host::Claude,
                agent: "haiku".to_owned(),
            },
            claude: None,
            codex: None,
            pi: None,
            opencode: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SessionTrigger {
    UserCorrection,
    TestFailThenPass,
    RepeatFailure,
    Revert,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GateTrigger {
    PrePush,
    PrCreate,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Triggers {
    pub session: Vec<SessionTrigger>,
    pub gate: Vec<GateTrigger>,
}

impl Default for Triggers {
    fn default() -> Self {
        Triggers {
            session: vec![
                SessionTrigger::UserCorrection,
                SessionTrigger::TestFailThenPass,
                SessionTrigger::RepeatFailure,
                SessionTrigger::Revert,
            ],
            gate: vec![GateTrigger::PrePush, GateTrigger::PrCreate],
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Runtime {
    #[default]
    Openshell,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Sandbox {
    pub runtime: Runtime,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Source {
    pub name: String,
    pub url: String,
    #[serde(default)]
    pub writable: bool,
    #[serde(default)]
    pub rev: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Rules {
    pub sources: Vec<Source>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Config {
    pub triage: Triage,
    pub triggers: Triggers,
    pub sandbox: Sandbox,
    pub rules: Rules,
}

impl Config {
    fn validate(&self) -> Result<()> {
        let per_host = [
            (Host::Claude, &self.triage.claude),
            (Host::Codex, &self.triage.codex),
            (Host::Pi, &self.triage.pi),
            (Host::Opencode, &self.triage.opencode),
        ];
        for (host, target) in per_host {
            if let Some(target) = target {
                if target.host != host {
                    bail!(
                        "triage.{} names host `{}`; a per-host key must name its own host",
                        host.as_str(),
                        target.host.as_str()
                    );
                }
            }
        }
        let writable = self.rules.sources.iter().filter(|s| s.writable).count();
        if writable > 1 {
            bail!("rules.sources: {writable} sources are writable; at most one may be");
        }
        let mut names = BTreeSet::new();
        for source in &self.rules.sources {
            if !is_kebab_case(&source.name) {
                bail!(
                    "rules.sources: source name `{}` must be lowercase kebab-case",
                    source.name
                );
            }
            if !names.insert(source.name.as_str()) {
                bail!("rules.sources: duplicate source name `{}`", source.name);
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    Env,
    Project,
    User,
    Default,
}

impl fmt::Display for Origin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Origin::Env => "env",
            Origin::Project => "project",
            Origin::User => "user",
            Origin::Default => "default",
        };
        f.write_str(name)
    }
}

#[derive(Debug, Clone)]
pub struct Inputs {
    pub cwd: PathBuf,
    pub vars: BTreeMap<String, String>,
}

impl Inputs {
    pub fn from_process() -> Result<Self> {
        Ok(Inputs {
            cwd: std::env::current_dir().context("reading the working directory")?,
            vars: collect_vars(std::env::vars_os())?,
        })
    }
}

/// Keeps Unicode variables. Skips non-Unicode ones unless oncebitten reads them,
/// in which case loading fails with the variable's name.
pub fn collect_vars(
    vars: impl IntoIterator<Item = (OsString, OsString)>,
) -> Result<BTreeMap<String, String>> {
    let mut out = BTreeMap::new();
    for (name, value) in vars {
        let lossy = name.to_string_lossy().into_owned();
        match (name.into_string(), value.into_string()) {
            (Ok(name), Ok(value)) => {
                out.insert(name, value);
            }
            _ if lossy.starts_with(ENV_PREFIX) || PATH_VARS.contains(&lossy.as_str()) => {
                bail!("environment variable {lossy} is not valid Unicode");
            }
            _ => {}
        }
    }
    Ok(out)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigPaths {
    pub user: PathBuf,
    pub project: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct Loaded {
    pub config: Config,
    pub origins: BTreeMap<String, Origin>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Windows,
    Unix,
}

impl Platform {
    pub fn current() -> Self {
        if cfg!(windows) {
            Platform::Windows
        } else {
            Platform::Unix
        }
    }

    pub(crate) fn is_absolute(self, path: &str) -> bool {
        match self {
            Platform::Unix => path.starts_with('/'),
            Platform::Windows => {
                let bytes = path.as_bytes();
                let drive = bytes.len() >= 3
                    && bytes[0].is_ascii_alphabetic()
                    && bytes[1] == b':'
                    && matches!(bytes[2], b'\\' | b'/');
                drive || path.starts_with(r"\\")
            }
        }
    }
}

pub fn user_config_path(vars: &BTreeMap<String, String>) -> Result<PathBuf> {
    user_config_path_for(vars, Platform::current())
}

pub fn user_config_path_for(
    vars: &BTreeMap<String, String>,
    platform: Platform,
) -> Result<PathBuf> {
    let base = match vars
        .get("XDG_CONFIG_HOME")
        .filter(|dir| platform.is_absolute(dir))
    {
        Some(dir) => PathBuf::from(dir),
        None => match platform {
            Platform::Windows => PathBuf::from(
                vars.get("APPDATA")
                    .filter(|dir| platform.is_absolute(dir))
                    .context("neither XDG_CONFIG_HOME nor APPDATA is an absolute path")?,
            ),
            Platform::Unix => PathBuf::from(
                vars.get("HOME")
                    .filter(|dir| platform.is_absolute(dir))
                    .context("neither XDG_CONFIG_HOME nor HOME is an absolute path")?,
            )
            .join(".config"),
        },
    };
    Ok(base.join("oncebitten").join(CONFIG_FILE))
}

pub fn repo_root(cwd: &Path) -> Option<PathBuf> {
    cwd.ancestors()
        .find(|dir| dir.join(GIT_MARKER).exists())
        .map(Path::to_path_buf)
}

pub fn project_config_path(cwd: &Path) -> Option<PathBuf> {
    let root_depth = cwd
        .ancestors()
        .position(|dir| dir.join(GIT_MARKER).exists())?;
    cwd.ancestors()
        .take(root_depth + 1)
        .map(|dir| dir.join(PROJECT_DIR).join(CONFIG_FILE))
        .find(|path| path.is_file())
}

pub fn config_paths(inputs: &Inputs) -> Result<ConfigPaths> {
    Ok(ConfigPaths {
        user: user_config_path(&inputs.vars)?,
        project: project_config_path(&inputs.cwd),
    })
}

pub fn load(inputs: &Inputs) -> Result<Loaded> {
    let paths = config_paths(inputs)?;
    let mut merged: BTreeMap<String, (toml::Value, Origin)> = BTreeMap::new();

    let defaults = toml::Value::try_from(Config::default())?;
    let defaults = defaults
        .as_table()
        .context("built-in defaults are not a table")?;
    merge(&mut merged, defaults, Origin::Default);

    if let Some(table) = read_layer(&paths.user)? {
        merge(&mut merged, &table, Origin::User);
    }
    if let Some(path) = &paths.project {
        if let Some(table) = read_layer(path)? {
            merge(&mut merged, &table, Origin::Project);
        }
    }
    let env = env_table(&inputs.vars);
    check(&env).context("invalid ONCEBITTEN_* environment variables")?;
    merge(&mut merged, &env, Origin::Env);
    let warnings = unknown_env_vars(&inputs.vars)
        .map(|name| format!("unknown environment variable {name} is ignored"))
        .collect();

    let mut table = toml::Table::new();
    let mut origins = BTreeMap::new();
    for (key, (value, origin)) in merged {
        set_leaf(&mut table, &key, value);
        origins.insert(key, origin);
    }
    let config: Config = toml::Value::Table(table)
        .try_into()
        .context("merged configuration is invalid")?;
    config.validate()?;
    Ok(Loaded {
        config,
        origins,
        warnings,
    })
}

pub fn render(loaded: &Loaded, with_origin: bool) -> Result<String> {
    let text = toml::to_string(&loaded.config)?;
    if !with_origin {
        return Ok(text);
    }
    let mut section = String::new();
    let mut out = String::new();
    for line in text.lines() {
        if line.starts_with('[') {
            section = line.trim_matches(['[', ']']).to_owned();
            match loaded.origins.get(&section) {
                Some(origin) => out.push_str(&format!("{line}  # origin: {origin}\n")),
                None => out.push_str(&format!("{line}\n")),
            }
            continue;
        }
        let origin = line.split_once(" = ").and_then(|(key, _)| {
            let path = if section.is_empty() {
                key.to_owned()
            } else {
                format!("{section}.{key}")
            };
            loaded.origins.get(&path)
        });
        match origin {
            Some(origin) => out.push_str(&format!("{line}  # origin: {origin}\n")),
            None => out.push_str(&format!("{line}\n")),
        }
    }
    Ok(out)
}

fn read_layer(path: &Path) -> Result<Option<toml::Table>> {
    if !path.is_file() {
        return Ok(None);
    }
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let table: toml::Table =
        toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
    check(&table).with_context(|| format!("invalid {}", path.display()))?;
    Ok(Some(table))
}

fn check(table: &toml::Table) -> Result<()> {
    let config: Config = toml::Value::Table(table.clone()).try_into()?;
    config.validate()
}

fn env_table(vars: &BTreeMap<String, String>) -> toml::Table {
    let mut table = toml::Table::new();
    for (name, key, is_list) in ENV_VARS {
        let Some(raw) = vars.get(*name).map(|v| v.trim()).filter(|v| !v.is_empty()) else {
            continue;
        };
        let value = if *is_list {
            toml::Value::Array(
                raw.split(',')
                    .map(|item| toml::Value::String(item.trim().to_owned()))
                    .collect(),
            )
        } else {
            toml::Value::String(raw.to_owned())
        };
        set_leaf(&mut table, key, value);
    }
    table
}

fn unknown_env_vars(vars: &BTreeMap<String, String>) -> impl Iterator<Item = &str> {
    vars.keys()
        .map(String::as_str)
        .filter(|name| name.starts_with(ENV_PREFIX))
        .filter(|name| !ENV_VARS.iter().any(|(known, _, _)| known == name))
}

fn merge(
    merged: &mut BTreeMap<String, (toml::Value, Origin)>,
    table: &toml::Table,
    origin: Origin,
) {
    let mut leaves = Vec::new();
    flatten("", table, &mut leaves);
    for (key, value) in leaves {
        merged.insert(key, (value, origin));
    }
}

fn flatten(prefix: &str, table: &toml::Table, leaves: &mut Vec<(String, toml::Value)>) {
    for (name, value) in table {
        let path = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}.{name}")
        };
        match value {
            toml::Value::Table(inner) => flatten(&path, inner, leaves),
            leaf => leaves.push((path, leaf.clone())),
        }
    }
}

fn set_leaf(table: &mut toml::Table, key: &str, value: toml::Value) {
    let Some((section, field)) = key.split_once('.') else {
        table.insert(key.to_owned(), value);
        return;
    };
    let entry = table
        .entry(section.to_owned())
        .or_insert_with(|| toml::Value::Table(toml::Table::new()));
    if let toml::Value::Table(inner) = entry {
        inner.insert(field.to_owned(), value);
    }
}
