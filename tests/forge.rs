use std::fs;
use std::path::{Path, PathBuf};

use oncebitten::forge::{self, Kind, NewPr};

/// Copies the fake forge binary into `dir` under the CLI's name and returns its path.
fn fake_cli(dir: &Path, cli: &str) -> PathBuf {
    let name = format!("{cli}{}", std::env::consts::EXE_SUFFIX);
    let target = dir.join(name);
    let built = Path::new(env!("CARGO_BIN_EXE_fake-forge"));
    // A hard link avoids a write handle on the executable: spawning a file that another
    // test thread is still writing fails with "Text file busy" on Linux.
    if fs::hard_link(built, &target).is_err() {
        fs::copy(built, &target).expect("copy fake forge");
    }
    target
}

fn calls(dir: &Path) -> Vec<Vec<String>> {
    fs::read_to_string(dir.join("calls.log"))
        .unwrap_or_default()
        .lines()
        .map(|line| serde_json::from_str(line).expect("argv line is a JSON array"))
        .collect()
}

fn argv(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|p| p.to_string()).collect()
}

#[test]
fn detects_github_gitlab_and_azure_remotes() {
    assert_eq!(
        forge::detect("https://github.com/o/r.git"),
        Some(Kind::Github)
    );
    assert_eq!(forge::detect("git@github.com:o/r.git"), Some(Kind::Github));
    assert_eq!(
        forge::detect("https://gitlab.com/g/r.git"),
        Some(Kind::Gitlab)
    );
    assert_eq!(
        forge::detect("https://gitlab.example.org/g/r.git"),
        Some(Kind::Gitlab)
    );
    assert_eq!(
        forge::detect("https://dev.azure.com/org/proj/_git/r"),
        Some(Kind::Azure)
    );
    assert_eq!(
        forge::detect("https://org.visualstudio.com/proj/_git/r"),
        Some(Kind::Azure)
    );
    assert_eq!(forge::detect("https://example.com/r.git"), None);
}

#[test]
fn unknown_remote_without_config_names_the_key() {
    let text = forge::select(None, Some("https://example.com/r.git"))
        .unwrap_err()
        .to_string();
    assert!(text.contains("forge"), "{text}");
    assert!(text.contains("https://example.com/r.git"), "{text}");
}

#[test]
fn configured_forge_overrides_the_remote() {
    let kind = forge::select(Some(Kind::Gitlab), Some("https://github.com/o/r.git")).unwrap();
    assert_eq!(kind, Kind::Gitlab);
}

#[test]
fn github_find_open_reads_number_and_url() {
    let dir = tempfile::tempdir().unwrap();
    let program = fake_cli(dir.path(), "gh");
    fs::write(
        dir.path().join("reply"),
        r#"[{"number":12,"url":"https://github.com/o/r/pull/12"}]"#,
    )
    .unwrap();
    let backend = forge::backend_with(Kind::Github, program, dir.path());

    let found = backend.find_open("feat/x").unwrap().expect("a PR");

    assert_eq!(found.number, 12);
    assert_eq!(found.url, "https://github.com/o/r/pull/12");
    assert_eq!(
        calls(dir.path()),
        vec![argv(&[
            "pr",
            "list",
            "--head",
            "feat/x",
            "--state",
            "open",
            "--json",
            "number,url"
        ])]
    );
}

#[test]
fn github_find_open_returns_none_for_empty_list() {
    let dir = tempfile::tempdir().unwrap();
    let program = fake_cli(dir.path(), "gh");
    fs::write(dir.path().join("reply"), "[]").unwrap();
    let backend = forge::backend_with(Kind::Github, program, dir.path());

    assert_eq!(backend.find_open("feat/x").unwrap(), None);
}

#[test]
fn github_open_passes_each_field_as_one_argument() {
    let dir = tempfile::tempdir().unwrap();
    let program = fake_cli(dir.path(), "gh");
    fs::write(dir.path().join("reply"), "https://github.com/o/r/pull/7\n").unwrap();
    let backend = forge::backend_with(Kind::Github, program, dir.path());
    let request = NewPr {
        head: "feat/x".into(),
        base: "main".into(),
        title: "Add a rule".into(),
        body: "two words\nand a newline".into(),
    };

    let created = backend.open(&request).unwrap();

    assert_eq!(created.number, 7);
    assert_eq!(created.url, "https://github.com/o/r/pull/7");
    let args = &calls(dir.path())[0];
    let body_at = args.iter().position(|a| a == "--body").unwrap();
    assert_eq!(args[body_at + 1], "two words\nand a newline");
    let title_at = args.iter().position(|a| a == "--title").unwrap();
    assert_eq!(args[title_at + 1], "Add a rule");
}

