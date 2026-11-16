use oncebitten::signal::{KIND_TOOL_ERROR, KIND_TOOL_OK};
use oncebitten::store::{NewEvent, Store};

fn event(ts: i64, repo: &str, kind: &str, tool: &str, summary: &str) -> NewEvent {
    NewEvent {
        ts,
        host: "codex".into(),
        session_id: "s1".into(),
        repo: Some(repo.into()),
        kind: kind.into(),
        tool: Some(tool.into()),
        summary: summary.into(),
    }
}

#[test]
fn successful_commands_lists_only_ok_shell_calls_in_the_repo_newest_first() {
    let store = Store::open_in_memory().unwrap();
    for event in [
        event(1, "/r", KIND_TOOL_OK, "Bash", "cargo fmt"),
        event(2, "/r", KIND_TOOL_ERROR, "Bash", "cargo build --bad"),
        event(3, "/r", KIND_TOOL_OK, "Edit", "src/lib.rs"),
        event(4, "/other", KIND_TOOL_OK, "Bash", "cargo test"),
        event(5, "/r", KIND_TOOL_OK, "bash", "cargo clippy"),
        event(6, "/r", KIND_TOOL_OK, "Bash", "git push --force"),
    ] {
        store.record(&event, &[]).unwrap();
    }
    let found = store
        .successful_commands("/r", "git push --force", 10)
        .unwrap();
    let summaries: Vec<&str> = found.iter().map(|e| e.summary.as_str()).collect();
    assert_eq!(summaries, ["cargo clippy", "cargo fmt"]);
}

#[test]
fn successful_commands_respects_the_limit() {
    let store = Store::open_in_memory().unwrap();
    for ts in 1..=5 {
        let summary = format!("cmd {ts}");
        store
            .record(&event(ts, "/r", KIND_TOOL_OK, "Bash", &summary), &[])
            .unwrap();
    }
    let found = store.successful_commands("/r", "none", 2).unwrap();
    assert_eq!(found.len(), 2);
    assert_eq!(found[0].summary, "cmd 5");
}

#[test]
fn successful_commands_match_a_verbatim_windows_repo_path() {
    let store = Store::open_in_memory().unwrap();
    store
        .record(
            &event(1, r"\\?\C:\work\r", KIND_TOOL_OK, "Bash", "cargo fmt"),
            &[],
        )
        .unwrap();
    let found = store.successful_commands(r"C:\work\r", "none", 10).unwrap();
    assert_eq!(found.len(), 1);
}

#[cfg(unix)]
#[test]
fn successful_commands_match_a_repo_reached_through_a_symlink() {
    let root = tempfile::TempDir::new().unwrap();
    let real = root.path().join("real");
    std::fs::create_dir(&real).unwrap();
    let link = root.path().join("link");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let store = Store::open_in_memory().unwrap();
    store
        .record(
            &event(1, link.to_str().unwrap(), KIND_TOOL_OK, "Bash", "cargo fmt"),
            &[],
        )
        .unwrap();
    let canonical = std::fs::canonicalize(&real).unwrap();
    let found = store
        .successful_commands(canonical.to_str().unwrap(), "none", 10)
        .unwrap();
    assert_eq!(found.len(), 1);
}
