mod common;

use std::collections::BTreeMap;

use common::Repo;
use oncebitten::config::Inputs;
use oncebitten::hits;
use oncebitten::store::HitsRow;

const RULE: &str = "id = \"no-unwrap\"\ndescription = \"Do not unwrap.\"\nmode = \"block\"\n\
                    status = \"proposed\"\nengine = \"ast-grep\"\nlanguage = \"rust\"\n";
const CHECK: &str = "id: no-unwrap\nlanguage: rust\nmessage: do not unwrap\n\
                     rule:\n  pattern: $X.unwrap()\n";
const NATIVE_RULE: &str = "id = \"native-probe\"\ndescription = \"Probe.\"\nmode = \"block\"\n\
                           status = \"proposed\"\nengine = \"native\"\n\
                           command = [\"cargo\", \"--version\"]\nfired_exit_codes = [1]\n";

fn inputs(repo: &Repo) -> Inputs {
    Inputs {
        cwd: repo.project(),
        vars: BTreeMap::new(),
    }
}

fn write_ast_grep_rule(repo: &Repo) -> std::path::PathBuf {
    let dir = repo.rule_dir("no-unwrap");
    repo.write(".oncebitten/rules/no-unwrap/rule.toml", RULE);
    repo.write(".oncebitten/rules/no-unwrap/check.yml", CHECK);
    write_fixtures(repo, "no-unwrap");
    dir
}

fn write_fixtures(repo: &Repo, id: &str) {
    repo.write(
        &format!(".oncebitten/rules/{id}/fixtures/bad.rs"),
        "fn f() { x.unwrap(); }\n",
    );
    repo.write(
        &format!(".oncebitten/rules/{id}/fixtures/fixed.rs"),
        "fn f() { x.expect(\"set\"); }\n",
    );
}

#[test]
fn counts_hits_in_tracked_files_at_head_and_keeps_three_samples() {
    let repo = Repo::new();
    repo.write("src/a.rs", "fn a() { x.unwrap(); y.unwrap(); }\n");
    repo.write(
        "src/b.rs",
        "fn b() {\n    z.unwrap();\n    w.unwrap();\n}\n",
    );
    repo.commit_all("init");
    // Untracked files are not part of HEAD and do not count.
    repo.write("src/untracked.rs", "fn u() { q.unwrap(); }\n");
    let dir = write_ast_grep_rule(&repo);

    let measured = hits::measure(&inputs(&repo), &repo.project(), &dir)
        .unwrap()
        .unwrap();

    assert_eq!(
        measured,
        HitsRow {
            count: 4,
            files: 2,
            samples: vec![
                "src/a.rs:1: x.unwrap()".to_owned(),
                "src/a.rs:1: y.unwrap()".to_owned(),
                "src/b.rs:2: z.unwrap()".to_owned(),
            ],
        }
    );
}

#[test]
fn a_clean_repo_measures_zero_hits() {
    let repo = Repo::new();
    repo.write("src/a.rs", "fn a() -> Option<u8> { None }\n");
    repo.commit_all("init");
    let dir = write_ast_grep_rule(&repo);

    let measured = hits::measure(&inputs(&repo), &repo.project(), &dir)
        .unwrap()
        .unwrap();

    assert_eq!(
        measured,
        HitsRow {
            count: 0,
            files: 0,
            samples: Vec::new(),
        }
    );
}

#[test]
fn a_native_rule_is_not_measured() {
    let repo = Repo::new();
    repo.write("src/a.rs", "fn a() {}\n");
    repo.commit_all("init");
    repo.write(".oncebitten/rules/native-probe/rule.toml", NATIVE_RULE);
    write_fixtures(&repo, "native-probe");
    let dir = repo.rule_dir("native-probe");

    let measured = hits::measure(&inputs(&repo), &repo.project(), &dir).unwrap();

    assert_eq!(measured, None);
}
