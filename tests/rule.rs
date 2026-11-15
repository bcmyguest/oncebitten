use std::fs;
use std::path::{Path, PathBuf};

use oncebitten::rule::{load_rule, Check, Mode, Status};
use tempfile::TempDir;

const AST_GREP_RULE: &str = "id = \"no-unwrap\"\n\
description = \"Do not call unwrap.\"\n\
mode = \"block\"\n\
status = \"proposed\"\n\
engine = \"ast-grep\"\n";

fn rule_dir(id: &str, toml: &str) -> (TempDir, PathBuf) {
    let root = TempDir::new().unwrap();
    let dir = root.path().join(id);
    fs::create_dir_all(dir.join("fixtures")).unwrap();
    fs::write(dir.join("rule.toml"), toml).unwrap();
    fs::write(dir.join("check.yml"), "id: no-unwrap\n").unwrap();
    fs::write(dir.join("fixtures/bad.rs"), "x.unwrap()\n").unwrap();
    fs::write(dir.join("fixtures/fixed.rs"), "x?\n").unwrap();
    (root, dir)
}

fn error_of(dir: &Path) -> String {
    format!("{:#}", load_rule(dir).unwrap_err())
}

#[test]
fn ast_grep_rule_loads_with_its_fields() {
    let (_root, dir) = rule_dir("no-unwrap", AST_GREP_RULE);
    let rule = load_rule(&dir).unwrap();

    assert_eq!(rule.id, "no-unwrap");
    assert_eq!(rule.description, "Do not call unwrap.");
    assert_eq!(rule.mode, Mode::Block);
    assert_eq!(rule.status, Status::Proposed);
    assert_eq!(rule.language, None);
    assert_eq!(rule.overrides, None);
    match rule.check {
        Check::AstGrep { file } => assert!(file.ends_with("check.yml")),
        other => panic!("expected ast-grep check, got {other:?}"),
    }
}

#[test]
fn native_rule_loads_with_its_command_and_language() {
    let toml = "id = \"no-unwrap\"\ndescription = \"Do not call unwrap.\"\nmode = \"steer\"\n\
status = \"active\"\nengine = \"native\"\nlanguage = \"rust\"\ncommand = [\"cargo\", \"clippy\"]\n";
    let (_root, dir) = rule_dir("no-unwrap", toml);
    let rule = load_rule(&dir).unwrap();

    assert_eq!(rule.mode, Mode::Steer);
    assert_eq!(rule.status, Status::Active);
    assert_eq!(rule.language.as_deref(), Some("rust"));
    assert_eq!(
        rule.check,
        Check::Native {
            command: vec!["cargo".to_owned(), "clippy".to_owned()]
        }
    );
}

#[test]
fn steer_and_retired_values_load() {
    let toml = AST_GREP_RULE
        .replace("\"block\"", "\"steer\"")
        .replace("\"proposed\"", "\"retired\"");
    let (_root, dir) = rule_dir("no-unwrap", &toml);
    let rule = load_rule(&dir).unwrap();

    assert_eq!(rule.mode, Mode::Steer);
    assert_eq!(rule.status, Status::Retired);
}

#[test]
fn id_must_equal_the_directory_name() {
    let (_root, dir) = rule_dir("other-name", AST_GREP_RULE);
    let message = error_of(&dir);

    assert!(message.contains("rule.toml"), "{message}");
    assert!(message.contains("directory name"), "{message}");
}

#[test]
fn id_must_be_kebab_case() {
    let toml = AST_GREP_RULE.replace("no-unwrap", "No_Unwrap");
    let (_root, dir) = rule_dir("No_Unwrap", &toml);
    let message = error_of(&dir);

    assert!(message.contains("kebab-case"), "{message}");
}

#[test]
fn unknown_key_is_rejected() {
    let toml = format!("{AST_GREP_RULE}severity = \"high\"\n");
    let (_root, dir) = rule_dir("no-unwrap", &toml);
    let message = error_of(&dir);

    assert!(message.contains("unknown field `severity`"), "{message}");
    assert!(message.contains("rule.toml"), "{message}");
}

#[test]
fn description_is_required() {
    let toml = AST_GREP_RULE.replace("description = \"Do not call unwrap.\"\n", "");
    let (_root, dir) = rule_dir("no-unwrap", &toml);
    let message = error_of(&dir);

    assert!(message.contains("description"), "{message}");
    assert!(message.contains("rule.toml"), "{message}");
}

#[test]
fn unknown_mode_and_status_are_rejected() {
    for (toml, bad) in [
        (AST_GREP_RULE.replace("\"block\"", "\"warn\""), "warn"),
        (
            AST_GREP_RULE.replace("\"proposed\"", "\"pending\""),
            "pending",
        ),
    ] {
        let (_root, dir) = rule_dir("no-unwrap", &toml);
        let message = error_of(&dir);

        assert!(message.contains(bad), "{message}");
        assert!(message.contains("rule.toml"), "{message}");
    }
}

