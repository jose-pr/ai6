//! Skills from elsewhere (`spec/v0/skills.md`): what a consumer shows before installing the
//! skills of the active overlays and reports after, and the check of a skill that was fetched.
//! The core never fetches: the caller supplies a [`SkillSource`] to [`fetch_planned`], or fetches
//! on its own and calls [`read_fetched_skill`].

use alloc::collections::{BTreeMap, BTreeSet};

use crate::catalog::Catalog;
use crate::compose::{composition_order, resolve};
use crate::defaults::{OverlayTree, SkillOrigin, skill_entries};
use crate::digest::{Digest, TreeEntry, digest_entries};
use crate::error::{Class, Error};
use crate::front_matter::{Case, read_front_matter, split_front_matter};
use crate::kinds::Format;
use crate::limits::Settings;
use crate::names::normalize_name;
use crate::read::Document;
use crate::source::{Source, host_is_private, source_host};

/// What two overlays must share to name one skill without colliding: a bundled skill is its
/// overlay's own, one from elsewhere is its source in full, as fetched, and its pin.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Held {
    Bundled(String),
    Elsewhere(Option<String>, Option<String>),
}

/// The skills offered for delivery, by name without regard to ASCII case.
#[derive(Default)]
pub(crate) struct Delivery {
    winners: BTreeMap<String, (String, String, Held)>,
    collisions: BTreeSet<String>,
}

impl Delivery {
    /// Records one skill offered. A skill from elsewhere never takes a name another overlay's
    /// skill holds: it is reported, and only the user's configuration could choose it. Returns
    /// whether it was recorded.
    pub(crate) fn offer(&mut self, name: &str, identity: &str, bundled: bool, held: Held) -> bool {
        let folded = name.to_lowercase();
        if let Some((_, _, earlier)) = self.winners.get(&folded)
            && *earlier != held
        {
            self.collisions.insert(folded.clone());
            if !bundled {
                return false;
            }
        }
        self.winners
            .insert(folded, (name.to_owned(), identity.to_owned(), held));
        true
    }

    pub(crate) fn delivered(&self) -> BTreeMap<String, String> {
        self.winners
            .values()
            .map(|(name, identity, _)| (name.clone(), identity.clone()))
            .collect()
    }

    pub(crate) fn collisions(&self) -> Vec<String> {
        self.collisions.iter().cloned().collect()
    }

    fn holds(&self, name: &str, identity: &str) -> bool {
        self.winners
            .get(&name.to_lowercase())
            .is_some_and(|(_, holder, _)| holder == identity)
    }
}

/// Where a planned skill comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum PlannedOrigin {
    /// A `SKILL.md` in the overlay.
    Bundled,
    /// A table that names a source.
    Source,
    /// A name a catalog offered once.
    Catalog,
    /// A name no catalog offers.
    Missing,
    /// A name more than one catalog offers and no configured order decides.
    Ambiguous,
}

impl PlannedOrigin {}

/// One skill of an active overlay, with where it comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct PlannedSkill {
    /// The name as the overlay writes it.
    pub name: String,
    /// The overlay that offers it.
    pub overlay: String,
    /// Where it comes from.
    pub origin: PlannedOrigin,
    /// The source as shown, without user information or a query.
    pub source: Option<String>,
    /// The digest it must have.
    pub pin: Option<String>,
    /// The catalog that resolved it.
    pub catalog: Option<String>,
    /// The catalogs that all offer it, when ambiguous, sorted.
    pub catalogs: Option<Vec<String>>,
}

/// A skill the caller is to fetch, and check with [`read_fetched_skill`].
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Fetch {
    /// The skill's name.
    pub name: String,
    /// The overlay that offers it.
    pub overlay: String,
    /// Where to fetch it from.
    pub from: Source,
    /// The digest it must have.
    pub pin: Option<String>,
}

