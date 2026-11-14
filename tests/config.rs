use std::fs;
use std::path::{Path, PathBuf};

use oncebitten::config::{
    load, project_config_path, user_config_path_for, Inputs, Origin, Platform,
};
use tempfile::TempDir;

struct Sandbox {
    root: TempDir,
}

impl Sandbox {
    fn new() -> Self {
        let sandbox = Sandbox {
            root: TempDir::new().unwrap(),
        };
        fs::create_dir_all(sandbox.xdg()).unwrap();
        fs::create_dir_all(sandbox.home()).unwrap();
        fs::create_dir_all(sandbox.repo().join(".git")).unwrap();
        sandbox
    }

    fn home(&self) -> PathBuf {
        self.root.path().join("home")
    }

    fn xdg(&self) -> PathBuf {
        self.root.path().join("xdg")
    }

    fn repo(&self) -> PathBuf {
        self.root.path().join("repo")
    }

    fn write_user(&self, text: &str) {
        write(&self.xdg().join("oncebitten/config.toml"), text);
    }

    fn write_project(&self, text: &str) {
        write(&self.repo().join(".oncebitten/config.toml"), text);
    }

    fn inputs(&self, extra: &[(&str, &str)]) -> Inputs {
        let mut vars = std::collections::BTreeMap::new();
        vars.insert("HOME".to_owned(), self.home().display().to_string());
        vars.insert(
            "XDG_CONFIG_HOME".to_owned(),
            self.xdg().display().to_string(),
        );
        for (name, value) in extra {
            vars.insert((*name).to_owned(), (*value).to_owned());
        }
        Inputs {
            cwd: self.repo(),
            vars,
        }
    }
}

fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

fn error_chain(result: anyhow::Result<impl std::fmt::Debug>) -> String {
    format!("{:#}", result.unwrap_err())
}

#[test]
fn defaults_apply_without_any_file_or_variable() {
    let sandbox = Sandbox::new();
    let loaded = load(&sandbox.inputs(&[])).unwrap();

    assert_eq!(loaded.config.triage.default.agent, "haiku");
    assert_eq!(loaded.config.triggers.gate.len(), 2);
    assert_eq!(loaded.config.triggers.session.len(), 4);
    assert!(loaded.config.rules.sources.is_empty());
    assert_eq!(loaded.origins["triage.default"], Origin::Default);
}

#[test]
fn user_file_overrides_default() {
    let sandbox = Sandbox::new();
    sandbox.write_user("[triage]\ndefault = \"codex:small\"\n");
    let loaded = load(&sandbox.inputs(&[])).unwrap();

    assert_eq!(loaded.config.triage.default.agent, "small");
    assert_eq!(loaded.origins["triage.default"], Origin::User);
}

#[test]
fn project_file_overrides_user_file() {
    let sandbox = Sandbox::new();
    sandbox.write_user("[triage]\ndefault = \"codex:small\"\n");
    sandbox.write_project("[triage]\ndefault = \"pi:default\"\n");
    let loaded = load(&sandbox.inputs(&[])).unwrap();

    assert_eq!(loaded.config.triage.default.agent, "default");
    assert_eq!(loaded.origins["triage.default"], Origin::Project);
}

#[test]
fn env_overrides_project_file() {
    let sandbox = Sandbox::new();
    sandbox.write_project("[triage]\ndefault = \"pi:default\"\n");
    let loaded =
        load(&sandbox.inputs(&[("ONCEBITTEN_TRIAGE_DEFAULT", "opencode:default")])).unwrap();

    assert_eq!(loaded.config.triage.default.agent, "default");
    assert_eq!(loaded.origins["triage.default"], Origin::Env);
}

