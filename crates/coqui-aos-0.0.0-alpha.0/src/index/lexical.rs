//! Okapi BM25 over the passages: the scorer that needs no model.

use alloc::string::String;
use alloc::vec::Vec;
use std::collections::HashMap;

const K1: f32 = 1.2;
const B: f32 = 0.75;

/// The words of `text`: runs of letters and digits, lowercased.
pub(super) fn words(text: &str) -> impl Iterator<Item = String> + '_ {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_lowercase)
}

/// The passages' term statistics.
#[derive(Debug, Default)]
pub(super) struct Lexical {
    /// Per passage, how often each word occurs and how many words it holds.
    frequencies: Vec<(HashMap<String, u32>, u32)>,
    /// In how many passages each word occurs.
    documents: HashMap<String, u32>,
    average: f32,
}

impl Lexical {
    /// The statistics of `texts`, one per passage; each is what [`words`] reads of the passage (its path, heading and text).
    pub(super) fn new<'a>(texts: impl Iterator<Item = [&'a str; 3]>) -> Self {
        let mut frequencies = Vec::new();
        let mut documents: HashMap<String, u32> = HashMap::new();
        let mut total = 0u64;
        for parts in texts {
            let mut counts: HashMap<String, u32> = HashMap::new();
            let mut length = 0u32;
            for part in parts {
                for word in words(part) {
                    *counts.entry(word).or_insert(0) += 1;
                    length += 1;
                }
            }
            for word in counts.keys() {
                *documents.entry(word.clone()).or_insert(0) += 1;
            }
            total += u64::from(length);
            frequencies.push((counts, length));
        }
        #[allow(clippy::cast_precision_loss)]
        let average = if frequencies.is_empty() {
            0.0
        } else {
            total as f32 / frequencies.len() as f32
        };
        Self {
            frequencies,
            documents,
            average,
        }
    }

    /// The score of passage `index` for `query`, which is [`words`] of the query text; 0 when none of them occurs.
    #[allow(clippy::cast_precision_loss)]
    pub(super) fn score(&self, index: usize, query: &[String]) -> f32 {
        let Some((counts, length)) = self.frequencies.get(index) else {
            return 0.0;
        };
        let count = self.frequencies.len() as f32;
        let mut score = 0.0;
        for word in query {
            let Some(&frequency) = counts.get(word) else {
                continue;
            };
            let holding = self.documents.get(word).copied().unwrap_or(0) as f32;
            let idf = (1.0 + (count - holding + 0.5) / (holding + 0.5)).ln();
            let frequency = frequency as f32;
            let norm = 1.0 - B + B * (*length as f32 / self.average.max(1.0));
            score += idf * frequency * (K1 + 1.0) / (frequency + K1 * norm);
        }
        score
    }
}
