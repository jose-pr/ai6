//! Redaction: removing named values and patterns from a history, only when asked, and saying what
//! was removed.

use std::time::Instant;

use regex::{Regex, RegexBuilder};
use serde::Serialize;
use serde_json::Value;

use crate::header::{Header, Redacted};
use crate::settings::{Bounds, PARSER_DEPTH, RedactionSettings};
use crate::{Body, Document, HistoryError, Line, strict};

/// The keys of an update that the format pairs lines by.
const UPDATE_KEPT: [&str; 2] = ["sessionUpdate", "toolCallId"];
/// The key of a permission request's call that the format pairs lines by.
const CALL_KEPT: [&str; 1] = ["toolCallId"];
/// The key of a permission option, and of an answer's outcome, that the format pairs lines by.
const OPTION_KEPT: [&str; 1] = ["optionId"];

/// The largest compiled pattern, in bytes.
const PATTERN_SIZE_LIMIT: usize = 1 << 20;

/// A redaction ran out of the time it was given.
struct Spent;

/// Which list a rule came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub enum RuleKind {
    /// An entry of `values`.
    Value,
    /// An entry of `patterns`.
    Pattern,
}

/// What one rule removed. It names the rule by its place in the settings, never by its text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct RuleReport {
    /// The list the rule is in.
    pub kind: RuleKind,
    /// Its position in that list.
    pub index: usize,
    /// How many removals it made.
    pub replaced: u64,
    /// In how many lines, the header counting as one.
    pub lines: u64,
}

/// What a redaction removed.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct RedactionReport {
    /// Removals in all.
    pub replaced: u64,
    /// Lines changed, the header counting as one.
    pub lines: u64,
    /// Each rule, in the order the rules ran.
    pub rules: Vec<RuleReport>,
}

/// Removes what [`RedactionSettings`] names from a header and its lines. Its `Debug` output shows
/// how many rules it holds and none of their text.
///
/// Strings are changed wherever they are in a payload, except the identifiers the format pairs
/// lines by, and those only in the place that pairs them: an update's `sessionUpdate` and
/// `toolCallId`, a permission request's `toolCall.toolCallId` and each option's `optionId`, an
/// answer's `outcome.optionId`. The same key anywhere else (inside `rawInput`, in `_meta`) is
/// changed like any text.
///
/// Every occurrence of a value or a match of a pattern is found in the original text, and the
/// stretches they cover are replaced once, so a value inside a longer one, or one that cuts a
/// pattern's match, leaves none of the longer text behind. A removal that leaves a line the
/// format refuses (a pattern that matches `end_turn`, say) is an error and changes nothing in
/// that line. Keys, the `method` and the time of a line are never touched.
pub struct Redactor {
    values: Vec<String>,
    value_slots: Vec<usize>,
    patterns: Vec<Regex>,
    pattern_slots: Vec<usize>,
    replacement: String,
    report: RedactionReport,
    /// Matches each rule found since the last commit to the report.
    hits: Vec<u64>,
    /// Replacements made since the last commit: matches that overlap are replaced once.
    replaced: u64,
}

impl core::fmt::Debug for Redactor {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Redactor")
            .field("values", &self.values.len())
            .field("patterns", &self.patterns.len())
            .field("report", &self.report)
            .finish_non_exhaustive()
    }
}

impl Redactor {
    /// A redactor for `settings`. An empty entry of `values` removes nothing.
    ///
    /// # Errors
    ///
    /// [`HistoryError::Pattern`] for a pattern that is not a regular expression, is too large, or
    /// matches the empty string (it would match everywhere).
    pub fn try_new(settings: &RedactionSettings) -> Result<Self, HistoryError> {
        let mut report = RedactionReport::default();
        let mut values = Vec::new();
        let mut value_slots = Vec::new();
        for (index, value) in settings.values.iter().enumerate() {
            if value.is_empty() {
                continue;
            }
            value_slots.push(report.rules.len());
            report.rules.push(RuleReport {
                kind: RuleKind::Value,
                index,
                replaced: 0,
                lines: 0,
            });
            values.push(value.clone());
        }
        let mut patterns = Vec::new();
        let mut pattern_slots = Vec::new();
        for (index, pattern) in settings.patterns.iter().enumerate() {
            let regex = RegexBuilder::new(pattern)
                .size_limit(PATTERN_SIZE_LIMIT)
                .nest_limit(64)
                .build()
                .map_err(|_| HistoryError::Pattern { index })?;
            if regex.is_match("") {
                return Err(HistoryError::Pattern { index });
            }
            pattern_slots.push(report.rules.len());
            report.rules.push(RuleReport {
                kind: RuleKind::Pattern,
                index,
                replaced: 0,
                lines: 0,
            });
            patterns.push(regex);
        }
        let hits = vec![0; report.rules.len()];
        Ok(Self {
            values,
            value_slots,
            patterns,
            pattern_slots,
            replacement: settings.replacement.clone(),
            report,
            hits,
            replaced: 0,
        })
    }

