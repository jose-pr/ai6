//! Overlays: directories of skills, rules, context, programs and environment that a loadout
//! and an Operation issue to an Agent's Sessions, in the Agent Overlay Spec's format (AOS
//! edition `0.1.0-draft`, `Boriken-Dev/agent-overlay-spec`).
//!
//! This module is a consumer of the AOS classes Manifest reader, Validator, Environment,
//! Integrity and Resolver, with the setup phases and dependency environments of Runtime and
//! setup modelled as pure functions: nothing here runs a script, installs a package, starts a
//! server or reads the network. Its reader takes TOML, JSON and YAML, through [`anycfg`] over
//! guatiao's value. Catalogs, icons and `about` beyond validation (the Store class) are not
//! read.
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

mod apply;
mod codecs;
mod compose;
mod defaults;
mod digest;
mod names;
mod read;
mod skills;
mod source;
mod subst;
mod validate;

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

pub use apply::{
    Change, Effective, Environment, Facts, Needs, Phase, Preparation, Refusal, ScriptRun,
    SetupRecord, SetupRun, Weakening, composed_environment, effective, preparation, read_dotenv,
    setup_phase,
};
pub use compose::{
    Composition, Cycle, Environments, LeftEnvironment, PackageConflict, PackageEnvironment,
    Resolution, compose, composition_order, environments, resolve,
};
pub use defaults::{
    DEFAULT_KEYS, Dependency, INTERPRETERS, OverlayTree, PartialLock, check_requirements,
    context_of, default_on, default_paths, dependencies_of, env_files_of, paths_of, rules_of,
    skills_of,
};
pub use digest::{
    Digest, DigestError, DigestErrorKind, TreeDigest, TreeEntry, TreeLimits, digest_entries,
    tree_digest,
};
pub use names::{
    EXCLUDED_COMPONENTS, current_os, is_reserved_variable, is_sensitive_variable, is_variable_name,
    normalize_name, os_matches, root_variable, state_variable, valid_overlay_name,
    valid_relative_path,
};
pub use read::{
    Document, DotenvLine, ReadError, ReadLimits, ScriptOutput, parse_dotenv, read_document,
    read_env_script_output,
};
pub use skills::{SKILL_INERT_KEYS, check_skill, skill_description};
pub use source::{Source, SourceError, SourceKind};
pub use subst::{Lookup, SubstitutionError, substitute};
pub use validate::{Issue, applies, named_files, valid_license, validate};

use crate::station::limits::OverlayLimits;

/// The manifest's file name at an overlay's root.
pub const MANIFEST: &str = "overlay.cfg";

/// The file a later AOS edition reads as the manifest when there is no `overlay.cfg`: its front
/// matter the manifest, its body the context. This reader does not read it, so an overlay that has
/// it and no `overlay.cfg` is refused rather than read as having no manifest.
pub const LATER_MANIFEST: &str = "OVERLAY.md";

/// Why an overlay asks for hooks, which a later AOS edition runs: a `hooks` table in its manifest,
/// or a `hook.<ext>` script at its root, by the extensions a default script takes. A `pre-tool`
/// hook can deny a call, so an overlay is refused rather than run without the guards its author
/// wrote. `None` when it asks for none.
pub fn hooks_asked(manifest: &Document, files: &OverlayTree) -> Option<String> {
    if manifest.contains_key("hooks") {
        return Some(format!("{MANIFEST} declares [hooks]"));
    }
    defaults::DEFAULT_SCRIPTS
        .iter()
        .map(|(extension, _, _)| format!("hook{extension}"))
        .find(|path| files.contains(path))
        .map(|path| format!("it ships {path}"))
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

/// An overlay directory as read: its tree digest, its files, its manifest and what is wrong
/// with it. Reading runs nothing.
#[derive(Debug, Clone, PartialEq)]
pub struct ReadOverlay {
    /// The tree digest and each member's.
    pub digest: TreeDigest,
    /// The manifest, empty when the overlay has none.
    pub manifest: Document,
    /// The problems with the manifest, sorted; those under `/about` do not stop it applying.
    pub issues: Vec<Issue>,
    /// The skills it offers, each checked as every delivered `SKILL.md` is, with the reason a
    /// skill fails.
    pub skills: Vec<(String, Result<(), String>)>,
}

impl ReadOverlay {
    /// The overlay's files.
    pub fn tree(&self) -> OverlayTree {
        OverlayTree::from_paths(self.digest.files.keys().cloned())
    }

    /// Whether it may be issued: its manifest applies and every skill passes.
    pub fn usable(&self) -> bool {
        applies(&self.issues) && self.skills.iter().all(|(_, check)| check.is_ok())
    }
}

/// Reads the overlay at `root` within `limits`: the tree digest of its files, its
/// `overlay.cfg` validated against them, and the `SKILL.md` checks of each skill it offers.
///
/// # Errors
///
/// Why the tree has no digest, why its manifest cannot be read, or that its manifest is an
/// [`LATER_MANIFEST`] this reader does not read.
pub fn read_overlay(root: &Path, limits: &OverlayLimits) -> Result<ReadOverlay, String> {
    let digest = tree_digest(root, &limits.tree()).map_err(|error| error.to_string())?;
    if digest.files.contains_key(LATER_MANIFEST) && !digest.files.contains_key(MANIFEST) {
        return Err(format!(
            "{LATER_MANIFEST}: this AI6 does not read {LATER_MANIFEST} yet, and the overlay has no \
             {MANIFEST}; it is refused rather than read as having no manifest"
        ));
    }
    let manifest = if digest.files.contains_key(MANIFEST) {
        let bytes =
            std::fs::read(root.join(MANIFEST)).map_err(|error| format!("{MANIFEST}: {error}"))?;
        read_document(&bytes, &limits.manifest()).map_err(|error| format!("{MANIFEST}: {error}"))?
    } else {
        Document::new()
    };
    let tree = OverlayTree::from_paths(digest.files.keys().cloned());
    if let Some(why) = hooks_asked(&manifest, &tree) {
        return Err(format!(
            "{why}: this AI6 does not run overlay hooks yet, so the overlay is refused rather than \
             run without them"
        ));
    }
    let issues = validate(&manifest, Some(&tree));
    let skills = skills_of(&manifest, &tree)
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
#[cfg(test)]
mod vectors;
