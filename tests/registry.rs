use std::fs;
use std::path::{Path, PathBuf};

use oncebitten::registry::{resolve, Layer, Layers, Resolved};
use tempfile::TempDir;

fn write_rule(parent: &Path, id: &str, extra: &str) -> PathBuf {
    let dir = parent.join(id);
    fs::create_dir_all(dir.join("fixtures")).unwrap();
    let toml = format!(
        "id = \"{id}\"\ndescription = \"Rule {id}.\"\nmode = \"block\"\n\
         status = \"active\"\nengine = \"ast-grep\"\n{extra}"
    );
    fs::write(dir.join("rule.toml"), toml).unwrap();
    fs::write(dir.join("check.yml"), "id: x\n").unwrap();
    fs::write(dir.join("fixtures/bad.rs"), "x.unwrap()\n").unwrap();
    fs::write(dir.join("fixtures/fixed.rs"), "x?\n").unwrap();
    dir
}

fn ids(resolved: &[Resolved]) -> Vec<&str> {
    resolved.iter().map(|rule| rule.id.as_str()).collect()
}

fn errors_of(layers: &Layers) -> String {
    let errors = resolve(layers).unwrap_err();
    errors
        .iter()
        .map(|error| format!("{error:#}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn source(name: &str, dir: &Path) -> (String, PathBuf) {
    (name.to_owned(), dir.to_path_buf())
}

#[test]
fn resolves_project_source_and_example_rules_with_qualified_ids() {
    let root = TempDir::new().unwrap();
    let project = root.path().join("project");
    let team = root.path().join("team");
    let examples = root.path().join("examples");
    write_rule(&project, "local-check", "");
    write_rule(&team, "team-check", "");
    write_rule(&examples, "example-check", "");
    let layers = Layers {
        project: Some(project),
        sources: vec![source("team", &team)],
        examples: Some(examples),
    };
    let resolved = resolve(&layers).unwrap();

    assert_eq!(
        ids(&resolved),
        ["local-check", "team/team-check", "examples/example-check"]
    );
    assert_eq!(resolved[0].layer, Layer::Project);
    assert_eq!(resolved[1].layer, Layer::Source("team".to_owned()));
    assert_eq!(resolved[2].layer, Layer::Example);
}

#[test]
fn source_rules_follow_config_order_then_examples() {
    let root = TempDir::new().unwrap();
    let zeta = root.path().join("zeta");
    let alpha = root.path().join("alpha");
    write_rule(&zeta, "one", "");
    write_rule(&alpha, "two", "");
    let layers = Layers {
        project: None,
        sources: vec![source("zeta", &zeta), source("alpha", &alpha)],
        examples: None,
    };

    assert_eq!(ids(&resolve(&layers).unwrap()), ["zeta/one", "alpha/two"]);
}

#[test]
fn override_replaces_the_source_rule() {
    let root = TempDir::new().unwrap();
    let project = root.path().join("project");
    let team = root.path().join("team");
    write_rule(
        &project,
        "no-unwrap",
        "overrides = \"team/no-unwrap\"\nreason = \"Tests may unwrap.\"\n",
    );
    write_rule(&team, "no-unwrap", "");
    write_rule(&team, "other", "");
    let layers = Layers {
        project: Some(project),
        sources: vec![source("team", &team)],
        examples: None,
    };
    let resolved = resolve(&layers).unwrap();

    assert_eq!(ids(&resolved), ["no-unwrap", "team/other"]);
    assert_eq!(resolved[0].layer, Layer::Project);
}

#[test]
fn project_rule_shadows_an_example_with_the_same_id() {
    let root = TempDir::new().unwrap();
    let project = root.path().join("project");
    let examples = root.path().join("examples");
    write_rule(&project, "no-unwrap", "");
    write_rule(&examples, "no-unwrap", "");
    let layers = Layers {
        project: Some(project),
        sources: Vec::new(),
        examples: Some(examples),
    };

    assert_eq!(ids(&resolve(&layers).unwrap()), ["no-unwrap"]);
}

#[test]
fn source_rule_shadows_an_example_with_the_same_id() {
    let root = TempDir::new().unwrap();
    let team = root.path().join("team");
    let examples = root.path().join("examples");
    write_rule(&team, "no-unwrap", "");
    write_rule(&examples, "no-unwrap", "");
    let layers = Layers {
        project: None,
        sources: vec![source("team", &team)],
        examples: Some(examples),
    };

    assert_eq!(ids(&resolve(&layers).unwrap()), ["team/no-unwrap"]);
}

#[test]
fn override_of_a_missing_id_is_an_error() {
    let root = TempDir::new().unwrap();
    let project = root.path().join("project");
    let team = root.path().join("team");
    write_rule(
        &project,
        "no-unwrap",
        "overrides = \"team/missing\"\nreason = \"Why.\"\n",
    );
    write_rule(&team, "other", "");
    let layers = Layers {
        project: Some(project),
        sources: vec![source("team", &team)],
        examples: None,
    };
    let message = errors_of(&layers);

    assert!(message.contains("team/missing"), "{message}");
    assert!(message.contains("rule.toml"), "{message}");
}

#[test]
fn override_without_a_reason_is_an_error() {
    let root = TempDir::new().unwrap();
    let project = root.path().join("project");
    let team = root.path().join("team");
    write_rule(&project, "no-unwrap", "overrides = \"team/no-unwrap\"\n");
    write_rule(&team, "no-unwrap", "");
    let layers = Layers {
        project: Some(project),
        sources: vec![source("team", &team)],
        examples: None,
    };
    let message = errors_of(&layers);

    assert!(message.contains("needs a reason"), "{message}");
}

#[test]
fn project_rule_clashing_with_a_source_id_needs_an_override() {
    let root = TempDir::new().unwrap();
    let project = root.path().join("project");
    let team = root.path().join("team");
    write_rule(&project, "no-unwrap", "");
    write_rule(&team, "no-unwrap", "");
    let layers = Layers {
        project: Some(project),
        sources: vec![source("team", &team)],
        examples: None,
    };
    let message = errors_of(&layers);

    assert!(message.contains("team/no-unwrap"), "{message}");
    assert!(message.contains("overrides"), "{message}");
}

#[test]
fn two_sources_defining_the_same_qualified_id_is_an_error() {
    let root = TempDir::new().unwrap();
    let first = root.path().join("first");
    let second = root.path().join("second");
    write_rule(&first, "no-unwrap", "");
    write_rule(&second, "no-unwrap", "");
    let layers = Layers {
        project: None,
        sources: vec![source("team", &first), source("team", &second)],
        examples: None,
    };
    let message = errors_of(&layers);

    assert!(message.contains("team/no-unwrap"), "{message}");
    assert!(message.contains("defined twice"), "{message}");
}

#[test]
fn every_invalid_rule_is_reported_with_its_file() {
    let root = TempDir::new().unwrap();
    let project = root.path().join("project");
    let broken = write_rule(&project, "broken-one", "");
    let other = write_rule(&project, "broken-two", "");
    fs::write(broken.join("rule.toml"), "id = \"broken-one\"\n").unwrap();
    fs::write(other.join("rule.toml"), "id = \"wrong\"\n").unwrap();
    let layers = Layers {
        project: Some(project),
        sources: Vec::new(),
        examples: None,
    };
    let errors = resolve(&layers).unwrap_err();

    assert_eq!(errors.len(), 2);
    let text: Vec<String> = errors.iter().map(|error| format!("{error:#}")).collect();
    assert!(text.iter().any(|t| t.contains("broken-one")), "{text:?}");
    assert!(text.iter().any(|t| t.contains("broken-two")), "{text:?}");
}