#[test]
fn github_read_returns_description_and_comment_bodies() {
    let dir = tempfile::tempdir().unwrap();
    let program = fake_cli(dir.path(), "gh");
    fs::write(
        dir.path().join("reply"),
        r#"{"body":"desc","comments":[{"body":"first"},{"body":"second"}]}"#,
    )
    .unwrap();
    let backend = forge::backend_with(Kind::Github, program, dir.path());

    let text = backend.read(3).unwrap();

    assert_eq!(text.description, "desc");
    assert_eq!(text.comments, vec!["first", "second"]);
    assert_eq!(
        calls(dir.path())[0],
        argv(&["pr", "view", "3", "--json", "body,comments"])
    );
}

#[test]
fn non_zero_exit_carries_stderr() {
    let dir = tempfile::tempdir().unwrap();
    let program = fake_cli(dir.path(), "gh");
    fs::write(dir.path().join("status"), "1").unwrap();
    fs::write(dir.path().join("stderr"), "no such pull request\n").unwrap();
    let backend = forge::backend_with(Kind::Github, program, dir.path());

    let text = backend.find_open("feat/x").unwrap_err().to_string();

    assert!(text.contains("no such pull request"), "{text}");
}

#[test]
fn missing_cli_binary_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let backend = forge::backend_with(Kind::Github, dir.path().join("absent-gh"), dir.path());

    let text = backend.find_open("feat/x").unwrap_err().to_string();

    assert!(text.contains("not installed"), "{text}");
}

#[test]
fn gitlab_find_open_uses_source_branch_and_web_url() {
    let dir = tempfile::tempdir().unwrap();
    let program = fake_cli(dir.path(), "glab");
    fs::write(
        dir.path().join("reply"),
        r#"[{"iid":4,"web_url":"https://gitlab.com/g/r/-/merge_requests/4"}]"#,
    )
    .unwrap();
    let backend = forge::backend_with(Kind::Gitlab, program, dir.path());

    let found = backend.find_open("feat/x").unwrap().expect("an MR");

    assert_eq!(found.number, 4);
    assert_eq!(found.url, "https://gitlab.com/g/r/-/merge_requests/4");
    assert!(calls(dir.path())[0].contains(&"--source-branch".to_string()));
}

#[test]
fn azure_find_open_uses_source_branch_and_active_status() {
    let dir = tempfile::tempdir().unwrap();
    let program = fake_cli(dir.path(), "az");
    fs::write(
        dir.path().join("reply"),
        r#"[{"pullRequestId":9,"_links":{"web":{"href":"https://dev.azure.com/o/p/_git/r/pullrequest/9"}}}]"#,
    )
    .unwrap();
    let backend = forge::backend_with(Kind::Azure, program, dir.path());

    let found = backend.find_open("feat/x").unwrap().expect("a PR");

    assert_eq!(found.number, 9);
    assert_eq!(found.url, "https://dev.azure.com/o/p/_git/r/pullrequest/9");
    assert_eq!(
        calls(dir.path())[0],
        argv(&[
            "repos",
            "pr",
            "list",
            "--source-branch",
            "feat/x",
            "--status",
            "active",
            "--output",
            "json"
        ])
    );
}

#[test]
fn gitlab_read_flattens_discussion_notes_and_skips_system_notes() {
    let dir = tempfile::tempdir().unwrap();
    let program = fake_cli(dir.path(), "glab");
    // Shape of `glab mr view --comments --output json`: the merge request with a
    // `Discussions` array appended, each discussion holding `notes`.
    fs::write(
        dir.path().join("reply"),
        r#"{"iid":4,"description":"desc","Discussions":[
            {"id":"a","individual_note":true,"notes":[{"body":"first","system":false}]},
            {"id":"b","individual_note":false,"notes":[
                {"body":"added 1 commit","system":true},
                {"body":"second","system":false},
                {"body":"reply","system":false}]}]}"#,
    )
    .unwrap();
    let backend = forge::backend_with(Kind::Gitlab, program, dir.path());

    let text = backend.read(4).unwrap();

    assert_eq!(text.description, "desc");
    assert_eq!(text.comments, vec!["first", "second", "reply"]);
    assert_eq!(
        calls(dir.path())[0],
        argv(&["mr", "view", "4", "--output", "json", "--comments"])
    );
}

#[test]
fn gitlab_open_takes_the_url_from_the_last_output_line() {
    let dir = tempfile::tempdir().unwrap();
    let program = fake_cli(dir.path(), "glab");
    fs::write(
        dir.path().join("reply"),
        "!17 Add a rule (feat/x)\n https://gitlab.com/g/r/-/merge_requests/17\n",
    )
    .unwrap();
    let backend = forge::backend_with(Kind::Gitlab, program, dir.path());
    let request = NewPr {
        head: "feat/x".into(),
        base: "main".into(),
        title: "Add a rule".into(),
        body: "body".into(),
    };

    let created = backend.open(&request).unwrap();

    assert_eq!(created.number, 17);
    assert_eq!(created.url, "https://gitlab.com/g/r/-/merge_requests/17");
}
