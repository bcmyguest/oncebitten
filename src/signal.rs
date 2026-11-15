use crate::store::{NewEvent, StoredEvent};

pub const KIND_PROMPT: &str = "prompt";
pub const KIND_TOOL_OK: &str = "tool-ok";
pub const KIND_TOOL_ERROR: &str = "tool-error";

pub const TOOL_FAILURE: &str = "tool-failure";
pub const REPEAT_FAILURE: &str = "repeat-failure";
pub const TEST_FAIL_THEN_PASS: &str = "test-fail-then-pass";
pub const USER_CORRECTION: &str = "user-correction";
pub const REVERT: &str = "revert";

pub const DEFAULT_CORRECTION_LEXICON: &[&str] = &[
    "no,",
    "no.",
    "no!",
    "not that",
    "that's not",
    "that's not right",
    "not right",
    "that's wrong",
    "that is wrong",
    "that's incorrect",
    "incorrect",
    "wrong",
    "stop",
    "please stop",
    "don't",
    "don't do",
    "do not",
    "do not use",
    "please don't",
    "never",
    "never use",
    "always",
    "always use",
    "instead",
    "again",
    "i told you",
    "i already told you",
    "i said",
    "as i said",
    "i asked you",
    "i didn't ask",
    "why did you",
    "why didn't you",
    "you forgot",
    "you missed",
    "you keep",
    "you always",
    "you never",
    "every time",
    "every single time",
    "undo",
    "revert that",
    "roll that back",
    "that broke",
    "you broke",
    "still broken",
    "doesn't work",
    "does not work",
    "not working",
    "isn't working",
    "still not working",
    "still doesn't work",
    "from now on",
    "going forward",
    "remember to",
    "make sure",
    "the rule is",
];
const TEST_RUNNERS: &[&str] = &[
    "cargo test",
    "pytest",
    "npm test",
    "vitest",
    "jest",
    "go test",
];
const EDIT_TOOLS: &[&str] = &["Edit", "Write", "MultiEdit"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Signal {
    pub kind: &'static str,
    pub detail: String,
}

pub fn derive(event: &NewEvent, history: &[StoredEvent]) -> Vec<Signal> {
    derive_with(event, history, DEFAULT_CORRECTION_LEXICON)
}

/// Phrases in `lexicon` that `text` contains, lowercased, in lexicon order, once each.
pub fn matched_phrases<P: AsRef<str>>(text: &str, lexicon: &[P]) -> Vec<String> {
    let lowered = normalize_apostrophes(&text.to_lowercase());
    let mut found: Vec<String> = Vec::new();
    for phrase in lexicon {
        let phrase = normalize_apostrophes(&phrase.as_ref().trim().to_lowercase());
        if !phrase.is_empty() && contains_phrase(&lowered, &phrase) && !found.contains(&phrase) {
            found.push(phrase);
        }
    }
    found
}

pub fn derive_with<P: AsRef<str>>(
    event: &NewEvent,
    history: &[StoredEvent],
    lexicon: &[P],
) -> Vec<Signal> {
    let mut signals = Vec::new();
    match event.kind.as_str() {
        KIND_PROMPT => {
            let phrases = matched_phrases(&event.summary, lexicon);
            if !phrases.is_empty() {
                signals.push(signal(USER_CORRECTION, &phrases.join(", ")));
            }
        }
        KIND_TOOL_ERROR => {
            signals.push(signal(TOOL_FAILURE, &event.summary));
            if repeats_failure(event, history) {
                signals.push(signal(REPEAT_FAILURE, &event.summary));
            }
        }
        KIND_TOOL_OK if is_test_run(&event.summary) && last_test_run_failed(history) => {
            signals.push(signal(TEST_FAIL_THEN_PASS, &event.summary));
        }
        _ => {}
    }
    if is_revert(event, history) {
        signals.push(signal(REVERT, &event.summary));
    }
    signals
}

pub fn normalize_command(command: &str) -> String {
    command
        .split_whitespace()
        .map(normalize_token)
        .collect::<Vec<_>>()
        .join(" ")
}

fn signal(kind: &'static str, detail: &str) -> Signal {
    Signal {
        kind,
        detail: detail.to_string(),
    }
}

fn normalize_token(token: &str) -> String {
    if is_hex(token) {
        return "<hex>".into();
    }
    if token.contains('/') || token.contains('\\') {
        return "<path>".into();
    }
    let mut out = String::new();
    let mut in_digits = false;
    for ch in token.chars() {
        if ch.is_ascii_digit() {
            if !in_digits {
                out.push_str("<n>");
            }
            in_digits = true;
        } else {
            out.push(ch);
            in_digits = false;
        }
    }
    out
}

fn is_hex(token: &str) -> bool {
    let digits = token.strip_prefix("0x").unwrap_or(token);
    digits.len() >= 7 && digits.chars().all(|c| c.is_ascii_hexdigit())
}

fn normalize_apostrophes(text: &str) -> String {
    text.replace('\u{2019}', "'")
}

fn contains_phrase(text: &str, phrase: &str) -> bool {
    text.match_indices(phrase).any(|(start, _)| {
        let before = text[..start].chars().next_back();
        let after = text[start + phrase.len()..].chars().next();
        let ends_in_word = phrase
            .chars()
            .next_back()
            .is_some_and(char::is_alphanumeric);
        !before.is_some_and(char::is_alphanumeric)
            && !(ends_in_word && after.is_some_and(char::is_alphanumeric))
    })
}

fn is_test_run(command: &str) -> bool {
    let lowered = command.to_lowercase();
    TEST_RUNNERS
        .iter()
        .any(|runner| contains_phrase(&lowered, runner))
}

fn last_test_run_failed(history: &[StoredEvent]) -> bool {
    history
        .iter()
        .rev()
        .find(|event| is_tool_event(event) && is_test_run(&event.summary))
        .is_some_and(|event| event.kind == KIND_TOOL_ERROR)
}

fn repeats_failure(event: &NewEvent, history: &[StoredEvent]) -> bool {
    let normalized = normalize_command(&event.summary);
    history.iter().any(|past| {
        past.kind == KIND_TOOL_ERROR
            && past.tool == event.tool
            && normalize_command(&past.summary) == normalized
    })
}

fn is_revert(event: &NewEvent, history: &[StoredEvent]) -> bool {
    if event.tool.as_deref() != Some("Bash") {
        return false;
    }
    let command = event.summary.as_str();
    if contains_phrase(command, "git revert") {
        return true;
    }
    let restores =
        contains_phrase(command, "git checkout --") || contains_phrase(command, "git restore");
    restores && touches_edited_file(command, history)
}

fn touches_edited_file(command: &str, history: &[StoredEvent]) -> bool {
    let edited: Vec<&str> = history
        .iter()
        .filter(|past| past.kind == KIND_TOOL_OK)
        .filter(|past| {
            past.tool
                .as_deref()
                .is_some_and(|tool| EDIT_TOOLS.contains(&tool))
        })
        .map(|past| past.summary.as_str())
        .collect();
    command.split_whitespace().any(|token| {
        edited
            .iter()
            .any(|path| token == *path || token.ends_with(&format!("/{path}")))
    })
}

fn is_tool_event(event: &StoredEvent) -> bool {
    event.kind == KIND_TOOL_OK || event.kind == KIND_TOOL_ERROR
}
