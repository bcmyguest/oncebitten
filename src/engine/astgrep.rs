use std::path::{Path, PathBuf};

use ast_grep_config::{from_yaml_string, GlobalRules, RuleCollection, RuleConfig};
use ast_grep_core::tree_sitter::LanguageExt;
use ast_grep_core::Language;
use ast_grep_language::SupportLang;

use super::{Finding, Outcome};

pub type Rule = RuleConfig<SupportLang>;

/// Runs the check file's rules on `files`, matching `files:` and `ignores:`
/// against each path as given. Gate code passes `run_in` instead.
pub fn run(check_file: &Path, files: &[PathBuf]) -> Outcome {
    let paths: Vec<(PathBuf, String)> = files
        .iter()
        .map(|file| (file.clone(), slashed(&file.display().to_string())))
        .collect();
    scan(check_file, &paths)
}

/// Runs the check file's rules on repo-relative `/`-separated paths read from `repo`.
pub fn run_in(check_file: &Path, repo: &Path, rel_paths: &[String]) -> Outcome {
    let paths: Vec<(PathBuf, String)> = rel_paths
        .iter()
        .map(|rel| (repo.join(rel.split('/').collect::<PathBuf>()), rel.clone()))
        .collect();
    scan(check_file, &paths)
}

/// Parses a check file. An empty file is an error: it defines no rule to run.
pub fn parse_rules(text: &str, shown: &str) -> Result<Vec<Rule>, String> {
    let rules: Vec<Rule> = from_yaml_string(text, &GlobalRules::default())
        .map_err(|error| format!("{shown}: {error}"))?;
    if rules.is_empty() {
        return Err(format!("{shown}: defines no rules"));
    }
    Ok(rules)
}

/// Builds the rule set that runs. Rules with `severity: off` are dropped here.
pub fn collection(rules: Vec<Rule>, shown: &str) -> Result<RuleCollection<SupportLang>, String> {
    RuleCollection::try_new(rules).map_err(|error| format!("{shown}: {error}"))
}

/// The rules that apply to a repo-relative `/`-separated path: the language
/// matches and the `files:` and `ignores:` globs admit the path.
pub fn select<'a>(collection: &'a RuleCollection<SupportLang>, rel_path: &str) -> Vec<&'a Rule> {
    collection.for_path(rel_path)
}

/// Converts path separators to `/` so globs match on every platform.
pub fn slashed(path: &str) -> String {
    path.replace('\\', "/")
}

fn scan(check_file: &Path, files: &[(PathBuf, String)]) -> Outcome {
    match scan_checked(check_file, files) {
        Ok(outcome) => outcome,
        Err(reason) => Outcome::CouldNotRun(reason),
    }
}

fn scan_checked(check_file: &Path, files: &[(PathBuf, String)]) -> Result<Outcome, String> {
    let shown = check_file.display().to_string();
    let text = std::fs::read_to_string(check_file).map_err(|error| format!("{shown}: {error}"))?;
    let collection = collection(parse_rules(&text, &shown)?, &shown)?;
    let mut findings = Vec::new();
    for (file, rel) in files {
        let Some(lang) = SupportLang::from_path(rel) else {
            continue;
        };
        let rules = select(&collection, rel);
        if rules.is_empty() {
            continue;
        }
        let source = std::fs::read_to_string(file)
            .map_err(|error| format!("{}: {error}", file.display()))?;
        let grep = lang.ast_grep(&source);
        let root = grep.root();
        for rule in rules {
            for found in root.find_all(&rule.matcher) {
                findings.push(Finding {
                    path: Some(file.clone()),
                    line: Some(found.start_pos().line() + 1),
                    message: rule.get_message(&found),
                    text: Some(found.text().to_string()),
                });
            }
        }
    }
    Ok(if findings.is_empty() {
        Outcome::Clean
    } else {
        Outcome::Fired(findings)
    })
}
