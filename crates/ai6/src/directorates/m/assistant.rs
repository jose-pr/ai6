//! Moneypenny's deployment settings: the `[assistant]` table of the global `agents.toml`.
//!
//! A person's personal repository is Moneypenny's working directory and memory for that
//! person; each directory group may map to a shared repository she reads, and one more may be
//! shared with everyone (`org`). Without a personal repository she works in an empty
//! directory and keeps nothing between conversations.
//!
//! ```toml
//! [assistant]
//! personal_repo = "/srv/moneypenny/{person_digest}.git"
//! allow_elevated = false
//!
//! [assistant.shared.ops]
//! repo = "ssh://git@git.example/ops/handbook.git"
//! write = false
//!
//! [assistant.org]
//! repo = "https://git.example/org/handbook.git"
//! ```
//!
//! Her scripts, network and skills are set there too, and per directory group or person a
//! table may narrow them, never widen them. Her skills come only from a catalog of skill
//! directories, each pinned by the digest `ai6 assistant pin-skill DIR` prints:
//!
//! ```toml
//! [assistant]
//! interpreters = ["python3"]
//! network = "limited"
//! allowed_hosts = ["docs.python.org"]
//! unsandboxed_scripts = false
//!
//! [assistant.skills.report-writer]
//! path = "skills/report-writer"    # absolute, or relative to the configuration directory
//! pin = "sha256:..."
//!
//! [assistant.group.contractors]
//! network = "denied"
//! skills = []
//!
//! [assistant.person."eve@sis"]
//! interpreters = []
//! ```

use std::collections::BTreeMap;
use std::path::Path;

use coqui_peer::{Digest as PinDigest, PeerId};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::directorates::m::harness::MAX_MANIFEST_FILE_BYTES;
use crate::directorates::section_00::agent::AssistantNetwork;
use crate::directorates::section_00::assistant::{
    AssistantReach, host_pattern, interpreter_name, valid_identity,
};

/// The name a person's identifier takes in `personal_repo`.
pub const PERSON_PLACEHOLDER: &str = "{person}";
/// The name of 16 hex digits of the SHA-256 of a person's identifier in `personal_repo`.
pub const PERSON_DIGEST_PLACEHOLDER: &str = "{person_digest}";
/// The name the organisation-wide shared repository is checked out under.
pub const ORG_SHARED: &str = "org";

/// The `[assistant]` table: where Moneypenny keeps each person's work and memory, what she
/// reads, what her scripts may run and reach, which skills she may use, and whether she may
/// run on an elevated Station. The default keeps nothing, shares nothing, runs `python3` in the
/// harness's sandbox, reaches no host, has no skill and refuses elevated Stations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AssistantSettings {
    personal_repo: Option<String>,
    shared: BTreeMap<String, SharedRepo>,
    org: Option<SharedRepo>,
    allow_elevated: bool,
    interpreters: Vec<String>,
    network: NetworkPolicy,
    allowed_hosts: Vec<String>,
    unsandboxed_scripts: bool,
    skills: BTreeMap<String, SkillEntry>,
    skills_unpinned: bool,
    skills_allow_tools: bool,
    group: BTreeMap<String, AssistantNarrowing>,
    person: BTreeMap<String, AssistantNarrowing>,
}

impl Default for AssistantSettings {
    fn default() -> Self {
        Self {
            personal_repo: None,
            shared: BTreeMap::new(),
            org: None,
            allow_elevated: false,
            interpreters: vec![DEFAULT_INTERPRETER.to_owned()],
            network: NetworkPolicy::Limited,
            allowed_hosts: Vec::new(),
            unsandboxed_scripts: false,
            skills: BTreeMap::new(),
            skills_unpinned: false,
            skills_allow_tools: false,
            group: BTreeMap::new(),
            person: BTreeMap::new(),
        }
    }
}

/// The interpreter Moneypenny's scripts run by default.
pub const DEFAULT_INTERPRETER: &str = "python3";

/// What Moneypenny's web tools and scripts may reach besides her MCP servers, strictest first.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum NetworkPolicy {
    /// Nothing: no web tool, and scripts reach no host.
    Denied,
    /// Only the hosts in `allowed_hosts`; with none, nothing.
    #[default]
    Limited,
    /// Any host, and web search; a weakening.
    Open,
}

impl NetworkPolicy {
    /// The policy as `[assistant]` and [`AssistantNetwork::policy`] spell it.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Denied => "denied",
            Self::Limited => "limited",
            Self::Open => "open",
        }
    }
}

/// One skill of the catalog (`[assistant.skills.<name>]`): a directory holding `SKILL.md` and
/// its files, and the digest of that directory it must have when it is installed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SkillEntry {
    path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pin: Option<PeerId>,
}

impl SkillEntry {
    /// The skill in the directory `path`: absolute, or in `agents.toml` relative to its
    /// configuration directory. Without a pin it loads only with `skills_unpinned`.
    pub fn new(path: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            pin: None,
        }
    }

    /// The digest the directory's
    /// [`ProjectManifest::read_tree`](crate::ProjectManifest::read_tree) must have.
    #[must_use]
    pub fn pinned(mut self, pin: PeerId) -> Self {
        self.pin = Some(pin);
        self
    }

    /// The skill's directory.
    pub fn path(&self) -> &str {
        &self.path
    }

    /// The digest its directory must have, when it is pinned.
    pub fn pin(&self) -> Option<PeerId> {
        self.pin
    }
}

/// An `[assistant.group.<group>]` or `[assistant.person.<id>]` table: the same keys as
/// `[assistant]` for scripts and network, and `skills`, the catalog skills it keeps, each
/// narrowing the deployment's setting for the members of that group or that person. A key left
/// out narrows nothing; a table that would widen a setting is refused when the roster loads.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AssistantNarrowing {
    interpreters: Option<Vec<String>>,
    network: Option<NetworkPolicy>,
    allowed_hosts: Option<Vec<String>>,
    unsandboxed_scripts: Option<bool>,
    skills: Option<Vec<String>>,
}

impl AssistantNarrowing {
    /// A table that narrows nothing.
    pub fn new() -> Self {
        Self::default()
    }

    /// Only these of the deployment's interpreters.
    #[must_use]
    pub fn with_interpreters(mut self, interpreters: Vec<String>) -> Self {
        self.interpreters = Some(interpreters);
        self
    }

    /// A network policy no looser than the deployment's.
    #[must_use]
    pub fn with_network(mut self, network: NetworkPolicy) -> Self {
        self.network = Some(network);
        self
    }

    /// Only these hosts; under a `limited` deployment, only hosts it lists.
    #[must_use]
    pub fn with_allowed_hosts(mut self, hosts: Vec<String>) -> Self {
        self.allowed_hosts = Some(hosts);
        self
    }

    /// Unsandboxed scripts kept off (`false`) where the deployment allows them.
    #[must_use]
    pub fn with_unsandboxed_scripts(mut self, allow: bool) -> Self {
        self.unsandboxed_scripts = Some(allow);
        self
    }

    /// Only these of the catalog's skills.
    #[must_use]
    pub fn with_skills(mut self, skills: Vec<String>) -> Self {
        self.skills = Some(skills);
        self
    }

    /// The skills it keeps, when it narrows them.
    pub fn skills(&self) -> Option<&[String]> {
        self.skills.as_deref()
    }

