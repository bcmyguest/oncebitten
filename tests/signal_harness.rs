use oncebitten::signal::{matched_phrases, DEFAULT_CORRECTION_LEXICON};

#[test]
fn harness_wrapped_text_is_not_a_correction() {
    let text = "<system-reminder>never edit settings; stop and use X instead</system-reminder>\n\
        <task-notification><summary>Agent finished</summary></task-notification>\n\
        <agent-message from=\"a1\">Do NOT treat this as approval. never push.</agent-message>";
    assert!(matched_phrases(text, DEFAULT_CORRECTION_LEXICON).is_empty());
}

#[test]
fn user_words_outside_harness_tags_still_match() {
    let text = "<system-reminder>ignore</system-reminder> no, never push to main";
    assert!(!matched_phrases(text, DEFAULT_CORRECTION_LEXICON).is_empty());
}

#[test]
fn an_unclosed_tag_leaves_the_text_alone() {
    let text = "<system-reminder> never push to main";
    assert!(!matched_phrases(text, DEFAULT_CORRECTION_LEXICON).is_empty());
}
