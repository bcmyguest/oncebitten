use std::path::Path;

use ast_grep_core::tree_sitter::LanguageExt;
use ast_grep_core::Language;
use ast_grep_language::SupportLang;
use oncebitten::engine::{astgrep, diff, Outcome};
use oncebitten::rule::read_diff_rule;
use serde_json::{json, Value};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let report = match args.as_slice() {
        [mode, rule, target] if mode == "diff" => {
            check_diff(Path::new(rule), target).unwrap_or_else(|reason| error(target, &reason))
        }
        [rule, target] => {
            check(Path::new(rule), target).unwrap_or_else(|reason| error(target, &reason))
        }
        _ => error(
            "",
            "usage: oncebitten-checker <rule.yml> <file> | diff <rule.toml> <file>",
        ),
    };
    println!("{report}");
}

fn error(file: &str, reason: &str) -> Value {
    json!({"status": "error", "file": file, "reason": reason})
}

fn check(rule_path: &Path, target: &str) -> Result<Value, String> {
    let shown = rule_path.display().to_string();
    let yaml =
        std::fs::read_to_string(rule_path).map_err(|error| format!("rule {shown}: {error}"))?;
    let rules = astgrep::parse_rules(&yaml, &shown)?;
    let lang = SupportLang::from_path(Path::new(target))
        .ok_or_else(|| format!("{target}: no language for this file extension"))?;
    if !rules.iter().any(|rule| rule.language == lang) {
        return Err(format!("no rule in the file applies to {lang}"));
    }
    let collection = astgrep::collection(rules, &shown)?;
    let applicable = astgrep::select(&collection, &astgrep::slashed(target));
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

/// `target` lists changed paths, one per line.
fn check_diff(rule_path: &Path, target: &str) -> Result<Value, String> {
    let shown = rule_path.display().to_string();
    let rule = read_diff_rule(rule_path)
        .map_err(|error| format!("{error:#}"))?
        .ok_or_else(|| format!("rule {shown} is not engine = \"diff\""))?;
    let text =
        std::fs::read_to_string(target).map_err(|error| format!("input {target}: {error}"))?;
    let changed: Vec<String> = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect();
    match diff::run(&rule.when_changed, &rule.require_changed, &changed) {
        Outcome::Clean => Ok(json!({"status": "clean", "file": target, "matches": []})),
        Outcome::Fired(findings) => {
            let matches: Vec<Value> = findings
                .iter()
                .map(|finding| {
                    json!({
                        "rule": rule.id,
                        "path": finding.path.as_ref().map(|path| path.display().to_string()),
                        "message": finding.message,
                    })
                })
                .collect();
            Ok(json!({"status": "fired", "file": target, "matches": matches}))
        }
        Outcome::CouldNotRun(reason) => Err(reason),
    }
}
