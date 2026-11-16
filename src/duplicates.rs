use std::collections::BTreeSet;
use std::fmt;

/// Jaccard score at or above which two texts count as possible duplicates.
/// 0.5 keeps a reworded rule ("never use unwrap" vs "stop using unwrap" in
/// the same scope) and drops rules that share only a word or two. A match is
/// a prompt to the user, who confirms, so the bar stays low.
pub const SIMILARITY_THRESHOLD: f64 = 0.5;

/// One proposal as the duplicate check sees it.
pub struct Candidate<'a> {
    pub id: &'a str,
    pub class: Option<&'a str>,
    pub text: Option<&'a str>,
}

/// A possible duplicate: the other proposal's id and the reason it matched.
#[derive(Debug, Clone, PartialEq)]
pub struct Mark {
    pub of: String,
    pub reason: String,
}

impl fmt::Display for Mark {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "possible duplicate of {} ({})", self.of, self.reason)
    }
}

/// Lowercased alphanumeric words of `text`. Punctuation and backticks split
/// words; no stop words are removed.
pub fn tokens(text: &str) -> BTreeSet<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// Jaccard similarity of the token sets: shared words over all distinct words.
/// Two texts with no words score 0.
pub fn jaccard(a: &str, b: &str) -> f64 {
    let (left, right) = (tokens(a), tokens(b));
    let union = left.union(&right).count();
    if union == 0 {
        return 0.0;
    }
    left.intersection(&right).count() as f64 / union as f64
}

/// The first of `others` that `new` may duplicate. A shared mistake class
/// wins over text; the text check runs only when the classes differ or
/// either is absent.
pub fn first_match(new: &Candidate, others: &[Candidate]) -> Option<Mark> {
    others.iter().find_map(|old| {
        let reason = if new.class.is_some() && new.class == old.class {
            "same mistake_class".to_string()
        } else {
            let score = jaccard(new.text?, old.text?);
            if score < SIMILARITY_THRESHOLD {
                return None;
            }
            format!("similar text {score:.2}")
        };
        Some(Mark {
            of: old.id.to_string(),
            reason,
        })
    })
}
