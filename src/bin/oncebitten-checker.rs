use std::path::Path;

use ast_grep_config::{from_yaml_string, GlobalRules, RuleConfig};
use ast_grep_core::tree_sitter::LanguageExt;
use ast_grep_core::Language;
use ast_grep_language::SupportLang;
use serde_json::{json, Value};

type Rule = RuleConfig<SupportLang>;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let report = match args.as_slice() {
        [rule, target] => {
            check(Path::new(rule), target).unwrap_or_else(|reason| error(target, &reason))
        }
        _ => error("", "usage: oncebitten-checker <rule.yml> <file>"),
    };
    println!("{report}");
}

fn error(file: &str, reason: &str) -> Value {
    json!({"status": "error", "file": file, "reason": reason})
}

fn check(rule_path: &Path, target: &str) -> Result<Value, String> {
    let yaml = std::fs::read_to_string(rule_path)
        .map_err(|error| format!("rule {}: {error}", rule_path.display()))?;
    let rules: Vec<Rule> = from_yaml_string(&yaml, &GlobalRules::default())
        .map_err(|error| format!("rule: {error}"))?;
    if rules.is_empty() {
        return Err("rule file defines no rules".to_owned());
    }
    let lang = SupportLang::from_path(Path::new(target))
        .ok_or_else(|| format!("{target}: no language for this file extension"))?;
    let applicable: Vec<&Rule> = rules.iter().filter(|rule| rule.language == lang).collect();
    if applicable.is_empty() {
        return Err(format!("no rule in the file applies to {lang}"));
    }
    let source =
        std::fs::read_to_string(target).map_err(|error| format!("input {target}: {error}"))?;
    let grep = lang.ast_grep(&source);
    let mut matches = Vec::new();
    for rule in applicable {
        for found in grep.root().find_all(&rule.matcher) {
            matches.push(json!({
                "rule": rule.id,
                "line": found.start_pos().line() + 1,
                "text": found.text(),
            }));
        }
    }
    let status = if matches.is_empty() { "clean" } else { "fired" };
    Ok(json!({"status": status, "file": target, "matches": matches}))
}
