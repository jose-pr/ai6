//! Operation cover policies for Agent-authored work.

use std::fs;
use std::path::Path;

use regex::{Regex, RegexBuilder};
use serde::Deserialize;

use crate::directorates::m::roster::OperationIdentity;

const COVERS_FILE: &str = "covers.toml";

/// One commit inspected by a Operation cover policy.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoverCommit {
    /// The full commit identifier.
    pub sha: String,
    /// Commit author in `Name <email>` form.
    pub author: String,
    /// Commit committer in `Name <email>` form.
    pub committer: String,
    /// Full commit message.
    pub message: String,
}

/// One rule violated by one commit.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CoverViolation {
    /// The first twelve characters of the offending commit ID.
    pub commit: String,
    /// The violated rule name.
    pub rule: String,
    /// The offending value or expected value.
    pub detail: String,
}

/// Summary of one Mission's cover audit.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CoverReport {
    /// Policy name applied to the Mission.
    pub policy: String,
    /// Number of commits inspected.
    pub commits: usize,
    /// Whether any commit violated the policy.
    pub blown: bool,
    /// Policy violations, in commit and rule order.
    pub violations: Vec<CoverViolation>,
}

/// Snapshot of the cover settings needed to audit a Mission after restart.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct CoverAudit {
    pub(crate) policy: String,
    pub(crate) base: String,
    pub(crate) identity: Option<OperationIdentity>,
    pub(crate) fail_on_blow: bool,
}

/// A validated cover policy.
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct Cover {
    name: String,
    description: String,
    agent_author: bool,
    trailers: Vec<String>,
    suggest: String,
    model: bool,
    footer: String,
    forbidden: Vec<Regex>,
    announce: bool,
}

impl Cover {
    /// Returns the cover's name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Builds the instructions added to the Agent's initial Mission context.
    pub fn briefing(
        &self,
        identity: Option<&OperationIdentity>,
        agent: &str,
        model: &str,
    ) -> String {
        let mut lines = vec![
            format!("Cover: {}. {}", self.name, self.description)
                .trim_end()
                .to_owned(),
        ];
        if let Some(identity) = identity {
            lines.push(format!(
                "Commit as {} <{}>; Git is set up for it.",
                identity.name, identity.email
            ));
        }
        if self.trailers.is_empty() {
            lines.push("Add no Co-Authored-By or other attribution trailer to commits.".to_owned());
        } else if !self.trailers.iter().any(|trailer| trailer == "*") {
            lines.push(format!(
                "The only attribution trailers allowed: {}.",
                self.trailers.join(", ")
            ));
        }
        if !self.suggest.is_empty() {
            lines.push(format!(
                "End each commit message with: {}",
                fill(&self.suggest, agent, model)
            ));
        }
        if !self.model {
            lines.push(
                "Do not name the model you run on in commits, comments or pull requests."
                    .to_owned(),
            );
        }
        if !self.announce {
            lines.push("Do not write \"generated with\" or similar notices.".to_owned());
        }
        if !self.footer.is_empty() {
            lines.push(format!("Every commit message ends with: {}", self.footer));
        }
        lines.join("\n")
    }

    /// Checks commit identity and message content against this cover.
    pub fn check_commits(
        &self,
        commits: &[CoverCommit],
        identity: Option<&OperationIdentity>,
        model: &str,
    ) -> Vec<CoverViolation> {
        let mut violations = Vec::new();
        for commit in commits {
            let short_sha = commit.sha.chars().take(12).collect::<String>();
            if let Some(identity) = identity.filter(|_| !self.agent_author) {
                let expected = format!("{} <{}>", identity.name, identity.email);
                for (role, value) in [("author", &commit.author), ("committer", &commit.committer)]
                {
                    if !value.is_empty() && value != &expected {
                        violations.push(violation(
                            &short_sha,
                            role,
                            format!("{value}, not {expected}"),
                        ));
                    }
                }
            }
            for (key, value) in trailers(&commit.message) {
                if is_attribution_trailer(key)
                    && !self
                        .trailers
                        .iter()
                        .any(|allowed| allowed == "*" || allowed.eq_ignore_ascii_case(key))
                {
                    violations.push(violation(&short_sha, "trailer", format!("{key}: {value}")));
                }
            }
            if !self.model
                && !model.is_empty()
                && commit
                    .message
                    .to_lowercase()
                    .contains(&model.to_lowercase())
            {
                violations.push(violation(
                    &short_sha,
                    "model",
                    format!("names the model {model}"),
                ));
            }
            if !self.announce
                && let Some(found) = announcement_regex().find(&commit.message)
            {
                violations.push(violation(
                    &short_sha,
                    "announce",
                    found.as_str().trim().to_owned(),
                ));
            }
            for pattern in &self.forbidden {
                if let Some(found) = pattern.find(&commit.message) {
                    violations.push(violation(&short_sha, "forbid", found.as_str().to_owned()));
                }
            }
            if !self.footer.is_empty()
                && !commit.message.trim_end().ends_with(self.footer.trim_end())
            {
                violations.push(violation(
                    &short_sha,
                    "footer",
                    format!("does not end with {:?}", self.footer),
                ));
            }
        }
        violations
    }
}