    /// What has been removed so far.
    #[must_use]
    pub fn report(&self) -> &RedactionReport {
        &self.report
    }

    /// What the header says of the redaction so far.
    #[must_use]
    pub fn mark(&self) -> Redacted {
        Redacted::new(self.report.rules.len() as u64, self.report.replaced)
    }

    /// `text` with every stretch a value or a pattern matches replaced, or `None` when none is.
    fn text(&mut self, text: &str) -> Option<String> {
        self.text_until(text, None).unwrap_or(None)
    }

    /// As `text`, giving up with `Err` once `until` has passed, between matches and between rules.
    fn text_until(&mut self, text: &str, until: Option<Instant>) -> Result<Option<String>, Spent> {
        let late = || until.is_some_and(|until| Instant::now() >= until);
        let mut spans: Vec<(usize, usize)> = Vec::new();
        for (rule, value) in self.values.iter().enumerate() {
            let slot = self.value_slots[rule];
            let mut from = 0;
            while let Some(at) = text[from..].find(value.as_str()) {
                if late() {
                    self.forget();
                    return Err(Spent);
                }
                let start = from + at;
                spans.push((start, start + value.len()));
                self.hits[slot] += 1;
                // One character on, so an occurrence that overlaps this one is found too.
                from = start + text[start..].chars().next().map_or(1, char::len_utf8);
            }
        }
        for (rule, regex) in self.patterns.iter().enumerate() {
            let slot = self.pattern_slots[rule];
            if late() {
                self.hits.iter_mut().for_each(|hits| *hits = 0);
                self.replaced = 0;
                return Err(Spent);
            }
            for found in regex.find_iter(text) {
                spans.push((found.start(), found.end()));
                self.hits[slot] += 1;
                if late() {
                    self.hits.iter_mut().for_each(|hits| *hits = 0);
                    self.replaced = 0;
                    return Err(Spent);
                }
            }
        }
        if spans.is_empty() {
            return Ok(None);
        }
        spans.sort_unstable();
        let mut out = String::with_capacity(text.len());
        let mut copied = 0;
        let (mut start, mut end) = spans[0];
        for &(next_start, next_end) in &spans[1..] {
            if next_start < end {
                end = end.max(next_end);
                continue;
            }
            out.push_str(&text[copied..start]);
            out.push_str(&self.replacement);
            self.replaced += 1;
            copied = end;
            (start, end) = (next_start, next_end);
        }
        out.push_str(&text[copied..start]);
        out.push_str(&self.replacement);
        self.replaced += 1;
        out.push_str(&text[end..]);
        Ok(Some(out))
    }

    /// `text` redacted, or `None` once `until` has passed. The removals are counted in the report
    /// when `count` is set.
    pub(crate) fn redact_until(
        &mut self,
        text: &str,
        until: Instant,
        count: bool,
    ) -> Option<String> {
        let out = self.text_until(text, Some(until)).ok()?;
        if count {
            self.settle();
        } else {
            self.forget();
        }
        Some(out.unwrap_or_else(|| text.to_owned()))
    }

    /// `text` with every stretch a value or a pattern matches replaced, the removals counted in
    /// the report. For a caller that redacts text of its own making, not a history's lines.
    pub fn redact_text(&mut self, text: &str) -> String {
        let out = self.text(text);
        self.settle();
        out.unwrap_or_else(|| text.to_owned())
    }

    /// Redacts the strings under `root`; the keys `kept` of `root` itself, when their values are
    /// strings, stay as they are.
    fn walk(&mut self, root: &mut Value, kept: &[&str]) {
        let mut stack = vec![(root, true)];
        while let Some((value, top)) = stack.pop() {
            match value {
                Value::String(text) => {
                    if let Some(changed) = self.text(text) {
                        *text = changed;
                    }
                }
                Value::Array(items) => stack.extend(items.iter_mut().map(|item| (item, false))),
                Value::Object(map) => {
                    for (key, item) in map.iter_mut() {
                        if !(top && kept.contains(&key.as_str()) && item.is_string()) {
                            stack.push((item, false));
                        }
                    }
                }
                _ => {}
            }
        }
    }

    /// Commits the removals counted since the last commit to the report.
    fn settle(&mut self) {
        let mut changed = false;
        for (slot, hits) in self.hits.iter_mut().enumerate() {
            if *hits > 0 {
                let rule = &mut self.report.rules[slot];
                rule.replaced += *hits;
                rule.lines += 1;
                *hits = 0;
                changed = true;
            }
        }
        self.report.replaced += self.replaced;
        self.replaced = 0;
        if changed {
            self.report.lines += 1;
        }
    }

    /// Drops the removals counted since the last commit.
    fn forget(&mut self) {
        self.hits.iter_mut().for_each(|hits| *hits = 0);
        self.replaced = 0;
    }

