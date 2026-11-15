use std::path::{Path, PathBuf};

use ast_grep_config::{from_yaml_string, GlobalRules, RuleCollection, RuleConfig};
use ast_grep_core::tree_sitter::LanguageExt;
use ast_grep_core::Language;
use ast_grep_language::SupportLang;

use super::{Finding, Outcome};

type Rule = RuleConfig<SupportLang>;

pub fn run(check_file: &Path, files: &[PathBuf]) -> Outcome {
    match run_checked(check_file, files) {
        Ok(outcome) => outcome,
        Err(reason) => Outcome::CouldNotRun(reason),
    }
}

fn run_checked(check_file: &Path, files: &[PathBuf]) -> Result<Outcome, String> {
    let collection = load_collection(check_file)?;
    let mut findings = Vec::new();
    for file in files {
        let Some(lang) = SupportLang::from_path(file) else {
            continue;
        };
        let rules = collection.for_path(file);
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

fn load_collection(check_file: &Path) -> Result<RuleCollection<SupportLang>, String> {
    let shown = check_file.display();
    let text = std::fs::read_to_string(check_file).map_err(|error| format!("{shown}: {error}"))?;
    let rules: Vec<Rule> = from_yaml_string(&text, &GlobalRules::default())
        .map_err(|error| format!("{shown}: {error}"))?;
    if rules.is_empty() {
        return Err(format!("{shown}: defines no rules"));
    }
    RuleCollection::try_new(rules).map_err(|error| format!("{shown}: {error}"))
}
