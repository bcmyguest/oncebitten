use std::fs;
use std::path::{Path, PathBuf};

use oncebitten::config::{
    collect_vars, load, project_config_path, user_config_path_for, Inputs, Origin, Platform,
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
        "[[rules.sources]]\nname = \"a\"\nurl = \"https://example.invalid/a.git\"\nwritable = true\n\n\
         [[rules.sources]]\nname = \"b\"\nurl = \"https://example.invalid/b.git\"\nwritable = true\n",
    );
    let message = error_chain(load(&sandbox.inputs(&[])));
    assert!(message.contains("at most one may be"), "{message}");
}

#[test]
fn one_writable_source_among_read_only_sources_is_accepted() {
    let sandbox = Sandbox::new();
    sandbox.write_project(
        "[[rules.sources]]\nname = \"a\"\nurl = \"https://example.invalid/a.git\"\n\n\
         [[rules.sources]]\nname = \"b\"\nurl = \"https://example.invalid/b.git\"\nwritable = true\nrev = \"v1\"\n",
    );
    let loaded = load(&sandbox.inputs(&[])).unwrap();

    assert_eq!(loaded.config.rules.sources.len(), 2);
    assert!(loaded.config.rules.sources[1].writable);
    assert_eq!(loaded.config.rules.sources[1].rev.as_deref(), Some("v1"));
}

#[test]
fn source_without_name_is_rejected() {
    let sandbox = Sandbox::new();
    sandbox.write_project("[[rules.sources]]\nurl = \"https://example.invalid/a.git\"\n");
    let message = error_chain(load(&sandbox.inputs(&[])));

    assert!(message.contains("missing field `name`"), "{message}");
    assert!(message.contains(".oncebitten"), "{message}");
}

#[test]
fn source_name_must_be_kebab_case() {
    let sandbox = Sandbox::new();
    sandbox.write_project(
        "[[rules.sources]]\nname = \"Team_Rules\"\nurl = \"https://example.invalid/a.git\"\n",
    );
    let message = error_chain(load(&sandbox.inputs(&[])));

    assert!(message.contains("Team_Rules"), "{message}");
    assert!(message.contains("kebab-case"), "{message}");
}

#[test]
fn duplicate_source_names_are_rejected() {
    let sandbox = Sandbox::new();
    sandbox.write_project(
        "[[rules.sources]]\nname = \"team\"\nurl = \"https://example.invalid/a.git\"\n\n\
         [[rules.sources]]\nname = \"team\"\nurl = \"https://example.invalid/b.git\"\n",
    );
    let message = error_chain(load(&sandbox.inputs(&[])));

    assert!(
        message.contains("duplicate source name `team`"),
        "{message}"
    );
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
fn windows_without_appdata_falls_back_to_home_appdata_roaming() {
    let path = user_config_path_for(&vars(&[("HOME", r"C:\Users\b")]), Platform::Windows).unwrap();

    assert!(
        path.starts_with(
            std::path::PathBuf::from(r"C:\Users\b")
                .join("AppData")
                .join("Roaming")
        ),
        "{}",
        path.display()
    );
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

#[test]
fn project_rules_sources_replace_the_whole_user_list() {
    let sandbox = Sandbox::new();
    sandbox.write_user(
        "[[rules.sources]]\nname = \"a\"\nurl = \"https://example.com/a.git\"\n\
         [[rules.sources]]\nname = \"b\"\nurl = \"https://example.com/b.git\"\n",
    );
    sandbox.write_project("[[rules.sources]]\nname = \"c\"\nurl = \"https://example.com/c.git\"\n");
    let loaded = load(&sandbox.inputs(&[])).unwrap();

    let urls: Vec<&str> = loaded
        .config
        .rules
        .sources
        .iter()
        .map(|source| source.url.as_str())
        .collect();
    assert_eq!(urls, ["https://example.com/c.git"]);
    assert_eq!(loaded.origins["rules.sources"], Origin::Project);
}

#[test]
fn empty_env_variable_counts_as_unset() {
    let sandbox = Sandbox::new();
    sandbox.write_project("[triage]\ndefault = \"pi:default\"\n");
    let loaded = load(&sandbox.inputs(&[("ONCEBITTEN_TRIAGE_DEFAULT", "  ")])).unwrap();

    assert_eq!(loaded.config.triage.default.agent, "default");
    assert_eq!(loaded.origins["triage.default"], Origin::Project);
}

#[test]
fn unknown_env_variable_loads_with_a_warning() {
    let sandbox = Sandbox::new();
    let loaded = load(&sandbox.inputs(&[("ONCEBITTEN_TRIAGE_DEFUALT", "pi:default")])).unwrap();

    assert_eq!(loaded.config.triage.default.agent, "haiku");
    assert_eq!(loaded.warnings.len(), 1, "{:?}", loaded.warnings);
    assert!(
        loaded.warnings[0].contains("ONCEBITTEN_TRIAGE_DEFUALT"),
        "{:?}",
        loaded.warnings
    );
}

#[test]
fn known_env_variables_produce_no_warning() {
    let sandbox = Sandbox::new();
    let loaded = load(&sandbox.inputs(&[("ONCEBITTEN_TRIAGE_DEFAULT", "pi:default")])).unwrap();

    assert!(loaded.warnings.is_empty(), "{:?}", loaded.warnings);
}

#[test]
fn unix_empty_home_is_an_error() {
    let message = error_chain(user_config_path_for(&vars(&[("HOME", "")]), Platform::Unix));

    assert!(message.contains("HOME"), "{message}");
}

#[test]
fn unix_relative_home_is_an_error() {
    let message = error_chain(user_config_path_for(
        &vars(&[("HOME", "home/b")]),
        Platform::Unix,
    ));

    assert!(message.contains("HOME"), "{message}");
}

#[test]
fn windows_empty_appdata_is_an_error() {
    let message = error_chain(user_config_path_for(
        &vars(&[("APPDATA", "")]),
        Platform::Windows,
    ));

    assert!(message.contains("APPDATA"), "{message}");
}

#[test]
fn windows_relative_appdata_is_an_error() {
    let message = error_chain(user_config_path_for(
        &vars(&[("APPDATA", r"AppData\Roaming")]),
        Platform::Windows,
    ));

    assert!(message.contains("APPDATA"), "{message}");
}

#[cfg(unix)]
#[test]
fn non_unicode_unrelated_variable_is_skipped() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let vars = collect_vars([
        (OsString::from("HOME"), OsString::from("/home/b")),
        (OsString::from("UNRELATED"), OsString::from_vec(vec![0xff])),
    ])
    .unwrap();

    assert_eq!(vars.get("HOME").map(String::as_str), Some("/home/b"));
    assert!(!vars.contains_key("UNRELATED"));
}

#[cfg(unix)]
#[test]
fn non_unicode_relevant_variable_is_an_error() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    for name in ["HOME", "ONCEBITTEN_TRIAGE_DEFAULT"] {
        let message = error_chain(collect_vars([(
            OsString::from(name),
            OsString::from_vec(vec![0xff]),
        )]));

        assert!(message.contains(name), "{message}");
    }
}

