//! Green ink: which irreversible steps need a person's approval, by Agent clearance.
//!
//! An Agent asks before an irreversible step (a push to a protected ref, a merge, a release,
//! a deletion, a change of clearance); an operator signs or refuses, and the decision is kept
//! in the Registry ([`crate::ApprovalStore::request_approval`]). An Operation's
//! [`GreenInkPolicy`] says which steps need that: the `strict` preset (the default) asks for
//! every built-in kind at every clearance and lets `desk` Agents ask for nothing, `licensed`
//! lets `00` Agents push and merge unattended, and every part can be chosen individually.
//!
//! An approval the Agent could ignore would be a log entry, so two things hold a step: a
//! harness gate that reads each shell command with [`classify`] and refuses a governed step no
//! live approval covers, and credentials for protected targets that Q leases only against an
//! approval.

mod classify;

pub use classify::{GitContext, Repository, classify};

use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::time::Duration;

use globset::{Glob, GlobMatcher};
use serde::{Deserialize, Serialize};

use crate::directorates::m::roster::{CLEARANCES, ClearanceLevel};

/// The kinds of step every Operation governs.
pub const BUILTIN_KINDS: [&str; 9] = [
    "push",
    "merge",
    "release",
    "delete",
    "clearance",
    SAFE_HOUSE_DELETE,
    ISSUE_CLOSE,
    ISSUE_ASSIGN,
    ISSUE_COMMENT,
];
/// The kind a class authorisation names to approve an Agent's proposal to create issues, from
/// an unexposed Run whose proposal has no findings; its target is the tracker instance.
/// A proposal waits for a person otherwise, not for green ink, so it is not a governed step.
pub const ISSUE_CREATE: &str = "issue-create";
/// The kind of step that closes or reopens an issue, or moves it into a done status; its
/// target names the issue as `TRACKER:ITEM`.
pub const ISSUE_CLOSE: &str = "issue-close";
/// The kind of step that changes an issue's assignees; its target names the issue.
pub const ISSUE_ASSIGN: &str = "issue-assign";
/// The kind of step that posts on an issue an exposed Run may not post on directly; its
/// target names the issue.
pub const ISSUE_COMMENT: &str = "issue-comment";
/// The kind of step that removes a kept safe house holding work that exists nowhere else; its
/// target names the safe house and is compared as exact text.
pub const SAFE_HOUSE_DELETE: &str = "safe-house-delete";
/// The refs a push needs green ink for unless the Operation lists others: the usual default
/// branches, release branches and tags.
pub const DEFAULT_PROTECTED: [&str; 4] = [
    "refs/heads/main",
    "refs/heads/master",
    "refs/heads/release/*",
    "refs/tags/*",
];
/// How long a request waits for a decision before it expires as refused.
pub const DEFAULT_REQUEST_TTL: Duration = Duration::from_secs(24 * 60 * 60);
/// How long an approval lasts when the signer does not choose.
pub const DEFAULT_APPROVAL_TTL: Duration = Duration::from_secs(60 * 60);
/// The `blocked_reason` of a Mission waiting for green ink.
pub const GREEN_INK_REASON: &str = "green-ink";

/// Why a Mission is held: its role requires a decided green ink request before `done`, and its
/// run ended with none.
pub const NO_APPROVAL_REASON: &str = "no approval was decided";

/// What an Agent asks to do: a kind of step and what it acts on.
///
/// `kind` is a built-in ([`BUILTIN_KINDS`]) or one the Operation adds. `target` is free text
/// naming the object, such as `origin:refs/heads/main`, `PR 42` or `tag v1.2.0`; a `push`
/// target is `[REMOTE:]REF`, where a bare branch name stands for `refs/heads/NAME`.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActionScope {
    /// Lowercase kind name of `a-z`, `0-9`, `-` and `_`.
    pub kind: String,
    /// What the step acts on, without control characters.
    pub target: String,
}

impl ActionScope {
    /// A validated scope.
    ///
    /// # Errors
    ///
    /// `InvalidInput` for an empty kind or target, or one with characters the fields above do
    /// not allow.
    pub fn try_new(kind: impl Into<String>, target: impl Into<String>) -> io::Result<Self> {
        let scope = Self {
            kind: kind.into(),
            target: target.into(),
        };
        scope.validate()?;
        Ok(scope)
    }

