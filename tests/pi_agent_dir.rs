use std::collections::BTreeMap;
use std::path::Path;

use oncebitten::hooks::{install_target, target_path, Scope};
use oncebitten::host::{Host, ALL_HOSTS};
use tempfile::TempDir;

fn vars(value: &str) -> BTreeMap<String, String> {
    BTreeMap::from([("PI_CODING_AGENT_DIR".to_owned(), value.to_owned())])
}

#[test]
fn pi_user_install_follows_an_absolute_agent_dir() {
    let home = TempDir::new().unwrap();
    let agent = TempDir::new().unwrap();
    let path = install_target(
        Host::Pi,
        Scope::User,
        home.path(),
        Path::new("unused"),
        &vars(&agent.path().display().to_string()),
    )
    .unwrap();
    assert_eq!(path, agent.path().join("extensions").join("oncebitten.js"));
}

#[test]
fn pi_agent_dir_expands_a_leading_tilde_against_home() {
    let home = TempDir::new().unwrap();
    let at = |value: &str| {
        install_target(
            Host::Pi,
            Scope::User,
            home.path(),
            Path::new("unused"),
            &vars(value),
        )
        .unwrap()
    };
    assert_eq!(
        at("~/pi-agent"),
        home.path()
            .join("pi-agent")
            .join("extensions")
            .join("oncebitten.js")
    );
    assert_eq!(
        at("~"),
        home.path().join("extensions").join("oncebitten.js")
    );
}

#[test]
fn empty_or_unset_agent_dir_keeps_the_default_path() {
    let home = TempDir::new().unwrap();
    let project = TempDir::new().unwrap();
    let default = target_path(Host::Pi, Scope::User, home.path(), project.path());
    for vars in [BTreeMap::new(), vars("")] {
        let path =
            install_target(Host::Pi, Scope::User, home.path(), project.path(), &vars).unwrap();
        assert_eq!(path, default);
    }
}

#[test]
fn relative_agent_dir_is_an_error() {
    let home = TempDir::new().unwrap();
    let error = install_target(
        Host::Pi,
        Scope::User,
        home.path(),
        Path::new("unused"),
        &vars("pi-agent"),
    )
    .unwrap_err();
    assert!(error.to_string().contains("relative"), "{error}");
}

#[test]
fn agent_dir_changes_only_the_pi_user_install() {
    let home = TempDir::new().unwrap();
    let project = TempDir::new().unwrap();
    let agent = TempDir::new().unwrap();
    let vars = vars(&agent.path().display().to_string());
    for host in ALL_HOSTS {
        for scope in [Scope::User, Scope::Project] {
            if host == Host::Pi && scope == Scope::User {
                continue;
            }
            assert_eq!(
                install_target(host, scope, home.path(), project.path(), &vars).unwrap(),
                target_path(host, scope, home.path(), project.path()),
                "{host:?} {scope:?}"
            );
        }
    }
}