/// The built-in and configuration-defined cover policies.
#[derive(Debug, Clone)]
pub struct Covers {
    covers: std::collections::BTreeMap<String, Cover>,
}

impl Default for Covers {
    fn default() -> Self {
        let mut covers = std::collections::BTreeMap::new();
        covers.insert(
            "covert".to_owned(),
            Cover {
                name: "covert".to_owned(),
                description: "Nothing in the work says an Agent did it.".to_owned(),
                agent_author: false,
                trailers: Vec::new(),
                suggest: String::new(),
                model: false,
                footer: String::new(),
                forbidden: forbidden(&[r"claude\.ai/code/session"]),
                announce: false,
            },
        );
        covers.insert(
            "partial".to_owned(),
            Cover {
                name: "partial".to_owned(),
                description: "The work may say AI helped, but never which model or Agent."
                    .to_owned(),
                agent_author: false,
                trailers: vec!["Assisted-by".to_owned()],
                suggest: "Assisted-by: AI".to_owned(),
                model: false,
                footer: String::new(),
                forbidden: forbidden(&[r"claude\.ai/code/session"]),
                announce: false,
            },
        );
        covers.insert(
            "overt".to_owned(),
            Cover {
                name: "overt".to_owned(),
                description: "Full disclosure: the Agent and its model may be named.".to_owned(),
                agent_author: true,
                trailers: vec!["*".to_owned()],
                suggest: String::new(),
                model: true,
                footer: String::new(),
                forbidden: Vec::new(),
                announce: true,
            },
        );
        Self { covers }
    }
}

impl Covers {
    /// Loads `covers.toml` from a configuration directory; missing files use built-ins.
    pub fn load(config_dir: impl AsRef<Path>) -> Result<Self, CoverError> {
        let path = config_dir.as_ref().join(COVERS_FILE);
        if !path.exists() {
            return Ok(Self::default());
        }
        let source = fs::read_to_string(&path).map_err(|error| {
            CoverError(format!(
                "{}: could not read cover policies: {error}",
                path.display()
            ))
        })?;
        Self::parse_toml(&source)
            .map_err(|error| CoverError(format!("{}: {error}", path.display())))
    }

    /// Parses custom cover policies, replacing built-ins with matching names.
    pub fn parse_toml(source: &str) -> Result<Self, CoverError> {
        let document: CoversDocument = toml::from_str(source)
            .map_err(|error| CoverError(format!("invalid cover policy: {error}")))?;
        let mut covers = Self::default().covers;
        for (name, definition) in document.cover {
            let cover = definition.resolve(name.clone())?;
            covers.insert(name, cover);
        }
        Ok(Self { covers })
    }

    /// Finds a named cover policy.
    pub fn get(&self, name: &str) -> Result<&Cover, CoverError> {
        self.covers.get(name).ok_or_else(|| {
            CoverError(format!(
                "no cover {name:?} (covers: {})",
                self.covers.keys().cloned().collect::<Vec<_>>().join(", ")
            ))
        })
    }

    pub(crate) fn from_config_dir(config_dir: impl AsRef<Path>) -> Result<Self, CoverError> {
        Self::load(config_dir)
    }

    /// Returns available cover names in sorted order.
    pub fn names(&self) -> Vec<&str> {
        self.covers.keys().map(String::as_str).collect()
    }
}