    /// Checks the characters of a scope read from storage or a request.
    ///
    /// # Errors
    ///
    /// `InvalidInput` naming the field that is not acceptable.
    pub fn validate(&self) -> io::Result<()> {
        if !valid_kind(&self.kind) {
            return Err(invalid(
                "the step kind must be 1 or more bytes of a-z, 0-9, '-' or '_'",
            ));
        }
        if self.target.trim().is_empty() || self.target.contains(char::is_control) {
            return Err(invalid(
                "the step target must be non-empty, without control characters",
            ));
        }
        Ok(())
    }

    /// Checks a scope an Agent asks about against the Station's `[approvals]` bounds.
    ///
    /// # Errors
    ///
    /// `InvalidInput` naming the field longer than `limits` allows.
    pub fn check_size(&self, limits: &crate::station::limits::ApprovalLimits) -> io::Result<()> {
        if !limits.kind_bytes.allows(self.kind.len()) {
            return Err(invalid(&format!(
                "the step kind must be at most {} bytes",
                limits.kind_bytes
            )));
        }
        if !limits.target_bytes.allows(self.target.len()) {
            return Err(invalid(&format!(
                "the step target must be at most {} bytes",
                limits.target_bytes
            )));
        }
        Ok(())
    }

    /// Checks that the target names something a person can decide on, from its shape alone (no
    /// tracker or remote is asked): `merge` a pull or merge request (`PR N`, `PR OWNER/REPO#N`, or
    /// the URL of one); `release`, `push` and `delete` a ref or tag name with no whitespace in it,
    /// not just the word `tag`; `issue-close`, `issue-assign` and `issue-comment` an issue as
    /// `TRACKER:ITEM`. Other kinds, including `clearance`, [`SAFE_HOUSE_DELETE`] and any kind an
    /// Operation adds, are not looked at.
    ///
    /// # Errors
    ///
    /// `InvalidInput` naming the kind and the shapes it accepts.
    pub fn check_target(&self) -> io::Result<()> {
        let refused = |shapes: &str| {
            Err(invalid(&format!(
                "a {} step names {shapes}, and {:?} is not one",
                self.kind, self.target
            )))
        };
        let target = self.target.trim();
        match self.kind.as_str() {
            "merge" => {
                let number =
                    |text: &str| !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit());
                let is_url = target.starts_with("https://") || target.starts_with("http://");
                let shaped = if is_url {
                    let mut tail = target.trim_end_matches('/').rsplit('/');
                    let (id, word) = (tail.next().unwrap_or_default(), tail.next());
                    number(id) && matches!(word, Some("pull" | "merge_requests"))
                } else {
                    let rest = target
                        .strip_prefix("PR ")
                        .or_else(|| target.strip_prefix("pr "))
                        .map(str::trim);
                    rest.is_some_and(|rest| match rest.rsplit_once('#') {
                        Some((repository, id)) => {
                            number(id) && !repository.chars().any(char::is_whitespace)
                        }
                        None => number(rest),
                    })
                };
                if !shaped {
                    return refused(
                        "a pull or merge request as `PR N`, `PR OWNER/REPO#N` or its URL",
                    );
                }
            }
            "release" | "push" | "delete" => {
                let named = target
                    .rsplit_once(':')
                    .map_or(target, |(_, named)| named)
                    .trim();
                let named = if self.kind == "release" {
                    named
                        .strip_prefix("tag ")
                        .or_else(|| named.strip_prefix("release "))
                        .unwrap_or(named)
                        .trim()
                } else {
                    named
                };
                let bare_word = self.kind == "release" && matches!(named, "tag" | "release");
                if named.is_empty() || bare_word || named.chars().any(char::is_whitespace) {
                    return refused("a ref or tag name, with no whitespace in it");
                }
            }
            ISSUE_CLOSE | ISSUE_ASSIGN | ISSUE_COMMENT
                if !target.split_once(':').is_some_and(|(tracker, item)| {
                    !tracker.is_empty() && !item.trim().is_empty()
                }) =>
            {
                return refused("an issue as `TRACKER:ITEM`");
            }
            _ => {}
        }
        Ok(())
    }

    /// The ref a `push` target names: the part after `REMOTE:`, with a bare branch name
    /// expanded to `refs/heads/NAME`.
    pub fn pushed_ref(&self) -> String {
        let reference = self
            .target
            .rsplit_once(':')
            .map_or(self.target.as_str(), |(_, reference)| reference)
            .trim();
        if reference.starts_with("refs/") {
            reference.to_owned()
        } else {
            format!("refs/heads/{reference}")
        }
    }

    /// The remote a `push` or `delete` target names before its ref, if it names one.
    pub fn remote(&self) -> Option<&str> {
        self.target
            .rsplit_once(':')
            .map(|(remote, _)| remote.trim())
            .filter(|remote| !remote.is_empty())
    }

    /// Whether an approval of this scope covers the step `attempted`: the same kind and, for
    /// `push` and `delete`, the same ref on the same remote (an approval that names no remote
    /// covers every remote; a step that names none is covered only by such an approval); for
    /// [`SAFE_HOUSE_DELETE`], exactly the same target; for other kinds, the same target
    /// ignoring case, `#` and spacing, so `PR 42` does not cover `PR other/repo#42`.
    pub fn covers(&self, attempted: &ActionScope) -> bool {
        if self.kind != attempted.kind {
            return false;
        }
        if self.kind == SAFE_HOUSE_DELETE {
            return self.target == attempted.target;
        }
        if matches!(self.kind.as_str(), "push" | "delete") {
            let remotes = match (self.remote(), attempted.remote()) {
                (None, _) => true,
                (Some(approved), Some(attempted)) => approved == attempted,
                (Some(_), None) => false,
            };
            return remotes && self.pushed_ref() == attempted.pushed_ref();
        }
        let plain = |target: &str| {
            target
                .to_lowercase()
                .replace('#', " ")
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
        };
        plain(&self.target) == plain(&attempted.target)
    }

    /// Whether `other` asks for the same step as this scope: the same kind and target however
    /// the target is written. A pushed or released ref is compared by its full name (`v1.2`,
    /// `tag v1.2`, `refs/tags/v1.2` and `origin:refs/tags/v1.2` are one tag), and a remote only
    /// when both name one; other targets as in [`ActionScope::covers`].
    pub fn same_step(&self, other: &Self) -> bool {
        if self.kind != other.kind {
            return false;
        }
        if matches!(self.kind.as_str(), "push" | "delete" | "release") {
            let remotes = match (self.remote(), other.remote()) {
                (Some(left), Some(right)) => left == right,
                _ => true,
            };
            return remotes && self.stepped_ref() == other.stepped_ref();
        }
        self.covers(other) && other.covers(self)
    }

    /// The full ref a `push`, `delete` or `release` target names; a release names a tag.
    fn stepped_ref(&self) -> String {
        if self.kind != "release" {
            return self.pushed_ref();
        }
        let named = self
            .target
            .rsplit_once(':')
            .map_or(self.target.as_str(), |(_, named)| named)
            .trim();
        let named = named
            .strip_prefix("tag ")
            .or_else(|| named.strip_prefix("release "))
            .unwrap_or(named)
            .trim();
        if named.starts_with("refs/") {
            named.to_owned()
        } else {
            format!("refs/tags/{named}")
        }
    }
}

