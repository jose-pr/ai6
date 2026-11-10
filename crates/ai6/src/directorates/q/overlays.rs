//! Overlays: directories of skills, rules, context, programs and environment that a loadout
//! and an Operation issue to an Agent's Sessions, in the Agent Overlay Spec's format. Reading,
//! validating, composing and the tree digest are the `coqui-aos` crate's; this module is what is
//! the host's: the `[overlay.<name>]` records, which overlays a Session is issued, the refusal
//! of hooks until AI6 runs them, and the checks on the skills it delivers.
//!
//! Nothing here runs a script, installs a package, starts a server or reads the network.
//!
//! An operator names each overlay once, with where it comes from and the tree digest it must
//! have:
//!
//! ```toml
//! [overlay.engineering]
//! source = "https://github.com/example/agents.git@4b825dc642cb6eb9a060e54bf8d69288fbee4904#overlays/engineering"
//! pin = "sha256:..."                 # what `ai6 overlay pin DIR` prints
//!
//! [overlay.rust]
//! path = "/srv/overlays/rust"
//! pin = "sha256:..."
//!
//! [loadout.007]
//! overlays = ["engineering"]
//! ```
//!
//! and an Operation adds its own (`overlays = ["rust"]`). [`issue_overlays`] turns the two
//! lists and each overlay's manifest into the overlays a Session gets, in AOS composition
//! order, or a refusal. An Operation's list only adds: it cannot take away what the loadout
//! issues.

mod skills;

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use coqui_aos::{
    Digest, Document, FileSystem, INTERPRETERS, Issue, NonDefaultSetting, OverlayTree, Source,
    StdFs, TreeDigest, applies, composition_order, hooks_of, normalize_name,
    read_overlay as read_aos, resolve, valid_overlay_name, validate,
};

pub use skills::{SKILL_INERT_KEYS, SkillSummary, check_skill, skill_description, skill_summary};

use crate::station::limits::OverlayLimits;

/// Why an overlay asks for hooks, which AI6 does not run: a `hooks` table in its manifest, or a
/// `hook.<ext>` script at its root. A `pre-tool` hook can deny a call, so an overlay is refused
/// rather than run without the guards its author wrote. `None` when it asks for none.
pub fn hooks_asked(manifest: &Document, files: &OverlayTree) -> Option<String> {
    if manifest.contains_key("hooks") {
        return Some("its manifest declares [hooks]".to_owned());
    }
    let interpreters: Vec<String> = INTERPRETERS.iter().map(|name| (*name).to_owned()).collect();
    ["windows", "linux"].into_iter().find_map(|os| {
        let (hooks, not_run) = hooks_of(manifest, files, os, &interpreters);
        hooks
            .into_iter()
            .find_map(|hook| hook.path)
            .or_else(|| not_run.into_iter().next())
            .map(|path| format!("it ships {path}"))
    })
}

/// A setting an overlay's delivery reports each time it is issued, weaker than the default or
/// one the host found: an AOS non-default setting (`unpinned`, `env-script`, ...) or one of the
/// host's own (`read-only-not-enforced`, `skill-collision`). Never a value.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Weakening {
    /// The identifier.
    pub id: String,
    /// What it is about: a variable, a path, an overlay or a site; empty for `unpinned`.
    pub subject: String,
}

impl From<&NonDefaultSetting> for Weakening {
    fn from(setting: &NonDefaultSetting) -> Self {
        Self {
            id: setting.id.as_str().to_owned(),
            subject: setting.subject.clone(),
        }
    }
}

/// Where an overlay comes from.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OverlayFrom {
    /// An AOS source, `[git+]<location>[@<ref>][#<path>]`.
    Source(Source),
    /// A directory on the Station, absolute or relative to the configuration directory.
    Path(PathBuf),
}

/// One overlay a loadout or an Operation may issue: an `[overlay.<name>]` table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverlayRecord {
    name: String,
    from: OverlayFrom,
    pin: Option<Digest>,
    secrets: Vec<String>,
}

impl OverlayRecord {
    /// An overlay named `name` (its identity once normalized) from `from`, unpinned and needing
    /// no secret to fetch.
    pub fn new(name: impl Into<String>, from: OverlayFrom) -> Self {
        Self {
            name: name.into(),
            from,
            pin: None,
            secrets: Vec::new(),
        }
    }

    /// The record with the tree digest its files must have.
    #[must_use]
    pub fn pinned(mut self, pin: Digest) -> Self {
        self.pin = Some(pin);
        self
    }

    /// The record with the `[secret.<name>]` names its source is fetched with.
    #[must_use]
    pub fn with_secrets(mut self, secrets: Vec<String>) -> Self {
        self.secrets = secrets;
        self
    }