    /// The reach this table leaves at most: its keys, and for each key it leaves out the
    /// widest value, which capping by the deployment's reach turns into the deployment's.
    fn as_cap(&self, deployment: &AssistantReach) -> AssistantReach {
        let network = self.network.unwrap_or(NetworkPolicy::Open);
        let hosts = self.allowed_hosts.clone();
        AssistantReach {
            interpreters: self
                .interpreters
                .clone()
                .unwrap_or_else(|| deployment.interpreters.clone()),
            network: match (network, hosts) {
                (NetworkPolicy::Open, None) => AssistantNetwork {
                    policy: NetworkPolicy::Open.as_str().to_owned(),
                    allowed_hosts: Vec::new(),
                },
                // A host list narrows the hosts; `limited` without one keeps the deployment's.
                (NetworkPolicy::Open | NetworkPolicy::Limited, hosts) => AssistantNetwork {
                    policy: NetworkPolicy::Limited.as_str().to_owned(),
                    allowed_hosts: hosts
                        .unwrap_or_else(|| deployment.network.allowed_hosts.clone()),
                },
                (NetworkPolicy::Denied, _) => AssistantNetwork {
                    policy: NetworkPolicy::Denied.as_str().to_owned(),
                    allowed_hosts: Vec::new(),
                },
            },
            unsandboxed_scripts: self.unsandboxed_scripts.unwrap_or(true),
        }
    }

    /// Checks the table's names and that it narrows `settings`, naming each key as
    /// `<prefix>.<key>`.
    fn validate(&self, prefix: &str, settings: &AssistantSettings) -> Result<(), String> {
        if let Some(interpreters) = &self.interpreters {
            for name in interpreters {
                if !interpreter_name(name) {
                    return Err(format!(
                        "{prefix}.interpreters: {name:?} is not an interpreter name"
                    ));
                }
                if !settings.interpreters.contains(name) {
                    return Err(format!(
                        "{prefix}.interpreters: {name:?} is not in assistant.interpreters; a \
                         table may only narrow it"
                    ));
                }
            }
        }
        if let Some(network) = self.network
            && network > settings.network
        {
            return Err(format!(
                "{prefix}.network: {:?} is looser than assistant.network ({:?}); a table may \
                 only narrow it",
                network.as_str(),
                settings.network.as_str()
            ));
        }
        if let Some(hosts) = &self.allowed_hosts {
            for host in hosts {
                if !host_pattern(host) {
                    return Err(format!(
                        "{prefix}.allowed_hosts: {host:?} is not a host name"
                    ));
                }
                if settings.network == NetworkPolicy::Limited
                    && !settings.allowed_hosts.contains(host)
                {
                    return Err(format!(
                        "{prefix}.allowed_hosts: {host:?} is not in assistant.allowed_hosts; a \
                         table may only narrow them"
                    ));
                }
            }
        }
        if self.unsandboxed_scripts == Some(true) && !settings.unsandboxed_scripts {
            return Err(format!(
                "{prefix}.unsandboxed_scripts: true while assistant.unsandboxed_scripts is \
                 false; a table may only narrow it"
            ));
        }
        for skill in self.skills.iter().flatten() {
            if !settings.skills.contains_key(skill) {
                return Err(format!(
                    "{prefix}.skills: {skill:?} is not in the catalog (assistant.skills); a \
                     table may only narrow it"
                ));
            }
        }
        Ok(())
    }
}

/// The plugin name the installed catalog is loaded under: Claude Code calls each skill
/// `ai6:<name>`.
pub const SKILLS_PLUGIN: &str = "ai6";

/// The `SKILL.md` front matter keys that grant nothing and run nothing
/// (code.claude.com/docs/en/skills): any other key, such as `allowed-tools`, `hooks`,
/// `shell`, `model` or `context`, or one Claude Code adds later, needs `skills_allow_tools`.
pub const SKILL_INERT_KEYS: [&str; 12] = [
    "name",
    "description",
    "when_to_use",
    "argument-hint",
    "arguments",
    "disable-model-invocation",
    "user-invocable",
    "disallowed-tools",
    "paths",
    "metadata",
    "license",
    "compatibility",
];

/// Whether `name` may name a skill: 1 to 64 lowercase letters, digits and `-`, starting with a
/// letter or digit, as Claude Code names skills.
pub fn skill_name(name: &str) -> bool {
    (1..=64).contains(&name.len())
        && name
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// Checks the skill in `dir` as the catalog's `name`: a `SKILL.md` that is a regular file of at
/// most [`MAX_MANIFEST_FILE_BYTES`], UTF-8, opening with `---` front matter that parses as YAML
/// with `name` equal to `name` and a non-empty `description`. Unless `allow_tools`, its front
/// matter has only [`SKILL_INERT_KEYS`] and it has no injected command (`` !`command` ``, or a
/// fenced block opened with `` ```! ``), since those grant tools or run commands in her
/// Session. Reads the file; never follows a link.
///
/// # Errors
///
/// A message saying what is wrong with it.
pub fn check_skill(dir: &Path, name: &str, allow_tools: bool) -> Result<(), String> {
    let path = dir.join("SKILL.md");
    let metadata = std::fs::symlink_metadata(dir)
        .and_then(|metadata| {
            if metadata.is_dir() {
                std::fs::symlink_metadata(&path)
            } else {
                Err(std::io::Error::other(
                    "the skill is not a directory (a link is never followed)",
                ))
            }
        })
        .map_err(|error| format!("{}: {error}", path.display()))?;
    if !metadata.file_type().is_file() || metadata.len() > MAX_MANIFEST_FILE_BYTES {
        return Err(format!(
            "{} is not a regular file of at most {MAX_MANIFEST_FILE_BYTES} bytes",
            path.display()
        ));
    }
    let text =
        std::fs::read_to_string(&path).map_err(|error| format!("{}: {error}", path.display()))?;
    let (yaml, _) = front_matter(&text)
        .ok_or("SKILL.md does not open with front matter between two `---` lines")?;
    let keys: BTreeMap<String, serde_json::Value> =
        serde_saphyr::from_str(yaml).map_err(|error| {
            format!(
                "SKILL.md front matter is not a YAML mapping: {}",
                error.to_string().lines().next().unwrap_or_default()
            )
        })?;
    if keys.get("name").and_then(serde_json::Value::as_str) != Some(name) {
        return Err(format!("SKILL.md front matter does not have name: {name}"));
    }
    let description = keys
        .get("description")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    if description.trim().is_empty() {
        return Err("SKILL.md front matter has no description".to_owned());
    }
    if allow_tools {
        return Ok(());
    }
    if let Some(key) = keys
        .keys()
        .find(|key| !SKILL_INERT_KEYS.contains(&key.as_str()))
    {
        return Err(format!(
            "SKILL.md front matter sets {key:?}, which can grant tools, run commands or change \
             her Session; only assistant.skills_allow_tools = true lets it load"
        ));
    }
    if injects_commands(&text) {
        return Err(
            "SKILL.md injects a command (!`…` or a ```! block), which runs before she reads \
             it; only assistant.skills_allow_tools = true lets it load"
                .to_owned(),
        );
    }
    Ok(())
}

/// The YAML between the opening `---` line and the next `---` line, and the text after it.
fn front_matter(text: &str) -> Option<(&str, &str)> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let rest = text
        .strip_prefix("---\r\n")
        .or_else(|| text.strip_prefix("---\n"))?;
    let mut at = 0;
    for line in rest.split_inclusive('\n') {
        if line.trim_end_matches(['\r', '\n']) == "---" {
            return Some((&rest[..at], &rest[at + line.len()..]));
        }
        at += line.len();
    }
    None
}

/// Whether `text` holds a command Claude Code runs when the skill loads: `!` then a backtick
/// anywhere (Claude Code runs it only at a line start or after whitespace; any is refused), or
/// a fence of three or more backticks or tildes followed by `!`.
fn injects_commands(text: &str) -> bool {
    text.contains("!`")
        || text.lines().any(|line| {
            let line = line.trim_start();
            ['`', '~'].iter().any(|fence| {
                let after = line.trim_start_matches(*fence);
                line.len() - after.len() >= 3 && after.trim_start().starts_with('!')
            })
        })
}

