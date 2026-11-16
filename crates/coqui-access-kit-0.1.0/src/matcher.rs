//! Tying the identity an authenticator obtained to an account in a directory.
//!
//! An authenticator that has more than a name answers a [`Lookup`] with `by: "claims"` and
//! its [`Claims`]. A [`Matcher`] holds ordered rules that turn claims into lookups of a
//! [`Directory`], by a template: `{san.upn}`, `{subject.cn|lower}`, or an LDAP filter such
//! as `(userPrincipalName={san.upn})`. The directory, never the credential, decides the
//! account and its groups.
//!
//! Rules are tried in order. A rule asks the directory for every candidate its template
//! renders; if they name exactly one account that account is the match, if they name more
//! than one the match is [`Match::Ambiguous`] and the walk ends, and if none the next rule is
//! tried. A directory that cannot answer is an `Err`, never a fall-through. A rule may name a
//! [`PinSource`]: the one account it found must then also hold one of the certificate's
//! pins there.
//!
//! In a `filter` rule every value a claim puts into the template is escaped (RFC 4515, see
//! [`escape_filter_value`]); the template's own text is the filter's structure. A claim with
//! several values renders one candidate per value, at most [`MAX_CANDIDATES`] in all.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use coqui_access::guatiao::library::ProviderError;
use coqui_access::{Claims, Directory, Lookup, Person, error};
use coqui_builder::Describe;

use crate::pins::{PinCheck, PinSet, PinSource};

/// The most lookups one rule may render from the claims it is given.
pub const MAX_CANDIDATES: usize = 16;

/// What each rule's `by` may be: the lookups a directory answers (`name`, `email`, `id`) or
/// an LDAP `filter`.
const BYS: [&str; 4] = ["name", "email", "id", "filter"];

/// One way to find the account: a template over the claims, and what it renders is.
///
/// ```toml
/// [[rule]]
/// by = "filter"
/// template = "(userPrincipalName={san.upn})"
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Describe)]
#[map(deny_unknown_fields)]
#[non_exhaustive]
pub struct MatchRule {
    /// What the rendered text is: `name`, `email` or `id` (the directory attribute it is
    /// looked up by), or `filter` (an LDAP filter, values escaped).
    pub by: String,
    /// Text with `{claim}` and `{claim|filter|...}` parts; `{{` and `}}` are braces. The
    /// filters are `lower`, `upper`, `local` (before the last `@`) and `domain` (after it).
    /// It names at least one claim. Stripping a realm with `local` loses what ties the name
    /// to it, so such a rule also needs `domain` or `pins`.
    pub template: String,
    /// The name of a pin source (given to the builder): the one account this rule finds must
    /// also hold one of the certificate's pins there.
    pub pins: Option<String>,
    /// The realm a `local` filter strips: a claim value passed through `local` counts only
    /// when it ends `@<domain>` (any case), so `alice@evil.example` is not `alice`. A rule
    /// that strips a domain says which, or names `pins`; one with no `local` has none.
    pub domain: Option<String>,
}

impl MatchRule {
    /// A rule rendering `template` looked up `by`.
    pub fn new(by: impl Into<String>, template: impl Into<String>) -> Self {
        Self {
            by: by.into(),
            template: template.into(),
            pins: None,
            domain: None,
        }
    }

    /// This rule keeping, for `local`, only values of `domain`.
    #[must_use]
    pub fn domain(mut self, domain: impl Into<String>) -> Self {
        self.domain = Some(domain.into());
        self
    }

    /// This rule also requiring the account to hold a pin in the source called `source`.
    #[must_use]
    pub fn pins(mut self, source: impl Into<String>) -> Self {
        self.pins = Some(source.into());
        self
    }
}

