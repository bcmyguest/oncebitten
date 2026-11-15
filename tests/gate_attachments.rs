mod common;

use std::collections::BTreeMap;
use std::path::Path;

use common::{both, code, FakeForge, Repo};
use oncebitten::config::Inputs;
use oncebitten::engine::Outcome;
use oncebitten::forge;
use oncebitten::gate::{self, Report, Stage};

/// A repo with an active `ui-screenshot` diff rule (src/ui/** needs
/// docs/screenshots/*.png) and a `feature` branch that changes src/ui.
fn ui_repo(config: &str) -> Repo {
    let repo = Repo::new();
    repo.add_example("ui-screenshot", "active");
    repo.write(".oncebitten/config.toml", config);
    repo.write("src/ui/view.rs", "a\n");
    let sync = repo.run(&["rules", "sync"], &[]);
    assert_eq!(code(&sync), 0, "{}", both(&sync));
    repo.commit_all("base");
    repo.git(&["checkout", "-q", "-b", "feature"]);
    repo.write("src/ui/view.rs", "b\n");
    repo.commit_all("change the ui");
    repo
}

fn inputs(repo: &Repo) -> Inputs {
    let root = repo.root.path();
    let vars: BTreeMap<String, String> = [
        ("HOME", root.join("home")),
        ("XDG_CONFIG_HOME", root.join("xdg-config")),
        ("XDG_DATA_HOME", root.join("data")),
        ("GIT_CONFIG_GLOBAL", root.join("empty.gitconfig")),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v.to_string_lossy().into_owned()))
    .collect();
    Inputs {
        cwd: repo.project(),
        vars,
    }
}

fn run(repo: &Repo, stage: Stage, base: Option<&str>, fake: &FakeForge) -> Report {
    let program = fake.program().to_path_buf();
    let forge_for =
        move |kind: forge::Kind, dir: &Path| forge::backend_with(kind, program.clone(), dir);
    gate::run_with_forge(&inputs(repo), stage, base, &forge_for)
        .unwrap_or_else(|_| panic!("gate failed"))
}

fn fired(report: &Report) -> bool {
    report
        .checked
        .iter()
        .any(|item| matches!(item.outcome, Outcome::Fired(_)))
}

const GITHUB: &str = "forge = \"github\"\n";

fn pr_fake(description: &str, comments: &[&str]) -> FakeForge {
    let fake = FakeForge::new("gh");
    fake.set(
        "reply.0",
        r#"[{"number":7,"url":"https://github.com/o/r/pull/7"}]"#,
    );
    let comments: Vec<_> = comments
        .iter()
        .map(|c| serde_json::json!({ "body": c }))
        .collect();
    fake.set(
        "reply.1",
        &serde_json::json!({ "body": description, "comments": comments }).to_string(),
    );
    fake
}

#[test]
fn a_screenshot_in_the_description_satisfies_the_rule() {
    let repo = ui_repo(GITHUB);
    let fake = pr_fake("![ui](https://example.test/shots/home.png)", &[]);

    let report = run(&repo, Stage::Pr, Some("main"), &fake);

    assert!(!fired(&report));
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
}

#[test]
fn a_github_attachment_in_a_comment_satisfies_the_rule() {
    let repo = ui_repo(GITHUB);
    let fake = pr_fake(
        "no picture here",
        &["![img](https://github.com/user-attachments/assets/3f2a1c9e-5b7d-4e8a-9c1f-0a2b3c4d5e6f)"],
    );

    let report = run(&repo, Stage::Pr, Some("main"), &fake);

    assert!(!fired(&report));
}

#[test]
fn a_pr_without_an_attachment_leaves_the_rule_fired_without_a_warning() {
    let repo = ui_repo(GITHUB);
    let fake = pr_fake("just words", &["a comment"]);

    let report = run(&repo, Stage::Pr, Some("main"), &fake);

    assert!(fired(&report));
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
}

#[test]
fn a_forge_failure_fires_the_rule_with_one_warning() {
    let repo = ui_repo(GITHUB);
    let fake = FakeForge::new("gh");
    fake.set("status", "1");
    fake.set("stderr", "gh: not logged in\n");

    let report = run(&repo, Stage::Pr, Some("main"), &fake);

    assert!(fired(&report));
    assert_eq!(report.warnings.len(), 1, "{:?}", report.warnings);
    assert!(
        report.warnings[0].contains("not logged in"),
        "{:?}",
        report.warnings
    );
}

#[test]
fn no_open_pr_fires_the_rule_with_one_warning() {
    let repo = ui_repo(GITHUB);
    let fake = FakeForge::new("gh");
    fake.set("reply", "[]");

    let report = run(&repo, Stage::Pr, Some("main"), &fake);

    assert!(fired(&report));
    assert_eq!(report.warnings.len(), 1, "{:?}", report.warnings);
    assert!(
        report.warnings[0].contains("no open pull request"),
        "{:?}",
        report.warnings
    );
}

#[test]
fn an_unknown_remote_without_config_fires_the_rule_with_one_warning() {
    let repo = ui_repo("");
    let fake = FakeForge::new("gh");

    let report = run(&repo, Stage::Pr, Some("main"), &fake);

    assert!(fired(&report));
    assert_eq!(report.warnings.len(), 1, "{:?}", report.warnings);
    assert!(fake.calls().is_empty());
}

#[test]
fn pre_push_never_calls_the_forge() {
    let repo = ui_repo(GITHUB);
    let fake = pr_fake("![ui](https://example.test/shots/home.png)", &[]);

    let report = run(&repo, Stage::PrePush, Some("main"), &fake);

    assert!(fired(&report));
    assert!(report.warnings.is_empty());
    assert!(fake.calls().is_empty());
}

#[test]
fn the_forge_is_not_called_when_no_diff_rule_fired() {
    let repo = ui_repo(GITHUB);
    repo.write("docs/screenshots/home.png", "png\n");
    repo.commit_all("add the screenshot");
    let fake = FakeForge::new("gh");

    let report = run(&repo, Stage::Pr, Some("main"), &fake);

    assert!(!fired(&report));
    assert!(fake.calls().is_empty());
}
