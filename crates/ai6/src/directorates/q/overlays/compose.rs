//! Resolving `requires`, the composition order and what the active overlays contribute
//! together (`spec/v0/dependencies.md`, `composition.md`, `runtime.md#environments`).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde_json::Value as Data;

use super::defaults::{OverlayTree, context_of, declared, dependencies_of, rules_of, skills_of};
use super::names::{normalize_name, root_variable};
use super::read::Document;

/// A cycle among `requires`, as the chain `a -> b -> a`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cycle(pub Vec<String>);

impl fmt::Display for Cycle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "cycle: {}", self.0.join(" -> "))
    }
}

impl std::error::Error for Cycle {}

/// The overlays visited from the requested ones, and the requirements no overlay offers.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Resolution {
    /// Each overlay once, after what it requires: the order to install in.
    pub install_order: Vec<String>,
    /// `(requirer, name)` for each required name nothing offers; the requirer is empty for a
    /// name asked for directly.
    pub missing: Vec<(String, String)>,
}

/// Visits each requested overlay in order, each of its `requires` first. `available` maps a
/// normalized identity to its manifest.
///
/// # Errors
///
/// A [`Cycle`] naming the chain.
pub fn resolve(
    available: &BTreeMap<String, Document>,
    requested: &[String],
) -> Result<Resolution, Cycle> {
    fn visit(
        available: &BTreeMap<String, Document>,
        name: String,
        chain: &mut Vec<String>,
        out: &mut Resolution,
    ) -> Result<(), Cycle> {
        if out.install_order.contains(&name) {
            return Ok(());
        }
        if let Some(at) = chain.iter().position(|seen| *seen == name) {
            let mut cycle = chain[at..].to_vec();
            cycle.push(name);
            return Err(Cycle(cycle));
        }
        let Some(manifest) = available.get(&name) else {
            out.missing
                .push((chain.last().cloned().unwrap_or_default(), name));
            return Ok(());
        };
        chain.push(name.clone());
        for required in declared(manifest, "requires")
            .iter()
            .filter_map(Data::as_str)
        {
            visit(available, normalize_name(required), chain, out)?;
        }
        chain.pop();
        out.install_order.push(name);
        Ok(())
    }
    let mut out = Resolution::default();
    for name in requested {
        visit(available, normalize_name(name), &mut Vec::new(), &mut out)?;
    }
    Ok(out)
}

/// `active` sorted ascending by `(priority, name, identity)`, strings by code point; `name`
/// is the manifest's, or the identity when it has none.
pub fn composition_order(overlays: &BTreeMap<String, Document>, active: &[String]) -> Vec<String> {
    let key = |identity: &String| {
        let manifest = overlays.get(identity);
        let priority = manifest
            .and_then(|manifest| manifest.get("priority"))
            .and_then(Data::as_i64)
            .unwrap_or(500);
        let name = manifest
            .and_then(|manifest| manifest.get("name"))
            .and_then(Data::as_str)
            .filter(|name| !name.is_empty())
            .unwrap_or(identity)
            .to_owned();
        (priority, name, identity.clone())
    };
    let mut sorted = active.to_vec();
    sorted.sort_by_cached_key(key);
    sorted
}

/// What the active overlays contribute together, in composition order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Composition {
    /// The order to install in.
    pub install_order: Vec<String>,
    /// The active overlays in composition order.
    pub active: Vec<String>,
    /// `(requirer, name)` for each requirement nothing offers.
    pub missing: Vec<(String, String)>,
    /// Each active overlay's root variable.
    pub root_variables: BTreeMap<String, String>,
    /// Routing lines, overlay by overlay.
    pub routing: Vec<String>,
    /// `(overlay, rules file)`.
    pub rules: Vec<(String, String)>,
    /// `(overlay, context file)`.
    pub context: Vec<(String, String)>,
    /// Each skill delivered, by its name, and the overlay it comes from.
    pub skills: BTreeMap<String, String>,
    /// Skill names, in lower case, that two overlays offer; the later one is delivered.
    pub skill_collisions: Vec<String>,
}