/// The rules a matcher uses unless told otherwise: the UPN as a name, the email address as
/// an email. The subject's common name is not among them: it is whatever the issuer let the
/// holder type, so a deployment that trusts it says so in a rule of its own.
pub fn default_rules() -> Vec<MatchRule> {
    vec![
        MatchRule::new("name", "{san.upn}"),
        MatchRule::new("email", "{san.email}"),
    ]
}

/// A matcher as plain data: ordered rules, [`default_rules`] when the key is absent.
#[derive(Debug, Clone, PartialEq, Eq, Describe)]
#[map(deny_unknown_fields)]
#[non_exhaustive]
pub struct MatcherConfig {
    /// The rules, in the order they are tried. An empty list matches nobody.
    #[map(default = default_rules())]
    pub rule: Vec<MatchRule>,
}

impl Default for MatcherConfig {
    fn default() -> Self {
        Self {
            rule: default_rules(),
        }
    }
}

/// An invalid rule. The message never quotes the template.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct MatcherError(String);

impl fmt::Display for MatcherError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl core::error::Error for MatcherError {}

impl From<MatcherError> for ProviderError {
    fn from(failure: MatcherError) -> Self {
        error::bad_value(&failure.0)
    }
}

fn fail(index: usize, message: &str) -> MatcherError {
    MatcherError(format!("rule {index}: {message}"))
}

/// What a match came to.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Match {
    /// Exactly one account, as the directory knows it.
    Found(Person),
    /// No rule found anybody.
    NotFound,
    /// A rule found more than one account: refused, never a choice.
    Ambiguous,
}

/// A match and what to tell whoever runs the service about it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Matched {
    /// What was found.
    pub result: Match,
    /// Why a pin source counted as no pins (a file with the wrong permissions), in the
    /// order met. Never a pin or a claim.
    pub notes: Vec<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Filter {
    Lower,
    Upper,
    Local,
    Domain,
}

impl Filter {
    fn apply(self, value: &str) -> Option<String> {
        match self {
            Self::Lower => Some(value.to_lowercase()),
            Self::Upper => Some(value.to_uppercase()),
            // A value with more than one `@` has no realm to strip: what is left of two is a name that holds an `@`.
            _ if value.matches('@').count() > 1 => None,
            Self::Local => Some(
                value
                    .rsplit_once('@')
                    .map_or(value, |(local, _)| local)
                    .to_owned(),
            ),
            Self::Domain => value.rsplit_once('@').map(|(_, domain)| domain.to_owned()),
        }
    }
}

enum Piece {
    Text(String),
    Field { name: String, filters: Vec<Filter> },
}

fn parse(index: usize, template: &str) -> Result<Vec<Piece>, MatcherError> {
    let mut pieces = Vec::new();
    let mut text = String::new();
    let mut chars = template.chars().peekable();
    while let Some(character) = chars.next() {
        match character {
            '{' if chars.peek() == Some(&'{') => {
                chars.next();
                text.push('{');
            }
            '}' if chars.peek() == Some(&'}') => {
                chars.next();
                text.push('}');
            }
            '}' => return Err(fail(index, "the template has an unmatched }")),
            '{' => {
                let mut inner = String::new();
                loop {
                    match chars.next() {
                        Some('}') => break,
                        Some('{') | None => {
                            return Err(fail(index, "the template has an unmatched {"));
                        }
                        Some(other) => inner.push(other),
                    }
                }
                let mut parts = inner.split('|');
                let name = parts.next().unwrap_or_default();
                let valid = !name.is_empty()
                    && name.bytes().all(|byte| {
                        byte.is_ascii_lowercase()
                            || byte.is_ascii_digit()
                            || matches!(byte, b'.' | b'_' | b'-')
                    });
                if !valid {
                    return Err(fail(index, "a claim name is empty or not lowercase words"));
                }
                let mut filters = Vec::new();
                for word in parts {
                    filters.push(match word {
                        "lower" => Filter::Lower,
                        "upper" => Filter::Upper,
                        "local" => Filter::Local,
                        "domain" => Filter::Domain,
                        _ => {
                            return Err(fail(
                                index,
                                "a filter is not lower, upper, local or domain",
                            ));
                        }
                    });
                }
                if !text.is_empty() {
                    pieces.push(Piece::Text(std::mem::take(&mut text)));
                }
                pieces.push(Piece::Field {
                    name: name.to_owned(),
                    filters,
                });
            }
            other => text.push(other),
        }
    }
    if !text.is_empty() {
        pieces.push(Piece::Text(text));
    }
    Ok(pieces)
}

