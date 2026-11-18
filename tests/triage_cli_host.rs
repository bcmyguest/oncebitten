mod common;

use std::env;
use std::fs;
use std::iter;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use common::{both, code, stdout, Repo};
use serde_json::Value;
use tempfile::TempDir;

const CORRECTION: &str = "No, stop.\n\"Don't\" run 100% of ^ & | the \"suite\".\nTry again.";
const HOOKS_OFF: &str = r#"{"disableAllHooks": true}"#;
const REPLY: &str = r#"{"type":"result","is_error":false,"structured_output":{"repeatable":true,"user_correction":true,"root_cause":"The agent ignored an explicit stop request.","proposed_fix_kind":"none","generality":"general"}}"#;
const BACKGROUND_OFF: &str = "[triage]\nbackground = false\n";
const BACKGROUND_TIMEOUT: Duration = Duration::from_secs(60);
const POLL_INTERVAL: Duration = Duration::from_millis(200);

struct FakeHost {
    root: TempDir,
}

struct HostEnv {
    path: String,
    calls: String,
}

impl HostEnv {
    fn pairs(&self) -> [(&str, &str); 2] {
        [
            ("PATH", self.path.as_str()),
            ("FAKE_HOST_DIR", self.calls.as_str()),
        ]
    }
}

impl FakeHost {
    fn new() -> Self {
        let host = FakeHost {
            root: TempDir::new().unwrap(),
        };
        fs::create_dir_all(host.bin()).unwrap();
        fs::create_dir_all(host.calls()).unwrap();
        install(&host.bin());
        host
    }

    fn bin(&self) -> PathBuf {
        self.root.path().join("bin")
    }

    fn calls(&self) -> PathBuf {
        self.root.path().join("calls")
    }

    fn set_reply(&self, reply: &str) {
        fs::write(self.calls().join("reply"), reply).unwrap();
    }

    fn env(&self) -> HostEnv {
        HostEnv {
            path: path_with(&self.bin()),
            calls: self.calls().display().to_string(),
        }
    }

    fn call_count(&self) -> usize {
        fs::read_dir(self.calls())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().starts_with("stdin."))
            .count()
    }

    fn stdin_of(&self, call: usize) -> String {
        fs::read_to_string(self.calls().join(format!("stdin.{call}"))).unwrap()
    }

    fn args_of(&self, call: usize) -> Vec<String> {
        let text = fs::read_to_string(self.calls().join(format!("args.{call}"))).unwrap();
        serde_json::from_str(&text).unwrap()
    }
}

fn host_exe() -> &'static Path {
    Path::new(env!("CARGO_BIN_EXE_fake-host"))
}

#[cfg(windows)]
fn install(bin: &Path) {
    let wrapper = format!("@echo off\r\n\"{}\" %*\r\n", host_exe().display());
    fs::write(bin.join("claude.cmd"), wrapper).unwrap();
}

#[cfg(not(windows))]
fn install(bin: &Path) {
    fs::copy(host_exe(), bin.join("claude")).unwrap();
}

fn path_with(dir: &Path) -> String {
    let existing = env::var_os("PATH").unwrap_or_default();
    env::join_paths(iter::once(dir.to_path_buf()).chain(env::split_paths(&existing)))
        .unwrap()
        .into_string()
        .expect("PATH is UTF-8")
}

fn write_user_config(repo: &Repo, text: &str) {
    let dir = repo.root.path().join("xdg-config").join("oncebitten");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("config.toml"), text).unwrap();
}

fn prompt_payload(session: &str, cwd: &Path, prompt: &str) -> String {
    serde_json::json!({
        "session_id": session,
        "cwd": cwd.display().to_string(),
        "hook_event_name": "UserPromptSubmit",
        "prompt": prompt,
    })
    .to_string()
}

fn capture_correction(repo: &Repo, host: &HostEnv) -> std::process::Output {
    let payload = prompt_payload("s1", &repo.project(), CORRECTION);
    repo.run_with_stdin(&["capture", "--host", "claude"], &host.pairs(), &payload)
}

fn cluster_id(repo: &Repo, host: &HostEnv) -> String {
    let out = repo.run(&["evidence", "list", "--all"], &host.pairs());
    assert_eq!(code(&out), 0, "{}", both(&out));
    stdout(&out)
        .lines()
        .find_map(|line| line.split_whitespace().next().map(str::to_owned))
        .expect("evidence list shows a cluster")
}