/// A repository a directory group (or everyone) shares with Moneypenny: checked out read-only
/// for her; a change to it is a proposal the person confirms, or immediate with `write`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SharedRepo {
    repo: String,
    #[serde(default)]
    write: bool,
}

impl SharedRepo {
    /// The repository at `repo` (a URL, `user@host:path` or a local path), changed only by
    /// proposal.
    pub fn new(repo: impl Into<String>) -> Self {
        Self {
            repo: repo.into(),
            write: false,
        }
    }

    /// Lets a member's change through without a proposal; a weakening.
    #[must_use]
    pub fn writable(mut self, write: bool) -> Self {
        self.write = write;
        self
    }

    /// The repository.
    pub fn repo(&self) -> &str {
        &self.repo
    }

    /// Whether a member's change is committed without a proposal.
    pub fn write(&self) -> bool {
        self.write
    }
}

/// Where a repository is, which decides whether AI6 may create it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum RepoKind {
    /// A local path or `file://` URL: created as an empty bare repository on first use.
    Local,
    /// `ssh://`, `git://`, `http(s)://` or `user@host:path`: reached with the Station user's Git
    /// credentials, and must exist.
    Remote,
}

impl RepoKind {
    /// The kind of `repo`.
    ///
    /// # Errors
    ///
    /// A message for an empty value, control characters, or a URL scheme Git would run a
    /// program for (`ext::`, `fd::`) or AI6 does not know.
    pub fn of(repo: &str) -> Result<Self, String> {
        if repo.trim().is_empty() || repo.trim() != repo || repo.chars().any(char::is_control) {
            return Err(format!(
                "repository {repo:?} is empty or has control characters"
            ));
        }
        if repo.contains("::") {
            return Err(format!(
                "repository {repo:?} names a Git remote helper, which runs a program"
            ));
        }
        if let Some((scheme, _)) = repo.split_once("://") {
            return match scheme.to_ascii_lowercase().as_str() {
                "file" => Ok(Self::Local),
                "ssh" | "git" | "http" | "https" => Ok(Self::Remote),
                _ => Err(format!(
                    "repository {repo:?} has an unknown scheme {scheme:?}"
                )),
            };
        }
        if is_scp_like(repo) {
            return Ok(Self::Remote);
        }
        Ok(Self::Local)
    }
}

/// `user@host:path`, the form Git reads as SSH: a colon before any slash, and not a drive
/// letter.
fn is_scp_like(repo: &str) -> bool {
    let Some(colon) = repo.find(':') else {
        return false;
    };
    let before = &repo[..colon];
    colon > 1 && !before.contains('/') && !before.contains('\\')
}

/// Where the path of `repo` starts: after `scheme://user@host` up to its first `/`, after the
/// `:` of `user@host:`, and at once for a local path. A URL with no path has none, at its end.
fn path_start(repo: &str) -> usize {
    if let Some(scheme) = repo.find("://") {
        let authority = scheme + 3;
        return repo[authority..]
            .find('/')
            .map_or(repo.len(), |slash| authority + slash);
    }
    if is_scp_like(repo) {
        return repo.find(':').map_or(0, |colon| colon + 1);
    }
    0
}

impl AssistantSettings {
    /// Settings that keep nothing, share nothing, run `python3` scripts in the harness's
    /// sandbox with no host to reach, and refuse elevated Stations.
    pub fn new() -> Self {
        Self::default()
    }

    /// Gives each person a personal repository: `template` with [`PERSON_PLACEHOLDER`] or
    /// [`PERSON_DIGEST_PLACEHOLDER`] filled in.
    #[must_use]
    pub fn with_personal_repo(mut self, template: impl Into<String>) -> Self {
        self.personal_repo = Some(template.into());
        self
    }

    /// Shares `repo` with the members of directory group `group`.
    #[must_use]
    pub fn with_shared(mut self, group: impl Into<String>, repo: SharedRepo) -> Self {
        self.shared.insert(group.into(), repo);
        self
    }

    /// Shares `repo` with everyone, checked out as [`ORG_SHARED`].
    #[must_use]
    pub fn with_org(mut self, repo: SharedRepo) -> Self {
        self.org = Some(repo);
        self
    }

    /// Lets a Station running elevated (root, or a Windows administrator's elevated token)
    /// run Moneypenny's harness; a weakening.
    #[must_use]
    pub fn with_allow_elevated(mut self, allow: bool) -> Self {
        self.allow_elevated = allow;
        self
    }

    /// The interpreters her scripts may run (default `python3`); empty, she runs no script.
    #[must_use]
    pub fn with_interpreters(mut self, interpreters: Vec<String>) -> Self {
        self.interpreters = interpreters;
        self
    }

    /// What her web tools and scripts may reach: `policy`, and under
    /// [`NetworkPolicy::Limited`] the hosts in `allowed_hosts` (`example.com`, or
    /// `*.example.com` for every name under it). `Open` is a weakening.
    #[must_use]
    pub fn with_network(mut self, policy: NetworkPolicy, allowed_hosts: Vec<String>) -> Self {
        self.network = policy;
        self.allowed_hosts = allowed_hosts;
        self
    }

    /// Lets her scripts run without a sandbox on a Station whose harness has none (native
    /// Windows for Claude Code); a weakening. Off, such a Station starts her without scripts.
    #[must_use]
    pub fn with_unsandboxed_scripts(mut self, allow: bool) -> Self {
        self.unsandboxed_scripts = allow;
        self
    }

    /// Narrows the scripts, network and skills of the members of directory group `group`.
    #[must_use]
    pub fn with_group(mut self, group: impl Into<String>, narrowing: AssistantNarrowing) -> Self {
        self.group.insert(group.into(), narrowing);
        self
    }

    /// Narrows the scripts, network and skills of the person `person`.
    #[must_use]
    pub fn with_person(mut self, person: impl Into<String>, narrowing: AssistantNarrowing) -> Self {
        self.person.insert(person.into(), narrowing);
        self
    }

    /// Adds the skill `name` to the catalog.
    #[must_use]
    pub fn with_skill(mut self, name: impl Into<String>, skill: SkillEntry) -> Self {
        self.skills.insert(name.into(), skill);
        self
    }

    /// Lets a catalog skill without a pin load; a weakening.
    #[must_use]
    pub fn with_skills_unpinned(mut self, allow: bool) -> Self {
        self.skills_unpinned = allow;
        self
    }

    /// Lets a catalog skill whose front matter can grant tools or run commands, or that
    /// injects commands, load ([`check_skill`]); a weakening.
    #[must_use]
    pub fn with_skills_allow_tools(mut self, allow: bool) -> Self {
        self.skills_allow_tools = allow;
        self
    }

    /// The interpreters her scripts may run.
    pub fn interpreters(&self) -> &[String] {
        &self.interpreters
    }

    /// The network policy.
    pub fn network(&self) -> NetworkPolicy {
        self.network
    }

    /// The hosts a `limited` policy reaches.
    pub fn allowed_hosts(&self) -> &[String] {
        &self.allowed_hosts
    }

    /// Whether her scripts may run unsandboxed where the harness has no sandbox.
    pub fn unsandboxed_scripts(&self) -> bool {
        self.unsandboxed_scripts
    }

    /// Whether any directory group has a narrowing table, so a job's reach and skills cannot
    /// be capped without knowing the person's groups.
    pub fn has_group_narrowing(&self) -> bool {
        !self.group.is_empty()
    }