    /// The name as the record's key writes it.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The identity: the name normalized, as AOS compares overlays.
    pub fn identity(&self) -> String {
        normalize_name(&self.name)
    }

    /// Where it comes from.
    pub fn from(&self) -> &OverlayFrom {
        &self.from
    }

    /// The tree digest its files must have; `None` only with `[overlays] unpinned`.
    pub fn pin(&self) -> Option<Digest> {
        self.pin
    }

    /// The secrets a private source is fetched with, by name; never their values.
    pub fn secrets(&self) -> &[String] {
        &self.secrets
    }
}

/// An overlay directory as read: its tree digest, its manifest and what is wrong with it.
/// Reading runs nothing.
#[derive(Debug, Clone, PartialEq)]
pub struct ReadOverlay {
    /// The tree digest and each member's.
    pub digest: TreeDigest,
    tree: OverlayTree,
    /// The manifest, empty when the overlay has none.
    pub manifest: Document,
    /// The problems with the manifest, sorted; those under `/about` do not stop it applying.
    pub issues: Vec<Issue>,
    /// The skills it offers, each checked as every delivered `SKILL.md` is, with the reason a
    /// skill fails.
    pub skills: Vec<(String, Result<(), String>)>,
}

impl ReadOverlay {
    /// The overlay's files, with the text of its manifest.
    pub fn tree(&self) -> OverlayTree {
        self.tree.clone()
    }

    /// Whether it may be issued: its manifest applies and every skill passes.
    pub fn usable(&self) -> bool {
        applies(&self.issues) && self.skills.iter().all(|(_, check)| check.is_ok())
    }
}

/// Reads the overlay at `root` within `limits`: the tree digest of its files, its manifest
/// (`OVERLAY.md` or `overlay.cfg`) validated against them, and the `SKILL.md` checks of each
/// skill it offers.
///
/// # Errors
///
/// Why the tree has no digest, why its manifest cannot be read, or that it asks for hooks.
pub fn read_overlay(root: &Path, limits: &OverlayLimits) -> Result<ReadOverlay, String> {
    let settings = limits.settings();
    let digest =
        coqui_aos::tree_digest(&StdFs, root, &settings).map_err(|error| error.to_string())?;
    let mut tree = OverlayTree::from_paths(digest.files.keys().cloned());
    let largest = settings.file_bytes.get().unwrap_or(u64::MAX);
    for name in ["OVERLAY.md", "overlay.cfg"] {
        if tree.contains(name) {
            let bytes = StdFs
                .read(&root.join(name), largest)
                .map_err(|error| format!("{name}: {error}"))?;
            tree = tree.with_bytes(name, bytes);
        }
    }
    let manifest = read_aos(&tree, &settings)
        .map_err(|error| error.to_string())?
        .manifest;
    if let Some(why) = hooks_asked(&manifest, &tree) {
        return Err(format!(
            "{why}: this AI6 does not run overlay hooks yet, so the overlay is refused rather than \
             run without them"
        ));
    }
    let issues = validate(&manifest, Some(&tree));
    let skills = coqui_aos::skills_of(&manifest, &tree)
        .into_iter()
        .map(|name| {
            let checked = check_skill(
                &root.join("skills").join(&name),
                &name,
                false,
                "a skill that grants tools",
                limits.file_bytes,
            );
            (name, checked)
        })
        .collect();
    Ok(ReadOverlay {
        digest,
        tree,
        manifest,
        issues,
        skills,
    })
}

/// Which list asked for an overlay.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverlayList {
    /// The Agent's loadout.
    Loadout,
    /// The Operation.
    Operation,
}

impl fmt::Display for OverlayList {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Loadout => "the loadout",
            Self::Operation => "the Operation",
        })
    }
}

/// Why a Session's overlays cannot be issued. Each names the overlay; none is a warning.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OverlayRefusal {
    /// A list names something that is no overlay identity.
    InvalidName {
        /// The list.
        list: OverlayList,
        /// The name as written.
        name: String,
    },
    /// A list names an overlay no `[overlay.<name>]` defines.
    Unknown {
        /// The list.
        list: OverlayList,
        /// The name as written.
        name: String,
    },
    /// An overlay requires one no `[overlay.<name>]` defines, or whose manifest was not read.
    Missing {
        /// The overlay that requires it; empty when a list named it.
        by: String,
        /// The identity required.
        name: String,
    },
    /// `requires` makes a cycle.
    Cycle(Vec<String>),
    /// An overlay has no pin and `[overlays] unpinned` is off.
    Unpinned(String),
}

