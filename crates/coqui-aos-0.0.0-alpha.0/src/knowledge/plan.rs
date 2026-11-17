//! The `[knowledge] serve` hint: which files a consumer leaves out of the context because the
//! tools serve them.

use alloc::string::String;
use alloc::vec::Vec;

use serde_json::Value;

use crate::defaults::{OverlayTree, context_of, rules_of};
use crate::read::Document;

/// Whether an overlay is served in place of its `rules` and `context` files, and which files that
/// leaves out of the context.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct KnowledgePlan {
    /// Whether the overlay is served.
    pub served: bool,
    /// The `rules` and `context` files left out of the context, in order, once each.
    pub left_out: Vec<String>,
}

/// The plan for `manifest` over `tree`. The user's choice wins over the overlay's hint; the body of
/// `OVERLAY.md` is never left out, since no tool serves it.
#[must_use]
pub fn knowledge_plan(
    manifest: &Document,
    tree: &OverlayTree,
    user: Option<bool>,
) -> KnowledgePlan {
    let hint = manifest
        .get("knowledge")
        .and_then(|table| table.get("serve"))
        == Some(&Value::Bool(true));
    let served = user.unwrap_or(hint);
    let mut left_out: Vec<String> = Vec::new();
    if served {
        for path in rules_of(manifest, tree)
            .into_iter()
            .chain(context_of(manifest, tree))
        {
            if path != "OVERLAY.md" && !left_out.contains(&path) {
                left_out.push(path);
            }
        }
    }
    KnowledgePlan { served, left_out }
}
