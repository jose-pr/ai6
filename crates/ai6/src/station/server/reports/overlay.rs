//! What a Run reads of its overlays' knowledge: `kb_list`, `kb_read` and `kb_search`.
//!
//! The files, their templates and the answers are coqui-aos's: this module renders nothing. It
//! hands coqui-aos the Run's overlays and who is asking, and ranks `kb_search`'s files with the
//! Report search, so a passage ranks as a Report of the same words does.

use std::sync::Arc;

use coqui_aos::{
    CallerFacts, KnowledgeOverlay, KnowledgeSession, KnowledgeSettings, Passage, Renderers,
    SearchAnswer, SearchBackend, SearchFailure, SearchMode, SearchRequest, ToolCall,
    knowledge_lines, served_texts,
};
use serde_json::Value;

use super::ReportDesk;
use crate::directorates::m::guides;
use crate::directorates::registry::knowledge::{ReportQuery, matching};
use crate::directorates::registry::reports::Report;
use crate::station::overlays::RunKnowledge;

/// A Run's overlays and the bounds on what they serve.
#[derive(Clone, Default)]
pub(in crate::station::server) struct OverlayKnowledge {
    overlays: Arc<Vec<KnowledgeOverlay>>,
    settings: KnowledgeSettings,
}

impl OverlayKnowledge {
    /// Whether no overlay serves anything, so the tools have nothing to answer.
    pub(in crate::station::server) fn is_empty(&self) -> bool {
        self.overlays.is_empty()
    }

    /// The answer to `tool` (`kb_list`, `kb_read` or `kb_search`) called with `arguments` by
    /// `caller`: the text of coqui-aos's result, which says why when it refused.
    pub(in crate::station::server) fn call(
        &self,
        tool: &str,
        arguments: Value,
        caller: &CallerFacts,
    ) -> String {
        let search = ReportSearch {
            settings: &self.settings,
            caller,
            engines: Renderers::all(),
        };
        let session = KnowledgeSession::new(&self.overlays, &self.settings, caller)
            .search(&search)
            .engines(Renderers::all());
        session.call(&ToolCall::new(tool, arguments)).text()
    }
}

impl ReportDesk {
    /// This desk with the knowledge of a Run's overlays, which its Report tools then serve as
    /// `kb_list`, `kb_read` and `kb_search`.
    #[must_use]
    pub(in crate::station::server) fn with_knowledge(mut self, knowledge: RunKnowledge) -> Self {
        self.knowledge = OverlayKnowledge {
            overlays: Arc::new(knowledge.overlays),
            settings: knowledge.settings,
        };
        self
    }

    /// The Run's overlay knowledge.
    pub(in crate::station::server) fn overlay_knowledge(&self) -> &OverlayKnowledge {
        &self.knowledge
    }
}

/// `kb_search` over the files an overlay serves, ranked as the Report search ranks Reports.
struct ReportSearch<'a> {
    settings: &'a KnowledgeSettings,
    caller: &'a CallerFacts,
    engines: Renderers,
}

impl SearchBackend for ReportSearch<'_> {
    fn modes(&self) -> Vec<SearchMode> {
        vec![SearchMode::Words]
    }

    fn search(
        &self,
        overlay: &KnowledgeOverlay,
        request: &SearchRequest,
    ) -> Result<SearchAnswer, SearchFailure> {
        if request.mode.is_some_and(|mode| mode != SearchMode::Words) {
            return Err(SearchFailure::mode_unavailable(
                "only a search by the words used is offered",
            ));
        }
        let texts = served_texts(overlay, self.settings, self.caller, &self.engines);
        // Every served file is searched, whatever directory it is in; only the words rank it.
        let files: Vec<Report> = texts
            .iter()
            .filter_map(|(path, text)| guides::report_of(path.clone(), &format!("kb/{path}"), text))
            .collect();
        let query = ReportQuery::words(&request.query);
        let ranked = matching(files, &query);
        let count = ranked.len();
        let passages = ranked
            .into_iter()
            .enumerate()
            .filter_map(|(rank, file)| {
                let text = texts.get(&file.id)?;
                let lines = knowledge_lines(text);
                let (last, shown) = self.head(&lines);
                Some(Passage::new(
                    file.id,
                    file.title,
                    1,
                    last,
                    (count - rank) as f64,
                    shown,
                ))
            })
            .collect();
        Ok(SearchAnswer::new(SearchMode::Words, passages))
    }
}

impl ReportSearch<'_> {
    /// The first lines of a file that fit one `kb_read`, with the number of the last: a passage
    /// is the start of the file a search ranked, which `kb_read` then reads on from.
    fn head(&self, lines: &[&str]) -> (usize, String) {
        let mut shown = String::new();
        let mut last = 0;
        for line in lines {
            let next = shown.len() + line.len() + 1;
            if last > 0 && !self.settings.max_read_bytes.allows(next as u64) {
                break;
            }
            shown.push_str(line);
            shown.push('\n');
            last += 1;
        }
        (last.max(1), shown.trim_end_matches('\n').to_owned())
    }
}