#[test]
fn ast_grep_rule_without_check_file_names_it() {
    let (_root, dir) = rule_dir("no-unwrap", AST_GREP_RULE);
    fs::remove_file(dir.join("check.yml")).unwrap();
    let message = error_of(&dir);

    assert!(message.contains("check.yml"), "{message}");
}

#[test]
fn native_rule_needs_a_command() {
    let toml = AST_GREP_RULE.replace("engine = \"ast-grep\"", "engine = \"native\"");
    let (_root, dir) = rule_dir("no-unwrap", &toml);
    let message = error_of(&dir);

    assert!(message.contains("non-empty command"), "{message}");
    assert!(message.contains("rule.toml"), "{message}");
}

#[test]
fn ast_grep_rule_rejects_a_command() {
    let toml = format!("{AST_GREP_RULE}command = [\"cargo\"]\n");
    let (_root, dir) = rule_dir("no-unwrap", &toml);
    let message = error_of(&dir);

    assert!(
        message.contains("only for engine = \"native\""),
        "{message}"
    );
}

#[test]
fn override_with_reason_loads() {
    let toml = format!(
        "{AST_GREP_RULE}overrides = \"team/no-unwrap\"\nreason = \"Team allows unwrap in tests.\"\n"
    );
    let (_root, dir) = rule_dir("no-unwrap", &toml);
    let rule = load_rule(&dir).unwrap();

    let overrides = rule.overrides.unwrap();
    assert_eq!(overrides.target, "team/no-unwrap");
    assert_eq!(overrides.reason, "Team allows unwrap in tests.");
}

#[test]
fn override_without_reason_is_rejected() {
    let toml = format!("{AST_GREP_RULE}overrides = \"team/no-unwrap\"\n");
    let (_root, dir) = rule_dir("no-unwrap", &toml);
    let message = error_of(&dir);

    assert!(message.contains("needs a reason"), "{message}");
    assert!(message.contains("rule.toml"), "{message}");
}

#[test]
fn reason_without_override_is_rejected() {
    let toml = format!("{AST_GREP_RULE}reason = \"No target.\"\n");
    let (_root, dir) = rule_dir("no-unwrap", &toml);
    let message = error_of(&dir);

    assert!(message.contains("reason needs overrides"), "{message}");
}

#[test]
fn override_target_must_be_qualified() {
    let toml = format!("{AST_GREP_RULE}overrides = \"no-unwrap\"\nreason = \"Why.\"\n");
    let (_root, dir) = rule_dir("no-unwrap", &toml);
    let message = error_of(&dir);

    assert!(message.contains("<source>/<id>"), "{message}");
}

#[test]
fn fixtures_need_exactly_one_bad_and_one_fixed() {
    let (_root, dir) = rule_dir("no-unwrap", AST_GREP_RULE);
    fs::remove_file(dir.join("fixtures/fixed.rs")).unwrap();
    let message = error_of(&dir);
    assert!(message.contains("fixtures"), "{message}");

    fs::write(dir.join("fixtures/fixed.rs"), "x?\n").unwrap();
    fs::write(dir.join("fixtures/bad.txt"), "x.unwrap()\n").unwrap();
    let message = error_of(&dir);
    assert!(message.contains("exactly one bad.*"), "{message}");
}

fn diff_rule_dir(toml: &str) -> (TempDir, PathBuf) {
    let root = TempDir::new().unwrap();
    let dir = root.path().join("keep-changelog");
    fs::create_dir_all(dir.join("fixtures")).unwrap();
    fs::write(dir.join("rule.toml"), toml).unwrap();
    fs::write(dir.join("fixtures/bad.diff"), "--- a/src/lib.rs\n").unwrap();
    fs::write(dir.join("fixtures/fixed.diff"), "--- a/CHANGELOG.md\n").unwrap();
    (root, dir)
}

const DIFF_RULE: &str = "id = \"keep-changelog\"\ndescription = \"Touch the changelog.\"\n\
mode = \"block\"\nstatus = \"active\"\nengine = \"diff\"\n\
when_changed = [\"src/**/*.rs\"]\nrequire_changed = [\"CHANGELOG.md\"]\n";

#[test]
fn diff_rule_loads_with_its_globs_and_no_check_file() {
    let (_root, dir) = diff_rule_dir(DIFF_RULE);
    let rule = load_rule(&dir).unwrap();

    assert_eq!(
        rule.check,
        Check::Diff {
            when_changed: vec!["src/**/*.rs".to_owned()],
            require_changed: vec!["CHANGELOG.md".to_owned()],
        }
    );
}