/// What a policy says about one step.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Requirement {
    /// The Agent may take the step without asking.
    NotRequired,
    /// The step waits for a person's approval.
    GreenInk,
    /// The Agent may not ask: its clearance may not request, or the Operation does not
    /// govern the kind.
    Refused,
}

/// Named green ink policies.
#[non_exhaustive]
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, guatiao::ToValue, guatiao::FromValue, guatiao::Schema,
)]
#[map(rename_all = "lowercase")]
pub enum GreenInkPreset {
    /// Every governed step needs green ink at every clearance; `desk` may not ask. The
    /// default.
    #[schema(label = "Strict: every step waits for a person")]
    Strict,
    /// As `strict`, except `00` Agents push and merge without asking.
    #[schema(label = "Licensed: as strict, but 00 Agents push and merge without asking")]
    Licensed,
}

/// An Operation's green ink policy.
#[derive(Debug, Clone)]
pub struct GreenInkPolicy {
    kinds: BTreeSet<String>,
    requesters: BTreeSet<String>,
    unattended: BTreeMap<String, BTreeSet<String>>,
    protected: Vec<String>,
    matchers: Vec<GlobMatcher>,
    request_ttl: Duration,
    approval_ttl: Duration,
    deny: Vec<String>,
    park: Vec<String>,
    rules: Vec<StepEntry>,
}

/// One entry of a policy's `deny` or `park` list, compiled.
#[derive(Debug, Clone)]
struct StepEntry {
    rule: StepRule,
    kind: String,
    entry: String,
    matcher: GlobMatcher,
}

/// What a policy does with a step it names in `deny` or `park`.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepRule {
    /// The step is refused: no request is filed and no approval covers it.
    Deny,
    /// The step is filed and waits for a person, whatever the clearance, the unattended list
    /// or a class authorisation says.
    Park,
}