#[test]
fn merge_is_per_key_not_per_file() {
    let sandbox = Sandbox::new();
    sandbox.write_user("[triage]\ndefault = \"codex:small\"\n[sandbox]\nruntime = \"openshell\"\n");
    sandbox.write_project("[triggers]\ngate = [\"pr-create\"]\n");
    let loaded = load(&sandbox.inputs(&[])).unwrap();

    assert_eq!(loaded.config.triage.default.agent, "small");
    assert_eq!(loaded.config.triggers.gate.len(), 1);
    assert_eq!(loaded.config.triggers.session.len(), 4);
    assert_eq!(loaded.origins["triage.default"], Origin::User);
    assert_eq!(loaded.origins["triggers.gate"], Origin::Project);
    assert_eq!(loaded.origins["triggers.session"], Origin::Default);
}

#[test]
fn env_list_is_comma_separated() {
    let sandbox = Sandbox::new();
    let loaded =
        load(&sandbox.inputs(&[("ONCEBITTEN_TRIGGERS_GATE", "pre-push, pr-create")])).unwrap();

    assert_eq!(loaded.config.triggers.gate.len(), 2);
    assert_eq!(loaded.origins["triggers.gate"], Origin::Env);
}

#[test]
fn per_host_key_accepts_its_own_host() {
    let sandbox = Sandbox::new();
    sandbox.write_user("[triage]\npi = \"pi:default\"\nopencode = \"opencode:default\"\n");
    let loaded = load(&sandbox.inputs(&[])).unwrap();

    assert_eq!(loaded.config.triage.pi.unwrap().agent, "default");
    assert!(loaded.config.triage.opencode.is_some());
}

#[test]
fn invalid_host_agent_values_are_rejected() {
    let cases = [
        ("bogus:x", "unknown host"),
        ("claude", "must be <host>:<agent>"),
        ("claude:", "empty agent"),
    ];
    for (value, expected) in cases {
        let sandbox = Sandbox::new();
        sandbox.write_user(&format!("[triage]\ndefault = \"{value}\"\n"));
        let message = error_chain(load(&sandbox.inputs(&[])));
        assert!(message.contains(expected), "{value}: {message}");
    }
}

#[test]
fn invalid_env_host_agent_value_is_rejected() {
    let sandbox = Sandbox::new();
    let message = error_chain(load(
        &sandbox.inputs(&[("ONCEBITTEN_TRIAGE_DEFAULT", "claude")]),
    ));
    assert!(message.contains("must be <host>:<agent>"), "{message}");
}

#[test]
fn per_host_key_naming_another_host_is_rejected() {
    let sandbox = Sandbox::new();
    sandbox.write_user("[triage]\npi = \"claude:haiku\"\n");
    let message = error_chain(load(&sandbox.inputs(&[])));
    assert!(message.contains("per-host key"), "{message}");
}

#[test]
fn two_writable_sources_are_rejected() {
    let sandbox = Sandbox::new();
    sandbox.write_project(
        "[[rules.sources]]\nurl = \"https://example.invalid/a.git\"\nwritable = true\n\n\
         [[rules.sources]]\nurl = \"https://example.invalid/b.git\"\nwritable = true\n",
    );
    let message = error_chain(load(&sandbox.inputs(&[])));
    assert!(message.contains("at most one may be"), "{message}");
}

#[test]
fn one_writable_source_among_read_only_sources_is_accepted() {
    let sandbox = Sandbox::new();
    sandbox.write_project(
        "[[rules.sources]]\nurl = \"https://example.invalid/a.git\"\n\n\
         [[rules.sources]]\nurl = \"https://example.invalid/b.git\"\nwritable = true\nrev = \"v1\"\n",
    );
    let loaded = load(&sandbox.inputs(&[])).unwrap();

    assert_eq!(loaded.config.rules.sources.len(), 2);
    assert!(loaded.config.rules.sources[1].writable);
    assert_eq!(loaded.config.rules.sources[1].rev.as_deref(), Some("v1"));
}

#[test]
fn unknown_top_level_key_is_rejected() {
    let sandbox = Sandbox::new();
    sandbox.write_user("[bogus]\nvalue = 1\n");
    let message = error_chain(load(&sandbox.inputs(&[])));
    assert!(message.contains("unknown field"), "{message}");
}