/// Resolves `requested` among `overlays` and composes the result. `trees` gives each
/// overlay's files, from which its defaults follow.
///
/// # Errors
///
/// A [`Cycle`] among `requires`.
pub fn compose(
    overlays: &BTreeMap<String, Document>,
    requested: &[String],
    trees: &BTreeMap<String, OverlayTree>,
) -> Result<Composition, Cycle> {
    let resolution = resolve(overlays, requested)?;
    let active = composition_order(overlays, &resolution.install_order);
    let empty_tree = OverlayTree::default();
    let tree = |identity: &str| trees.get(identity).unwrap_or(&empty_tree);
    let mut composition = Composition {
        root_variables: active
            .iter()
            .map(|identity| (identity.clone(), root_variable(identity)))
            .collect(),
        install_order: resolution.install_order,
        missing: resolution.missing,
        ..Composition::default()
    };
    let mut winners: BTreeMap<String, (String, String)> = BTreeMap::new();
    let mut collisions = BTreeSet::new();
    for identity in &active {
        let manifest = &overlays[identity];
        composition.routing.extend(
            declared(manifest, "routing")
                .iter()
                .filter_map(Data::as_str)
                .map(str::to_owned),
        );
        for path in rules_of(manifest, tree(identity)) {
            composition.rules.push((identity.clone(), path));
        }
        for path in context_of(manifest, tree(identity)) {
            composition.context.push((identity.clone(), path));
        }
        for name in skills_of(manifest, tree(identity)) {
            let folded = name.to_lowercase();
            if winners.contains_key(&folded) {
                collisions.insert(folded.clone());
            }
            winners.insert(folded, (name, identity.clone()));
        }
    }
    composition.skills = winners.into_values().collect();
    composition.skill_collisions = collisions.into_iter().collect();
    composition.active = active;
    Ok(composition)
}

/// One environment the active overlays install packages into.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct PackageEnvironment {
    /// `python` or `node`.
    pub runtime: String,
    /// `shared`, `isolated` or a name.
    pub environment: String,
    /// Its members, in composition order.
    pub overlays: Vec<String>,
    /// `(overlay, dependency file)`.
    pub files: Vec<(String, String)>,
    /// Whether it installs pinned: every member's file locked (one member, for Node.js).
    pub locked: bool,
}

/// Two members of one environment asking for one package at different versions.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct PackageConflict {
    /// `python` or `node`.
    pub runtime: String,
    /// The environment.
    pub environment: String,
    /// The package, its name normalized.
    pub package: String,
    /// The overlays that ask for it.
    pub overlays: Vec<String>,
}

/// What a removed overlay's dependencies leave.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct LeftEnvironment {
    /// The removed overlay.
    pub overlay: String,
    /// `python` or `node`.
    pub runtime: String,
    /// The environment it was in.
    pub environment: String,
    /// `rebuilt` while another member remains, else `deleted`.
    pub then: String,
}

/// The environments of the active overlays, the conflicts that block overlays, and what each
/// removed overlay leaves.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Environments {
    /// Each environment installed.
    pub environments: Vec<PackageEnvironment>,
    /// Each conflict.
    pub conflicts: Vec<PackageConflict>,
    /// The overlays a conflict keeps from activating, in the order given.
    pub blocked: Vec<String>,
    /// What removed overlays leave.
    pub left: Vec<LeftEnvironment>,
}

/// Package name to the exact version or range a dependency file asks for.
fn pins(runtime: &str, text: Option<&str>) -> BTreeMap<String, String> {
    let Some(text) = text else {
        return BTreeMap::new();
    };
    if runtime == "node" {
        let declared = serde_json::from_str::<Data>(text)
            .ok()
            .and_then(|value| value.get("dependencies").and_then(Data::as_object).cloned())
            .unwrap_or_default();
        return declared
            .into_iter()
            .map(|(name, spec)| {
                (
                    name,
                    spec.as_str()
                        .map_or_else(|| spec.to_string(), str::to_owned),
                )
            })
            .collect();
    }
    let joined = text.replace("\\\r\n", " ").replace("\\\n", " ");
    let mut out = BTreeMap::new();
    for line in joined.lines() {
        if let Some((name, version)) = pinned_requirement(line.trim()) {
            out.insert(name, version);
        }
    }
    out
}

/// `name[extras] == version` at the start of a requirement line: the name normalized as
/// PEP 503 does, and the version.
fn pinned_requirement(line: &str) -> Option<(String, String)> {
    let bytes = line.as_bytes();
    if !bytes.first().is_some_and(u8::is_ascii_alphanumeric) {
        return None;
    }
    let mut end = 0;
    while bytes
        .get(end)
        .is_some_and(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        end += 1;
    }
    let mut name = &line[..end];
    while name.ends_with(['.', '_', '-']) {
        name = &name[..name.len() - 1];
    }
    let mut rest = line[name.len()..].trim_start();
    if !line[name.len()..].starts_with(|c: char| c.is_whitespace() || matches!(c, '[' | '=')) {
        return None;
    }
    if let Some(extras) = rest.strip_prefix('[') {
        rest = extras.split_once(']')?.1.trim_start();
    }
    let rest = rest
        .strip_prefix("===")
        .or_else(|| rest.strip_prefix("=="))?
        .trim_start();
    let version: String = rest
        .chars()
        .take_while(|c| !c.is_whitespace() && !matches!(c, ';' | '\\'))
        .collect();
    if version.is_empty() {
        return None;
    }
    let mut normalized = String::new();
    for c in name.chars() {
        if matches!(c, '-' | '_' | '.') {
            if !normalized.ends_with('-') {
                normalized.push('-');
            }
        } else {
            normalized.push(c.to_ascii_lowercase());
        }
    }
    Some((normalized, version))
}