fn forbidden(patterns: &[&str]) -> Vec<Regex> {
    patterns
        .iter()
        .map(|pattern| {
            RegexBuilder::new(pattern)
                .case_insensitive(true)
                .build()
                .expect("built-in cover regex is valid")
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct CoverError(String);

impl std::fmt::Display for CoverError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for CoverError {}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct CoversDocument {
    cover: std::collections::BTreeMap<String, CoverDefinition>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct CoverDefinition {
    description: String,
    agent_author: bool,
    trailers: Vec<String>,
    suggest: String,
    model: bool,
    footer: String,
    forbid: Vec<String>,
    announce: bool,
}

impl CoverDefinition {
    fn resolve(self, name: String) -> Result<Cover, CoverError> {
        let forbidden = self
            .forbid
            .into_iter()
            .map(|pattern| {
                RegexBuilder::new(&pattern)
                    .case_insensitive(true)
                    .build()
                    .map_err(|error| {
                        CoverError(format!("cover {name:?}: invalid forbid pattern: {error}"))
                    })
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Cover {
            name,
            description: self.description,
            agent_author: self.agent_author,
            trailers: self.trailers,
            suggest: self.suggest,
            model: self.model,
            footer: self.footer,
            forbidden,
            announce: self.announce,
        })
    }
}

fn fill(template: &str, agent: &str, model: &str) -> String {
    template.replace("{agent}", agent).replace("{model}", model)
}

fn violation(commit: &str, rule: &str, detail: String) -> CoverViolation {
    CoverViolation {
        commit: commit.to_owned(),
        rule: rule.to_owned(),
        detail,
    }
}

fn trailers(message: &str) -> Vec<(&str, &str)> {
    let Some(last_paragraph) = message
        .trim()
        .rsplit("\n\n")
        .find(|paragraph| !paragraph.trim().is_empty())
    else {
        return Vec::new();
    };
    last_paragraph
        .lines()
        .filter_map(|line| {
            let (key, value) = line.trim().split_once(':')?;
            let mut chars = key.chars();
            (chars
                .next()
                .is_some_and(|first| first.is_ascii_alphabetic())
                && chars.all(|character| character.is_ascii_alphanumeric() || character == '-'))
            .then_some((key, value.trim_start()))
        })
        .collect()
}

fn is_attribution_trailer(key: &str) -> bool {
    matches!(
        key.to_ascii_lowercase().as_str(),
        "co-authored-by" | "generated-by" | "assisted-by" | "ai-assisted" | "written-by"
    ) || key.to_ascii_lowercase().ends_with("-session")
}

fn announcement_regex() -> &'static Regex {
    use std::sync::OnceLock;
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"(?i)generated (with|by) |🤖").expect("built-in announcement regex is valid")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity() -> OperationIdentity {
        OperationIdentity {
            name: "Field Station".to_owned(),
            email: "station@example.invalid".to_owned(),
        }
    }

    fn commit(message: &str, author: &str) -> CoverCommit {
        CoverCommit {
            sha: "a".repeat(40),
            author: author.to_owned(),
            committer: author.to_owned(),
            message: message.to_owned(),
        }
    }

    #[test]
    fn builtin_covers_enforce_identity_disclosure_and_model_rules() {
        let covers = Covers::default();
        let identity = identity();
        let bad = commit(
            "fix: it\n\nGenerated with model-x\n\nCo-Authored-By: Bot <b@example.invalid>",
            "Bot <b@example.invalid>",
        );
        let violations =
            covers
                .get("covert")
                .unwrap()
                .check_commits(&[bad], Some(&identity), "model-x");
        let rules = violations
            .iter()
            .map(|violation| violation.rule.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            rules,
            ["author", "committer", "trailer", "model", "announce"]
        );

        let partial = covers.get("partial").unwrap();
        assert!(
            partial
                .check_commits(
                    &[commit(
                        "fix: it\n\nAssisted-by: AI",
                        "Field Station <station@example.invalid>"
                    )],
                    Some(&identity),
                    "model-x",
                )
                .is_empty()
        );
        let overt = covers.get("overt").unwrap();
        assert!(
            overt
                .check_commits(
                    &[commit(
                        "Generated with model-x",
                        "Bot <bot@example.invalid>"
                    )],
                    None,
                    "model-x"
                )
                .is_empty()
        );
    }

    #[test]
    fn custom_covers_validate_fields_and_patterns() {
        let covers = Covers::parse_toml(
            "[cover.house]\ntrailers = ['Assisted-by']\nfooter = 'Universal Exports'\nforbid = ['internal\\.example']\n",
        )
        .unwrap();
        let cover = covers.get("house").unwrap();
        assert_eq!(cover.name(), "house");
        let bad = commit(
            "fix: it\n\nAssisted-by: AI\n\nSee internal.example\n\nnot footer",
            "Field Station <station@example.invalid>",
        );
        let rules = cover
            .check_commits(&[bad], Some(&identity()), "")
            .into_iter()
            .map(|violation| violation.rule)
            .collect::<Vec<_>>();
        assert_eq!(rules, ["forbid", "footer"]);
        assert!(Covers::parse_toml("[cover.x]\nmodle = true\n").is_err());
        assert!(Covers::parse_toml("[cover.x]\nforbid = ['(']\n").is_err());
        assert!(Covers::default().get("unknown").is_err());
    }

    #[test]
    fn forbidden_patterns_match_without_case_sensitivity() {
        let cover = Covers::default().get("covert").unwrap().clone();
        let violations = cover.check_commits(
            &[commit(
                "docs: add a link https://CLAUDE.AI/CODE/SESSION/secret",
                "Field Station <station@example.invalid>",
            )],
            Some(&identity()),
            "",
        );
        assert_eq!(
            violations
                .iter()
                .map(|v| v.rule.as_str())
                .collect::<Vec<_>>(),
            ["forbid"]
        );
    }

    #[test]
    fn briefing_includes_identity_and_filled_suggestion() {
        let covers = Covers::parse_toml(
            "[cover.house]\ntrailers = ['Assisted-by']\nsuggest = 'Assisted-by: AI ({agent})'\n",
        )
        .unwrap();
        let text = covers
            .get("house")
            .unwrap()
            .briefing(Some(&identity()), "007", "model-x");
        assert!(text.contains("Commit as Field Station <station@example.invalid>"));
        assert!(text.contains("Assisted-by: AI (007)"));
        assert!(text.contains("Do not name the model"));
    }
}
