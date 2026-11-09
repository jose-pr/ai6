//! What an agent offers before any session: the options a session starts with, its modes, and the sessions it has.
//!
//! An agent's answer is untrusted, so it is read as ACP's own types and bounded by [`DescribeLimits`]: what cannot be
//! read is skipped, lists are cut at their limit, an item whose identifier is longer than the text limit is dropped,
//! since it could not be named back, and other text is cut there. Nothing in it is acted on.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use agent_client_protocol::schema::v1::{
    ListSessionsResponse, SessionConfigKind, SessionConfigOption, SessionConfigSelectOptions,
    SessionInfo, SessionModeState,
};
use serde_json::Value;

/// The `agentCapabilities._meta` key with which an agent says it answers [`SESSION_OPTIONS_METHOD`].
pub const SESSION_OPTIONS_CAPABILITY: &str = "dev.boriken.coqui.acp/session_options";

/// The request for what a session can be started with, before any session: `{}`, answered
/// `{"configOptions": [SessionConfigOption], "modes": SessionModeState, "unknown": {part: reason}}`, where a part the
/// agent cannot tell is left out and may say why under `unknown`.
pub const SESSION_OPTIONS_METHOD: &str = "_coqui_acp/session_options";

/// What [`Exchange::describe`](crate::Exchange::describe) keeps of an agent's answer, and how many pages of
/// `session/list` it asks for. Every limit takes any value, 0 included.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct DescribeLimits {
    /// The most options kept. Default 64.
    pub options: usize,
    /// The most values one option keeps, across its groups. Default 1024.
    pub values: usize,
    /// The most modes kept. Default 64.
    pub modes: usize,
    /// The most sessions kept, across pages. Default 500.
    pub sessions: usize,
    /// The longest text kept, in UTF-8 bytes: a longer identifier drops its item, other text is cut here. Default 4096.
    pub text: usize,
    /// The most `session/list` requests, each following the last page's `nextCursor`; 0 asks none. Default 10: the
    /// 500 sessions at 50 a page, and at most ten request timeouts spent on an agent that always gives a cursor.
    pub pages: usize,
}

impl Default for DescribeLimits {
    fn default() -> Self {
        Self::new()
    }
}

impl DescribeLimits {
    /// The defaults each field names.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            options: 64,
            values: 1024,
            modes: 64,
            sessions: 500,
            text: 4096,
            pages: 10,
        }
    }

    /// The most options kept.
    #[must_use]
    pub const fn options(mut self, options: usize) -> Self {
        self.options = options;
        self
    }

    /// The most values one option keeps, across its groups.
    #[must_use]
    pub const fn values(mut self, values: usize) -> Self {
        self.values = values;
        self
    }

    /// The most modes kept.
    #[must_use]
    pub const fn modes(mut self, modes: usize) -> Self {
        self.modes = modes;
        self
    }

    /// The most sessions kept, across pages.
    #[must_use]
    pub const fn sessions(mut self, sessions: usize) -> Self {
        self.sessions = sessions;
        self
    }

    /// The longest text kept, in UTF-8 bytes.
    #[must_use]
    pub const fn text(mut self, bytes: usize) -> Self {
        self.text = bytes;
        self
    }

    /// The most `session/list` requests.
    #[must_use]
    pub const fn pages(mut self, pages: usize) -> Self {
        self.pages = pages;
        self
    }
}

/// One part of what an agent offers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Answer<T> {
    /// What the agent said.
    Known(T),
    /// The agent offers the question and could not tell, for this reason, in its words.
    Unknown(String),
    /// The agent does not offer the question, so it was not asked.
    NotSupported,
}

/// What an agent offers before any session; see [`Exchange::describe`](crate::Exchange::describe).
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct Description {
    /// The options a session can start with, as ACP's `SessionConfigOption`s at the values a session starts with.
    /// A model is the option of category `model`, an effort the one of category `thought_level`.
    pub options: Answer<Vec<SessionConfigOption>>,
    /// The modes a session can start in.
    pub modes: Answer<SessionModeState>,
    /// The sessions the agent lists, from `session/list`'s pages in order.
    pub sessions: Answer<SessionList>,
}

/// The sessions an agent lists, and whether they are all it listed.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct SessionList {
    /// The sessions kept, in the agent's order.
    pub sessions: Vec<SessionInfo>,
    /// `None` when these are all the agent listed; otherwise why the list stops short: `a later page failed: <why>`,
    /// `the sessions limit was reached` or `the page limit was reached`.
    pub incomplete: Option<String>,
}

/// What one `session/list` page added, as [`sessions_page`] reads it.
pub(crate) struct Page {
    /// The next page's cursor; `None` when the agent gave none, or an empty one.
    pub(crate) next: Option<String>,
    /// The sessions limit left out a session this page held.
    pub(crate) cut: bool,
}

/// `text` cut to `max` bytes, on a character's boundary.
pub(crate) fn cut(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_string()
}

fn cut_in_place(text: &mut String, max: usize) {
    if text.len() > max {
        *text = cut(text, max);
    }
}