    /// The deployment's own reach, before any table narrows it.
    pub fn deployment_reach(&self) -> AssistantReach {
        AssistantReach {
            interpreters: self.interpreters.clone(),
            network: AssistantNetwork {
                policy: self.network.as_str().to_owned(),
                allowed_hosts: self.allowed_hosts.clone(),
            },
            unsandboxed_scripts: self.unsandboxed_scripts,
        }
    }

    /// What `person`, a member of `groups`, lets her run and reach: the deployment's reach
    /// narrowed by every table of those groups and by the person's own.
    pub fn reach_for(&self, person: &str, groups: &[String]) -> AssistantReach {
        let deployment = self.deployment_reach();
        self.group
            .iter()
            .filter(|(group, _)| groups.contains(group))
            .map(|(_, narrowing)| narrowing)
            .chain(self.person.get(person))
            .fold(deployment.clone(), |reach, narrowing| {
                reach.capped_by(&narrowing.as_cap(&deployment))
            })
    }

    /// The skills catalog, by name.
    pub fn skills(&self) -> &BTreeMap<String, SkillEntry> {
        &self.skills
    }

    /// Whether a catalog skill without a pin loads.
    pub fn skills_unpinned(&self) -> bool {
        self.skills_unpinned
    }

    /// Whether a catalog skill may grant tools or run commands.
    pub fn skills_allow_tools(&self) -> bool {
        self.skills_allow_tools
    }

    /// The skills `person`, a member of `groups`, is given: every catalog skill, less what a
    /// table of one of those groups or the person's own leaves out.
    pub fn skills_for(&self, person: &str, groups: &[String]) -> Vec<String> {
        let tables = self
            .group
            .iter()
            .filter(|(group, _)| groups.contains(group))
            .map(|(_, narrowing)| narrowing)
            .chain(self.person.get(person));
        let mut skills: Vec<String> = self.skills.keys().cloned().collect();
        for kept in tables.filter_map(AssistantNarrowing::skills) {
            skills.retain(|skill| kept.contains(skill));
        }
        skills
    }

    /// Makes each catalog path that is relative absolute under `base`, the directory of the
    /// `agents.toml` that gave it.
    pub(crate) fn resolve_paths(&mut self, base: &Path) {
        for skill in self.skills.values_mut() {
            if !Path::new(&skill.path).is_absolute() {
                skill.path = base.join(&skill.path).to_string_lossy().into_owned();
            }
        }
    }

    /// The personal repository template, when there is one.
    pub fn personal_repo(&self) -> Option<&str> {
        self.personal_repo.as_deref()
    }

    /// The shared repositories, by directory group.
    pub fn shared(&self) -> &BTreeMap<String, SharedRepo> {
        &self.shared
    }

    /// The repository shared with everyone, when there is one.
    pub fn org(&self) -> Option<&SharedRepo> {
        self.org.as_ref()
    }

    /// Whether an elevated Station may run Moneypenny's harness.
    pub fn allow_elevated(&self) -> bool {
        self.allow_elevated
    }

    /// Refuses a personal repository template that does not name the person exactly once,
    /// inside the repository's path (not its scheme, user or host), or that some person's
    /// identifier would fill in as a shared or the organisation's repository.
    fn check_template(&self, template: &str) -> Result<(), String> {
        let named = template.matches(PERSON_PLACEHOLDER).count()
            + template.matches(PERSON_DIGEST_PLACEHOLDER).count();
        if named != 1 {
            return Err(format!(
                "names the person {named} times; it takes exactly one {PERSON_PLACEHOLDER} or \
                 {PERSON_DIGEST_PLACEHOLDER}, so each person has a repository of their own"
            ));
        }
        let (at, placeholder) = [PERSON_PLACEHOLDER, PERSON_DIGEST_PLACEHOLDER]
            .into_iter()
            .find_map(|placeholder| template.find(placeholder).map(|at| (at, placeholder)))
            .unwrap_or_default();
        if at < path_start(template) {
            return Err(format!(
                "puts {placeholder} before the repository's path; it goes inside a path segment"
            ));
        }
        let (prefix, suffix) = (&template[..at], &template[at + placeholder.len()..]);
        let shared = self
            .shared
            .iter()
            .map(|(group, repo)| (format!("assistant.shared.{group}"), repo))
            .chain(
                self.org
                    .iter()
                    .map(|repo| ("assistant.org".to_owned(), repo)),
            );
        for (key, repo) in shared {
            // Compared without case: two spellings name one repository on a case-insensitive
            // filesystem.
            let repo = repo.repo.to_lowercase();
            let (prefix, suffix) = (prefix.to_lowercase(), suffix.to_lowercase());
            let filled = repo.len() > prefix.len() + suffix.len()
                && repo.starts_with(&prefix)
                && repo.ends_with(&suffix)
                && repo
                    .get(prefix.len()..repo.len() - suffix.len())
                    .is_some_and(|person| {
                        if placeholder == PERSON_DIGEST_PLACEHOLDER {
                            person.len() == 16 && person.bytes().all(|b| b.is_ascii_hexdigit())
                        } else {
                            person_safe(person)
                        }
                    });
            if filled {
                return Err(format!(
                    "is {key}'s repository for some person, whose memory would go there"
                ));
            }
        }
        Ok(())
    }

    /// Checks every repository and group name: repositories of a known [`RepoKind`], a
    /// template whose only placeholders are the two person ones, and group names of 1 to 64
    /// letters, digits, `.`, `-` and `_`, not starting with `.` and not [`ORG_SHARED`]. Checks
    /// the scripts and network settings, the skills catalog (names as [`skill_name`] takes
    /// them, absolute paths, a pin on each unless `skills_unpinned`, each skill's `SKILL.md`
    /// as [`check_skill`] reads it, so the directories must be readable wherever the roster is
    /// loaded), and that every group and person table only narrows them.
    ///
    /// # Errors
    ///
    /// A message naming the first setting out of bounds, as `assistant.<key>`.
    pub fn validate(&self) -> Result<(), String> {
        if let Some(template) = &self.personal_repo {
            RepoKind::of(template)
                .map_err(|reason| format!("assistant.personal_repo: {reason}"))?;
            let mut rest = template
                .replace(PERSON_PLACEHOLDER, "")
                .replace(PERSON_DIGEST_PLACEHOLDER, "");
            rest.retain(|c| c == '{' || c == '}');
            if !rest.is_empty() {
                return Err(format!(
                    "assistant.personal_repo: {template:?} has a placeholder other than \
                     {PERSON_PLACEHOLDER} and {PERSON_DIGEST_PLACEHOLDER}"
                ));
            }
            self.check_template(template)
                .map_err(|reason| format!("assistant.personal_repo: {template:?} {reason}"))?;
        }
        for (group, shared) in &self.shared {
            if !file_safe(group) || group == ORG_SHARED {
                return Err(format!(
                    "assistant.shared: group {group:?} is not 1 to 64 letters, digits, '.', '-' \
                     and '_', or is {ORG_SHARED:?}"
                ));
            }
            RepoKind::of(&shared.repo)
                .map_err(|reason| format!("assistant.shared.{group}: {reason}"))?;
        }
        if let Some(org) = &self.org {
            RepoKind::of(&org.repo).map_err(|reason| format!("assistant.org: {reason}"))?;
        }
        if let Some(name) = self
            .interpreters
            .iter()
            .find(|name| !interpreter_name(name))
        {
            return Err(format!(
                "assistant.interpreters: {name:?} is not an interpreter name"
            ));
        }
        if let Some(host) = self.allowed_hosts.iter().find(|host| !host_pattern(host)) {
            return Err(format!(
                "assistant.allowed_hosts: {host:?} is not a host name (lowercase, with a dot, \
                 `*.` only in front)"
            ));
        }
        for (name, skill) in &self.skills {
            let key = format!("assistant.skills.{name}");
            if !skill_name(name) {
                return Err(format!(
                    "{key}: a skill name is 1 to 64 lowercase letters, digits and '-'"
                ));
            }
            if skill.path.chars().any(char::is_control) || !Path::new(&skill.path).is_absolute() {
                return Err(format!(
                    "{key}.path: {:?} is not an absolute path (only agents.toml may give one \
                     relative to its directory)",
                    skill.path
                ));
            }
            if skill.pin.is_none() && !self.skills_unpinned {
                return Err(format!(
                    "{key}: no pin (`ai6 assistant pin-skill DIR` prints it; \
                     assistant.skills_unpinned = true loads it without one)"
                ));
            }
            check_skill(Path::new(&skill.path), name, self.skills_allow_tools)
                .map_err(|reason| format!("{key}: {reason}"))?;
        }
        for (group, narrowing) in &self.group {
            if !valid_identity(group) {
                return Err(format!("assistant.group: {group:?} is not a group name"));
            }
            narrowing.validate(&format!("assistant.group.{group}"), self)?;
        }
        for (person, narrowing) in &self.person {
            if !valid_identity(person) {
                return Err(format!("assistant.person: {person:?} is not a person"));
            }
            narrowing.validate(&format!("assistant.person.{person}"), self)?;
        }
        Ok(())
    }