#[test]
fn diff_rule_without_require_changed_is_rejected() {
    let toml = DIFF_RULE.replace("require_changed = [\"CHANGELOG.md\"]\n", "");
    let (_root, dir) = diff_rule_dir(&toml);
    let message = error_of(&dir);

    assert!(message.contains("require_changed"), "{message}");
    assert!(message.contains("rule.toml"), "{message}");
}

#[test]
fn when_changed_on_an_ast_grep_rule_is_rejected() {
    let toml = format!("{AST_GREP_RULE}when_changed = [\"src/**\"]\n");
    let (_root, dir) = rule_dir("no-unwrap", &toml);
    let message = error_of(&dir);

    assert!(message.contains("only for engine = \"diff\""), "{message}");
}

#[test]
fn checklist_mode_is_rejected() {
    let (_root, dir) = rule_dir(
        "no-unwrap",
        &AST_GREP_RULE.replace("\"block\"", "\"checklist\""),
    );
    let message = error_of(&dir);

    assert!(message.contains("checklist"), "{message}");
}

#[test]
fn native_rule_loads_its_fired_exit_codes() {
    let toml = "id = \"no-unwrap\"\ndescription = \"Do not call unwrap.\"\nmode = \"block\"\n\
status = \"active\"\nengine = \"native\"\ncommand = [\"cargo\", \"clippy\"]\nfired_exit_codes = [101]\n";
    let (_root, dir) = rule_dir("no-unwrap", toml);
    let rule = load_rule(&dir).unwrap();

    assert_eq!(rule.fired_exit_codes, vec![101]);
}

#[test]
fn fired_exit_codes_without_native_engine_is_an_error() {
    let toml = format!("{AST_GREP_RULE}fired_exit_codes = [1]\n");
    let (_root, dir) = rule_dir("no-unwrap", &toml);
    let message = error_of(&dir);

    assert!(
        message.contains("fired_exit_codes is only for engine = \"native\""),
        "{message}"
    );
}

#[test]
fn fired_exit_codes_containing_zero_is_an_error() {
    let toml = "id = \"no-unwrap\"\ndescription = \"Do not call unwrap.\"\nmode = \"block\"\n\
status = \"active\"\nengine = \"native\"\ncommand = [\"cargo\", \"clippy\"]\nfired_exit_codes = [0]\n";
    let (_root, dir) = rule_dir("no-unwrap", toml);
    let message = error_of(&dir);

    assert!(message.contains("must not contain 0"), "{message}");
}

#[test]
fn native_rule_loads_its_fired_output_contains() {
    let toml = "id = \"no-unwrap\"\ndescription = \"Do not call unwrap.\"\nmode = \"block\"\n\
status = \"active\"\nengine = \"native\"\ncommand = [\"cargo\", \"clippy\"]\nfired_exit_codes = [101]\n\
fired_output_contains = \"unwrap_used\"\n";
    let (_root, dir) = rule_dir("no-unwrap", toml);
    let rule = load_rule(&dir).unwrap();

    assert_eq!(rule.fired_output_contains.as_deref(), Some("unwrap_used"));
}

#[test]
fn fired_output_contains_without_native_engine_is_an_error() {
    let toml = format!("{AST_GREP_RULE}fired_output_contains = \"x\"\n");
    let (_root, dir) = rule_dir("no-unwrap", &toml);
    let message = error_of(&dir);

    assert!(
        message.contains("fired_output_contains is only for engine = \"native\""),
        "{message}"
    );
}

#[test]
fn fired_output_contains_without_fired_exit_codes_is_an_error() {
    let toml = "id = \"no-unwrap\"\ndescription = \"Do not call unwrap.\"\nmode = \"block\"\n\
status = \"active\"\nengine = \"native\"\ncommand = [\"cargo\", \"clippy\"]\n\
fired_output_contains = \"unwrap_used\"\n";
    let (_root, dir) = rule_dir("no-unwrap", toml);
    let message = error_of(&dir);

    assert!(
        message.contains("fired_output_contains needs fired_exit_codes"),
        "{message}"
    );
}

#[test]
fn empty_fired_output_contains_is_an_error() {
    let toml = "id = \"no-unwrap\"\ndescription = \"Do not call unwrap.\"\nmode = \"block\"\n\
status = \"active\"\nengine = \"native\"\ncommand = [\"cargo\", \"clippy\"]\nfired_exit_codes = [101]\n\
fired_output_contains = \"\"\n";
    let (_root, dir) = rule_dir("no-unwrap", toml);
    let message = error_of(&dir);

    assert!(
        message.contains("fired_output_contains is empty"),
        "{message}"
    );
}