/// Whether `shape` (a filter with its values replaced) is one parenthesised group, however
/// nested.
fn one_group(shape: &str) -> bool {
    let mut depth = 0_i32;
    let mut chars = shape.chars().peekable();
    if chars.peek() != Some(&'(') {
        return false;
    }
    let mut first = true;
    for character in chars {
        if !first && depth == 0 {
            return false;
        }
        first = false;
        match character {
            '(' => depth += 1,
            ')' => depth -= 1,
            _ => {}
        }
        if depth < 0 {
            return false;
        }
    }
    depth == 0
}

struct Compiled {
    by: &'static str,
    filter: bool,
    pieces: Vec<Piece>,
    pins: Option<String>,
    /// The realm `local` keeps, lower case.
    domain: Option<String>,
}

/// Whether `value` is `<something>@<domain>`, in any case.
fn in_realm(value: &str, domain: &str) -> bool {
    value.matches('@').count() == 1
        && value
            .rsplit_once('@')
            .is_some_and(|(local, found)| !local.is_empty() && found.eq_ignore_ascii_case(domain))
}

impl Compiled {
    fn new(index: usize, rule: &MatchRule) -> Result<Self, MatcherError> {
        let Some(by) = BYS.iter().find(|word| **word == rule.by) else {
            return Err(fail(index, "by is not name, email, id or filter"));
        };
        let pieces = parse(index, &rule.template)?;
        if !pieces
            .iter()
            .any(|piece| matches!(piece, Piece::Field { .. }))
        {
            return Err(fail(
                index,
                "the template names no claim, so it would look up the same account for everyone",
            ));
        }
        let strips = pieces.iter().any(|piece| {
            matches!(piece, Piece::Field { filters, .. } if filters.contains(&Filter::Local))
        });
        let domain = match rule.domain.as_deref() {
            None => None,
            Some(domain) => {
                if domain.is_empty()
                    || domain.contains('@')
                    || domain.chars().any(|c| c.is_control() || c.is_whitespace())
                {
                    return Err(fail(index, "the domain is not a domain name"));
                }
                if !strips {
                    return Err(fail(
                        index,
                        "a domain applies to a template that uses local",
                    ));
                }
                Some(domain.to_ascii_lowercase())
            }
        };
        if strips && domain.is_none() && rule.pins.is_none() {
            return Err(fail(
                index,
                "local strips the realm from a name: say which with domain, or require pins",
            ));
        }
        let filter = *by == "filter";
        if filter {
            let shape: String = pieces
                .iter()
                .map(|piece| match piece {
                    Piece::Text(text) => text.as_str(),
                    Piece::Field { .. } => "x",
                })
                .collect();
            if !one_group(&shape) {
                return Err(fail(index, "a filter is one balanced parenthesised group"));
            }
        }
        Ok(Self {
            by,
            filter,
            pieces,
            pins: rule.pins.clone(),
            domain,
        })
    }