/// What a consumer shows before installing and reports after.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct SkillPlan {
    /// Every skill of the active overlays, in composition order and by name.
    pub skills: Vec<PlannedSkill>,
    /// `(name, overlay)` of each name no catalog offers.
    pub missing: Vec<(String, String)>,
    /// `(name, overlay, catalogs)` of each name more than one catalog offers.
    pub ambiguous: Vec<(String, String, Vec<String>)>,
    /// `(name, overlay)` of each skill from elsewhere with no pin.
    pub unpinned: Vec<(String, String)>,
    /// `(id, name, overlay)`: `insecure-transport` and `private-source`.
    pub non_defaults: Vec<(String, String, String)>,
    /// `(name, overlay)` of each skill from elsewhere a consumer that does not fetch leaves out.
    pub not_fetched: Vec<(String, String)>,
    /// `(name, overlay)` of each bundled skill whose `SKILL.md` names another skill.
    pub name_mismatch: Vec<(String, String)>,
    /// `(name, overlay)` of each skill from elsewhere kept from a name another skill holds.
    pub shadowing: Vec<(String, String)>,
    /// What is delivered, by name, and the overlay it comes from.
    pub delivered: BTreeMap<String, String>,
    /// The names, lower case, two skills of different origin share.
    pub collisions: Vec<String>,
    /// What the caller is to fetch: the delivered skills from elsewhere.
    pub to_fetch: Vec<Fetch>,
}

/// The `name` a bundled `SKILL.md` gives, when its front matter reads; none otherwise.
fn bundled_name(data: &[u8], settings: &Settings) -> Option<String> {
    let found = read_front_matter(data, settings);
    match found.data?.get("name")? {
        serde_json::Value::String(name) => Some(name.clone()),
        other => Some(other.to_string()),
    }
}

/// The catalog taken: the first in the user's order that offers the name; without one, all.
fn ranked<'a, T>(offers: Vec<(&'a str, T)>, rank: &[String]) -> Vec<(&'a str, T)> {
    for label in rank {
        if let Some(at) = offers
            .iter()
            .position(|(offered, _)| normalize_name(offered) == *label)
        {
            let mut offers = offers;
            return vec![offers.swap_remove(at)];
        }
    }
    offers
}