impl fmt::Display for OverlayRefusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidName { list, name } => {
                write!(formatter, "{list} names {name:?}, which is no overlay name")
            }
            Self::Unknown { list, name } => {
                write!(
                    formatter,
                    "{list} names overlay {name:?}, which no [overlay.<name>] defines"
                )
            }
            Self::Missing { by, name } if by.is_empty() => {
                write!(formatter, "overlay {name:?} has no manifest read")
            }
            Self::Missing { by, name } => write!(
                formatter,
                "overlay {by:?} requires {name:?}, which no [overlay.<name>] defines"
            ),
            Self::Cycle(chain) => write!(
                formatter,
                "overlays require each other: {}",
                chain.join(" -> ")
            ),
            Self::Unpinned(name) => write!(
                formatter,
                "overlay {name:?} has no pin (`ai6 overlay pin DIR` prints it; \
                 station.overlays.unpinned = true issues it without one)"
            ),
        }
    }
}

impl std::error::Error for OverlayRefusal {}

/// One overlay a Session is issued.
#[derive(Debug, Clone, PartialEq)]
pub struct IssuedOverlay {
    /// Its identity.
    pub identity: String,
    /// Its record.
    pub record: OverlayRecord,
    /// Its manifest.
    pub manifest: Document,
}

/// Which overlay a digest on a launch's record is for.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct IssuedDigest {
    /// The overlay's identity.
    pub overlay: String,
    /// Its tree digest, `sha256:<hex>`.
    pub digest: String,
}

/// The overlays a Session is issued.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct IssuedOverlays {
    /// Each identity once, after what it requires: the order to fetch and prepare them in.
    pub install_order: Vec<String>,
    /// The overlays in AOS composition order, `(priority, name, identity)` ascending: a later
    /// one wins a conflict.
    pub overlays: Vec<IssuedOverlay>,
    /// What is reported each time they are issued: `unpinned` for each overlay with no pin.
    pub weakenings: Vec<Weakening>,
}

fn identities(
    list: OverlayList,
    names: &[String],
    records: &BTreeMap<String, OverlayRecord>,
) -> Result<Vec<String>, OverlayRefusal> {
    names
        .iter()
        .map(|name| {
            if !valid_overlay_name(name) {
                return Err(OverlayRefusal::InvalidName {
                    list,
                    name: name.clone(),
                });
            }
            let identity = normalize_name(name);
            if !records.contains_key(&identity) {
                return Err(OverlayRefusal::Unknown {
                    list,
                    name: name.clone(),
                });
            }
            Ok(identity)
        })
        .collect()
}

/// The overlays a Session gets: the loadout's list, then the Operation's, each overlay's
/// `requires` resolved first and each issued once, in composition order. `records` holds the
/// `[overlay.<name>]` tables by identity and `manifests` each one's manifest as read; a
/// required overlay must be in both. With `allow_unpinned` an overlay with no pin is issued
/// and reported; without, it is refused.
///
/// A Session must be complete, so a missing requirement refuses it rather than leaving the
/// overlay out.
///
/// # Errors
///
/// An [`OverlayRefusal`] naming the first name that is invalid or unknown, the first
/// requirement missing, a cycle, or an overlay with no pin.
pub fn issue_overlays(
    loadout: &[String],
    operation: &[String],
    records: &BTreeMap<String, OverlayRecord>,
    manifests: &BTreeMap<String, Document>,
    allow_unpinned: bool,
) -> Result<IssuedOverlays, OverlayRefusal> {
    let mut requested = identities(OverlayList::Loadout, loadout, records)?;
    requested.extend(identities(OverlayList::Operation, operation, records)?);
    let available: BTreeMap<String, Document> = manifests
        .iter()
        .filter(|(identity, _)| records.contains_key(*identity))
        .map(|(identity, manifest)| (identity.clone(), manifest.clone()))
        .collect();
    let resolution =
        resolve(&available, &requested).map_err(|cycle| OverlayRefusal::Cycle(cycle.0))?;
    if let Some((by, name)) = resolution.missing.first() {
        return Err(OverlayRefusal::Missing {
            by: by.clone(),
            name: name.clone(),
        });
    }
    let mut issued = IssuedOverlays::default();
    for identity in composition_order(&available, &resolution.install_order) {
        let record = records[&identity].clone();
        if record.pin.is_none() {
            if !allow_unpinned {
                return Err(OverlayRefusal::Unpinned(identity));
            }
            issued.weakenings.push(Weakening {
                id: "unpinned".to_owned(),
                subject: identity.clone(),
            });
        }
        issued.overlays.push(IssuedOverlay {
            manifest: available[&identity].clone(),
            identity,
            record,
        });
    }
    issued.install_order = resolution.install_order;
    Ok(issued)
}

#[cfg(test)]
mod tests;