#[test]
fn triage_cli_asks_a_host_found_by_bare_name_on_path() {
    let repo = Repo::new();
    write_user_config(&repo, BACKGROUND_OFF);
    let host = FakeHost::new();
    host.set_reply(REPLY);
    let env = host.env();

    let captured = capture_correction(&repo, &env);
    assert_eq!(code(&captured), 0, "{}", both(&captured));
    let id = cluster_id(&repo, &env);

    let out = repo.run(&["triage", &id, "--host", "claude"], &env.pairs());
    assert_eq!(code(&out), 0, "{}", both(&out));
    let verdict: Value = serde_json::from_str(&stdout(&out)).expect("stdout is the verdict JSON");
    assert_eq!(verdict["proposed_fix_kind"], "none");

    assert_eq!(host.call_count(), 2);
    assert!(host.stdin_of(1).contains(CORRECTION));
    assert!(!host.args_of(0).iter().any(|arg| arg == HOOKS_OFF));
    assert!(host.args_of(1).iter().any(|arg| arg == HOOKS_OFF));
}

#[test]
fn capture_starts_background_triage_and_queues_the_answer() {
    let repo = Repo::new();
    let host = FakeHost::new();
    host.set_reply(REPLY);
    let env = host.env();

    let captured = capture_correction(&repo, &env);
    assert_eq!(code(&captured), 0, "{}", both(&captured));
    let id = cluster_id(&repo, &env);

    let deadline = Instant::now() + BACKGROUND_TIMEOUT;
    loop {
        let out = repo.run(&["proposals", "list"], &env.pairs());
        assert_eq!(code(&out), 0, "{}", both(&out));
        let listed = stdout(&out)
            .lines()
            .any(|line| line.split_whitespace().next() == Some(id.as_str()));
        if listed {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "no proposal row for {id} within {BACKGROUND_TIMEOUT:?}:\n{}",
            stdout(&out)
        );
        thread::sleep(POLL_INTERVAL);
    }
}

fn wait_for_failure_note(repo: &Repo, env: &HostEnv, id: &str) -> String {
    let deadline = Instant::now() + BACKGROUND_TIMEOUT;
    loop {
        let out = repo.run(&["proposals", "list"], &env.pairs());
        assert_eq!(code(&out), 0, "{}", both(&out));
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        if stderr.contains(id) {
            return stderr;
        }
        assert!(
            Instant::now() < deadline,
            "no failure note for {id} within {BACKGROUND_TIMEOUT:?}:\n{}",
            both(&out)
        );
        thread::sleep(POLL_INTERVAL);
    }
}

#[test]
fn a_failed_background_triage_is_reported_by_proposals_list() {
    let repo = Repo::new();
    let host = FakeHost::new();
    host.set_reply("not json");
    let env = host.env();

    let captured = capture_correction(&repo, &env);
    assert_eq!(code(&captured), 0, "{}", both(&captured));
    let id = cluster_id(&repo, &env);

    let note = wait_for_failure_note(&repo, &env, &id);
    assert!(note.contains("triage failed"), "{note}");
    assert!(note.contains("claude"), "{note}");
    assert!(note.contains(&format!("oncebitten triage {id}")), "{note}");
    let listed = repo.run(&["proposals", "list"], &env.pairs());
    assert_eq!(stdout(&listed), "", "a failure is not a proposal");
}

#[test]
fn a_later_answer_clears_the_failure_note() {
    let repo = Repo::new();
    let host = FakeHost::new();
    host.set_reply("not json");
    let env = host.env();

    let captured = capture_correction(&repo, &env);
    assert_eq!(code(&captured), 0, "{}", both(&captured));
    let id = cluster_id(&repo, &env);
    wait_for_failure_note(&repo, &env, &id);

    host.set_reply(REPLY);
    let retried = repo.run(&["triage", &id, "--host", "claude"], &env.pairs());
    assert_eq!(code(&retried), 0, "{}", both(&retried));

    let listed = repo.run(&["proposals", "list"], &env.pairs());
    let stderr = String::from_utf8_lossy(&listed.stderr);
    assert!(!stderr.contains("triage failed"), "{stderr}");
    assert!(stdout(&listed).contains(&id));
}

fn reply_with(fix_kind: &str, draft: Value) -> String {
    serde_json::json!({
        "type": "result",
        "is_error": false,
        "structured_output": {
            "repeatable": true,
            "user_correction": true,
            "root_cause": "The agent ignored an explicit stop request.",
            "proposed_fix_kind": fix_kind,
            "generality": "general",
            "draft": draft,
        },
    })
    .to_string()
}

fn prompt_draft(regex: &str) -> Value {
    let event = |prompt: &str| serde_json::json!({"event": "prompt", "prompt": prompt}).to_string();
    serde_json::json!({
        "id": "stop-means-stop",
        "description": "stop when the user says stop",
        "mode": "steer",
        "engine": "steer",
        "prompt": regex,
        "bad": {"name": "bad.json", "content": event("run the whole test suite")},
        "fixed": {"name": "fixed.json", "content": event("run the linter")},
        "corpus": [{"name": "rename.json", "content": event("rename the config module")}],
        "rationale": "Before you finish, confirm the user did not ask you to stop.",
    })
}

