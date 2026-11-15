use std::fs;
use std::path::{Path, PathBuf};

use oncebitten::engine::{astgrep, diff, native, Outcome};
use oncebitten::rule::{load_rule, Check, Rule, Status};
use tempfile::TempDir;

fn example(id: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples")
        .join(id)
}

fn fixture(id: &str, name: &str) -> PathBuf {
    example(id).join("fixtures").join(name)
}

fn load(id: &str) -> Rule {
    load_rule(&example(id)).unwrap_or_else(|error| panic!("{id}: {error:#}"))
}

fn changed_paths(path: &Path) -> Vec<String> {
    fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect()
}

fn run_ast_grep(id: &str, name: &str) -> Outcome {
    let rule = load(id);
    let Check::AstGrep { file } = &rule.check else {
        panic!("{id} is not an ast-grep rule");
    };
    astgrep::run(file, &[fixture(id, name)])
}

fn run_diff(id: &str, name: &str) -> Outcome {
    let rule = load(id);
    let Check::Diff {
        when_changed,
        require_changed,
    } = &rule.check
    else {
        panic!("{id} is not a diff rule");
    };
    diff::run(
        when_changed,
        require_changed,
        &changed_paths(&fixture(id, name)),
    )
}

fn assert_fires(outcome: &Outcome, id: &str) {
    assert!(matches!(outcome, Outcome::Fired(_)), "{id}: {outcome:?}");
}

fn assert_clean(outcome: &Outcome, id: &str) {
    assert_eq!(outcome, &Outcome::Clean, "{id}");
}

#[test]
fn every_example_is_proposed_so_check_skips_it() {
    for id in [
        "py-utcnow",
        "py-no-print",
        "js-moment",
        "ts-no-as-any",
        "rust-no-unwrap",
        "arch-diagram",
        "ui-screenshot",
    ] {
        assert_eq!(load(id).status, Status::Proposed, "{id}");
    }
}

#[test]
fn py_utcnow_fires_on_bad_and_not_on_fixed() {
    let bad = run_ast_grep("py-utcnow", "bad.py");
    assert_fires(&bad, "py-utcnow bad");
    assert_clean(&run_ast_grep("py-utcnow", "fixed.py"), "py-utcnow fixed");
}

#[test]
fn py_no_print_fires_on_bad_and_not_on_fixed() {
    let bad = run_ast_grep("py-no-print", "bad.py");
    assert_fires(&bad, "py-no-print bad");
    assert_clean(
        &run_ast_grep("py-no-print", "fixed.py"),
        "py-no-print fixed",
    );
}

#[test]
fn js_moment_fires_on_bad_and_not_on_fixed() {
    let bad = run_ast_grep("js-moment", "bad.js");
    assert_fires(&bad, "js-moment bad");
    assert_clean(&run_ast_grep("js-moment", "fixed.js"), "js-moment fixed");
}

#[test]
fn ts_no_as_any_fires_on_bad_and_not_on_fixed() {
    let bad = run_ast_grep("ts-no-as-any", "bad.ts");
    assert_fires(&bad, "ts-no-as-any bad");
    assert_clean(
        &run_ast_grep("ts-no-as-any", "fixed.ts"),
        "ts-no-as-any fixed",
    );
}

#[test]
fn rust_no_unwrap_fires_through_clippy_on_bad_and_not_on_fixed() {
    let rule = load("rust-no-unwrap");
    let Check::Native { command } = &rule.check else {
        panic!("rust-no-unwrap is not a native rule");
    };
    let fired = |name: &str| {
        let crate_dir = TempDir::new().unwrap();
        fs::write(
            crate_dir.path().join("Cargo.toml"),
            "[package]\nname = \"example\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .unwrap();
        fs::create_dir(crate_dir.path().join("src")).unwrap();
        fs::copy(
            fixture("rust-no-unwrap", name),
            crate_dir.path().join("src/main.rs"),
        )
        .unwrap();
        native::run(command, &rule.fired_exit_codes, crate_dir.path())
    };

    assert_fires(&fired("bad.rs"), "rust-no-unwrap bad");
    assert_clean(&fired("fixed.rs"), "rust-no-unwrap fixed");
}

#[test]
fn arch_diagram_fires_on_bad_and_not_on_fixed() {
    assert_fires(&run_diff("arch-diagram", "bad.diff"), "arch-diagram bad");
    assert_clean(
        &run_diff("arch-diagram", "fixed.diff"),
        "arch-diagram fixed",
    );
}

#[test]
fn ui_screenshot_fires_on_bad_and_not_on_fixed() {
    assert_fires(&run_diff("ui-screenshot", "bad.diff"), "ui-screenshot bad");
    assert_clean(
        &run_diff("ui-screenshot", "fixed.diff"),
        "ui-screenshot fixed",
    );
}

#[test]
fn load_for_check_includes_the_embedded_examples() {
    let repo = TempDir::new().unwrap();
    let status = std::process::Command::new("git")
        .arg("init")
        .arg("-q")
        .current_dir(repo.path())
        .status()
        .unwrap();
    assert!(status.success());

    let home = repo.path().join("home").display().to_string();
    let vars = [
        ("HOME".to_owned(), home.clone()),
        ("XDG_CONFIG_HOME".to_owned(), home.clone()),
        ("XDG_DATA_HOME".to_owned(), home),
    ]
    .into_iter()
    .collect();
    let inputs = oncebitten::config::Inputs {
        cwd: repo.path().to_path_buf(),
        vars,
    };
    let loaded = oncebitten::rules::load_for_check(&inputs).unwrap();
    let ids: Vec<&str> = loaded.rules.iter().map(|rule| rule.id.as_str()).collect();

    assert!(ids.contains(&"examples/py-utcnow"), "{ids:?}");
    assert!(ids.contains(&"examples/rust-no-unwrap"), "{ids:?}");
    assert_eq!(ids.len(), 7, "{ids:?}");
}