    /// Every lookup the claims render, none when a claim is missing or empty.
    fn candidates(&self, claims: &Claims) -> Result<Vec<Lookup>, ProviderError> {
        // Each piece's possible texts.
        let mut options: Vec<Vec<String>> = Vec::new();
        for piece in &self.pieces {
            match piece {
                Piece::Text(text) => options.push(vec![text.clone()]),
                Piece::Field { name, filters } => {
                    let mut values: Vec<String> = Vec::new();
                    for value in claims.get(name) {
                        if let Some(domain) = &self.domain
                            && filters.contains(&Filter::Local)
                            && !in_realm(value, domain)
                        {
                            continue;
                        }
                        let mut value = Some(value.clone());
                        for filter in filters {
                            value = value.and_then(|value| filter.apply(&value));
                        }
                        if let Some(value) = value.filter(|value| !value.is_empty()) {
                            let value = if self.filter {
                                escape_filter_value(&value)
                            } else {
                                value
                            };
                            if !values.contains(&value) {
                                values.push(value);
                            }
                        }
                    }
                    if values.is_empty() {
                        return Ok(Vec::new());
                    }
                    options.push(values);
                }
            }
        }
        let mut rendered = vec![String::new()];
        for choices in &options {
            if rendered.len().saturating_mul(choices.len()) > MAX_CANDIDATES {
                return Err(error::bad_value(
                    "the claims render more lookups than a rule may ask for",
                ));
            }
            rendered = rendered
                .iter()
                .flat_map(|head| choices.iter().map(move |tail| format!("{head}{tail}")))
                .collect();
        }
        Ok(rendered
            .into_iter()
            .map(|value| Lookup {
                by: self.by.to_owned(),
                value,
                claims: None,
            })
            .collect())
    }
}

/// Builds a [`Matcher`] and gives it the pin sources its rules name.
pub struct MatcherBuilder {
    config: MatcherConfig,
    sources: BTreeMap<String, Arc<dyn PinSource>>,
}

impl fmt::Debug for MatcherBuilder {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MatcherBuilder")
            .field("rules", &self.config.rule.len())
            .field("sources", &self.sources.len())
            .finish()
    }
}

impl MatcherBuilder {
    /// Gives the rules that say `pins = "<name>"` this source.
    #[must_use]
    pub fn pin_source(mut self, name: impl Into<String>, source: Arc<dyn PinSource>) -> Self {
        self.sources.insert(name.into(), source);
        self
    }

    /// Checks the rules and compiles them.
    ///
    /// # Errors
    ///
    /// As [`Matcher::new`], and a rule naming a pin source that was not given.
    pub fn build(self) -> Result<Matcher, MatcherError> {
        let mut rules = Vec::with_capacity(self.config.rule.len());
        for (index, rule) in self.config.rule.iter().enumerate() {
            let compiled = Compiled::new(index, rule)?;
            if let Some(name) = &compiled.pins
                && !self.sources.contains_key(name)
            {
                return Err(fail(
                    index,
                    "the rule names a pin source that was not given",
                ));
            }
            rules.push(compiled);
        }
        Ok(Matcher {
            rules,
            sources: self.sources,
        })
    }
}

/// Ordered rules that tie claims to a directory account.
pub struct Matcher {
    rules: Vec<Compiled>,
    sources: BTreeMap<String, Arc<dyn PinSource>>,
}

impl fmt::Debug for Matcher {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Matcher")
            .field("rules", &self.rules.len())
            .field("sources", &self.sources.len())
            .finish()
    }
}

/// The [`default_rules`].
impl Default for Matcher {
    fn default() -> Self {
        Self::new(MatcherConfig::default()).expect("the default rules are valid")
    }
}

impl Matcher {
    /// Starts a matcher that takes pin sources.
    pub fn builder(config: MatcherConfig) -> MatcherBuilder {
        MatcherBuilder {
            config,
            sources: BTreeMap::new(),
        }
    }

    /// Checks `config` and compiles it, for rules that name no pin source.
    ///
    /// # Errors
    ///
    /// A `by` other than `name`, `email`, `id` or `filter`; a template with an unmatched
    /// brace, a claim name that is not lowercase words, a filter that is not `lower`,
    /// `upper`, `local` or `domain`, or no claim at all; a `filter` template that is not one
    /// balanced parenthesised group; a rule naming a pin source; a template using `local`
    /// with neither a `domain` nor `pins`; a `domain` that is empty, holds `@` or whitespace,
    /// or is given to a template without `local`.
    pub fn new(config: MatcherConfig) -> Result<Self, MatcherError> {
        Self::builder(config).build()
    }

