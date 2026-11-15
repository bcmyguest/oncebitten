mod common;

use oncebitten::doctor::gate_hook;
use oncebitten::host::{Host, ALL_HOSTS};
use oncebitten::signal::{
    REPEAT_FAILURE, REVERT, TEST_FAIL_THEN_PASS, TOOL_FAILURE, USER_CORRECTION,
};

const DOGFOOD: &str = "repos:\n  - repo: local\n    hooks:\n      - id: oncebitten-gate-dogfood\n        name: oncebitten gate (from source)\n        entry: cargo run -q --bin oncebitten -- gate pre-push\n        language: system\n        stages: [pre-push]\n";

#[test]
fn gate_hook_finds_a_local_hook_by_its_entry() {
    assert_eq!(
        gate_hook(DOGFOOD).as_deref(),
        Some("oncebitten-gate-dogfood")
    );
}

#[test]
fn gate_hook_accepts_plain_and_quoted_entries() {
    for entry in [
        "oncebitten gate pre-push",
        "\"oncebitten gate pre-push\"",
        "'uvx oncebitten gate pre-push'",
        r"C:\tools\oncebitten.exe gate pre-push",
        "/usr/local/bin/oncebitten gate",
    ] {
        let text = format!("    hooks:\n      - id: mine\n        entry: {entry}\n");
        assert_eq!(gate_hook(&text).as_deref(), Some("mine"), "{entry}");
    }
}

#[test]
fn gate_hook_still_finds_published_ids() {
    assert_eq!(
        gate_hook("      - id: oncebitten-gate-python\n").as_deref(),
        Some("oncebitten-gate-python")
    );
}

#[test]
fn gate_hook_ignores_other_entries_and_comments() {
    for text in [
        "      - id: x\n        entry: oncebitten check\n",
        "      - id: x\n        entry: cargo run -- gate pre-push\n",
        "      - id: x\n        entry: echo oncebitten gate\n",
        "      - id: x\n        # entry: oncebitten gate pre-push\n",
        "      - id: x\n        name: oncebitten gate\n",
    ] {
        assert_eq!(gate_hook(text), None, "{text}");
    }
}

#[test]
fn signals_follow_the_host_capabilities() {
    let full = [
        USER_CORRECTION,
        TOOL_FAILURE,
        REPEAT_FAILURE,
        TEST_FAIL_THEN_PASS,
        REVERT,
    ];
    for host in ALL_HOSTS {
        let signals = host.capabilities().signals();
        match host {
            Host::Codex | Host::OpenCode => assert_eq!(signals, [USER_CORRECTION], "{host:?}"),
            _ => assert_eq!(signals, full, "{host:?}"),
        }
    }
}

#[cfg(unix)]
mod cli {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    use super::common::{both, stdout, Repo};
    use super::DOGFOOD;

    fn doctor(repo: &Repo, hosts: &[&str]) -> String {
        let bin = repo.root.path().join("bin");
        fs::create_dir_all(&bin).unwrap();
        for host in hosts {
            let path = bin.join(host);
            fs::write(&path, "#!/bin/sh\nexit 0\n").unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let path = bin.display().to_string();
        let output = repo.run(&["doctor"], &[("PATH", &path)]);
        format!("{}{}", stdout(&output), both(&output))
    }

    #[test]
    fn a_local_gate_hook_satisfies_the_pre_push_check() {
        let repo = Repo::new();
        repo.write(".pre-commit-config.yaml", DOGFOOD);
        let out = doctor(&repo, &[]);
        assert!(
            out.contains("ok    pre-push hook: oncebitten-gate-dogfood"),
            "{out}"
        );
    }

    #[test]
    fn installed_hosts_list_the_signals_they_can_produce() {
        let repo = Repo::new();
        let out = doctor(&repo, &["codex", "pi"]);
        assert!(
            out.contains(
                "ok    signals codex: produces user-correction; \
                 cannot produce tool-failure, repeat-failure, test-fail-then-pass, revert"
            ),
            "{out}"
        );
        assert!(
            out.contains(
                "ok    signals pi: produces user-correction, tool-failure, repeat-failure, \
                 test-fail-then-pass, revert"
            ),
            "{out}"
        );
        assert!(!out.contains("signals claude"), "{out}");
    }
}
