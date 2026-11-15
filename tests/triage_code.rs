use std::cell::RefCell;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use oncebitten::config::{Host, Triage};
use oncebitten::evidence::{Cluster, Occurrence, Source};
use oncebitten::store::{Capture, NewEvent, Store, StoredEvent};
use oncebitten::triage::code::collect;
use oncebitten::triage::flow::{ask_cluster, prompt_input};
use oncebitten::triage::host::{Captured, Invocation};
use oncebitten::triage::prompt::build_prompt;
use oncebitten::triage::run::{RunError, Runner};
use tempfile::TempDir;

const SESSION: &str = "s1";
const EDITED_LINE: &str = "fn a() { x.unwrap(); }";

const VERDICT_WITH_DRAFT: &str = r#"{"repeatable":true,"user_correction":false,"root_cause":"unwrap in library code","proposed_fix_kind":"lint","generality":"project","draft":{"id":"no-unwrap","description":"Do not unwrap in library code.","mode":"block","engine":"native","language":null,"check":null,"command":["cargo","test"],"fired_exit_codes":[1],"fired_output_contains":null,"when_changed":null,"require_changed":null,"bad":{"name":"bad.rs","content":"__BAD__"},"fixed":{"name":"fixed.rs","content":"fn a() {}\n"},"corpus":null,"rationale":"unwrap panics."}}"#;

const VERDICT_NO_DRAFT: &str = r#"{"repeatable":true,"user_correction":false,"root_cause":"stale lockfile","proposed_fix_kind":"lint","generality":"project","draft":null}"#;

/// Records the prompt each call sends and answers with `stdout` as opencode does.
struct Recording {
    stdout: String,
    prompts: RefCell<Vec<String>>,
}

impl Recording {
    fn new(verdict: &str) -> Self {
        let event = serde_json::json!({"type": "text", "part": {"text": verdict}});
        Recording {
            stdout: format!("{event}\n"),
            prompts: RefCell::new(Vec::new()),
        }
    }

    fn last_prompt(&self) -> String {
        self.prompts.borrow().last().cloned().unwrap_or_default()
    }
}

impl Runner for Recording {
    fn run(&self, inv: &Invocation, _dir: &Path, _timeout: Duration) -> Result<Captured, RunError> {
        self.prompts
            .borrow_mut()
            .push(inv.stdin.clone().unwrap_or_default());
        Ok(Captured {
            stdout: self.stdout.clone(),
            answer: None,
        })
    }
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

/// A repo with two committed files. Returns the temp root, the repo and HEAD.
fn repo() -> (TempDir, PathBuf, String) {
    let root = TempDir::new().unwrap();
    let repo = root.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q"]);
    git(&repo, &["config", "user.email", "triage@example.invalid"]);
    git(&repo, &["config", "user.name", "triage"]);
    git(&repo, &["config", "commit.gpgsign", "false"]);
    fs::write(repo.join("lib.rs"), "fn a() {}\n").unwrap();
    fs::write(repo.join("other.rs"), "fn keep() {}\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-q", "-m", "init"]);
    let head = git(&repo, &["rev-parse", "HEAD"]);
    (root, repo, head)
}

/// The agent edits `file` in the session: the store keeps the edit tool event.
fn record_edit(store: &Store, repo: &Path, file: &str, ts: i64) {
    let event = NewEvent {
        ts,
        host: "opencode".into(),
        session_id: SESSION.into(),
        repo: Some(repo.display().to_string()),
        kind: "tool_ok".into(),
        tool: Some("edit".into()),
        summary: repo.join(file).display().to_string(),
    };
    store
        .record_captured(&event, &[], &Capture::default())
        .unwrap();
}

fn occurrence(repo: &Path, head: Option<&str>, id: i64) -> Occurrence {
    Occurrence {
        event: StoredEvent {
            id,
            ts: id,
            host: "opencode".into(),
            session_id: SESSION.into(),
            repo: Some(repo.display().to_string()),
            kind: "tool_ok".into(),
            tool: Some("bash".into()),
            summary: "cargo test".into(),
            git_head: head.map(str::to_string),
        },
        signals: vec!["repeat-failure".into()],
    }
}

fn cluster(occurrences: Vec<Occurrence>) -> Cluster {
    Cluster {
        id: "c1".into(),
        class: "failure",
        source: Source::Mined,
        tool: Some("bash".into()),
        pattern: "cargo test".into(),
        occurrences,
        rejection: None,
        decision: None,
    }
}