#[test]
fn use_examples_lists_shipped_example_ids() {
    let sandbox = Sandbox::new();
    sandbox.write_project("[rules]\nuse_examples = [\"rust-no-unwrap\"]\n");
    let loaded = load(&sandbox.inputs(&[])).unwrap();

    assert_eq!(loaded.config.rules.use_examples, ["rust-no-unwrap"]);
}

#[test]
fn unknown_use_examples_id_is_rejected() {
    let sandbox = Sandbox::new();
    sandbox.write_project("[rules]\nuse_examples = [\"no-such-example\"]\n");
    let message = error_chain(load(&sandbox.inputs(&[])));

    assert!(message.contains("names no shipped example"), "{message}");
}

#[test]
fn capture_phrases_default_to_the_built_in_lexicon() {
    let sandbox = Sandbox::new();
    let loaded = load(&sandbox.inputs(&[])).unwrap();
    let phrases = &loaded.config.capture.correction_phrases;
    assert!(phrases.iter().any(|p| p == "no,"));
    assert!(phrases.iter().any(|p| p == "doesn't work"));
    assert!(loaded.config.capture.extra_correction_phrases.is_empty());
}

#[test]
fn user_capture_phrases_replace_the_default_list() {
    let sandbox = Sandbox::new();
    sandbox.write_user("[capture]\ncorrection_phrases = [\"tabs\"]\n");
    let loaded = load(&sandbox.inputs(&[])).unwrap();
    assert_eq!(loaded.config.capture.correction_phrases, ["tabs"]);
    assert_eq!(loaded.origins["capture.correction_phrases"], Origin::User);
}

#[test]
fn extra_capture_phrases_extend_the_default_list() {
    let sandbox = Sandbox::new();
    sandbox.write_project("[capture]\nextra_correction_phrases = [\"tabs\"]\n");
    let loaded = load(&sandbox.inputs(&[])).unwrap();
    assert_eq!(loaded.config.capture.extra_correction_phrases, ["tabs"]);
    assert!(loaded.config.capture.correction_phrases.len() > 1);
}

#[test]
fn empty_or_blank_capture_phrases_are_rejected() {
    let sandbox = Sandbox::new();
    sandbox.write_user("[capture]\ncorrection_phrases = []\n");
    assert!(load(&sandbox.inputs(&[])).is_err());

    let sandbox = Sandbox::new();
    sandbox.write_user("[capture]\nextra_correction_phrases = [\"  \"]\n");
    assert!(load(&sandbox.inputs(&[])).is_err());
}

#[test]
fn capture_test_commands_default_to_empty() {
    let sandbox = Sandbox::new();
    let loaded = load(&sandbox.inputs(&[])).unwrap();
    assert!(loaded.config.capture.test_commands.is_empty());
}

#[test]
fn project_capture_test_commands_load_with_origin() {
    let sandbox = Sandbox::new();
    sandbox.write_project("[capture]\ntest_commands = [\"make check\", \"just test\"]\n");
    let loaded = load(&sandbox.inputs(&[])).unwrap();
    assert_eq!(
        loaded.config.capture.test_commands,
        ["make check", "just test"]
    );
    assert_eq!(loaded.origins["capture.test_commands"], Origin::Project);
}

#[test]
fn blank_capture_test_commands_are_rejected() {
    let sandbox = Sandbox::new();
    sandbox.write_user("[capture]\ntest_commands = [\" \"]\n");
    assert!(load(&sandbox.inputs(&[])).is_err());
}