type GroupKey = (String, String, String);

/// The environments `active` install into on `os`: a shared or named one holds every overlay
/// that chose it, in composition order. Two members asking for one package at different
/// versions conflict, and neither is activated.
///
/// # Errors
///
/// The path of a requirements file only some of whose requirements are hashed.
pub fn environments(
    overlays: &BTreeMap<String, Document>,
    active: &[String],
    trees: &BTreeMap<String, OverlayTree>,
    os: &str,
    removed: &[String],
) -> Result<Environments, String> {
    let empty_tree = OverlayTree::default();
    let tree = |identity: &str| trees.get(identity).unwrap_or(&empty_tree);
    let key = |runtime: &str, environment: &str, identity: &str| -> GroupKey {
        let owner = if environment == "isolated" {
            identity
        } else {
            ""
        };
        (runtime.to_owned(), environment.to_owned(), owner.to_owned())
    };
    let mut groups: Vec<(GroupKey, Vec<(String, super::defaults::Dependency)>)> = Vec::new();
    for identity in active {
        for dependency in
            dependencies_of(&overlays[identity], tree(identity), os).map_err(|lock| lock.0)?
        {
            let group = key(&dependency.runtime, &dependency.environment, identity);
            match groups.iter_mut().find(|(existing, _)| *existing == group) {
                Some((_, members)) => members.push((identity.clone(), dependency)),
                None => groups.push((group, vec![(identity.clone(), dependency)])),
            }
        }
    }
    let mut out = Environments::default();
    let mut blocked = BTreeSet::new();
    for ((runtime, name, _), members) in &groups {
        let mut asked: BTreeMap<String, Vec<(String, Vec<String>)>> = BTreeMap::new();
        for (identity, dependency) in members {
            for (package, version) in pins(runtime, tree(identity).text(&dependency.file)) {
                let versions = asked.entry(package).or_default();
                match versions
                    .iter_mut()
                    .find(|(existing, _)| *existing == version)
                {
                    Some((_, askers)) => askers.push(identity.clone()),
                    None => versions.push((version, vec![identity.clone()])),
                }
            }
        }
        for (package, versions) in asked {
            if versions.len() > 1 {
                let involved: Vec<String> = members
                    .iter()
                    .map(|(identity, _)| identity)
                    .filter(|identity| versions.iter().any(|(_, askers)| askers.contains(identity)))
                    .cloned()
                    .collect();
                blocked.extend(involved.iter().cloned());
                out.conflicts.push(PackageConflict {
                    runtime: runtime.clone(),
                    environment: name.clone(),
                    package,
                    overlays: involved,
                });
            }
        }
    }
    let mut remaining = BTreeSet::new();
    for (group, members) in &groups {
        let kept: Vec<_> = members
            .iter()
            .filter(|(identity, _)| !blocked.contains(identity))
            .collect();
        if kept.is_empty() {
            continue;
        }
        remaining.insert(group.clone());
        let locked = if group.0 == "node" {
            // npm installs one lock file; several overlays' locks cannot be combined.
            kept.len() == 1 && kept[0].1.lock.is_some()
        } else {
            kept.iter().all(|(_, dependency)| dependency.lock.is_some())
        };
        out.environments.push(PackageEnvironment {
            runtime: group.0.clone(),
            environment: group.1.clone(),
            overlays: kept.iter().map(|(identity, _)| identity.clone()).collect(),
            files: kept
                .iter()
                .map(|(identity, dependency)| (identity.clone(), dependency.file.clone()))
                .collect(),
            locked,
        });
    }
    for identity in removed {
        for dependency in
            dependencies_of(&overlays[identity], tree(identity), os).map_err(|lock| lock.0)?
        {
            let group = key(&dependency.runtime, &dependency.environment, identity);
            out.left.push(LeftEnvironment {
                overlay: identity.clone(),
                then: if remaining.contains(&group) {
                    "rebuilt"
                } else {
                    "deleted"
                }
                .to_owned(),
                runtime: dependency.runtime,
                environment: dependency.environment,
            });
        }
    }
    out.blocked = active
        .iter()
        .filter(|identity| blocked.contains(*identity))
        .cloned()
        .collect();
    Ok(out)
}
