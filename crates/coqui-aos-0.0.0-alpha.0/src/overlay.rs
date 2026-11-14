//! What a gate reads from an overlay's files: the manifest from `OVERLAY.md` or `overlay.cfg`, the
//! context it names and the problems it has.

use alloc::collections::BTreeMap;

use crate::defaults::{OverlayTree, context_ignored, context_of, harness_context_of};
use crate::error::{Class, Error};
use crate::front_matter::read_front_matter;
use crate::kinds::ManifestFile;
use crate::limits::Settings;
use crate::read::{Document, read_document};
use crate::validate::validate;

/// An overlay as a gate reads it.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct OverlayRead {
    /// The file the manifest came from, `OVERLAY.md` or `overlay.cfg`; none when there is none.
    pub manifest_file: Option<ManifestFile>,
    /// The manifest, empty when there is none.
    pub manifest: Document,
    /// The context entries, in order.
    pub context: Vec<String>,
    /// The context files ignored because a body is the context.
    pub context_ignored: Vec<String>,
    /// The pointers of the manifest's problems, sorted.
    pub issues: Vec<String>,
    /// Each harness's own context, for the harnesses whose hints write one.
    pub harness_context: Option<BTreeMap<String, Vec<String>>>,
}

/// Reads the manifest and context of the overlay whose files `tree` holds, with the text of
/// `OVERLAY.md` and `overlay.cfg` where it has them.
///
/// # Errors
///
/// `two-manifests` for an overlay with both files, the reading class of a manifest that does not
/// read (its line is the file's), and, for a body that is context, `too-large` past the context
/// bound and `invalid-utf8`.
pub fn read_overlay(tree: &OverlayTree, settings: &Settings) -> Result<OverlayRead, Error> {
    let has = |name: &str| tree.contains(name);
    if has("OVERLAY.md") && has("overlay.cfg") {
        return Err(Error::new(
            Class::TwoManifests,
            "OVERLAY.md and overlay.cfg",
        ));
    }
    let manifest_file = if has("OVERLAY.md") {
        Some(ManifestFile::OverlayMd)
    } else if has("overlay.cfg") {
        Some(ManifestFile::OverlayCfg)
    } else {
        None
    };
    let text_of = |name: &str| tree.bytes(name).unwrap_or_default();
    let mut manifest = Document::new();
    let mut body: &[u8] = b"";
    let mut body_line = 1;
    match manifest_file {
        Some(ManifestFile::OverlayCfg) => {
            manifest = read_document(text_of("overlay.cfg"), settings)?;
        }
        Some(_) => {
            let data = text_of("OVERLAY.md");
            let read = read_front_matter(data, settings);
            if let Some(error) = read.error {
                return Err(error);
            }
            manifest = read.data.unwrap_or_default();
            body = &data[read.body_offset.unwrap_or(0).min(data.len())..];
            body_line = read.body_line.unwrap_or(1);
        }
        None => {}
    }
    let context = context_of(&manifest, tree);
    let hints = manifest.get("harness").and_then(|hints| hints.as_object());
    let mut named = context.clone();
    for (harness, entry) in hints.into_iter().flatten() {
        if entry.get("context").is_some() {
            named.extend(harness_context_of(&manifest, harness, tree));
        }
    }
    let body_is_named = named.iter().any(|entry| entry == "OVERLAY.md");
    let has_body = body
        .iter()
        .any(|byte| !matches!(byte, b' ' | b'\t' | b'\r' | b'\n'));
    if body_is_named && has_body {
        if !settings.context_bytes.allows(body.len()) {
            return Err(Error::new(Class::TooLarge, "the body of OVERLAY.md")
                .over("context_bytes")
                .on_line(body_line));
        }
        if core::str::from_utf8(body).is_err() {
            return Err(Error::new(Class::InvalidUtf8, "the body of OVERLAY.md").on_line(body_line));
        }
    }
    let issues = validate(&manifest, Some(tree))
        .into_iter()
        .map(|issue| issue.at)
        .collect();
    let harness_context = hints.map(|hints| {
        hints
            .iter()
            .filter(|(_, entry)| entry.get("context").is_some())
            .map(|(harness, _)| {
                (
                    harness.clone(),
                    harness_context_of(&manifest, harness, tree),
                )
            })
            .collect()
    });
    Ok(OverlayRead {
        manifest_file,
        context_ignored: context_ignored(&manifest, tree),
        manifest,
        context,
        issues,
        harness_context,
    })
}