fn cut_optional(text: &mut Option<String>, max: usize) {
    if let Some(text) = text {
        cut_in_place(text, max);
    }
}

fn fits(id: &str, max: usize) -> bool {
    !id.is_empty() && id.len() <= max
}

/// One option, bounded, or `None` when it cannot be named.
fn option(mut option: SessionConfigOption, limits: &DescribeLimits) -> Option<SessionConfigOption> {
    let max = limits.text;
    if !fits(&option.id.0, max) {
        return None;
    }
    cut_in_place(&mut option.name, max);
    cut_optional(&mut option.description, max);
    option.meta = None;
    // A category is an identifier too, and a client does without one.
    if serde_json::to_value(&option.category)
        .ok()
        .and_then(|category| category.as_str().map(str::len))
        .is_some_and(|length| length > max)
    {
        option.category = None;
    }
    if let SessionConfigKind::Select(select) = &mut option.kind {
        if !fits(&select.current_value.0, max) {
            return None;
        }
        let mut left = limits.values;
        match &mut select.options {
            SessionConfigSelectOptions::Ungrouped(values) => {
                values.retain(|value| fits(&value.value.0, max));
                values.truncate(left);
                for value in values {
                    cut_in_place(&mut value.name, max);
                    cut_optional(&mut value.description, max);
                    value.meta = None;
                }
            }
            SessionConfigSelectOptions::Grouped(groups) => {
                groups.retain(|group| fits(&group.group.0, max));
                for group in groups.iter_mut() {
                    cut_in_place(&mut group.name, max);
                    group.meta = None;
                    group.options.retain(|value| fits(&value.value.0, max));
                    group.options.truncate(left);
                    left -= group.options.len();
                    for value in &mut group.options {
                        cut_in_place(&mut value.name, max);
                        cut_optional(&mut value.description, max);
                        value.meta = None;
                    }
                }
            }
            _ => return None,
        }
    }
    Some(option)
}

/// What an answer says of `part`, when it does not hold the part itself.
fn unknown(answer: &Value, part: &str, max: usize) -> Answer<Vec<SessionConfigOption>> {
    Answer::Unknown(why(answer, part, max))
}

fn why(answer: &Value, part: &str, max: usize) -> String {
    match answer.get("unknown").and_then(|unknown| unknown.get(part)) {
        Some(Value::String(reason)) => cut(reason, max),
        _ => format!("the agent did not say what {part} it has"),
    }
}

/// The options part of a [`SESSION_OPTIONS_METHOD`] answer.
pub(crate) fn options(answer: &Value, limits: &DescribeLimits) -> Answer<Vec<SessionConfigOption>> {
    let Some(listed) = answer.get("configOptions").and_then(Value::as_array) else {
        return unknown(answer, "configOptions", limits.text);
    };
    Answer::Known(
        listed
            .iter()
            .filter_map(|item| serde_json::from_value::<SessionConfigOption>(item.clone()).ok())
            .filter_map(|item| option(item, limits))
            .take(limits.options)
            .collect(),
    )
}

/// The modes part of a [`SESSION_OPTIONS_METHOD`] answer.
pub(crate) fn modes(answer: &Value, limits: &DescribeLimits) -> Answer<SessionModeState> {
    let max = limits.text;
    let Some(modes) = answer.get("modes").filter(|modes| modes.is_object()) else {
        return Answer::Unknown(why(answer, "modes", max));
    };
    let Ok(mut modes) = serde_json::from_value::<SessionModeState>(modes.clone()) else {
        return Answer::Unknown("the agent's modes could not be read".to_string());
    };
    if !fits(&modes.current_mode_id.0, max) {
        return Answer::Unknown("the agent's modes could not be read".to_string());
    }
    modes.meta = None;
    modes.available_modes.retain(|mode| fits(&mode.id.0, max));
    modes.available_modes.truncate(limits.modes);
    for mode in &mut modes.available_modes {
        cut_in_place(&mut mode.name, max);
        cut_optional(&mut mode.description, max);
        mode.meta = None;
    }
    Answer::Known(modes)
}

/// Adds one `session/list` page's sessions to `kept`, up to the sessions limit; `None` when the page cannot be read.
pub(crate) fn sessions_page(
    answer: Value,
    limits: &DescribeLimits,
    kept: &mut Vec<SessionInfo>,
) -> Option<Page> {
    let listed = serde_json::from_value::<ListSessionsResponse>(answer).ok()?;
    let max = limits.text;
    let room = limits.sessions.saturating_sub(kept.len());
    let mut readable = listed.sessions.into_iter().filter(|session| {
        fits(&session.session_id.0, max) && fits(&session.cwd.to_string_lossy(), max)
    });
    kept.extend(readable.by_ref().take(room).map(|mut session| {
        session.additional_directories.clear();
        cut_optional(&mut session.title, max);
        cut_optional(&mut session.updated_at, max);
        session.meta = None;
        session
    }));
    Some(Page {
        next: listed.next_cursor.filter(|cursor| !cursor.is_empty()),
        cut: readable.next().is_some(),
    })
}
