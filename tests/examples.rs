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

fn rust_no_unwrap_outcome(source: &str) -> Outcome {
    let rule = load("rust-no-unwrap");
    let Check::Native { command } = &rule.check else {
        panic!("rust-no-unwrap is not a native rule");
    };
    let crate_dir = TempDir::new().unwrap();
    fs::write(
        crate_dir.path().join("Cargo.toml"),
        "[package]\nname = \"example\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::create_dir(crate_dir.path().join("src")).unwrap();
    fs::write(crate_dir.path().join("src/main.rs"), source).unwrap();
    native::run_matching(
        command,
        &rule.fired_exit_codes,
        rule.fired_output_contains.as_deref(),
        crate_dir.path(),
    )
}

#[test]
fn rust_no_unwrap_needs_the_lint_diagnostic_to_fire() {
    let bad = fs::read_to_string(fixture("rust-no-unwrap", "bad.rs")).unwrap();
    let fixed = fs::read_to_string(fixture("rust-no-unwrap", "fixed.rs")).unwrap();

    assert_fires(&rust_no_unwrap_outcome(&bad), "rust-no-unwrap bad");
    assert_clean(&rust_no_unwrap_outcome(&fixed), "rust-no-unwrap fixed");
}

#[test]
fn rust_no_unwrap_crate_that_fails_to_compile_could_not_run() {
    let broken = "fn main() {\n    let x: i32 = \"not a number\";\n}\n";

    assert!(
        matches!(rust_no_unwrap_outcome(broken), Outcome::CouldNotRun(_)),
        "{:?}",
        rust_no_unwrap_outcome(broken)
    );
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

fn git_repo_with_project_config(config: Option<&str>) -> TempDir {
    let repo = TempDir::new().unwrap();
    let status = std::process::Command::new("git")
        .arg("init")
        .arg("-q")
        .current_dir(repo.path())
        .status()
        .unwrap();
    assert!(status.success());
    if let Some(config) = config {
        let dir = repo.path().join(".oncebitten");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("config.toml"), config).unwrap();
    }
    repo
}

fn inputs_for(repo: &TempDir) -> oncebitten::config::Inputs {
    let home = repo.path().join("home").display().to_string();
    let vars = [
        ("HOME".to_owned(), home.clone()),
        ("XDG_CONFIG_HOME".to_owned(), home.clone()),
        ("XDG_DATA_HOME".to_owned(), home),
    ]
    .into_iter()
    .collect();
    oncebitten::config::Inputs {
        cwd: repo.path().to_path_buf(),
        vars,
    }
}

#[test]
fn unlisted_examples_are_not_loaded() {
    let repo = git_repo_with_project_config(None);

    let rules = oncebitten::rules::load(&inputs_for(&repo), false).unwrap();

    assert!(
        rules.is_empty(),
        "{:?}",
        rules.iter().map(|r| &r.id).collect::<Vec<_>>()
    );
}

#[test]
fn listed_examples_load_as_active_and_only_those() {
    let repo = git_repo_with_project_config(Some(
        "[rules]\nuse_examples = [\"py-utcnow\", \"rust-no-unwrap\"]\n",
    ));

    let rules = oncebitten::rules::load(&inputs_for(&repo), false).unwrap();
    let mut ids: Vec<&str> = rules.iter().map(|rule| rule.id.as_str()).collect();
    ids.sort_unstable();

    assert_eq!(ids, ["examples/py-utcnow", "examples/rust-no-unwrap"]);
    assert!(rules
        .iter()
        .all(|rule| rule.rule.status == oncebitten::rule::Status::Active));
}
