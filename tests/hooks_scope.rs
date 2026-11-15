mod common;

use common::{both, code, stdout, Repo};

const HOSTS: [&str; 6] = ["claude", "codex", "cursor", "copilot", "opencode", "pi"];

fn installed_files(repo: &Repo, host: &str, scope: &str) -> String {
    let output = repo.run(&["hooks", "install", "--host", host, "--scope", scope], &[]);
    assert_eq!(code(&output), 0, "{}", both(&output));
    stdout(&output)
        .lines()
        .filter_map(|line| line.strip_prefix("changed  "))
        .map(|path| std::fs::read_to_string(path).unwrap())
        .collect()
}

#[test]
fn project_scope_writes_the_bare_command_for_every_host() {
    for host in HOSTS {
        let repo = Repo::new();

        let text = installed_files(&repo, host, "project");

        assert!(
            text.contains(&format!("oncebitten capture --host {host}")),
            "{host}: {text}"
        );
        assert!(
            text.contains(&format!("oncebitten steer --host {host}")),
            "{host}: {text}"
        );
        assert!(
            !text.contains(env!("CARGO_BIN_EXE_oncebitten")),
            "{host}: {text}"
        );
    }
}

#[test]
fn user_scope_keeps_the_absolute_path_for_every_host() {
    let exe = dunce::canonicalize(env!("CARGO_BIN_EXE_oncebitten")).unwrap();
    for host in HOSTS {
        let repo = Repo::new();

        let text = installed_files(&repo, host, "user");

        assert!(
            text.contains(&exe.display().to_string().replace('\\', "\\\\"))
                || text.contains(&exe.display().to_string()),
            "{host}: {text}"
        );
    }
}
