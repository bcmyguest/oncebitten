mod common;

use oncebitten::doctor::{gate_hook_id, parse_version};

#[test]
fn parse_version_reads_the_first_dotted_triple() {
    assert_eq!(parse_version("pre-commit 4.6.0\n"), Some((4, 6, 0)));
    assert_eq!(parse_version("pre-commit 3.2.0"), Some((3, 2, 0)));
    assert_eq!(parse_version("pre-commit 3.1"), None);
    assert_eq!(parse_version("pre-commit"), None);
}

#[test]
fn versions_compare_numerically() {
    assert!(parse_version("x 3.10.0").unwrap() >= oncebitten::doctor::MIN_PRE_COMMIT);
    assert!(parse_version("x 3.1.9").unwrap() < oncebitten::doctor::MIN_PRE_COMMIT);
}

#[test]
fn gate_hook_id_finds_either_hook() {
    let config = "repos:\n  - repo: https://github.com/bcmyguest/oncebitten\n    rev: v1\n    hooks:\n      - id: oncebitten-gate\n";
    assert_eq!(gate_hook_id(config), Some("oncebitten-gate"));
    assert_eq!(
        gate_hook_id("    hooks:\n      - id: \"oncebitten-gate-system\"  # pinned\n"),
        Some("oncebitten-gate-system")
    );
}

#[test]
fn gate_hook_id_ignores_other_hooks_and_comments() {
    assert_eq!(gate_hook_id("      - id: oncebitten-fmt\n"), None);
    assert_eq!(gate_hook_id("      # - id: oncebitten-gate\n"), None);
    assert_eq!(gate_hook_id(""), None);
}

#[cfg(unix)]
mod cli {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::process::Output;

    use oncebitten::proof::{self, BackendKind};
    use oncebitten::replay::{LocalRunner, Verdict};
    use oncebitten::rule::load_rule;

    use super::common::{both, code, stdout, Repo};

    const CHECKER: &str = env!("CARGO_BIN_EXE_oncebitten-checker");
    const GATE_HOOK: &str =
        "repos:\n  - repo: https://github.com/bcmyguest/oncebitten\n    rev: v1\n    hooks:\n      - id: oncebitten-gate\n";

    fn bin(repo: &Repo) -> PathBuf {
        repo.root.path().join("bin")
    }