    /// Finds the account `lookup` names.
    ///
    /// A lookup that is not `by: "claims"`, or has no claims, is looked up as it is, except a
    /// `by: "filter"` one, which is refused (`NotFound`): filters come only from the rules'
    /// escaped templates.
    ///
    /// # Errors
    ///
    /// When the directory or a pin source cannot answer, or a rule renders more than
    /// [`MAX_CANDIDATES`] lookups.
    pub fn resolve(
        &self,
        lookup: &Lookup,
        directory: &dyn Directory,
    ) -> Result<Matched, ProviderError> {
        let mut notes = Vec::new();
        let result = self.walk(lookup, directory, &mut notes)?;
        Ok(Matched { result, notes })
    }

    fn walk(
        &self,
        lookup: &Lookup,
        directory: &dyn Directory,
        notes: &mut Vec<String>,
    ) -> Result<Match, ProviderError> {
        let claims = match (&lookup.claims, lookup.by.as_str()) {
            (Some(claims), "claims") => claims,
            (None, "claims") => return Ok(Match::NotFound),
            // A filter is made by a rule's template, with its values escaped; one handed in
            // is somebody's raw text.
            (_, "filter") => return Ok(Match::NotFound),
            _ => {
                return Ok(match asked(directory, lookup.clone())? {
                    Asked::One(person) => Match::Found(person),
                    Asked::None => Match::NotFound,
                    Asked::Several => Match::Ambiguous,
                });
            }
        };
        let pins = PinSet::from_claims(claims);
        for rule in &self.rules {
            let mut found: Vec<Person> = Vec::new();
            for candidate in rule.candidates(claims)? {
                match asked(directory, candidate)? {
                    Asked::None => {}
                    Asked::Several => return Ok(Match::Ambiguous),
                    Asked::One(person) => {
                        if !found.iter().any(|known| known.id == person.id) {
                            found.push(person);
                        }
                    }
                }
            }
            match found.len() {
                0 => continue,
                1 => {}
                _ => return Ok(Match::Ambiguous),
            }
            let person = found.remove(0);
            let Some(name) = &rule.pins else {
                return Ok(Match::Found(person));
            };
            let Some(source) = self.sources.get(name) else {
                continue;
            };
            if pins.is_empty() {
                continue;
            }
            match source.check(&person, &pins)? {
                PinCheck::Holds => return Ok(Match::Found(person)),
                PinCheck::Refused(reason) => notes.push(reason),
                _ => {}
            }
        }
        Ok(Match::NotFound)
    }
}

enum Asked {
    None,
    One(Person),
    Several,
}

/// One lookup, an ambiguity being its own answer and every other error the caller's.
fn asked(directory: &dyn Directory, lookup: Lookup) -> Result<Asked, ProviderError> {
    match directory.lookup(lookup) {
        Ok(Some(person)) => Ok(Asked::One(person)),
        Ok(None) => Ok(Asked::None),
        Err(failure) if error::is_ambiguous(&failure) => Ok(Asked::Several),
        Err(failure) => Err(failure),
    }
}

/// Escapes `value` for the right-hand side of an LDAP filter assertion (RFC 4515): `*`, `(`,
/// `)`, `\` and NUL, every other control character, and every byte of a non-ASCII character,
/// as `\` and two lowercase hex digits. What is left is printable ASCII, so a value can never
/// end an assertion, start another, or become a wildcard.
pub fn escape_filter_value(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_graphic() && !matches!(byte, b'*' | b'(' | b')' | b'\\') || byte == b' ' {
            output.push(char::from(byte));
        } else {
            output.push('\\');
            output.push_str(&format!("{byte:02x}"));
        }
    }
    output
}
