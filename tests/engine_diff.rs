use oncebitten::engine::{diff, Outcome};

fn globs(patterns: &[&str]) -> Vec<String> {
    patterns.iter().map(|p| (*p).to_owned()).collect()
}

fn changed(paths: &[&str]) -> Vec<String> {
    paths.iter().map(|p| (*p).to_owned()).collect()
}

#[test]
fn trigger_without_requirement_fires_naming_the_trigger() {
    let outcome = diff::run(
        &globs(&["src/api/**"]),
        &globs(&["docs/diagrams/*.svg"]),
        &changed(&["src/api/users.rs"]),
    );

    match outcome {
        Outcome::Fired(findings) => {
            assert_eq!(findings.len(), 1);
            assert_eq!(
                findings[0].path.as_deref(),
                Some(std::path::Path::new("src/api/users.rs"))
            );
            assert!(findings[0].message.contains("src/api/users.rs"));
            assert!(findings[0].message.contains("docs/diagrams/*.svg"));
        }
        other => panic!("expected Fired, got {other:?}"),
    }
}

#[test]
fn trigger_with_requirement_is_clean() {
    let outcome = diff::run(
        &globs(&["src/api/**"]),
        &globs(&["docs/diagrams/*.svg"]),
        &changed(&["src/api/users.rs", "docs/diagrams/api.svg"]),
    );

    assert_eq!(outcome, Outcome::Clean);
}

#[test]
fn requirement_without_trigger_is_clean() {
    let outcome = diff::run(
        &globs(&["src/api/**"]),
        &globs(&["docs/diagrams/*.svg"]),
        &changed(&["docs/diagrams/api.svg"]),
    );

    assert_eq!(outcome, Outcome::Clean);
}

#[test]
fn no_changes_is_clean() {
    let outcome = diff::run(&globs(&["src/**"]), &globs(&["docs/**"]), &[]);

    assert_eq!(outcome, Outcome::Clean);
}

#[test]
fn any_of_several_trigger_globs_fires() {
    let outcome = diff::run(
        &globs(&["ui/**/*.tsx", "ui/**/*.css"]),
        &globs(&["screenshots/*.png"]),
        &changed(&["ui/button/style.css"]),
    );

    assert!(matches!(outcome, Outcome::Fired(_)));
}

#[test]
fn single_star_does_not_cross_directories() {
    let outcome = diff::run(
        &globs(&["src/*.rs"]),
        &globs(&["docs/**"]),
        &changed(&["src/nested/deep.rs"]),
    );

    assert_eq!(outcome, Outcome::Clean);
}

#[test]
fn double_star_crosses_directories() {
    let outcome = diff::run(
        &globs(&["src/**/*.rs"]),
        &globs(&["docs/**"]),
        &changed(&["src/nested/deep.rs"]),
    );

    assert!(matches!(outcome, Outcome::Fired(_)));
}

#[test]
fn invalid_trigger_glob_could_not_run() {
    let outcome = diff::run(
        &globs(&["src/["]),
        &globs(&["docs/**"]),
        &changed(&["src/a.rs"]),
    );

    assert!(matches!(outcome, Outcome::CouldNotRun(_)));
}

#[test]
fn invalid_requirement_glob_could_not_run_even_without_a_trigger() {
    let outcome = diff::run(
        &globs(&["src/**"]),
        &globs(&["docs/["]),
        &changed(&["README.md"]),
    );

    assert!(matches!(outcome, Outcome::CouldNotRun(_)));
}