impl PartialEq for GreenInkPolicy {
    fn eq(&self, other: &Self) -> bool {
        (
            &self.kinds,
            &self.requesters,
            &self.unattended,
            &self.protected,
            self.request_ttl,
            self.approval_ttl,
            &self.deny,
            &self.park,
        ) == (
            &other.kinds,
            &other.requesters,
            &other.unattended,
            &other.protected,
            other.request_ttl,
            other.approval_ttl,
            &other.deny,
            &other.park,
        )
    }
}

impl Default for GreenInkPolicy {
    fn default() -> Self {
        Self::preset(GreenInkPreset::Strict)
    }
}

impl GreenInkPolicy {
    /// The policy of a named preset.
    pub fn preset(preset: GreenInkPreset) -> Self {
        let unattended = match preset {
            GreenInkPreset::Strict => BTreeMap::new(),
            GreenInkPreset::Licensed => BTreeMap::from([(
                "00".to_owned(),
                ["push", "merge", ISSUE_CLOSE].map(str::to_owned).into(),
            )]),
        };
        let protected = DEFAULT_PROTECTED.map(str::to_owned).to_vec();
        Self {
            kinds: BUILTIN_KINDS.map(str::to_owned).into(),
            requesters: ["field", "00"].map(str::to_owned).into(),
            unattended,
            matchers: compile(&protected).unwrap_or_default(),
            protected,
            request_ttl: DEFAULT_REQUEST_TTL,
            approval_ttl: DEFAULT_APPROVAL_TTL,
            deny: Vec::new(),
            park: Vec::new(),
            rules: Vec::new(),
        }
    }

    /// What an Agent of `clearance` must do before the step in `scope`.
    pub fn requirement(&self, clearance: &str, scope: &ActionScope) -> Requirement {
        if !self.requesters.contains(clearance) || !self.kinds.contains(&scope.kind) {
            return Requirement::Refused;
        }
        if scope.kind == "push" && !self.is_protected(&scope.pushed_ref()) {
            return Requirement::NotRequired;
        }
        if self
            .unattended
            .get(clearance)
            .is_some_and(|kinds| kinds.contains(&scope.kind))
        {
            return Requirement::NotRequired;
        }
        Requirement::GreenInk
    }

    /// What the Operation's `deny` and `park` lists say of `scope`: the rule and the entry that
    /// matched. A step on both lists is denied.
    pub fn rule(&self, scope: &ActionScope) -> Option<(StepRule, &str)> {
        let named = matches!(scope.kind.as_str(), "push" | "delete" | "release")
            .then(|| scope.stepped_ref());
        let listed = |wanted: StepRule| {
            self.rules.iter().find(|entry| {
                entry.rule == wanted
                    && entry.kind == scope.kind
                    && (entry.matcher.is_match(&scope.target)
                        || named
                            .as_deref()
                            .is_some_and(|named| entry.matcher.is_match(named)))
            })
        };
        listed(StepRule::Deny)
            .or_else(|| listed(StepRule::Park))
            .map(|entry| (entry.rule, entry.entry.as_str()))
    }

    /// Whether a push to `reference` (a full ref such as `refs/heads/main`) needs green ink. A
    /// reference with a pattern character (`*`, `?`, `[`), such as every branch or
    /// `refs/heads/*ain`, always does: it may reach a protected ref.
    pub fn is_protected(&self, reference: &str) -> bool {
        if reference.contains(['*', '?', '[']) {
            return true;
        }
        self.matchers
            .iter()
            .any(|matcher| matcher.is_match(reference))
    }

    /// The kinds the Operation governs, built-ins first in name order.
    pub fn kinds(&self) -> impl Iterator<Item = &str> {
        self.kinds.iter().map(String::as_str)
    }

    /// The clearances whose Agents may ask.
    pub fn requesters(&self) -> impl Iterator<Item = &str> {
        self.requesters.iter().map(String::as_str)
    }

    /// The ref patterns a push needs green ink for.
    pub fn protected(&self) -> &[String] {
        &self.protected
    }

    /// How long a request waits for a decision.
    pub fn request_ttl(&self) -> Duration {
        self.request_ttl
    }

    /// How long an approval covers its step when the signer does not choose.
    pub fn approval_ttl(&self) -> Duration {
        self.approval_ttl
    }

    /// The settings looser than the `strict` default, each in a sentence an operator can act
    /// on.
    pub fn weakenings(&self) -> Vec<String> {
        let mut weakened = Vec::new();
        if self.requesters.contains("desk") {
            weakened.push("requesters: desk Agents may ask for green ink".to_owned());
        }
        for (clearance, kinds) in &self.unattended {
            for kind in kinds {
                weakened.push(format!(
                    "unattended: {clearance} Agents may {kind} without green ink"
                ));
            }
        }
        for pattern in DEFAULT_PROTECTED {
            if !self.protected.iter().any(|listed| listed == pattern) {
                weakened.push(format!("protected: a push to {pattern} needs no green ink"));
            }
        }
        weakened
    }
}

