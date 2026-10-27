//! Moneypenny's deployment settings: the `[assistant]` table of the global `agents.toml`.
//!
//! A person's personal repository is Moneypenny's working directory and memory for that
//! person; each directory group may map to a shared repository she reads, and one more may be
//! shared with everyone (`org`). Without a personal repository she works in an empty
//! directory and keeps nothing between conversations.
//!
//! ```toml
//! [assistant]
//! personal_repo = "/srv/moneypenny/{person}.git"
//! allow_elevated = false
//!
//! [assistant.shared.ops]
//! repo = "ssh://git@git.example/ops/handbook.git"
//! write = false
//!
//! [assistant.org]
//! repo = "https://git.example/org/handbook.git"
//! ```

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// The name a person's identifier takes in `personal_repo`.
pub const PERSON_PLACEHOLDER: &str = "{person}";
/// The name of 16 hex digits of the SHA-256 of a person's identifier in `personal_repo`.
pub const PERSON_DIGEST_PLACEHOLDER: &str = "{person_digest}";
/// The name the organisation-wide shared repository is checked out under.
pub const ORG_SHARED: &str = "org";

/// The `[assistant]` table: where Moneypenny keeps each person's work and memory, what she
/// reads, and whether she may run on an elevated Station. The default keeps nothing, shares
/// nothing and refuses elevated Stations.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AssistantSettings {
    personal_repo: Option<String>,
    shared: BTreeMap<String, SharedRepo>,
    org: Option<SharedRepo>,
    allow_elevated: bool,
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

impl AssistantSettings {
    /// Settings that keep nothing, share nothing and refuse elevated Stations.
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

    /// Checks every repository and group name: repositories of a known [`RepoKind`], a
    /// template whose only placeholders are the two person ones, and group names of 1 to 64
    /// letters, digits, `.`, `-` and `_`, not starting with `.` and not [`ORG_SHARED`].
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
        weakened
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

    #[test]
    fn the_default_keeps_nothing_and_refuses_elevation() {
        let settings = AssistantSettings::default();
        assert_eq!(settings.personal_repo_for("007"), Ok(None));
        assert!(!settings.allow_elevated());
        assert!(settings.weakenings().is_empty());
        assert_eq!(settings.shared_for(&["ops".to_owned()]).count(), 0);
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

        for bad in [
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
}