/// What a consumer shows before installing the skills of the active overlays and reports after:
/// each skill with where it comes from, each name no catalog offers or more than one does, the
/// non-default settings, the collisions and what each would do.
///
/// `catalogs` are the catalogs read, each with the label the consumer gave it; `order` is the
/// order of labels the user configured, if any; a consumer that does not `fetches` delivers
/// only bundled skills and says so of the rest. Where `trees` gives an overlay's texts, a bundled
/// `SKILL.md` naming another skill is not delivered.
///
/// # Errors
///
/// `cycle` when `requires` makes one, and `invalid-label` for a catalog label that is no overlay
/// name or two equal once normalized.
pub fn resolve_skills(
    overlays: &BTreeMap<String, Document>,
    requested: &[String],
    trees: &BTreeMap<String, OverlayTree>,
    catalogs: &[(&str, &Catalog)],
    order: &[&str],
    fetches: bool,
    settings: &Settings,
) -> Result<SkillPlan, Error> {
    let resolution = resolve(overlays, requested)
        .map_err(|cycle| Error::new(Class::Cycle, cycle.0.join(" -> ")))?;
    let active = composition_order(overlays, &resolution.install_order);
    crate::catalog::list_catalogs(catalogs)?;
    let rank: Vec<String> = order.iter().map(|label| normalize_name(label)).collect();
    let empty = OverlayTree::default();
    let mut plan = SkillPlan::default();
    let mut delivery = Delivery::default();
    for identity in &active {
        let tree = trees.get(identity).unwrap_or(&empty);
        for entry in skill_entries(&overlays[identity], tree) {
            let mut item = PlannedSkill {
                name: entry.name.clone(),
                overlay: identity.clone(),
                origin: match entry.origin {
                    SkillOrigin::Bundled => PlannedOrigin::Bundled,
                    SkillOrigin::Source => PlannedOrigin::Source,
                    _ => PlannedOrigin::Catalog,
                },
                source: entry.source.clone(),
                pin: entry.pin.clone(),
                catalog: None,
                catalogs: None,
            };
            let mut key = entry.key.clone();
            let mut from = entry.from.clone();
            if entry.origin == SkillOrigin::Catalog {
                let wanted = entry.name.to_lowercase();
                let mut offers = Vec::new();
                for (label, catalog) in catalogs {
                    let Some(skills) = &catalog.skills else {
                        continue;
                    };
                    for (listed, offer) in &skills.entries {
                        if listed.to_lowercase() == wanted {
                            offers.push((*label, offer));
                        }
                    }
                }
                let offers = ranked(offers, &rank);
                match offers.as_slice() {
                    [] => {
                        item.origin = PlannedOrigin::Missing;
                        plan.missing.push((entry.name.clone(), identity.clone()));
                    }
                    [(label, offer)] => {
                        item.catalog = Some((*label).to_owned());
                        item.source = Some(offer.source.shown());
                        key = Some(offer.source.key_with(None));
                        from = Some(offer.source.clone());
                        item.pin.clone_from(&offer.pin);
                    }
                    many => {
                        let mut labels: Vec<String> =
                            many.iter().map(|(label, _)| (*label).to_owned()).collect();
                        labels.sort();
                        item.origin = PlannedOrigin::Ambiguous;
                        item.catalogs = Some(labels.clone());
                        plan.ambiguous
                            .push((entry.name.clone(), identity.clone(), labels));
                    }
                }
            }
            let origin = item.origin;
            let pin = item.pin.clone();
            plan.skills.push(item);
            if matches!(origin, PlannedOrigin::Missing | PlannedOrigin::Ambiguous) {
                continue;
            }
            let bundled = origin == PlannedOrigin::Bundled;
            if bundled
                && let Some(text) = tree.bytes(&format!("skills/{}/SKILL.md", entry.name))
                && bundled_name(text, settings).is_some_and(|named| named != entry.name)
            {
                plan.name_mismatch
                    .push((entry.name.clone(), identity.clone()));
                continue;
            }
            if !bundled {
                if !fetches {
                    plan.not_fetched
                        .push((entry.name.clone(), identity.clone()));
                    continue;
                }
                if pin.is_none() {
                    plan.unpinned.push((entry.name.clone(), identity.clone()));
                }
                let key_text = key.clone().unwrap_or_default();
                let location = key_text.split('#').next().unwrap_or_default();
                let lower = location.to_ascii_lowercase();
                let lower = lower.strip_prefix("git+").unwrap_or(&lower);
                if lower.starts_with("http://") || lower.starts_with("git://") {
                    plan.non_defaults.push((
                        "insecure-transport".to_owned(),
                        entry.name.clone(),
                        identity.clone(),
                    ));
                }
                if host_is_private(source_host(location).as_deref()) {
                    plan.non_defaults.push((
                        "private-source".to_owned(),
                        entry.name.clone(),
                        identity.clone(),
                    ));
                }
            }
            let held = if bundled {
                Held::Bundled(identity.clone())
            } else {
                Held::Elsewhere(key.clone(), pin.clone())
            };
            if !delivery.offer(&entry.name, identity, bundled, held) {
                plan.shadowing.push((entry.name.clone(), identity.clone()));
            } else if !bundled && let Some(from) = from {
                plan.to_fetch.push(Fetch {
                    name: entry.name.clone(),
                    overlay: identity.clone(),
                    from,
                    pin,
                });
            }
        }
    }
    plan.delivered = delivery.delivered();
    plan.collisions = delivery.collisions();
    // A skill later displaced by a bundled one of the same name is not fetched.
    plan.to_fetch
        .retain(|fetch| delivery.holds(&fetch.name, &fetch.overlay));
    Ok(plan)
}

/// A skill that was fetched and passed the checks.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct FetchedSkill {
    /// The tree digest of what was fetched.
    pub digest: Digest,
    /// The front matter keys a consumer reports: every one besides the Agent Skills inert keys.
    pub reported: Vec<String>,
    /// The paths the digest covers, in the byte order of the digest's lines. A caller writes these
    /// and nothing else: the fetched tree may also carry entries under an excluded name
    /// (`node_modules`, `__pycache__`, `*.pyc`, `.git`), which the digest leaves out and Python,
    /// Node.js and Git load code from.
    pub files: Vec<String>,
}

/// The Agent Skills front matter keys that grant nothing; any other is reported.
const INERT_KEYS: [&str; 5] = [
    "name",
    "description",
    "license",
    "compatibility",
    "metadata",
];