/// The green ink settings as written in configuration (`[operation.ID.green_ink]`).
#[derive(
    Debug,
    Clone,
    Default,
    guatiao::ToValue,
    guatiao::FromValue,
    guatiao::Schema,
    guatiao_intake::Form,
)]
#[map(deny_unknown_fields)]
pub struct GreenInkDefinition {
    /// A starting point the values below are laid over.
    #[schema(title = "Starting point")]
    preset: Option<GreenInkPreset>,
    /// More kinds of step that wait for a person, beside the built-in ones.
    #[schema(title = "More kinds of step")]
    kinds: Option<Vec<String>>,
    /// The clearances whose Agents may ask.
    #[schema(title = "Who may ask")]
    requesters: Option<Vec<ClearanceLevel>>,
    /// For each clearance (desk, field or 00), the kinds of step it takes without asking.
    #[schema(title = "Without asking")]
    unattended: Option<BTreeMap<String, Vec<String>>>,
    /// Branch patterns a push to which always waits for a person, such as main.
    #[schema(title = "Protected branches")]
    protected: Option<Vec<String>>,
    /// How long a request waits for a decision before it expires as refused.
    #[schema(title = "A request waits", format = "seconds", default = Some(DEFAULT_REQUEST_TTL.as_secs()))]
    request_ttl_secs: Option<u64>,
    /// How long an approval covers its step when the signer does not choose. A standing Mission
    /// whose windows are further apart than this loses the approval before it can spend it.
    #[schema(title = "An approval lasts", format = "seconds", default = Some(DEFAULT_APPROVAL_TTL.as_secs()))]
    approval_ttl_secs: Option<u64>,
    /// Steps refused outright, each as `KIND PATTERN` (a glob on the target; a pushed or
    /// released ref is matched by its full name), such as `release refs/tags/v0.*`. No request
    /// is filed for one.
    #[schema(title = "Steps refused outright")]
    deny: Option<Vec<String>>,
    /// Steps that always wait for a person, each as `KIND PATTERN`: no clearance, unattended
    /// list or class authorisation lets one through.
    #[schema(title = "Steps that always wait for a person")]
    park: Option<Vec<String>>,
}

impl GreenInkDefinition {
    /// Validates the written values and applies them over the preset. `kinds` adds to the
    /// built-ins; `unattended` replaces the preset's list for each clearance it names.
    ///
    /// # Errors
    ///
    /// Returns a message naming the key for an invalid kind, an unknown clearance, an
    /// unattended kind the Operation does not govern, a pattern that is not a glob, or a zero
    /// request lifetime.
    pub fn resolve(&self) -> Result<GreenInkPolicy, String> {
        let mut policy = GreenInkPolicy::preset(self.preset.unwrap_or(GreenInkPreset::Strict));
        if let Some(kinds) = &self.kinds {
            for kind in kinds {
                if !valid_kind(kind) {
                    return Err(format!(
                        "green_ink kinds has invalid kind {kind:?}; use a-z, 0-9, '-' or '_'"
                    ));
                }
                policy.kinds.insert(kind.clone());
            }
        }
        if let Some(requesters) = &self.requesters {
            policy.requesters = requesters
                .iter()
                .map(|clearance| clearance.as_str().to_owned())
                .collect();
        }
        if let Some(unattended) = &self.unattended {
            for (clearance, kinds) in unattended {
                known_clearance(clearance, "unattended")?;
                if let Some(kind) = kinds.iter().find(|kind| !policy.kinds.contains(*kind)) {
                    return Err(format!(
                        "green_ink unattended names kind {kind:?}, which the Operation does not govern"
                    ));
                }
                policy
                    .unattended
                    .insert(clearance.clone(), kinds.iter().cloned().collect());
            }
            policy.unattended.retain(|_, kinds| !kinds.is_empty());
        }
        if let Some(protected) = &self.protected {
            policy.matchers = compile(protected)?;
            policy.protected.clone_from(protected);
        }
        if let Some(seconds) = self.request_ttl_secs {
            if seconds == 0 {
                return Err("green_ink request_ttl_secs must be at least 1".to_owned());
            }
            policy.request_ttl = Duration::from_secs(seconds);
        }
        if let Some(seconds) = self.approval_ttl_secs {
            if seconds == 0 {
                return Err("green_ink approval_ttl_secs must be at least 1".to_owned());
            }
            policy.approval_ttl = Duration::from_secs(seconds);
        }
        for (key, rule, listed) in [
            ("deny", StepRule::Deny, &self.deny),
            ("park", StepRule::Park, &self.park),
        ] {
            for entry in listed.iter().flatten() {
                let (kind, pattern) = entry.split_once(' ').ok_or_else(|| {
                    format!("green_ink {key} entry {entry:?} must read `KIND PATTERN`")
                })?;
                if !valid_kind(kind) {
                    return Err(format!(
                        "green_ink {key} entry {entry:?} has invalid kind {kind:?}"
                    ));
                }
                let matcher = Glob::new(pattern.trim())
                    .map_err(|error| format!("green_ink {key} pattern {pattern:?}: {error}"))?
                    .compile_matcher();
                policy.rules.push(StepEntry {
                    rule,
                    kind: kind.to_owned(),
                    entry: entry.clone(),
                    matcher,
                });
                match rule {
                    StepRule::Deny => policy.deny.push(entry.clone()),
                    StepRule::Park => policy.park.push(entry.clone()),
                }
            }
        }
        Ok(policy)
    }
}