    /// Puts an executable shell script named `name` on the doctor's PATH.
    fn fake(repo: &Repo, name: &str, body: &str) {
        let dir = bin(repo);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn doctor(repo: &Repo) -> Output {
        fs::create_dir_all(bin(repo)).unwrap();
        let path = bin(repo).display().to_string();
        repo.run(&["doctor"], &[("PATH", &path)])
    }

    fn line<'a>(output: &'a str, check: &str) -> &'a str {
        output
            .lines()
            .find(|line| line[6..].starts_with(&format!("{check}:")))
            .unwrap_or_else(|| panic!("no `{check}` line in:\n{output}"))
    }

    /// A repo every check passes on: pre-commit 4.6.0, the gate hook, a lock,
    /// a proven active rule, a reachable gateway and no agent hosts.
    fn healthy() -> Repo {
        let repo = Repo::new();
        fake(&repo, "pre-commit", "echo 'pre-commit 4.6.0'");
        fake(&repo, "openshell", "echo connected");
        repo.write(".pre-commit-config.yaml", GATE_HOOK);
        repo.add_example("py-utcnow", "active");
        let rule = load_rule(&repo.rule_dir("py-utcnow")).unwrap();
        let replayed = proof::replay_rule(
            &rule,
            &PathBuf::from(CHECKER),
            &LocalRunner,
            BackendKind::OpenShell,
        );
        assert_eq!(replayed.verdict, Verdict::Proven);
        let sync = repo.run(&["rules", "sync"], &[]);
        assert_eq!(code(&sync), 0, "{}", both(&sync));
        repo.commit_all("base");
        repo
    }

    fn home(repo: &Repo) -> PathBuf {
        repo.root.path().join("home")
    }

    fn files_under(dir: &Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    out.extend(files_under(&path));
                } else {
                    out.push(path);
                }
            }
        }
        out
    }

    #[test]
    fn a_healthy_repo_passes_every_check() {
        let repo = healthy();

        let output = doctor(&repo);

        assert_eq!(code(&output), 0, "{}", both(&output));
        let out = stdout(&output);
        assert!(out.lines().all(|l| l.starts_with("ok  ")), "{out}");
        assert_eq!(line(&out, "config"), "ok    config: loads");
        assert_eq!(line(&out, "pre-commit"), "ok    pre-commit: version 4.6.0");
        assert_eq!(
            line(&out, "pre-push hook"),
            "ok    pre-push hook: oncebitten-gate in .pre-commit-config.yaml"
        );
        assert_eq!(
            line(&out, "proof py-utcnow"),
            "ok    proof py-utcnow: valid"
        );
        assert_eq!(
            line(&out, "capture claude"),
            "ok    capture claude: not installed"
        );
        assert_eq!(
            line(&out, "openshell"),
            "ok    openshell: gateway reachable"
        );
    }

    #[test]
    fn doctor_writes_nothing() {
        let repo = healthy();
        let data = repo.root.path().join("data");

        let output = doctor(&repo);

        assert_eq!(code(&output), 0, "{}", both(&output));
        assert_eq!(repo.git(&["status", "--porcelain"]), "");
        assert!(files_under(&data)
            .iter()
            .all(|path| !path.ends_with("events.db")));
        assert!(line(&stdout(&output), "evidence store").contains("empty"));
    }

    #[test]
    fn an_old_pre_commit_fails() {
        let repo = healthy();
        fake(&repo, "pre-commit", "echo 'pre-commit 3.1.0'");

        let output = doctor(&repo);

        assert_eq!(code(&output), 1, "{}", both(&output));
        let out = stdout(&output);
        let pre_commit = line(&out, "pre-commit");
        assert!(
            pre_commit.starts_with("fail  pre-commit: version 3.1.0"),
            "{out}"
        );
        assert!(
            pre_commit.contains("fix: install pre-commit 3.2.0 or later"),
            "{out}"
        );
    }

    #[test]
    fn a_missing_pre_commit_fails() {
        let repo = healthy();
        fs::remove_file(bin(&repo).join("pre-commit")).unwrap();

        let output = doctor(&repo);

        assert_eq!(code(&output), 1, "{}", both(&output));
        assert!(line(&stdout(&output), "pre-commit").starts_with("fail"));
    }

    #[test]
    fn a_config_without_the_gate_hook_fails() {
        let repo = healthy();
        repo.write(".pre-commit-config.yaml", "repos: []\n");

        let output = doctor(&repo);

        assert_eq!(code(&output), 1, "{}", both(&output));
        let out = stdout(&output);
        assert!(line(&out, "pre-push hook").starts_with("fail"), "{out}");
        assert!(
            line(&out, "pre-push hook").contains("pre-commit install --hook-type pre-push"),
            "{out}"
        );
    }

    #[test]
    fn a_lock_pinning_an_unknown_source_fails() {
        let repo = healthy();
        repo.write(
            ".oncebitten/rules.lock",
            &format!(
                "[[source]]\nname = \"team\"\nurl = \"file:///nowhere\"\ncommit = \"{}\"\n",
                "a".repeat(40)
            ),
        );

        let output = doctor(&repo);

        assert_eq!(code(&output), 1, "{}", both(&output));
        let out = stdout(&output);
        assert!(
            line(&out, "rules.lock").contains("pins source `team`, which the config does not name"),
            "{out}"
        );
    }

    #[test]
    fn a_missing_lock_warns() {
        let repo = healthy();
        fs::remove_file(repo.project().join(".oncebitten/rules.lock")).unwrap();

        let output = doctor(&repo);

        assert_eq!(code(&output), 0, "{}", both(&output));
        assert!(line(&stdout(&output), "rules.lock").starts_with("warn"));
    }

    #[test]
    fn a_stale_proof_of_a_block_rule_fails() {
        let repo = healthy();
        let file = repo.rule_dir("py-utcnow").join("rule.toml");
        let text = fs::read_to_string(&file).unwrap();
        fs::write(
            &file,
            text.replace("description = \"", "description = \"Edited. "),
        )
        .unwrap();

        let output = doctor(&repo);

        assert_eq!(code(&output), 1, "{}", both(&output));
        let out = stdout(&output);
        let proof = line(&out, "proof py-utcnow");
        assert!(proof.starts_with("fail"), "{out}");
        assert!(proof.contains("stale"), "{out}");
        assert!(
            proof.contains("fix: run oncebitten replay py-utcnow"),
            "{out}"
        );
    }

    #[test]
    fn an_invalid_rule_fails() {
        let repo = healthy();
        fs::write(
            repo.rule_dir("py-utcnow").join("rule.toml"),
            "id = \"py-utcnow\"\n",
        )
        .unwrap();

        let output = doctor(&repo);

        assert_eq!(code(&output), 1, "{}", both(&output));
        assert!(line(&stdout(&output), "rules").starts_with("fail"));
    }

    #[test]
    fn an_expired_exception_warns() {
        let repo = healthy();
        repo.write(
            ".oncebitten/exceptions.toml",
            "[[exception]]\nrule = \"py-utcnow\"\npaths = [\"src/**\"]\nreason = \"legacy\"\nexpires = \"2000-01-01\"\n",
        );

        let output = doctor(&repo);

        assert_eq!(code(&output), 0, "{}", both(&output));
        let out = stdout(&output);
        let exceptions = line(&out, "exceptions");
        assert!(
            exceptions
                .starts_with("warn  exceptions: expired: py-utcnow on src/** until 2000-01-01"),
            "{out}"
        );
    }

    #[test]
    fn an_invalid_exceptions_file_fails() {
        let repo = healthy();
        repo.write(".oncebitten/exceptions.toml", "[[exception]]\nrule = 1\n");

        let output = doctor(&repo);

        assert_eq!(code(&output), 1, "{}", both(&output));
        assert!(line(&stdout(&output), "exceptions").starts_with("fail"));
    }

    #[test]
    fn a_present_host_without_the_hook_warns_until_installed() {
        let repo = healthy();
        fs::create_dir_all(home(&repo).join(".claude")).unwrap();

        let output = doctor(&repo);

        assert_eq!(code(&output), 0, "{}", both(&output));
        let out = stdout(&output);
        assert!(
            line(&out, "capture claude").starts_with(
                "warn  capture claude: host found, capture hook not installed \
                 (fix: oncebitten hooks install --host claude --scope user)"
            ),
            "{out}"
        );

        let install = repo.run(
            &["hooks", "install", "--host", "claude", "--scope", "user"],
            &[],
        );
        assert_eq!(code(&install), 0, "{}", both(&install));
        let output = doctor(&repo);
        assert!(
            line(&stdout(&output), "capture claude").starts_with("ok    capture claude: hook in"),
            "{}",
            both(&output)
        );
    }

    #[test]
    fn a_host_on_path_counts_as_present() {
        let repo = healthy();
        fake(&repo, "codex", "exit 0");

        let output = doctor(&repo);

        assert!(line(&stdout(&output), "capture codex").starts_with("warn"));
    }

    #[test]
    fn a_missing_openshell_warns_only() {
        let repo = healthy();
        fs::remove_file(bin(&repo).join("openshell")).unwrap();

        let output = doctor(&repo);

        assert_eq!(code(&output), 0, "{}", both(&output));
        assert!(line(&stdout(&output), "openshell").starts_with("warn  openshell: not on PATH"));
    }

    #[test]
    fn an_unreachable_gateway_warns_only() {
        let repo = healthy();
        fake(&repo, "openshell", "echo 'Connection refused' >&2; exit 1");

        let output = doctor(&repo);

        assert_eq!(code(&output), 0, "{}", both(&output));
        let out = stdout(&output);
        let openshell = line(&out, "openshell");
        assert!(
            openshell.starts_with("warn  openshell: gateway unreachable"),
            "{out}"
        );
        assert!(openshell.contains("Connection refused"), "{out}");
    }

    #[test]
    fn no_configured_gateway_warns_even_when_status_exits_zero() {
        let repo = healthy();
        fake(
            &repo,
            "openshell",
            "echo 'Gateway Status'; echo; echo '  Status: No gateway configured.'",
        );

        let output = doctor(&repo);

        assert_eq!(code(&output), 0, "{}", both(&output));
        let out = stdout(&output);
        let openshell = line(&out, "openshell");
        assert!(
            openshell.starts_with("warn  openshell: no gateway configured"),
            "{out}"
        );
    }

    #[test]
    fn a_corrupt_evidence_store_fails() {
        let repo = healthy();
        repo.write("../data/oncebitten/events.db", "not a database");

        let output = doctor(&repo);

        assert_eq!(code(&output), 1, "{}", both(&output));
        assert!(line(&stdout(&output), "evidence store").starts_with("fail"));
    }
}