/// A repo whose correction cluster is queued as a finding, and its id.
fn queued_finding(host: &FakeHost) -> (Repo, String) {
    let repo = Repo::new();
    write_user_config(&repo, BACKGROUND_OFF);
    host.set_reply(REPLY);
    let env = host.env();
    let captured = capture_correction(&repo, &env);
    assert_eq!(code(&captured), 0, "{}", both(&captured));
    let id = cluster_id(&repo, &env);
    let out = repo.run(&["triage", &id, "--host", "claude"], &env.pairs());
    assert_eq!(code(&out), 0, "{}", both(&out));
    assert!(list_line(&repo, &env, &id).contains("finding"));
    (repo, id)
}

fn list_line(repo: &Repo, env: &HostEnv, id: &str) -> String {
    let out = repo.run(&["proposals", "list"], &env.pairs());
    assert_eq!(code(&out), 0, "{}", both(&out));
    stdout(&out)
        .lines()
        .find(|line| line.split_whitespace().next() == Some(id))
        .unwrap_or_else(|| panic!("no row for {id}:\n{}", both(&out)))
        .to_owned()
}

fn rules_dir(repo: &Repo) -> PathBuf {
    repo.project().join(".oncebitten").join("rules.local")
}

#[test]
fn a_manual_triage_upgrades_a_finding_to_a_proven_prompt_steer_proposal() {
    let host = FakeHost::new();
    let (repo, id) = queued_finding(&host);
    let env = host.env();

    host.set_reply(&reply_with(
        "agent-workflow",
        prompt_draft("(?i)test suite"),
    ));
    let out = repo.run(&["triage", &id, "--host", "claude"], &env.pairs());
    assert_eq!(code(&out), 0, "{}", both(&out));
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(stderr.contains("proposed a proven rule"), "{stderr}");

    let line = list_line(&repo, &env, &id);
    assert!(line.contains("stop-means-stop"), "{line}");
    assert!(!line.contains("finding"), "{line}");
    let text =
        fs::read_to_string(rules_dir(&repo).join("stop-means-stop").join("rule.toml")).unwrap();
    assert!(text.contains("at = \"stop\""), "{text}");
    assert!(text.contains("prompt = \"(?i)test suite\""), "{text}");
}

#[test]
fn a_manual_triage_without_a_draft_keeps_the_finding_and_says_so() {
    let host = FakeHost::new();
    let (repo, id) = queued_finding(&host);
    let env = host.env();

    let out = repo.run(&["triage", &id, "--host", "claude"], &env.pairs());
    assert_eq!(code(&out), 0, "{}", both(&out));
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(stderr.contains("stays a finding"), "{stderr}");
    assert!(list_line(&repo, &env, &id).contains("finding"));
    assert!(!rules_dir(&repo).exists());
}

#[test]
fn a_background_triage_of_a_queued_finding_does_nothing() {
    let host = FakeHost::new();
    let (repo, id) = queued_finding(&host);
    let env = host.env();
    let calls = host.call_count();

    host.set_reply(&reply_with(
        "agent-workflow",
        prompt_draft("(?i)test suite"),
    ));
    let out = repo.run(
        &["triage", &id, "--host", "claude", "--queue"],
        &env.pairs(),
    );
    assert_eq!(code(&out), 0, "{}", both(&out));
    assert_eq!(host.call_count(), calls, "the host was asked");
    assert!(list_line(&repo, &env, &id).contains("finding"));
    assert!(!rules_dir(&repo).exists());
}

#[test]
fn a_prompt_draft_that_fails_validation_could_not_run_and_writes_nothing() {
    let host = FakeHost::new();
    let (repo, id) = queued_finding(&host);
    let env = host.env();
    let both_kinds = {
        let mut draft = prompt_draft("(?i)test suite");
        draft["bad"]["content"] =
            serde_json::json!({"event": "tool", "tool": "Bash", "command": "cargo test"})
                .to_string()
                .into();
        draft
    };
    let cases = [
        ("does not compile", prompt_draft("(?i)test suite(")),
        ("matches fixed", prompt_draft("(?i)run the")),
        (
            "matches a corpus prompt",
            prompt_draft("(?i)test suite|rename"),
        ),
        ("misses bad", prompt_draft("(?i)changelog")),
        (
            "matches the empty string",
            prompt_draft("(?i)(test suite)?"),
        ),
        ("a command and a prompt", both_kinds),
    ];
    for (why, draft) in cases {
        host.set_reply(&reply_with("agent-workflow", draft));
        let out = repo.run(&["triage", &id, "--host", "claude"], &env.pairs());
        assert_eq!(code(&out), 2, "{why}: {}", both(&out));
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        assert!(stderr.contains("could not run"), "{why}: {stderr}");
        assert!(!rules_dir(&repo).exists(), "{why}");
        assert!(list_line(&repo, &env, &id).contains("finding"), "{why}");
    }
}