/// Checks a skill fetched under `name`, given by relative path and content, before it is
/// delivered: its tree digest, then the front matter keys a consumer reports. The pin comes first,
/// so nothing unverified is parsed.
///
/// # Errors
///
/// The digest's classes for a tree it refuses, `pin-mismatch`, `not-a-skill` for no `SKILL.md`,
/// `skill-unreadable` for front matter that does not read or is not YAML, and
/// `skill-name-mismatch`.
pub fn read_fetched_skill(
    name: &str,
    files: &[(String, Vec<u8>)],
    pin: Option<&str>,
    settings: &Settings,
) -> Result<FetchedSkill, Error> {
    let entries: Vec<(Vec<&[u8]>, TreeEntry)> = files
        .iter()
        .map(|(path, bytes)| {
            (
                path.split('/').map(str::as_bytes).collect(),
                TreeEntry::File(bytes.clone()),
            )
        })
        .collect();
    crate::digest::check_entries(&entries, settings)?;
    let tree = digest_entries(entries.iter().map(|(path, entry)| (path.clone(), entry)))?;
    let digest = tree.digest;
    if let Some(pin) = pin
        && Digest::parse(pin).ok() != Some(digest)
    {
        return Err(Error::new(Class::PinMismatch, name));
    }
    let Some((_, skill)) = files.iter().find(|(path, _)| path == "SKILL.md") else {
        return Err(Error::new(Class::NotASkill, name));
    };
    let found = read_front_matter(skill, settings);
    if let Some(error) = found.error {
        let mut refused = Error::new(Class::SkillUnreadable, error.class.as_str());
        refused.line = error.line;
        return Err(refused);
    }
    let split = split_front_matter(skill, settings);
    if found.case != Case::Mapping
        || found.format != Some(Format::Yaml)
        || split
            .metadata
            .is_some_and(|metadata| metadata.starts_with(b"#!"))
    {
        return Err(Error::new(
            Class::SkillUnreadable,
            "front matter in YAML is required",
        ));
    }
    let data = found.data.unwrap_or_default();
    if data.get("name").and_then(serde_json::Value::as_str) != Some(name) {
        return Err(Error::new(Class::SkillNameMismatch, name));
    }
    let mut reported: Vec<String> = data
        .keys()
        .filter(|key| !INERT_KEYS.contains(&key.as_str()))
        .cloned()
        .collect();
    reported.sort();
    Ok(FetchedSkill {
        digest,
        reported,
        files: tree.files.into_keys().collect(),
    })
}

/// What a caller supplies to fetch skills: the core decides what to fetch and checks what came
/// back, and never touches a network.
pub trait SkillSource {
    /// The files of the skill at `source`, by path relative to the skill's directory.
    ///
    /// # Errors
    ///
    /// Whatever stopped the fetch.
    fn fetch(&self, source: &Source) -> Result<Vec<(String, Vec<u8>)>, Error>;
}

/// A skill of a plan, fetched and checked.
#[derive(Debug)]
#[non_exhaustive]
pub struct FetchedPlanned {
    /// The skill's name.
    pub name: String,
    /// The overlay that offers it.
    pub overlay: String,
    /// The check's outcome.
    pub outcome: Result<FetchedSkill, Error>,
}

/// Fetches each skill the plan says to fetch through `fetcher` and checks it against its pin.
/// A fetch that fails is an `io` error for that skill alone.
pub fn fetch_planned(
    plan: &SkillPlan,
    fetcher: &dyn SkillSource,
    settings: &Settings,
) -> Vec<FetchedPlanned> {
    plan.to_fetch
        .iter()
        .map(|fetch| FetchedPlanned {
            name: fetch.name.clone(),
            overlay: fetch.overlay.clone(),
            outcome: match fetcher.fetch(&fetch.from) {
                Ok(files) => {
                    read_fetched_skill(&fetch.name, &files, fetch.pin.as_deref(), settings)
                }
                Err(error) => Err(error),
            },
        })
        .collect()
}

impl PlannedOrigin {
    /// The word the vectors use.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Bundled => "bundled",
            Self::Source => "source",
            Self::Catalog => "catalog",
            Self::Missing => "missing",
            Self::Ambiguous => "ambiguous",
        }
    }

    /// What a word stands for.
    #[must_use]
    pub fn named(word: &str) -> Option<Self> {
        match word {
            "bundled" => Some(Self::Bundled),
            "source" => Some(Self::Source),
            "catalog" => Some(Self::Catalog),
            "missing" => Some(Self::Missing),
            "ambiguous" => Some(Self::Ambiguous),
            _ => None,
        }
    }
}
