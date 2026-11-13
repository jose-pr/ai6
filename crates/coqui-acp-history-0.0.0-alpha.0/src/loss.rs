//! The loss report: what an import could not carry into the target harness.

use serde::Serialize;

use guatiao::{FromValue, Schema, ToValue};

/// Signed thinking or encrypted reasoning the target cannot take, dropped.
pub const THINKING: &str = "thinking";
/// A tool call with no equivalent in the target, written as a plain note.
pub const TOOL_AS_NOTE: &str = "tool_as_note";
/// A content block whose bytes the history left out, known only by its digest.
pub const ATTACHMENT_BY_DIGEST: &str = "attachment_by_digest";
/// The target has no transcript of its own: the history was replayed to it as a prompt.
pub const REPLAYED_AS_PROMPT: &str = "replayed_as_prompt";
/// Turns of the history left out of a replay to fit its size limit; counted in items.
pub const HISTORY_CUT: &str = "history_cut";
/// One item of a replayed history cut short to fit its own limit.
pub const ITEM_CUT: &str = "item_cut";
/// Text a replay did not show because redacting it would have passed the redaction limits.
pub const REDACTION_LIMIT: &str = "redaction_limit";
/// A content block with no text (an image, audio) that a replay could only name.
pub const NON_TEXT_OMITTED: &str = "non_text_omitted";
/// The history's latest compaction boundary has no summary, so the seed starts earlier or at the
/// start.
pub const SUMMARY_MISSING: &str = "summary_missing";

/// One kind of thing an import could not carry, and how many.
#[derive(ToValue, FromValue, Schema, Serialize, Debug, Clone, PartialEq, Eq)]
#[map(crate = ::guatiao)]
#[non_exhaustive]
pub struct Loss {
    /// What was lost: [`THINKING`], [`TOOL_AS_NOTE`] and the other constants of this module. An
    /// open vocabulary; a consumer passes over a kind it does not know.
    pub kind: String,
    /// How many items.
    pub count: u64,
    /// What a person should know about it, never the lost content itself.
    pub detail: Option<String>,
}

impl Loss {
    /// `count` items of `kind`.
    #[must_use]
    pub fn new(kind: impl Into<String>, count: u64) -> Self {
        Self {
            kind: kind.into(),
            count,
            detail: None,
        }
    }

    /// The same loss with a `detail`.
    #[must_use]
    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }
}

/// The losses of one import, one entry for each kind.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct LossReport {
    losses: Vec<Loss>,
}

impl LossReport {
    /// A report of nothing lost.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Counts `count` more items of `kind`. A `detail` is kept only when the kind had none.
    pub fn add(&mut self, kind: &str, count: u64, detail: Option<&str>) {
        if count == 0 {
            return;
        }
        if let Some(loss) = self.losses.iter_mut().find(|loss| loss.kind == kind) {
            loss.count += count;
            if loss.detail.is_none() {
                loss.detail = detail.map(str::to_owned);
            }
        } else {
            self.losses.push(Loss {
                kind: kind.to_owned(),
                count,
                detail: detail.map(str::to_owned),
            });
        }
    }

    /// The losses, in the order each kind was first counted.
    #[must_use]
    pub fn losses(&self) -> &[Loss] {
        &self.losses
    }

    /// Whether nothing was lost.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.losses.is_empty()
    }

    /// The count of `kind`, 0 when none was lost.
    #[must_use]
    pub fn count(&self, kind: &str) -> u64 {
        self.losses
            .iter()
            .find(|loss| loss.kind == kind)
            .map_or(0, |loss| loss.count)
    }
}

impl IntoIterator for LossReport {
    type Item = Loss;
    type IntoIter = alloc::vec::IntoIter<Loss>;

    fn into_iter(self) -> Self::IntoIter {
        self.losses.into_iter()
    }
}

impl<'a> IntoIterator for &'a LossReport {
    type Item = &'a Loss;
    type IntoIter = core::slice::Iter<'a, Loss>;

    fn into_iter(self) -> Self::IntoIter {
        self.losses.iter()
    }
}

impl Extend<Loss> for LossReport {
    fn extend<T: IntoIterator<Item = Loss>>(&mut self, losses: T) {
        for loss in losses {
            self.add(&loss.kind, loss.count, loss.detail.as_deref());
        }
    }
}

impl FromIterator<Loss> for LossReport {
    fn from_iter<T: IntoIterator<Item = Loss>>(losses: T) -> Self {
        let mut report = Self::new();
        report.extend(losses);
        report
    }
}