    /// The personal repository of `person`, its template filled in; `None` without one.
    ///
    /// # Errors
    ///
    /// A message when the template names [`PERSON_PLACEHOLDER`] and `person` is not 1 to 64
    /// lowercase letters, digits, `.`, `_`, `@` and `-` starting with something other than `.`
    /// (two identifiers differing only in case would share a repository on a case-insensitive
    /// filesystem), which [`PERSON_DIGEST_PLACEHOLDER`] avoids.
    pub fn personal_repo_for(&self, person: &str) -> Result<Option<String>, String> {
        let Some(template) = &self.personal_repo else {
            return Ok(None);
        };
        let mut repo = template.replace(PERSON_DIGEST_PLACEHOLDER, &person_digest(person));
        if repo.contains(PERSON_PLACEHOLDER) {
            if !person_safe(person) {
                return Err(format!(
                    "the person's identifier cannot name a repository; give \
                     assistant.personal_repo {PERSON_DIGEST_PLACEHOLDER} instead of \
                     {PERSON_PLACEHOLDER}"
                ));
            }
            repo = repo.replace(PERSON_PLACEHOLDER, person);
        }
        Ok(Some(repo))
    }

    /// The shared repositories a member of `groups` reads, by the name each is checked out
    /// under: the groups' own, then [`ORG_SHARED`].
    pub fn shared_for<'a>(
        &'a self,
        groups: &[String],
    ) -> impl Iterator<Item = (&'a str, &'a SharedRepo)> {
        let groups = groups.to_vec();
        self.shared
            .iter()
            .filter(move |(group, _)| groups.contains(group))
            .map(|(group, repo)| (group.as_str(), repo))
            .chain(self.org.iter().map(|repo| (ORG_SHARED, repo)))
    }

    /// The shared repository `name` ([`ORG_SHARED`] or a group) a member of `groups` reads.
    pub fn shared_named(&self, name: &str, groups: &[String]) -> Option<&SharedRepo> {
        self.shared_for(groups)
            .find(|(shared, _)| *shared == name)
            .map(|(_, repo)| repo)
    }

    /// The settings looser than their defaults, each in a sentence an operator can act on.
    pub fn weakenings(&self) -> Vec<String> {
        let mut weakened = Vec::new();
        if self.allow_elevated {
            weakened.push(
                "assistant.allow_elevated = true: Moneypenny's harness runs on a Station \
                 running elevated or as root"
                    .to_owned(),
            );
        }
        for (group, shared) in &self.shared {
            if shared.write {
                weakened.push(format!(
                    "assistant.shared.{group}.write = true: Moneypenny commits a member's \
                     change to {group:?}'s repository without asking"
                ));
            }
        }
        if self.org.as_ref().is_some_and(|org| org.write) {
            weakened.push(
                "assistant.org.write = true: Moneypenny commits anyone's change to the \
                 organisation's repository without asking"
                    .to_owned(),
            );
        }
        if self.network == NetworkPolicy::Open {
            weakened.push(
                "assistant.network = \"open\": Moneypenny's web tools and scripts reach any \
                 host, and she may search the web"
                    .to_owned(),
            );
        }
        if self.unsandboxed_scripts {
            weakened.push(
                "assistant.unsandboxed_scripts = true: on a Station whose harness has no \
                 sandbox, Moneypenny's scripts run with no file or network limit"
                    .to_owned(),
            );
        }
        if self.skills_unpinned {
            weakened.push(
                "assistant.skills_unpinned = true: a catalog skill without a pin is installed \
                 for Moneypenny as its directory holds it"
                    .to_owned(),
            );
        }
        if self.skills_allow_tools {
            weakened.push(
                "assistant.skills_allow_tools = true: Moneypenny's skills may grant her tools, \
                 register hooks and run commands as they load"
                    .to_owned(),
            );
        }
        for (name, skill) in &self.skills {
            if let Some(pin) = skill.pin.filter(|pin| pin.digest() == PinDigest::Sha1) {
                weakened.push(format!("assistant.skills.{name}.pin is SHA-1 ({pin})"));
            }
        }
        weakened
    }

    /// What replacing `current` with these settings loosens or redirects, each in a sentence
    /// an operator can act on: a weakening `current` does not have, an interpreter, host or
    /// network policy it does not allow, a personal, shared or organisation repository that
    /// changes, a catalog skill added or with another path or pin, and a group or person
    /// table that is removed or changes a key it set. Empty when nothing does.
    pub fn changes_from(&self, current: &AssistantSettings) -> Vec<String> {
        let mut changes: Vec<String> = {
            let before = current.weakenings();
            self.weakenings()
                .into_iter()
                .filter(|weakened| !before.contains(weakened))
                .collect()
        };
        for name in self
            .interpreters
            .iter()
            .filter(|name| !current.interpreters.contains(name))
        {
            changes.push(format!("assistant.interpreters adds {name:?}"));
        }
        if self.network > current.network {
            changes.push(format!(
                "assistant.network loosens from {:?} to {:?}",
                current.network.as_str(),
                self.network.as_str()
            ));
        }
        for host in self
            .allowed_hosts
            .iter()
            .filter(|host| !current.allowed_hosts.contains(host))
        {
            changes.push(format!("assistant.allowed_hosts adds {host:?}"));
        }
        if self.personal_repo != current.personal_repo {
            changes.push(format!(
                "assistant.personal_repo changes from {:?} to {:?}: each person's work and \
                 memory go to another repository",
                current.personal_repo.as_deref().unwrap_or("none"),
                self.personal_repo.as_deref().unwrap_or("none")
            ));
        }
        let repos = |settings: &AssistantSettings| -> BTreeMap<String, String> {
            settings
                .shared
                .iter()
                .map(|(group, repo)| (format!("assistant.shared.{group}"), repo.repo.clone()))
                .chain(
                    settings
                        .org
                        .iter()
                        .map(|repo| ("assistant.org".to_owned(), repo.repo.clone())),
                )
                .collect()
        };
        let before = repos(current);
        for (key, repo) in repos(self) {
            match before.get(&key) {
                Some(was) if *was == repo => {}
                Some(was) => changes.push(format!("{key}.repo changes from {was:?} to {repo:?}")),
                None => changes.push(format!("{key}.repo is added: {repo:?}")),
            }
        }
        for (name, skill) in &self.skills {
            match current.skills.get(name) {
                None => changes.push(format!("assistant.skills.{name} is added")),
                Some(was) if was.path != skill.path => changes.push(format!(
                    "assistant.skills.{name}.path changes from {:?} to {:?}",
                    was.path, skill.path
                )),
                Some(was) if was.pin != skill.pin => changes.push(format!(
                    "assistant.skills.{name}.pin changes from {} to {}",
                    was.pin.map_or("none".to_owned(), |pin| pin.to_string()),
                    skill.pin.map_or("none".to_owned(), |pin| pin.to_string())
                )),
                Some(_) => {}
            }
        }
        for (kind, now, before) in [
            ("group", &self.group, &current.group),
            ("person", &self.person, &current.person),
        ] {
            for (name, was) in before {
                let key = format!("assistant.{kind}.{name}");
                let Some(table) = now.get(name) else {
                    changes.push(format!("{key} is removed: it no longer narrows anything"));
                    continue;
                };
                let (Ok(serde_json::Value::Object(was)), Ok(table)) =
                    (serde_json::to_value(was), serde_json::to_value(table))
                else {
                    continue;
                };
                for (field, value) in was.iter().filter(|(_, value)| !value.is_null()) {
                    let is = &table[field.as_str()];
                    if is.is_null() {
                        changes.push(format!(
                            "{key}.{field} is removed: it no longer narrows {field}"
                        ));
                    } else if is != value {
                        changes.push(format!("{key}.{field} changes from {value} to {is}"));
                    }
                }
            }
        }
        changes
    }
}

