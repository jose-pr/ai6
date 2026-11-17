//! The index as a [`SearchBackend`]: `words` always, `meaning` where an embedder of the index's model
//! is given.

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use super::{Embedder, Error, Hit, Index, Mode, SearchOptions};
use crate::{
    KnowledgeOverlay, Passage, SearchAnswer, SearchBackend, SearchFailure, SearchMode,
    SearchRequest, normalize_name,
};

/// Searches the committed index of each overlay.
#[derive(Default)]
pub struct IndexSearch {
    indexes: BTreeMap<String, Index>,
    embedder: Option<Box<dyn Embedder>>,
}

impl core::fmt::Debug for IndexSearch {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("IndexSearch")
            .field("overlays", &self.indexes.keys().collect::<Vec<_>>())
            .field("meaning", &self.embedder.is_some())
            .finish()
    }
}

impl IndexSearch {
    /// A backend with no index.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Searches `index` for the overlay called `identity`.
    #[must_use]
    pub fn index(mut self, identity: &str, index: Index) -> Self {
        self.indexes.insert(normalize_name(identity), index);
        self
    }

    /// Searches by meaning too, with the vectors of `embedder`'s model, for each index made with it.
    #[must_use]
    pub fn embedder(mut self, embedder: Box<dyn Embedder>) -> Self {
        self.embedder = Some(embedder);
        self
    }

    fn meaning_of(&self, index: &Index) -> bool {
        self.embedder
            .as_deref()
            .is_some_and(|embedder| index.settings().model.as_deref() == Some(embedder.model()))
    }
}

fn passages(hits: Vec<Hit>) -> Vec<Passage> {
    hits.into_iter()
        .map(|hit| Passage {
            path: hit.path,
            heading: hit.heading,
            from_line: hit.from_line,
            to_line: hit.to_line,
            score: f64::from(hit.score),
            text: hit.text,
        })
        .collect()
}

impl SearchBackend for IndexSearch {
    fn modes(&self) -> Vec<SearchMode> {
        let mut modes = alloc::vec![SearchMode::Words];
        if self.indexes.values().any(|index| self.meaning_of(index)) {
            modes.push(SearchMode::Meaning);
        }
        modes
    }

    fn search(
        &self,
        overlay: &KnowledgeOverlay,
        request: &SearchRequest,
    ) -> Result<SearchAnswer, SearchFailure> {
        let Some(index) = self.indexes.get(&normalize_name(&overlay.identity)) else {
            return Err(SearchFailure::unavailable("the overlay has no index yet"));
        };
        let options = |mode| SearchOptions {
            top: request.top,
            min_score: None,
            path: request.path.clone(),
            mode,
        };
        let embedder = self.embedder.as_deref();
        let by_words = |note: Option<String>| {
            index
                .search(&request.query, &options(Mode::Lexical), None)
                .map(|hits| {
                    let answer = SearchAnswer::new(SearchMode::Words, passages(hits));
                    match note {
                        Some(note) => answer.note(note),
                        None => answer,
                    }
                })
                .map_err(|error| SearchFailure::unavailable(error.to_string()))
        };
        match request.mode {
            Some(SearchMode::Words) => by_words(None),
            Some(SearchMode::Meaning) => {
                if !self.meaning_of(index) {
                    return Err(SearchFailure::mode_unavailable(
                        "the index holds no vectors of the embedder's model",
                    ));
                }
                index
                    .search(&request.query, &options(Mode::Semantic), embedder)
                    .map(|hits| SearchAnswer::new(SearchMode::Meaning, passages(hits)))
                    .map_err(|error| match error {
                        Error::Embed(_) => SearchFailure::unavailable(error.to_string()),
                        _ => SearchFailure::mode_unavailable(error.to_string()),
                    })
            }
            Some(_) => Err(SearchFailure::mode_unavailable(
                "this backend has no such mode",
            )),
            None if self.meaning_of(index) => {
                match index.search(&request.query, &options(Mode::Semantic), embedder) {
                    Ok(hits) => Ok(SearchAnswer::new(SearchMode::Meaning, passages(hits))),
                    Err(error) => by_words(Some(alloc::format!("by words: {error}"))),
                }
            }
            None => by_words(None),
        }
    }
}