    fn touched(&self) -> bool {
        self.replaced > 0
    }

    /// Redacts the header's text fields (not its `format`, `id`, time, digest or harness name).
    /// A changed header loses its digest; [`Document::seal`] makes a new one.
    ///
    /// # Errors
    ///
    /// The refusal a reader would give the redacted header; the header is left as it was.
    pub fn header(&mut self, header: &mut Header) -> Result<(), HistoryError> {
        header.to_line()?;
        let Ok(mut whole) = serde_json::to_value(&*header) else {
            return Ok(());
        };
        let kept = [
            "format",
            "version",
            "id",
            "createdAt",
            "digest",
            "acpVersion",
            "redacted",
        ];
        if let Value::Object(map) = &mut whole {
            for (key, item) in map.iter_mut() {
                if kept.contains(&key.as_str()) {
                    continue;
                }
                if let (true, Value::Object(origin)) = (key == "origin", &mut *item) {
                    for (field, text) in origin.iter_mut() {
                        if field != "harness" {
                            self.walk(text, &[]);
                        }
                    }
                } else {
                    self.walk(item, &[]);
                }
            }
        }
        if !self.touched() {
            return Ok(());
        }
        let checked =
            strict::parse(&whole.to_string(), 1, &Bounds::unbounded()).and_then(Header::from_value);
        match checked {
            Ok(mut changed) => {
                changed.digest = None;
                *header = changed;
                self.settle();
                Ok(())
            }
            Err(error) => {
                self.forget();
                Err(error)
            }
        }
    }

    /// Redacts a line, the one numbered `number`.
    ///
    /// # Errors
    ///
    /// The refusal a reader would give the line as redacted; the line is left as it was and its
    /// removals are not counted.
    pub fn line(&mut self, number: u64, line: &mut Line) -> Result<(), HistoryError> {
        if line.payload_depth() > PARSER_DEPTH {
            return line.to_json().map(|_| ());
        }
        let mut copy = line.clone();
        match &mut copy.body {
            Body::Prompt { prompt } => prompt.iter_mut().for_each(|block| self.walk(block, &[])),
            Body::Update { update } => self.walk(update, &UPDATE_KEPT),
            Body::PermissionRequest { tool_call, options } => {
                self.walk(tool_call, &CALL_KEPT);
                options
                    .iter_mut()
                    .for_each(|option| self.walk(option, &OPTION_KEPT));
            }
            Body::PermissionAnswer { outcome, .. } => self.walk(outcome, &OPTION_KEPT),
            Body::TurnEnd {
                error: Some(text), ..
            }
            | Body::Compaction {
                summary: Some(text),
                ..
            } => {
                if let Some(changed) = self.text(text) {
                    *text = changed;
                }
            }
            _ => {}
        }
        if let Some(meta) = &mut copy.meta {
            self.walk(meta, &[]);
        }
        if !self.touched() {
            return Ok(());
        }
        let checked = copy
            .to_json()
            .and_then(|text| strict::parse(&text, number, &Bounds::unbounded()))
            .and_then(|value| Line::from_value(value, number));
        match checked {
            Ok(_) => {
                *line = copy;
                self.settle();
                Ok(())
            }
            Err(error) => {
                self.forget();
                Err(error)
            }
        }
    }

    /// Redacts a whole document: its header, then each line, and when anything was removed marks
    /// the header with what was removed and drops its digest ([`Document::seal`] makes a new
    /// one). A document with nothing to remove is left exactly as it was. On an error the
    /// document is left as it was.
    ///
    /// # Errors
    ///
    /// The first line whose redaction the format refuses.
    pub fn document(&mut self, document: &mut Document) -> Result<RedactionReport, HistoryError> {
        let before = self.report.replaced;
        let mut header = document.header.clone();
        self.header(&mut header)?;
        let mut lines = document.lines.clone();
        for (at, line) in lines.iter_mut().enumerate() {
            self.line(at as u64 + 2, line)?;
        }
        if self.report.replaced > before {
            header.digest = None;
            header.redacted = Some(self.mark());
            document.header = header;
            document.lines = lines;
        }
        Ok(self.report.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_redaction_gives_up_when_its_time_has_passed_and_counts_nothing() {
        let settings = RedactionSettings {
            values: vec!["secret".into()],
            patterns: vec!["s[a-z]+t".into()],
            ..RedactionSettings::default()
        };
        let mut redactor = Redactor::try_new(&settings).unwrap();
        let text = "a secret here, and another secret";
        assert_eq!(redactor.redact_until(text, Instant::now(), true), None);
        assert_eq!(redactor.report().replaced, 0);
        let later = Instant::now() + core::time::Duration::from_secs(60);
        assert_eq!(
            redactor.redact_until(text, later, true).as_deref(),
            Some("a [redacted] here, and another [redacted]")
        );
        assert_eq!(redactor.report().replaced, 2);
    }
}
