//! One file format for the history of an Agent Client Protocol session, so a session can move
//! between machines and between harnesses.
//!
//! A history is JSON Lines in ACP's own vocabulary: a header, then the client's prompts, the
//! agent's updates, cancels and permission answers in the order they happened, with turn
//! boundaries, stop reasons and compaction boundaries. The format, its versions and its bounds are
//! specified in `docs/session-history.md`; this crate is its reference implementation.
//!
//! - [`Reader`] streams a history under [`Bounds`] and refuses what the format does not define.
//! - [`Writer`] checks each line as the reader would, so it cannot write what a reader refuses.
//! - [`Document`] holds a whole history; [`Document::seed`] finds what a new session is seeded
//!   from.
//! - [`Redactor`] removes values and patterns, only when asked, and reports what it removed.
//! - [`tools`] is the table that names each harness's built-in tools once.
//! - [`LossReport`] is what an import could not carry.
//!
//! ```
//! use coqui_acp_history::{Document, Header, Line, Origin, Stop};
//! use coqui_acp_history::StopReason;
//!
//! let mut doc = Document::new(Header::new("0b2e9a4c-7a35-4d2f-9f0e-0f4f3f1f2a11", Origin::new("claude")));
//! doc.lines.push(Line::text_prompt("hello"));
//! doc.lines.push(Line::turn_end(Stop::Acp(StopReason::EndTurn)));
//! let text = doc.to_string()?;
//! assert_eq!(text.parse::<Document>()?, doc);
//! # Ok::<(), coqui_acp_history::HistoryError>(())
//! ```

#![forbid(unsafe_code)]

extern crate alloc;

mod canon;
pub mod conversation;
mod document;
mod error;
mod header;
mod line;
mod loss;
mod order;
pub mod paging;
pub mod patch;
mod reader;
pub mod record;
mod redact;
pub mod replay;
mod seed;
mod settings;
mod shape;
mod strict;
pub mod tools;
mod writer;

pub use agent_client_protocol::schema::v1::StopReason;
pub use document::Document;
pub use error::{HistoryError, OrderRule};
pub use header::{FORMAT, Header, Origin, Redacted, VERSION};
pub use line::{ATTACHMENT_META_KEY, Body, Line, NATIVE_META_KEY, Stop, Trigger};
pub use loss::{
    ATTACHMENT_BY_DIGEST, HISTORY_CUT, ITEM_CUT, Loss, LossReport, NON_TEXT_OMITTED,
    REDACTION_LIMIT, REPLAYED_AS_PROMPT, SUMMARY_MISSING, THINKING, TOOL_AS_NOTE,
};
pub use order::{Boundary, Summary};
pub use reader::{Entry, Reader, ReaderBuilder};
pub use redact::{RedactionReport, Redactor, RuleKind, RuleReport};
pub use seed::{Fit, Seed, SeedPoint};
pub use settings::{
    Bounds, DEFAULT_DOCUMENT_MAX_TOTAL_BYTES, DEFAULT_MAX_DEPTH, DEFAULT_MAX_LINE_BYTES,
    DEFAULT_MAX_LINES, DEFAULT_MAX_TOTAL_BYTES, DEFAULT_MAX_VALUES, DEFAULT_REPLACEMENT,
    NonDefault, PARSER_DEPTH, ReaderSettings, RedactionSettings, WriterSettings, non_defaults,
};
pub use tools::Tool;
pub use writer::{Writer, WriterBuilder};