/// Whether an Agent of clearance `have` meets a minimum of `min`; an unknown clearance on
/// either side does not.
pub fn clearance_at_least(have: &str, min: &str) -> bool {
    let rank = |clearance| CLEARANCES.iter().position(|known| *known == clearance);
    matches!((rank(have), rank(min)), (Some(have), Some(min)) if have >= min)
}

fn known_clearance(clearance: &str, key: &str) -> Result<String, String> {
    if CLEARANCES.contains(&clearance) {
        Ok(clearance.to_owned())
    } else {
        Err(format!(
            "green_ink {key} has unknown clearance {clearance:?}; use desk, field or 00"
        ))
    }
}

fn compile(patterns: &[String]) -> Result<Vec<GlobMatcher>, String> {
    patterns
        .iter()
        .map(|pattern| {
            Glob::new(pattern)
                .map(|glob| glob.compile_matcher())
                .map_err(|error| format!("green_ink protected pattern {pattern:?}: {error}"))
        })
        .collect()
}

pub(crate) fn valid_kind(kind: &str) -> bool {
    !kind.is_empty()
        && kind
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"-_".contains(&byte))
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope(kind: &str, target: &str) -> ActionScope {
        ActionScope::try_new(kind, target).unwrap()
    }

    #[test]
    fn a_target_that_names_no_decidable_object_is_refused_per_kind() {
        let accepted = [
            ("merge", "PR 42"),
            ("merge", "PR other/repo#42"),
            ("merge", "PR #7"),
            ("merge", "https://github.com/o/r/pull/9"),
            ("merge", "https://gitlab.example/g/p/-/merge_requests/3"),
            ("release", "tag v1.2.0"),
            ("release", "origin:refs/tags/v1.2.0"),
            ("release", "v1.2.0"),
            ("push", "origin:refs/heads/main"),
            ("push", "origin:refs/heads/*"),
            ("push", "main"),
            ("delete", "origin:refs/heads/old"),
            ("issue-close", "gh:o/r#4"),
            ("issue-comment", "jira:OPS-12"),
            ("clearance", "anything at all"),
            (SAFE_HOUSE_DELETE, "a safe house"),
            ("deploy", "production now"),
        ];
        for (kind, target) in accepted {
            scope(kind, target)
                .check_target()
                .unwrap_or_else(|error| panic!("{kind} {target:?}: {error}"));
        }
        let refused = [
            ("merge", "PR banana"),
            ("merge", "PR"),
            ("merge", "42"),
            ("merge", "PR 4 2"),
            ("merge", "https://github.com/o/r/issues/9"),
            ("release", "tag "),
            ("release", "tag v1 v2"),
            ("release", "release"),
            ("push", "origin:two words"),
            ("delete", "origin: "),
            ("issue-close", "no-colon"),
            ("issue-assign", ":item"),
            ("issue-comment", "gh: "),
        ];
        for (kind, target) in refused {
            let error = scope(kind, target)
                .check_target()
                .expect_err(&format!("{kind} {target:?} names no decidable object"));
            assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
            assert!(error.to_string().contains(kind), "{error}");
        }
    }

    #[test]
    fn strict_asks_for_every_governed_step_and_desk_may_not_ask() {
        let policy = GreenInkPolicy::default();
        let main = scope("push", "origin:refs/heads/main");
        assert_eq!(policy.requirement("field", &main), Requirement::GreenInk);
        assert_eq!(policy.requirement("00", &main), Requirement::GreenInk);
        assert_eq!(policy.requirement("desk", &main), Requirement::Refused);
        assert_eq!(
            policy.requirement("00", &scope("merge", "PR 42")),
            Requirement::GreenInk
        );
        assert_eq!(
            policy.requirement("field", &scope("deploy", "production")),
            Requirement::Refused,
            "a kind the Operation does not govern"
        );
        assert!(policy.weakenings().is_empty());
    }

    #[test]
    fn only_protected_refs_need_green_ink_for_a_push() {
        let policy = GreenInkPolicy::default();
        for target in ["main", "origin:main", "refs/tags/v1.2.0", "release/2.0"] {
            assert_eq!(
                policy.requirement("field", &scope("push", target)),
                Requirement::GreenInk,
                "{target}"
            );
        }
        assert_eq!(
            policy.requirement("field", &scope("push", "origin:ai6/mission-7")),
            Requirement::NotRequired
        );
        assert_eq!(scope("push", "origin:main").pushed_ref(), "refs/heads/main");
    }

    #[test]
    fn licensed_lets_00_push_and_merge_unattended_and_says_so() {
        let policy = GreenInkPolicy::preset(GreenInkPreset::Licensed);
        assert_eq!(
            policy.requirement("00", &scope("push", "main")),
            Requirement::NotRequired
        );
        assert_eq!(
            policy.requirement("00", &scope("merge", "PR 1")),
            Requirement::NotRequired
        );
        assert_eq!(
            policy.requirement("00", &scope("release", "tag v1")),
            Requirement::GreenInk
        );
        assert_eq!(
            policy.requirement("field", &scope("push", "main")),
            Requirement::GreenInk
        );
        assert_eq!(policy.weakenings().len(), 3, "push, merge and issue-close");
    }

    #[test]
    fn configuration_picks_a_preset_then_overrides_each_part() {
        let strict: GreenInkDefinition = crate::directorates::m::records::from_toml("").unwrap();
        assert_eq!(strict.resolve().unwrap(), GreenInkPolicy::default());

        let custom: GreenInkDefinition = crate::directorates::m::records::from_toml(
            "preset = \"licensed\"\nkinds = [\"deploy\"]\nrequesters = [\"desk\", \"field\", \"00\"]\n\
             unattended = { \"00\" = [], field = [\"deploy\"] }\nprotected = [\"refs/heads/trunk\"]\n\
             request_ttl_secs = 600\n",
        )
        .unwrap();
        let policy = custom.resolve().unwrap();
        assert_eq!(
            policy.requirement("00", &scope("push", "trunk")),
            Requirement::GreenInk,
            "an empty list takes the preset's unattended kinds away"
        );
        assert_eq!(
            policy.requirement("field", &scope("deploy", "production")),
            Requirement::NotRequired
        );
        assert_eq!(
            policy.requirement("desk", &scope("merge", "PR 3")),
            Requirement::GreenInk
        );
        assert_eq!(
            policy.requirement("00", &scope("push", "main")),
            Requirement::NotRequired
        );
        assert_eq!(policy.request_ttl(), Duration::from_secs(600));
        assert!(policy.weakenings().len() >= 5);

        for bad in [
            "kinds = [\"Deploy\"]",
            "requesters = [\"admin\"]",
            "unattended = { \"00\" = [\"deploy\"] }",
            "protected = [\"refs/[\"]",
            "request_ttl_secs = 0",
        ] {
            let read = crate::directorates::m::records::from_toml::<GreenInkDefinition>(bad)
                .and_then(|definition| definition.resolve());
            assert!(read.is_err(), "{bad}");
        }
    }

    #[test]
    fn an_approval_covers_the_same_ref_however_it_was_written() {
        let approved = scope("push", "origin:main");
        assert!(approved.covers(&scope("push", "origin:refs/heads/main")));
        assert!(
            !approved.covers(&scope("push", "refs/heads/main")),
            "a step without a remote may go anywhere"
        );
        assert!(scope("push", "main").covers(&scope("push", "upstream:refs/heads/main")));
        assert!(scope("push", "main").covers(&scope("push", "refs/heads/main")));
        assert!(!scope("merge", "PR 42").covers(&scope("merge", "PR other/repo#42")));
        assert!(scope("merge", "PR o/r#42").covers(&scope("merge", "pr o/r #42")));
        assert!(!approved.covers(&scope("push", "upstream:refs/heads/main")));
        assert!(!approved.covers(&scope("push", "origin:refs/tags/main")));
        assert!(!approved.covers(&scope("delete", "origin:refs/heads/main")));
        assert!(scope("merge", "pr #42").covers(&scope("merge", "PR 42")));
        assert!(!scope("merge", "PR 42").covers(&scope("merge", "PR 421")));
        assert!(scope("release", "tag  v1.2.0").covers(&scope("release", "Tag v1.2.0")));
    }

    #[test]
    fn a_safe_house_delete_covers_exactly_its_own_target() {
        let approved = scope(SAFE_HOUSE_DELETE, "safe house m-1 on Station a:b");
        assert!(approved.covers(&scope(SAFE_HOUSE_DELETE, "safe house m-1 on Station a:b")));
        for other in [
            "safe house m-1 on Station x:b",
            "Safe House m-1 on Station a:b",
            "safe house  m-1 on Station a:b",
            "safe house m-1 on Station a:b2",
        ] {
            assert!(
                !approved.covers(&scope(SAFE_HOUSE_DELETE, other)),
                "{other:?}"
            );
        }
        assert!(!approved.covers(&scope("delete", "safe house m-1 on Station a:b")));
        assert_eq!(
            GreenInkPolicy::default().requirement("field", &approved),
            Requirement::GreenInk
        );
    }

    #[test]
    fn scopes_are_bounded_and_clearances_rank() {
        assert!(ActionScope::try_new("push", "main").is_ok());
        for (kind, target) in [
            ("", "main"),
            ("Push", "main"),
            ("push", " "),
            ("push", "main\nrm"),
        ] {
            assert!(
                ActionScope::try_new(kind, target).is_err(),
                "{kind:?} {target:?}"
            );
        }
        let long = ActionScope::try_new("push", "x".repeat(257)).unwrap();
        let limits = crate::station::limits::ApprovalLimits::default();
        assert!(long.check_size(&limits).is_err());
        assert!(
            ActionScope::try_new("push", "x".repeat(256))
                .unwrap()
                .check_size(&limits)
                .is_ok()
        );
        assert!(clearance_at_least("00", "field"));
        assert!(clearance_at_least("field", "field"));
        assert!(!clearance_at_least("desk", "field"));
        assert!(!clearance_at_least("root", "desk"));
    }

    #[test]
    fn one_step_written_two_ways_is_one_step() {
        let same = |left: (&str, &str), right: (&str, &str)| {
            scope(left.0, left.1).same_step(&scope(right.0, right.1))
        };
        assert!(same(
            ("release", "tag v1.2.0"),
            ("release", "refs/tags/v1.2.0")
        ));
        assert!(same(
            ("release", "v1.2.0"),
            ("release", "origin:refs/tags/v1.2.0")
        ));
        assert!(!same(("release", "tag v1.2.0"), ("release", "tag v1.2.1")));
        assert!(!same(("release", "origin:v1"), ("release", "upstream:v1")));
        assert!(same(("push", "main"), ("push", "origin:refs/heads/main")));
        assert!(!same(("push", "main"), ("delete", "main")));
        assert!(same(("merge", "PR 42"), ("merge", "pr #42")));
        assert!(!same(("merge", "PR 42"), ("merge", "PR other/repo#42")));
    }

    #[test]
    fn deny_and_park_lists_name_steps_and_a_bad_entry_is_refused_by_name() {
        let definition: GreenInkDefinition = crate::directorates::m::records::from_toml(
            "deny = [\"release refs/tags/v0.*\"]\npark = [\"merge PR 7\"]\napproval_ttl_secs = 86400\n",
        )
        .unwrap();
        let policy = definition.resolve().unwrap();
        assert_eq!(
            policy.rule(&scope("release", "tag v0.4.0")),
            Some((StepRule::Deny, "release refs/tags/v0.*"))
        );
        assert_eq!(
            policy.rule(&scope("merge", "PR 7")),
            Some((StepRule::Park, "merge PR 7"))
        );
        assert_eq!(policy.rule(&scope("merge", "PR 8")), None);
        assert_eq!(policy.rule(&scope("release", "tag v1.0.0")), None);
        assert_eq!(policy.approval_ttl(), Duration::from_secs(86_400));
        assert_eq!(
            GreenInkPolicy::default().approval_ttl(),
            DEFAULT_APPROVAL_TTL
        );

        for (text, named) in [
            ("deny = [\"release\"]", "KIND PATTERN"),
            ("park = [\"Merge x\"]", "invalid kind"),
            ("approval_ttl_secs = 0", "approval_ttl_secs"),
        ] {
            let bad: GreenInkDefinition = crate::directorates::m::records::from_toml(text).unwrap();
            let message = bad.resolve().unwrap_err();
            assert!(message.contains(named), "{text}: {message}");
        }
    }
}
