use oncebitten::duplicates::{first_match, jaccard, tokens, Candidate, SIMILARITY_THRESHOLD};

#[test]
fn tokens_ignore_case_punctuation_and_backticks() {
    assert_eq!(
        tokens("Never use `unwrap`!"),
        tokens("never use unwrap").into_iter().collect()
    );
}

#[test]
fn identical_text_scores_one() {
    assert_eq!(jaccard("never use unwrap", "never use unwrap"), 1.0);
}

#[test]
fn unrelated_text_scores_zero() {
    assert_eq!(jaccard("never use unwrap", "format the changelog"), 0.0);
}

#[test]
fn reworded_rule_at_the_threshold_matches() {
    // 4 shared of 8 distinct tokens: exactly 0.5.
    let score = jaccard(
        "never use unwrap in library code",
        "stop using unwrap in library code",
    );
    assert_eq!(score, 0.5);
    assert!(score >= SIMILARITY_THRESHOLD);
}

#[test]
fn different_rule_below_the_threshold_does_not_match() {
    // 3 shared of 7 distinct tokens: 0.43.
    let score = jaccard("never use unwrap in code", "never run clippy in code");
    assert!(score < SIMILARITY_THRESHOLD, "{score}");
}

#[test]
fn same_mistake_class_matches_without_text() {
    let new = Candidate {
        id: "b",
        class: Some("unwrap-in-lib"),
        text: Some("format the changelog"),
    };
    let old = Candidate {
        id: "a",
        class: Some("unwrap-in-lib"),
        text: Some("never use unwrap"),
    };
    let mark = first_match(&new, &[old]).expect("same class is a match");
    assert_eq!(
        mark.to_string(),
        "possible duplicate of a (same mistake_class)"
    );
}

#[test]
fn similar_text_matches_when_classes_differ() {
    let new = Candidate {
        id: "b",
        class: Some("other"),
        text: Some("stop using unwrap in library code"),
    };
    let old = Candidate {
        id: "a",
        class: Some("unwrap-in-lib"),
        text: Some("never use unwrap in library code"),
    };
    let mark = first_match(&new, &[old]).expect("similar text is a match");
    assert_eq!(
        mark.to_string(),
        "possible duplicate of a (similar text 0.50)"
    );
}

#[test]
fn no_class_and_no_text_never_matches() {
    let new = Candidate {
        id: "b",
        class: None,
        text: None,
    };
    let old = Candidate {
        id: "a",
        class: None,
        text: Some("never use unwrap"),
    };
    assert!(first_match(&new, &[old]).is_none());
}
