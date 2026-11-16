//! Prompt intent: cosine similarity between a prompt embedding and shipped example sets.

use std::collections::BTreeMap;

use anyhow::{bail, Context, Result};
use model2vec_rs::model::StaticModel;

pub const RULE_REQUEST: &str = "rule-request";
pub const CORRECTION: &str = "correction";
pub const FRUSTRATION: &str = "frustration";
pub const NEUTRAL: &str = "neutral";

/// Classes with example sets. `NEUTRAL` is the fallback when no example clears the threshold.
pub const SCORED_CLASSES: [&str; 3] = [RULE_REQUEST, CORRECTION, FRUSTRATION];

pub const SHIPPED_EXAMPLES: &str = include_str!("../intent/examples.toml");

pub trait Embed {
    fn embed(&self, text: &str) -> Vec<f32>;
}

impl Embed for StaticModel {
    fn embed(&self, text: &str) -> Vec<f32> {
        self.encode_single(text)
    }
}

#[derive(Debug, Clone)]
pub struct Example {
    pub class: &'static str,
    pub text: String,
    pub vector: Vec<f32>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Match {
    /// Index of the best example in [`Bank::examples`].
    pub example: usize,
    pub score: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Prediction {
    pub class: &'static str,
    pub score: f32,
    pub example: Option<usize>,
}

#[derive(Debug, Clone, Default)]
pub struct Bank {
    examples: Vec<Example>,
}

impl Bank {
    /// Parses `class = ["example", ...]` tables and embeds every example.
    pub fn from_toml(source: &str, embedder: &impl Embed) -> Result<Self> {
        let table: BTreeMap<String, Vec<String>> =
            toml::from_str(source).context("parsing intent examples")?;
        let mut examples = Vec::new();
        for (key, texts) in table {
            let Some(class) = SCORED_CLASSES.iter().copied().find(|class| *class == key) else {
                bail!("unknown intent class `{key}`");
            };
            for text in texts {
                let vector = embedder.embed(&text);
                examples.push(Example {
                    class,
                    text,
                    vector,
                });
            }
        }
        Ok(Self { examples })
    }

    pub fn examples(&self) -> &[Example] {
        &self.examples
    }

    /// The example with the highest cosine similarity to `query`. Ties keep the earlier example.
    pub fn best_match(&self, query: &[f32]) -> Option<Match> {
        let mut best: Option<Match> = None;
        for (example, entry) in self.examples.iter().enumerate() {
            let score = cosine(query, &entry.vector);
            if best.is_none_or(|current| score > current.score) {
                best = Some(Match { example, score });
            }
        }
        best
    }

    /// The class of the best match when its score reaches `threshold`, else `NEUTRAL`.
    pub fn classify(&self, query: &[f32], threshold: f32) -> Prediction {
        match self.best_match(query) {
            Some(found) if found.score >= threshold => Prediction {
                class: self.examples[found.example].class,
                score: found.score,
                example: Some(found.example),
            },
            Some(found) => Prediction {
                class: NEUTRAL,
                score: found.score,
                example: None,
            },
            None => Prediction {
                class: NEUTRAL,
                score: 0.0,
                example: None,
            },
        }
    }
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() {
        return 0.0;
    }
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }
    dot / (norm_a * norm_b)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixed(BTreeMap<&'static str, Vec<f32>>);

    impl Embed for Fixed {
        fn embed(&self, text: &str) -> Vec<f32> {
            self.0.get(text).cloned().unwrap_or_default()
        }
    }

    fn fixed() -> Fixed {
        Fixed(BTreeMap::from([
            ("rule", vec![1.0, 0.0]),
            ("fix", vec![0.0, 1.0]),
        ]))
    }

    fn bank() -> Bank {
        Bank::from_toml(
            "\"rule-request\" = [\"rule\"]\ncorrection = [\"fix\"]\n",
            &fixed(),
        )
        .expect("bank")
    }

    #[test]
    fn cosine_of_identical_vectors_is_one() {
        assert!((cosine(&[1.0, 2.0, 3.0], &[1.0, 2.0, 3.0]) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn cosine_of_orthogonal_vectors_is_zero() {
        assert!(cosine(&[1.0, 0.0], &[0.0, 1.0]).abs() < 1e-6);
    }

    #[test]
    fn cosine_with_zero_vector_is_zero() {
        assert_eq!(cosine(&[0.0, 0.0], &[1.0, 2.0]), 0.0);
    }

    #[test]
    fn bank_rejects_unknown_class() {
        let err = Bank::from_toml("other = [\"rule\"]\n", &fixed()).expect_err("unknown class");
        assert!(err.to_string().contains("unknown intent class"));
    }

    #[test]
    fn classify_returns_class_of_nearest_example() {
        let bank = bank();
        let prediction = bank.classify(&[0.1, 0.9], 0.5);
        assert_eq!(prediction.class, CORRECTION);
        let example = prediction.example.expect("matched example");
        assert_eq!(bank.examples()[example].text, "fix");
    }

    #[test]
    fn classify_falls_back_to_neutral_below_threshold() {
        let prediction = bank().classify(&[1.0, 1.0], 0.8);
        assert_eq!(prediction.class, NEUTRAL);
        assert_eq!(prediction.example, None);
        assert!((prediction.score - 0.707).abs() < 1e-3);
    }

    #[test]
    fn classify_on_empty_bank_is_neutral() {
        let prediction = Bank::default().classify(&[1.0], 0.0);
        assert_eq!(prediction.class, NEUTRAL);
        assert_eq!(prediction.score, 0.0);
    }

    #[test]
    fn shipped_examples_parse_with_every_scored_class() {
        let bank = Bank::from_toml(SHIPPED_EXAMPLES, &fixed()).expect("shipped examples parse");
        for class in SCORED_CLASSES {
            assert!(bank.examples().iter().any(|e| e.class == class), "{class}");
        }
    }
}
