//! Whether a harness may load the settings a working directory carries.
//!
//! A checkout can configure the harness that works in it: Claude Code reads hooks and MCP
//! servers from `.claude/settings.json`, `.claude/settings.local.json` and `.mcp.json`, and
//! Codex its `.codex/` layer. In a Mission driven by an issue, whoever wrote the branch
//! could write those. An Operation's [`HarnessPolicy`] decides: `ignore` (the default)
//! never loads them, `pinned` loads them only when each file matches the Operation's pin,
//! and `load` loads whatever is there, a reported weakening. AI6's own settings (the green
//! ink hook, the Station's MCP servers) are passed on the command line and apply either way.
//!
//! ```toml
//! [operation.G.harness]
//! project_settings = "pinned"
//! pins = { ".claude/settings.json" = "sha256:...", ".mcp.json" = "sha256:..." }
//! ```

use std::collections::BTreeMap;
use std::path::Path;

use coqui_peer::PeerId;
use serde::{Deserialize, Serialize};

/// The project settings files a harness reads from its working directory, relative to it.
pub const PROJECT_SETTINGS_FILES: [&str; 5] = [
    ".claude/settings.json",
    ".claude/settings.local.json",
    ".mcp.json",
    ".codex/config.toml",
    ".codex/hooks.json",
];

/// What a harness may do with the working directory's own settings.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProjectSettings {
    /// Never load them.
    #[default]
    Ignore,
    /// Load them only when every settings file matches its pin.
    Pinned,
    /// Load whatever is there.
    Load,
}

impl ProjectSettings {
    /// `ignore`, `pinned` or `load`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ignore => "ignore",
            Self::Pinned => "pinned",
            Self::Load => "load",
        }
    }
}

/// An Operation's harness policy (`[operation.ID.harness]`); `ignore` unless configured.
#[non_exhaustive]
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HarnessPolicy {
    /// What a harness may do with the working directory's own settings.
    #[serde(default)]
    pub project_settings: ProjectSettings,
    /// Under `pinned`, the digest each settings file must have, by its path relative to the
    /// working directory. A settings file with no pin must be absent.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub pins: BTreeMap<String, PeerId>,
}

impl HarnessPolicy {
    /// The policy that loads project settings when `pins` match.
    pub fn pinned(pins: impl IntoIterator<Item = (String, PeerId)>) -> Self {
        Self {
            project_settings: ProjectSettings::Pinned,
            pins: pins.into_iter().collect(),
        }
    }

    /// The policy with `project_settings` and no pins.
    pub fn with(project_settings: ProjectSettings) -> Self {
        Self {
            project_settings,
            pins: BTreeMap::new(),
        }
    }

    /// Checks the pins' paths: relative, inside the working directory, no `..`.
    ///
    /// # Errors
    ///
    /// Names the first path that is not.
    pub fn validate(&self) -> Result<(), String> {
        for path in self.pins.keys() {
            if path.is_empty()
                || path.starts_with(['/', '\\'])
                || path.contains(':')
                || path.split(['/', '\\']).any(|part| part == "..")
            {
                return Err(format!(
                    "harness pin {path:?} must be a path inside the working directory"
                ));
            }
        }
        Ok(())
    }

    /// How this policy is looser than the default, if it is.
    pub fn weakening(&self) -> Option<String> {
        (self.project_settings == ProjectSettings::Load).then(|| {
            "harness.project_settings = \"load\": a harness loads the hooks and MCP servers its \
             working directory carries"
                .to_owned()
        })
    }

    /// Whether a harness working in `root` may load its project settings; `Err` says why
    /// not when the policy is `pinned`. Reads the settings files.
    ///
    /// # Errors
    ///
    /// Under `ignore`, names the policy; under `pinned`, names each file that differs from
    /// its pin, is missing, or is present with no pin.
    pub fn check(&self, root: &Path) -> Result<(), String> {
        match self.project_settings {
            ProjectSettings::Load => Ok(()),
            ProjectSettings::Ignore => Err("the Operation ignores project settings".to_owned()),
            ProjectSettings::Pinned => {
                let mut paths: Vec<&str> = PROJECT_SETTINGS_FILES.to_vec();
                paths.extend(self.pins.keys().map(String::as_str));
                paths.sort_unstable();
                paths.dedup();
                let mut differences = Vec::new();
                for path in paths {
                    let found = std::fs::read(root.join(path)).ok().map(|bytes| {
                        self.pins
                            .get(path)
                            .map_or_else(|| PeerId::of(&bytes), |pin| pin.digest().of(&bytes))
                    });
                    match (self.pins.get(path), found) {
                        (Some(pin), Some(found)) if *pin == found => {}
                        (Some(pin), Some(found)) => {
                            differences.push(format!("{path}: expected {pin}, found {found}"));
                        }
                        (Some(pin), None) => {
                            differences.push(format!("{path}: expected {pin}, found nothing"));
                        }
                        (None, Some(found)) => {
                            differences.push(format!("{path}: not pinned ({found})"));
                        }
                        (None, None) => {}
                    }
                }
                if differences.is_empty() {
                    Ok(())
                } else {
                    Err(differences.join("; "))
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pinned_settings_load_only_while_every_file_matches_its_pin() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join(".claude")).unwrap();
        let settings = br#"{"hooks":{}}"#;
        std::fs::write(root.path().join(".claude/settings.json"), settings).unwrap();
        let policy =
            HarnessPolicy::pinned([(".claude/settings.json".to_owned(), PeerId::of(settings))]);
        assert_eq!(policy.check(root.path()), Ok(()));

        std::fs::write(root.path().join(".mcp.json"), "{}").unwrap();
        let why = policy.check(root.path()).unwrap_err();
        assert!(why.contains(".mcp.json: not pinned (sha256:"), "{why}");
        std::fs::remove_file(root.path().join(".mcp.json")).unwrap();

        std::fs::write(
            root.path().join(".claude/settings.json"),
            br#"{"hooks":{"PreToolUse":[]}}"#,
        )
        .unwrap();
        let why = policy.check(root.path()).unwrap_err();
        assert!(
            why.starts_with(&format!(
                ".claude/settings.json: expected {}, found sha256:",
                PeerId::of(settings)
            )),
            "{why}"
        );
    }

    #[test]
    fn ignore_is_the_default_and_load_is_a_weakening() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(
            HarnessPolicy::default().project_settings,
            ProjectSettings::Ignore
        );
        assert!(HarnessPolicy::default().check(root.path()).is_err());
        assert!(HarnessPolicy::default().weakening().is_none());
        let load = HarnessPolicy::with(ProjectSettings::Load);
        assert_eq!(load.check(root.path()), Ok(()));
        assert!(load.weakening().is_some());
        assert!(
            HarnessPolicy::pinned([("../escape".to_owned(), PeerId::of(b""))])
                .validate()
                .is_err()
        );
    }
}