#[test]
fn unknown_nested_key_is_rejected() {
    let sandbox = Sandbox::new();
    sandbox.write_project("[triage]\nfallback = \"claude:haiku\"\n");
    let message = error_chain(load(&sandbox.inputs(&[])));
    assert!(message.contains("unknown field"), "{message}");
}

#[test]
fn unknown_trigger_name_is_rejected() {
    let sandbox = Sandbox::new();
    sandbox.write_user("[triggers]\nsession = [\"mood\"]\n");
    let message = error_chain(load(&sandbox.inputs(&[])));
    assert!(message.contains("mood"), "{message}");
}

#[test]
fn project_config_is_found_from_a_subdirectory() {
    let sandbox = Sandbox::new();
    sandbox.write_project("[triage]\ndefault = \"pi:default\"\n");
    let nested = sandbox.repo().join("src/deep");
    fs::create_dir_all(&nested).unwrap();

    assert_eq!(
        project_config_path(&nested),
        Some(sandbox.repo().join(".oncebitten/config.toml"))
    );
}

#[test]
fn project_config_above_the_git_root_is_ignored() {
    let sandbox = Sandbox::new();
    write(
        &sandbox.root.path().join(".oncebitten/config.toml"),
        "[triage]\ndefault = \"pi:default\"\n",
    );

    assert_eq!(project_config_path(&sandbox.repo()), None);
}

fn vars(pairs: &[(&str, &str)]) -> std::collections::BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
        .collect()
}

fn assert_user_file_under(path: &Path, base: &str) {
    assert!(path.starts_with(base), "{}", path.display());
    assert!(
        path.ends_with("oncebitten/config.toml"),
        "{}",
        path.display()
    );
}

#[test]
fn windows_user_path_comes_from_appdata() {
    let path = user_config_path_for(
        &vars(&[("APPDATA", r"C:\Users\b\AppData\Roaming")]),
        Platform::Windows,
    )
    .unwrap();

    assert_user_file_under(&path, r"C:\Users\b\AppData\Roaming");
}

#[test]
fn windows_absolute_xdg_config_home_takes_precedence() {
    let path = user_config_path_for(
        &vars(&[
            ("XDG_CONFIG_HOME", r"D:\cfg"),
            ("APPDATA", r"C:\Users\b\AppData\Roaming"),
        ]),
        Platform::Windows,
    )
    .unwrap();

    assert_user_file_under(&path, r"D:\cfg");
}

#[test]
fn windows_unc_xdg_config_home_is_absolute() {
    let path = user_config_path_for(
        &vars(&[
            ("XDG_CONFIG_HOME", r"\\server\share\cfg"),
            ("APPDATA", r"C:\Users\b\AppData\Roaming"),
        ]),
        Platform::Windows,
    )
    .unwrap();

    assert_user_file_under(&path, r"\\server\share\cfg");
}

#[test]
fn windows_relative_xdg_config_home_is_ignored() {
    let path = user_config_path_for(
        &vars(&[
            ("XDG_CONFIG_HOME", "cfg"),
            ("APPDATA", r"C:\Users\b\AppData\Roaming"),
        ]),
        Platform::Windows,
    )
    .unwrap();

    assert_user_file_under(&path, r"C:\Users\b\AppData\Roaming");
}

#[test]
fn windows_without_appdata_is_an_error_and_home_is_not_used() {
    let message = error_chain(user_config_path_for(
        &vars(&[("HOME", r"C:\Users\b")]),
        Platform::Windows,
    ));

    assert!(message.contains("APPDATA"), "{message}");
}

#[test]
fn unix_user_path_uses_home_and_ignores_appdata() {
    let path = user_config_path_for(
        &vars(&[
            ("HOME", "/home/b"),
            ("APPDATA", r"C:\Users\b\AppData\Roaming"),
        ]),
        Platform::Unix,
    )
    .unwrap();

    assert!(path.starts_with("/home/b"), "{}", path.display());
    assert!(
        path.ends_with(".config/oncebitten/config.toml"),
        "{}",
        path.display()
    );
}
