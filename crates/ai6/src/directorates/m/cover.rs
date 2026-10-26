//! Operation cover policies for Agent-authored work.

use std::fs;
use std::path::Path;

use regex::{Regex, RegexBuilder};
use serde::Deserialize;

use crate::directorates::m::roster::CoverIdentity;
use crate::directorates::q::safehouse::Change;

const COVERS_FILE: &str = "covers.toml";

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
    /// Whether the safe house kept a history to inspect. `false` means the policy could
    /// not be checked, which is not the same as passing it.
    #[serde(default = "checkable")]
    pub checkable: bool,
    /// Whether any commit violated the policy.
    pub blown: bool,
    /// Policy violations, in commit and rule order.
    pub violations: Vec<CoverViolation>,
}

fn checkable() -> bool {
    true
}

/// Snapshot of the cover settings needed to audit a Mission after restart.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct CoverAudit {
    pub(crate) policy: String,
    pub(crate) base: String,
    pub(crate) identity: Option<CoverIdentity>,
    pub(crate) fail_on_blow: bool,
}

/// How much of the work's origin a cover policy lets a Mission disclose outside the Operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Disclosure {
    /// Nothing says an Agent did the work.
    Hidden,
    /// The work may say AI helped, but not which model or Agent.
    Assisted,
    /// The Agent, its model and provider Sessions may be named.
    Full,
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
    /// A cover that hides everything: no Agent author, no attribution trailer, no model
    /// name, no footer and no announcement. The `with_*` methods loosen or add to it.
    pub fn new(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            agent_author: false,
            trailers: Vec::new(),
            suggest: String::new(),
            model: false,
            footer: String::new(),
            forbidden: Vec::new(),
            announce: false,
        }
    }

    /// Whether commits may carry the Agent as author instead of the Operation's cover
    /// identity.
    #[must_use]
    pub fn with_agent_author(mut self, allowed: bool) -> Self {
        self.agent_author = allowed;
        self
    }

    /// The attribution trailers allowed, by key (`Assisted-by`); `*` allows any.
    #[must_use]
    pub fn with_trailers(mut self, trailers: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.trailers = trailers.into_iter().map(Into::into).collect();
        self
    }

    /// The line each commit message is asked to end with; `{agent}` and `{model}` are
    /// filled in.
    #[must_use]
    pub fn with_suggestion(mut self, line: impl Into<String>) -> Self {
        self.suggest = line.into();
        self
    }

    /// Whether the work may name the model.
    #[must_use]
    pub fn with_model(mut self, allowed: bool) -> Self {
        self.model = allowed;
        self
    }

    /// The text every commit message must end with.
    #[must_use]
    pub fn with_footer(mut self, footer: impl Into<String>) -> Self {
        self.footer = footer.into();
        self
    }

    /// Whether the work may say it was generated (`generated with`, a robot emoji).
    #[must_use]
    pub fn with_announce(mut self, allowed: bool) -> Self {
        self.announce = allowed;
        self
    }

    /// Adds patterns no commit message may contain, matched without regard to case.
    ///
    /// # Errors
    ///
    /// Returns an error naming the cover when a pattern is not a valid regular expression.
    pub fn with_forbidden(
        mut self,
        patterns: impl IntoIterator<Item = impl AsRef<str>>,
    ) -> Result<Self, CoverError> {
        for pattern in patterns {
            let compiled = RegexBuilder::new(pattern.as_ref())
                .case_insensitive(true)
                .build()
                .map_err(|error| {
                    CoverError(format!(
                        "cover {:?}: invalid forbid pattern: {error}",
                        self.name
                    ))
                })?;
            self.forbidden.push(compiled);
        }
        Ok(self)
    }

    /// Returns the cover's name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns what a message sent outside the Operation may reveal under this cover.
    pub fn disclosure(&self) -> Disclosure {
        if self.model {
            Disclosure::Full
        } else if self.announce || !self.trailers.is_empty() {
            Disclosure::Assisted
        } else {
            Disclosure::Hidden
        }
    }

    /// Builds the instructions added to the Agent's initial Mission context.
    pub fn briefing(&self, identity: Option<&CoverIdentity>, agent: &str, model: &str) -> String {
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

    /// The message of a safe-house checkpoint commit: it names no Agent or model, and ends
    /// with the cover's footer when it has one, so the commit keeps the cover.
    pub fn checkpoint_message(&self) -> String {
        let subject = "Checkpoint of work in progress";
        if self.footer.is_empty() {
            subject.to_owned()
        } else {
            format!("{subject}\n\n{}", self.footer.trim_end())
        }
    }

    /// Checks commit identity and message content against this cover.
    pub fn check_commits(
        &self,
        commits: &[Change],
        identity: Option<&CoverIdentity>,
        model: &str,
    ) -> Vec<CoverViolation> {
        let mut violations = Vec::new();
        for commit in commits {
            let short_sha = commit.id.chars().take(12).collect::<String>();
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
        let mut covers = Self::default();
        for (name, definition) in document.cover {
            covers.insert(definition.resolve(name)?);
        }
        Ok(covers)
    }

    /// Adds a cover policy, returning the one it replaces with the same name.
    pub fn insert(&mut self, cover: Cover) -> Option<Cover> {
        self.covers.insert(cover.name.clone(), cover)
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

/// A cover policy from the keys of its `[cover.<name>]` table, in any serde format.
#[cfg(any(feature = "sqlite", feature = "postgres"))]
pub(crate) fn cover_from<'de, D>(name: String, definition: D) -> Result<Cover, CoverError>
where
    D: serde::Deserializer<'de>,
    D::Error: std::fmt::Display,
{
    CoverDefinition::deserialize(definition)
        .map_err(|error| CoverError(format!("cover {name:?}: {error}")))?
        .resolve(name)
}

impl CoverDefinition {
    fn resolve(self, name: String) -> Result<Cover, CoverError> {
        Cover::new(name, self.description)
            .with_agent_author(self.agent_author)
            .with_trailers(self.trailers)
            .with_suggestion(self.suggest)
            .with_model(self.model)
            .with_footer(self.footer)
            .with_announce(self.announce)
            .with_forbidden(self.forbid)
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

    fn identity() -> CoverIdentity {
        CoverIdentity {
            name: "Field Station".to_owned(),
            email: "station@example.invalid".to_owned(),
        }
    }

    fn commit(message: &str, author: &str) -> Change {
        Change {
            id: "a".repeat(40),
            author: author.to_owned(),
            committer: author.to_owned(),
            message: message.to_owned(),
        }
    }

    #[test]
    fn a_checkpoint_commit_keeps_every_cover() {
        let covers = Covers::parse_toml(
            "[cover.signed]\ndescription = \"x\"\nfooter = \"Filed by Field Station.\"\n",
        )
        .unwrap();
        let author = "Field Station <station@example.invalid>";
        for name in covers.names() {
            let cover = covers.get(name).unwrap();
            let checkpoint = commit(&cover.checkpoint_message(), author);
            assert_eq!(
                cover.check_commits(&[checkpoint], Some(&identity()), "model-x"),
                Vec::new(),
                "{name}"
            );
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