/// 16 hex digits of the SHA-256 of `person`, the same everywhere.
pub fn person_digest(person: &str) -> String {
    Sha256::digest(person.as_bytes())
        .iter()
        .take(8)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Whether `person` may stand in a path or URL as it is: lowercase only, so no two
/// identifiers name one repository on a case-insensitive filesystem.
fn person_safe(person: &str) -> bool {
    (1..=64).contains(&person.len())
        && !person.starts_with('.')
        && person.chars().all(|c| {
            c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '@' | '-')
        })
}

/// Whether `name` is a safe directory name: 1 to 64 of letters, digits, `.`, `-` and `_`,
/// not starting with `.`.
pub fn file_safe(name: &str) -> bool {
    (1..=64).contains(&name.len())
        && !name.starts_with('.')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::directorates::m::harness::ProjectManifest;

    #[test]
    fn the_default_keeps_nothing_and_refuses_elevation() {
        let settings = AssistantSettings::default();
        assert_eq!(settings.personal_repo_for("007"), Ok(None));
        assert!(!settings.allow_elevated());
        assert!(settings.weakenings().is_empty());
        assert_eq!(settings.shared_for(&["ops".to_owned()]).count(), 0);
    }

    #[test]
    fn scripts_default_to_python3_in_the_sandbox_reaching_no_host() {
        let settings = AssistantSettings::default();
        assert_eq!(settings.interpreters(), ["python3"]);
        assert_eq!(settings.network(), NetworkPolicy::Limited);
        assert!(!settings.unsandboxed_scripts());
        let reach = settings.reach_for("007", &[]);
        assert_eq!(reach.interpreters, ["python3"]);
        assert!(reach.network.reaches_nothing());
        assert!(!reach.unsandboxed_scripts);
        let empty: AssistantSettings = toml::from_str("").unwrap();
        assert_eq!(empty, settings, "an empty table is the default");
    }

    #[test]
    fn group_and_person_tables_narrow_and_a_widening_one_is_refused_by_key() {
        let toml = r#"
            interpreters = ["python3", "node"]
            network = "limited"
            allowed_hosts = ["docs.python.org", "pypi.org"]
            unsandboxed_scripts = true
            [group.contractors]
            network = "denied"
            [group.analysts]
            interpreters = ["python3"]
            allowed_hosts = ["pypi.org"]
            [person."eve@sis"]
            unsandboxed_scripts = false
        "#;
        let settings: AssistantSettings = toml::from_str(toml).unwrap();
        assert_eq!(settings.validate(), Ok(()));
        let analyst = settings.reach_for("q", &["analysts".to_owned()]);
        assert_eq!(analyst.interpreters, ["python3"]);
        assert_eq!(analyst.network.policy, "limited");
        assert_eq!(analyst.network.allowed_hosts, ["pypi.org"]);
        assert!(analyst.unsandboxed_scripts);
        let both = settings.reach_for("q", &["analysts".to_owned(), "contractors".to_owned()]);
        assert_eq!(both.network.policy, "denied", "the strictest table wins");
        assert_eq!(both.interpreters, ["python3"]);
        let eve = settings.reach_for("eve@sis", &[]);
        assert_eq!(eve.interpreters, ["python3", "node"]);
        assert_eq!(eve.network.allowed_hosts, ["docs.python.org", "pypi.org"]);
        assert!(!eve.unsandboxed_scripts);
        let outsider = settings.reach_for("m", &["nobody".to_owned()]);
        assert_eq!(outsider, settings.deployment_reach());

        for (table, key) in [
            (
                "[group.ops]\nnetwork = \"open\"",
                "assistant.group.ops.network",
            ),
            (
                "[group.ops]\ninterpreters = [\"bash\"]",
                "assistant.group.ops.interpreters",
            ),
            (
                "[group.ops]\nallowed_hosts = [\"evil.example\"]",
                "assistant.group.ops.allowed_hosts",
            ),
            (
                "[person.eve]\nnetwork = \"open\"",
                "assistant.person.eve.network",
            ),
        ] {
            let widening =
                format!("interpreters = [\"python3\"]\nallowed_hosts = [\"pypi.org\"]\n{table}");
            let settings: AssistantSettings = toml::from_str(&widening).unwrap();
            let refused = settings.validate().unwrap_err();
            assert!(refused.starts_with(key), "{refused}");
        }
        let unsandboxed: AssistantSettings =
            toml::from_str("[group.ops]\nunsandboxed_scripts = true").unwrap();
        assert!(
            unsandboxed
                .validate()
                .unwrap_err()
                .starts_with("assistant.group.ops.unsandboxed_scripts")
        );
        // Under an open deployment a table may list any hosts: that narrows it.
        let open: AssistantSettings =
            toml::from_str("network = \"open\"\n[group.ops]\nallowed_hosts = [\"a.example\"]")
                .unwrap();
        assert_eq!(open.validate(), Ok(()));
        let ops = open.reach_for("q", &["ops".to_owned()]);
        assert_eq!(ops.network.policy, "limited");
        assert_eq!(ops.network.allowed_hosts, ["a.example"]);
        assert!(toml::from_str::<AssistantSettings>("[group.ops]\nshell = true").is_err());
        for bad in [
            "interpreters = [\"/bin/sh\"]",
            "allowed_hosts = [\"*\"]",
            "allowed_hosts = [\"localhost\"]",
            "network = \"everything\"",
        ] {
            let parsed = toml::from_str::<AssistantSettings>(bad);
            assert!(
                parsed.is_err() || parsed.unwrap().validate().is_err(),
                "{bad}"
            );
        }
    }

    #[test]
    fn an_open_network_and_unsandboxed_scripts_are_weakenings() {
        let loose = AssistantSettings::new()
            .with_network(NetworkPolicy::Open, Vec::new())
            .with_unsandboxed_scripts(true);
        let weakened = loose.weakenings();
        assert_eq!(weakened.len(), 2, "{weakened:?}");
        assert!(weakened[0].starts_with("assistant.network = \"open\""));
        assert!(weakened[1].starts_with("assistant.unsandboxed_scripts = true"));
        let narrowed = AssistantSettings::new()
            .with_network(NetworkPolicy::Denied, Vec::new())
            .with_interpreters(Vec::new());
        assert!(narrowed.weakenings().is_empty());
        assert_eq!(narrowed.validate(), Ok(()));
    }

    #[test]
    fn a_template_names_a_safe_person_or_their_digest() {
        let plain = AssistantSettings::new().with_personal_repo("/srv/mp/{person}.git");
        assert_eq!(
            plain.personal_repo_for("q.branch@sis"),
            Ok(Some("/srv/mp/q.branch@sis.git".to_owned()))
        );
        for unsafe_id in [
            "../root",
            ".hidden",
            "a/b",
            "a b",
            "",
            &"x".repeat(65),
            "Alice",
            "q.Branch",
        ] {
            let refused = plain.personal_repo_for(unsafe_id).unwrap_err();
            assert!(
                refused.contains("{person_digest}"),
                "{unsafe_id:?}: {refused}"
            );
        }
        let hashed = AssistantSettings::new().with_personal_repo("file:///srv/mp/{person_digest}");
        let expanded = hashed.personal_repo_for("../root").unwrap().unwrap();
        assert_eq!(
            expanded,
            format!("file:///srv/mp/{}", person_digest("../root"))
        );
        assert_eq!(person_digest("../root").len(), 16);
        assert_ne!(person_digest("a"), person_digest("b"));
    }

    #[test]
    fn repositories_are_classified_and_helpers_refused() {
        for (repo, kind) in [
            ("/srv/mp/a.git", RepoKind::Local),
            ("C:\\srv\\mp\\a.git", RepoKind::Local),
            ("C:/srv/mp/a.git", RepoKind::Local),
            ("file:///srv/a.git", RepoKind::Local),
            ("ssh://git@host/a.git", RepoKind::Remote),
            ("https://host/a.git", RepoKind::Remote),
            ("git@host:org/a.git", RepoKind::Remote),
        ] {
            assert_eq!(RepoKind::of(repo), Ok(kind), "{repo}");
        }
        for repo in ["ext::sh -c touch% /tmp/x", "fd::3", "gopher://x", "", " a"] {
            assert!(RepoKind::of(repo).is_err(), "{repo:?}");
        }
    }

    #[test]
    fn what_a_record_changes_is_listed_against_the_current_one() {
        let current: AssistantSettings = toml::from_str(
            "personal_repo = \"/srv/mp/{person_digest}.git\"\n\
             network = \"denied\"\n\
             allow_elevated = true\n\
             [shared.ops]\nrepo = \"/srv/ops.git\"\n\
             [skills.report]\npath = \"/skills/report\"\n\
             [group.contractors]\ninterpreters = []\nnetwork = \"denied\"\n\
             [person.\"eve@sis\"]\nskills = []\n",
        )
        .unwrap();
        assert!(current.changes_from(&current).is_empty());
        // Weakening nothing against the defaults, yet loosening and redirecting what is set.
        let next: AssistantSettings = toml::from_str(
            "personal_repo = \"/srv/elsewhere/{person_digest}.git\"\n\
             network = \"limited\"\n\
             allowed_hosts = [\"docs.python.org\"]\n\
             allow_elevated = true\n\
             [shared.ops]\nrepo = \"/srv/ops-2.git\"\n\
             [org]\nrepo = \"/srv/org.git\"\n\
             [skills.report]\npath = \"/skills/report-2\"\n\
             [skills.extra]\npath = \"/skills/extra\"\n\
             [group.contractors]\nnetwork = \"denied\"\n",
        )
        .unwrap();
        assert!(
            next.weakenings()
                .iter()
                .all(|weakened| weakened.contains("allow_elevated")),
            "{:?}",
            next.weakenings()
        );
        let changes = next.changes_from(&current);
        for expected in [
            "assistant.network loosens from \"denied\" to \"limited\"",
            "assistant.allowed_hosts adds \"docs.python.org\"",
            "assistant.personal_repo changes from \"/srv/mp/{person_digest}.git\" to",
            "assistant.shared.ops.repo changes from \"/srv/ops.git\" to \"/srv/ops-2.git\"",
            "assistant.org.repo is added",
            "assistant.skills.report.path changes",
            "assistant.skills.extra is added",
            "assistant.group.contractors.interpreters is removed",
            "assistant.person.eve@sis is removed",
        ] {
            assert!(
                changes.iter().any(|change| change.starts_with(expected)),
                "{expected}: {changes:#?}"
            );
        }
        // A weakening already in force is not news.
        assert!(
            !changes
                .iter()
                .any(|change| change.contains("allow_elevated")),
            "{changes:#?}"
        );
        assert_eq!(changes.len(), 9, "{changes:#?}");
    }

    #[test]
    fn settings_are_checked_and_weakenings_listed() {
        let toml = r#"
            personal_repo = "/srv/mp/{person}.git"
            allow_elevated = true
            [shared.ops]
            repo = "git@host:ops/handbook.git"
            write = true
            [shared.field]
            repo = "/srv/field.git"
            [org]
            repo = "https://host/org.git"
        "#;
        let settings: AssistantSettings = toml::from_str(toml).unwrap();
        assert_eq!(settings.validate(), Ok(()));
        let weakened = settings.weakenings();
        assert_eq!(weakened.len(), 2, "{weakened:?}");
        assert!(weakened[0].contains("assistant.allow_elevated"));
        assert!(weakened[1].contains("assistant.shared.ops.write"));
        let read: Vec<_> = settings
            .shared_for(&["field".to_owned(), "nobody".to_owned()])
            .map(|(name, repo)| (name, repo.repo()))
            .collect();
        assert_eq!(
            read,
            [("field", "/srv/field.git"), ("org", "https://host/org.git")]
        );
        assert!(
            settings
                .shared_named("ops", &["field".to_owned()])
                .is_none()
        );

        for good in [
            "personal_repo = \"/srv/{person}.git\"\n[shared.ops]\nrepo = \"/srv/ops/h.git\"",
            "personal_repo = \"ssh://git@host/people/{person_digest}.git\"",
            "personal_repo = \"git@host:people/{person}.git\"",
            "personal_repo = \"file:///srv/{person}.git\"",
        ] {
            let settings: AssistantSettings = toml::from_str(good).unwrap();
            assert_eq!(settings.validate(), Ok(()), "{good}");
        }
        for bad in [
            // The person exactly once, inside the path.
            "personal_repo = \"/srv/shared.git\"",
            "personal_repo = \"/srv/{person}/{person_digest}.git\"",
            "personal_repo = \"/srv/{person}-{person}.git\"",
            "personal_repo = \"https://{person}.example/x.git\"",
            "personal_repo = \"ssh://{person}@host/x.git\"",
            "personal_repo = \"{person}@host:x.git\"",
            // Never a shared repository, for any person.
            "personal_repo = \"/srv/{person}.git\"\n[shared.ops]\nrepo = \"/srv/ops.git\"",
            "personal_repo = \"/srv/{person_digest}.git\"\n[org]\nrepo = \"/SRV/0123456789abcdef.git\"",
            "personal_repo = \"/srv/{user}.git\"",
            "personal_repo = \"ext::sh\"",
            "[shared.org]\nrepo = \"/srv/x.git\"",
            "[shared.\"../x\"]\nrepo = \"/srv/x.git\"",
        ] {
            let settings: AssistantSettings = toml::from_str(bad).unwrap();
            assert!(settings.validate().is_err(), "{bad}");
        }
        assert!(toml::from_str::<AssistantSettings>("shell = true").is_err());
    }

    /// Writes a skill directory `name` under `root` whose `SKILL.md` has `front` after its
    /// name and description, and `body`.
    fn skill(root: &Path, name: &str, front: &str, body: &str) -> String {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: Writes reports.\n{front}---\n\n{body}\n"),
        )
        .unwrap();
        dir.to_string_lossy().into_owned()
    }

    fn pin_of(dir: &str) -> PeerId {
        ProjectManifest::read_tree(Path::new(dir)).unwrap().digest()
    }

    #[test]
    fn a_catalog_skill_needs_a_pin_and_a_skill_md_that_grants_nothing() {
        let root = tempfile::tempdir().unwrap();
        let plain = skill(root.path(), "report-writer", "", "Write a report.");
        let pinned = AssistantSettings::new().with_skill(
            "report-writer",
            SkillEntry::new(&plain).pinned(pin_of(&plain)),
        );
        assert_eq!(pinned.validate(), Ok(()));
        assert!(pinned.weakenings().is_empty());

        let unpinned =
            AssistantSettings::new().with_skill("report-writer", SkillEntry::new(&plain));
        let refused = unpinned.validate().unwrap_err();
        assert!(
            refused.contains("assistant.skills.report-writer") && refused.contains("no pin"),
            "{refused}"
        );
        let loosened = unpinned.with_skills_unpinned(true);
        assert_eq!(loosened.validate(), Ok(()));
        assert!(loosened.weakenings()[0].contains("skills_unpinned"));

        // Each way a skill can grant tools or run commands names the skill, and loads only
        // with skills_allow_tools.
        for (name, front, body) in [
            ("tools", "allowed-tools: Read, Bash(git *)\n", "Use git."),
            ("hooks", "hooks:\n  Stop: []\n", "Hooked."),
            ("model", "model: opus\n", "Think hard."),
            ("merged", "<<: {allowed-tools: Bash}\n", "Merged."),
            ("inline", "", "Status: !`git status`"),
            ("fenced", "", "```!\ngit status\n```"),
            ("tilde", "", "  ~~~~ !\ncurl x\n~~~~"),
        ] {
            let dir = skill(root.path(), name, front, body);
            let catalog = |allow: bool| {
                AssistantSettings::new()
                    .with_skill(name, SkillEntry::new(&dir).pinned(pin_of(&dir)))
                    .with_skills_allow_tools(allow)
            };
            let refused = catalog(false).validate().unwrap_err();
            assert!(
                refused.contains(&format!("assistant.skills.{name}"))
                    && refused.contains("skills_allow_tools"),
                "{name}: {refused}"
            );
            assert_eq!(catalog(true).validate(), Ok(()), "{name}");
            assert!(
                catalog(true).weakenings()[0].contains("skills_allow_tools"),
                "{name}"
            );
        }

        // SKILL.md must be what the catalog says it is.
        for (front, why) in [
            ("name: other\ndescription: x\n", "name: wrong"),
            ("name: wrong\n", "description"),
            ("- a\n", "mapping"),
        ] {
            let dir = root.path().join("wrong");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("SKILL.md"), format!("---\n{front}---\nBody\n")).unwrap();
            let dir = dir.to_string_lossy().into_owned();
            let settings = AssistantSettings::new()
                .with_skill("wrong", SkillEntry::new(&dir).pinned(pin_of(&dir)))
                .with_skills_allow_tools(true);
            assert!(settings.validate().is_err(), "{why}");
        }
        std::fs::write(
            root.path().join("wrong").join("SKILL.md"),
            "no front matter",
        )
        .unwrap();
        let dir = root.path().join("wrong").to_string_lossy().into_owned();
        let settings = AssistantSettings::new()
            .with_skill("wrong", SkillEntry::new(&dir).pinned(pin_of(&dir)));
        assert!(settings.validate().unwrap_err().contains("front matter"));

        // A path a SQL roster or a builder gives must be absolute; a name must be a skill name.
        let relative = AssistantSettings::new().with_skill(
            "report-writer",
            SkillEntry::new("skills/report-writer").pinned(pin_of(&plain)),
        );
        assert!(relative.validate().unwrap_err().contains("absolute"));
        let mut resolved = relative.clone();
        resolved.resolve_paths(root.path());
        std::fs::rename(&plain, root.path().join("skills-dir")).unwrap();
        std::fs::create_dir_all(root.path().join("skills")).unwrap();
        std::fs::rename(
            root.path().join("skills-dir"),
            root.path().join("skills").join("report-writer"),
        )
        .unwrap();
        assert_eq!(resolved.validate(), Ok(()));
        let named = AssistantSettings::new().with_skill(
            "Report_Writer",
            SkillEntry::new(resolved.skills()["report-writer"].path()),
        );
        assert!(named.validate().is_err());
    }

    #[test]
    fn group_and_person_tables_only_narrow_the_skills_a_person_gets() {
        let root = tempfile::tempdir().unwrap();
        let mut settings = AssistantSettings::new();
        for name in ["charts", "report-writer", "slides"] {
            let dir = skill(root.path(), name, "", "Do it.");
            settings = settings.with_skill(name, SkillEntry::new(&dir).pinned(pin_of(&dir)));
        }
        let settings = settings
            .with_group(
                "contractors",
                AssistantNarrowing::new().with_skills(vec!["charts".to_owned()]),
            )
            .with_group(
                "ops",
                AssistantNarrowing::new()
                    .with_skills(vec!["charts".to_owned(), "slides".to_owned()]),
            )
            .with_person("eve", AssistantNarrowing::new().with_skills(Vec::new()));
        assert_eq!(settings.validate(), Ok(()));
        let groups = |names: &[&str]| names.iter().map(|g| (*g).to_owned()).collect::<Vec<_>>();
        assert_eq!(
            settings.skills_for("q", &groups(&[])),
            ["charts", "report-writer", "slides"]
        );
        assert_eq!(
            settings.skills_for("q", &groups(&["ops"])),
            ["charts", "slides"]
        );
        assert_eq!(
            settings.skills_for("q", &groups(&["ops", "contractors"])),
            ["charts"]
        );
        assert!(settings.skills_for("eve", &groups(&[])).is_empty());

        let widening = settings.clone().with_group(
            "field",
            AssistantNarrowing::new().with_skills(vec!["lockpicking".to_owned()]),
        );
        let refused = widening.validate().unwrap_err();
        assert!(
            refused.contains("assistant.group.field.skills") && refused.contains("lockpicking"),
            "{refused}"
        );
        let toml = r#"
            [skills.charts]
            path = "/srv/skills/charts"
            pin = "sha256:0000000000000000000000000000000000000000000000000000000000000000"
            [group.contractors]
            skills = []
            [person."eve@sis"]
            skills = ["charts"]
        "#;
        let parsed: AssistantSettings = toml::from_str(toml).unwrap();
        assert_eq!(parsed.skills_for("eve@sis", &[]), ["charts"]);
        assert!(
            parsed
                .skills_for("eve@sis", &["contractors".to_owned()])
                .is_empty()
        );
        assert!(toml::from_str::<AssistantSettings>("[group.x]\nshell = true").is_err());

        // One table narrows scripts, network and skills together.
        let toml = r#"
            network = "open"
            [skills.charts]
            path = "/srv/skills/charts"
            pin = "sha256:0000000000000000000000000000000000000000000000000000000000000000"
            [group.contractors]
            network = "denied"
            skills = []
        "#;
        let parsed: AssistantSettings = toml::from_str(toml).unwrap();
        let contractors = ["contractors".to_owned()];
        assert!(parsed.skills_for("q", &contractors).is_empty());
        assert!(
            parsed
                .reach_for("q", &contractors)
                .network
                .reaches_nothing()
        );
        assert_eq!(parsed.skills_for("q", &[]), ["charts"]);
        assert_eq!(parsed.reach_for("q", &[]).network.policy, "open");
    }
}