/// A repo whose session edited `lib.rs` (now `EDITED_LINE`) and `other.rs`
/// without recording an edit to it.
fn edited_session() -> (TempDir, PathBuf, Store, Cluster) {
    let (root, repo, head) = repo();
    let store = Store::open_in_memory().unwrap();
    record_edit(&store, &repo, "lib.rs", 1);
    fs::write(repo.join("lib.rs"), format!("{EDITED_LINE}\n")).unwrap();
    fs::write(repo.join("other.rs"), "fn keep() { y.unwrap(); }\n").unwrap();
    let cluster = cluster(vec![occurrence(&repo, Some(&head), 1)]);
    (root, repo, store, cluster)
}

fn with_bad(bad: &str) -> String {
    VERDICT_WITH_DRAFT.replace("__BAD__", &bad.replace('\n', "\\n"))
}

#[test]
fn collect_sends_the_hunks_of_files_the_session_edited() {
    let (_root, repo, store, cluster) = edited_session();
    let code = collect(&store, &cluster.occurrences, 16 * 1024).unwrap();
    assert!(
        code.diff.contains(&format!("+{EDITED_LINE}")),
        "{}",
        code.diff
    );
    assert!(code.diff.contains("lib.rs"));
    assert!(
        !code.diff.contains("y.unwrap"),
        "unedited file leaked: {}",
        code.diff
    );
    assert!(repo.join("lib.rs").exists());
}

#[test]
fn collect_reads_nothing_without_a_stored_git_head() {
    let (_root, repo, store, _) = edited_session();
    let code = collect(&store, &[occurrence(&repo, None, 1)], 16 * 1024).unwrap();
    assert!(code.is_empty());
}

#[test]
fn collect_caps_the_diff_at_max_bytes() {
    let (_root, _repo, store, cluster) = edited_session();
    let code = collect(&store, &cluster.occurrences, 40).unwrap();
    assert!(code.diff.ends_with("[truncated]"), "{}", code.diff);
    assert!(code.diff.len() <= 40 + "[truncated]".len());
}

#[test]
fn a_fixture_quoting_an_added_line_passes_despite_crlf_and_trailing_spaces() {
    let (_root, _repo, store, cluster) = edited_session();
    let code = collect(&store, &cluster.occurrences, 16 * 1024).unwrap();
    assert!(code.quoted_by(&format!("fn a() {{}}\r\n{EDITED_LINE}   \r\n")));
    assert!(!code.quoted_by("fn b() {}\n"));
}

#[test]
fn a_draft_whose_bad_fixture_quotes_no_changed_code_is_could_not_run() {
    let (_root, _repo, store, cluster) = edited_session();
    let runner = Recording::new(&with_bad("fn b() {}\n"));
    let err = ask_cluster(
        &runner,
        Host::Opencode,
        "plan",
        &store,
        &cluster,
        &Triage::default(),
        Duration::from_secs(5),
    )
    .unwrap_err();
    assert!(err.to_string().contains("could not run"), "{err}");
    assert!(err.to_string().contains("bad"), "{err}");
}

#[test]
fn a_draft_whose_bad_fixture_quotes_changed_code_is_kept() {
    let (_root, _repo, store, cluster) = edited_session();
    let runner = Recording::new(&with_bad(&format!("fn a() {{}}\n{EDITED_LINE}\n")));
    let answer = ask_cluster(
        &runner,
        Host::Opencode,
        "plan",
        &store,
        &cluster,
        &Triage::default(),
        Duration::from_secs(5),
    )
    .unwrap();
    assert!(answer.draft.is_some());
}

#[test]
fn the_prompt_carries_the_hunks_by_default() {
    let (_root, _repo, store, cluster) = edited_session();
    let runner = Recording::new(VERDICT_NO_DRAFT);
    ask_cluster(
        &runner,
        Host::Opencode,
        "plan",
        &store,
        &cluster,
        &Triage::default(),
        Duration::from_secs(5),
    )
    .unwrap();
    assert!(runner.last_prompt().contains(&format!("+{EDITED_LINE}")));
}

#[test]
fn send_code_false_sends_the_prompt_without_code() {
    let (_root, _repo, store, cluster) = edited_session();
    let runner = Recording::new(&with_bad("fn b() {}\n"));
    let triage = Triage {
        send_code: false,
        ..Triage::default()
    };
    let answer = ask_cluster(
        &runner,
        Host::Opencode,
        "plan",
        &store,
        &cluster,
        &triage,
        Duration::from_secs(5),
    )
    .unwrap();
    assert_eq!(runner.last_prompt(), build_prompt(&prompt_input(&cluster)));
    assert!(answer.draft.is_some(), "no code means no fixture check");
}

#[test]
fn no_hunks_keep_the_prompt_and_skip_the_fixture_check() {
    let (_root, repo, head) = repo();
    let store = Store::open_in_memory().unwrap();
    let cluster = cluster(vec![occurrence(&repo, Some(&head), 1)]);
    let runner = Recording::new(&with_bad("fn b() {}\n"));
    let answer = ask_cluster(
        &runner,
        Host::Opencode,
        "plan",
        &store,
        &cluster,
        &Triage::default(),
        Duration::from_secs(5),
    )
    .unwrap();
    assert_eq!(runner.last_prompt(), build_prompt(&prompt_input(&cluster)));
    assert!(answer.draft.is_some());
}

#[test]
fn triage_config_defaults_send_code_on_with_a_16_kib_cap() {
    let triage = Triage::default();
    assert!(triage.send_code);
    assert_eq!(triage.max_code_bytes, 16 * 1024);
    let parsed: Triage = toml::from_str("send_code = false\nmax_code_bytes = 2048\n").unwrap();
    assert!(!parsed.send_code);
    assert_eq!(parsed.max_code_bytes, 2048);
}

const VERDICT_DIFF_DRAFT: &str = r#"{"repeatable":true,"user_correction":false,"root_cause":"edits outside the test dir","proposed_fix_kind":"lint","generality":"project","draft":{"id":"paths","description":"Changes need a test.","mode":"block","engine":"diff","language":null,"check":null,"command":null,"fired_exit_codes":null,"fired_output_contains":null,"when_changed":["*.rs"],"require_changed":["*.rs"],"bad":{"name":"bad.txt","content":"lib.rs\n"},"fixed":{"name":"fixed.txt","content":"other.rs\n"},"corpus":null,"rationale":"paths name the change."}}"#;

#[test]
fn a_diff_draft_whose_bad_lists_changed_paths_is_kept() {
    let (_root, _repo, store, cluster) = edited_session();
    let runner = Recording::new(VERDICT_DIFF_DRAFT);
    let answer = ask_cluster(
        &runner,
        Host::Opencode,
        "plan",
        &store,
        &cluster,
        &Triage::default(),
        Duration::from_secs(5),
    )
    .unwrap();
    assert!(answer.draft.is_some());
}

/// A repo where `lib.rs` changed from `x`, `b`, `y` to `A`, `b`, `C`: one hunk
/// whose post-image is `A`, `b`, `C` with `b` as context. Returns the temp root,
/// the repo, the store with the edit recorded, and the head the session started from.
fn context_session() -> (TempDir, PathBuf, Store, String) {
    let (root, repo, _) = repo();
    fs::write(repo.join("lib.rs"), "x\nb\ny\n").unwrap();
    git(&repo, &["commit", "-q", "-am", "middle"]);
    let head = git(&repo, &["rev-parse", "HEAD"]);
    let store = Store::open_in_memory().unwrap();
    record_edit(&store, &repo, "lib.rs", 1);
    fs::write(repo.join("lib.rs"), "A\nb\nC\n").unwrap();
    (root, repo, store, head)
}

#[test]
fn a_fixture_quoting_context_between_added_lines_passes() {
    let (_root, repo, store, head) = context_session();
    let code = collect(&store, &[occurrence(&repo, Some(&head), 1)], 16 * 1024).unwrap();
    assert!(code.quoted_by("A\nb\nC\n"), "{}", code.diff);
}

#[test]
fn a_fixture_of_context_lines_alone_is_not_a_quote_of_the_change() {
    let (_root, repo, store, head) = context_session();
    let code = collect(&store, &[occurrence(&repo, Some(&head), 1)], 16 * 1024).unwrap();
    assert!(!code.quoted_by("b\n"), "{}", code.diff);
}

#[cfg(windows)]
fn msys_form(path: &str) -> String {
    let text = path.replace('\\', "/");
    let (drive, rest) = text.split_at(1);
    format!(
        "/{}{}",
        drive.to_ascii_lowercase(),
        rest.replacen(':', "", 1)
    )
}

#[cfg(windows)]
#[test]
fn collect_matches_msys_and_mixed_case_edit_paths_on_windows() {
    let (_root, repo, head) = repo();
    let store = Store::open_in_memory().unwrap();
    let event = NewEvent {
        ts: 1,
        host: "opencode".into(),
        session_id: SESSION.into(),
        repo: Some(repo.display().to_string()),
        kind: "tool_ok".into(),
        tool: Some("edit".into()),
        summary: msys_form(&repo.join("lib.rs").display().to_string()),
    };
    store
        .record_captured(&event, &[], &Capture::default())
        .unwrap();
    fs::write(repo.join("lib.rs"), format!("{EDITED_LINE}\n")).unwrap();
    let code = collect(&store, &[occurrence(&repo, Some(&head), 1)], 16 * 1024).unwrap();
    assert!(
        code.diff.contains(&format!("+{EDITED_LINE}")),
        "{}",
        code.diff
    );
}
