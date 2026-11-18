//! Rules that decide which methods a sign-in may use, from where and how it is made.
//!
//! A rule says when it applies (purpose, client network, kind of client, service, who) and
//! which chains of methods it allows; a chain is a set of methods that must all succeed and
//! name one account. Rules are tried in order. Before anyone is named the rules collect what
//! to offer; once a step has named someone, the first rule that applies decides alone. An
//! address that is not known never satisfies a `networks` condition, so a stricter rule
//! placed last is where it falls.
//!
//! ```
//! use coqui_access::{SignInContext, SignInPolicy, methods, purposes};
//! use coqui_access_signin::{RuleDef, SignInRules};
//!
//! let rules = SignInRules::builder()
//!     .rule(RuleDef::new([["password"], ["certificate"]]).networks(["10.0.0.0/8"]))
//!     .rule(RuleDef::new([["certificate"]]))
//!     .build()
//!     .unwrap();
//! let from = |address: &str| SignInContext {
//!     purpose: purposes::SIGN_IN.to_owned(),
//!     address: Some(address.to_owned()),
//!     ..SignInContext::default()
//! };
//! let inside = rules.decide(from("10.1.2.3")).unwrap();
//! assert_eq!(inside.methods.len(), 2);
//! let outside = rules.decide(from("203.0.113.7")).unwrap();
//! assert_eq!(outside.methods[0].method, methods::CERTIFICATE);
//! ```
//!
//! [`SignInConfig`] is the same thing as plain data, and the configuration [`SignInRules`]
//! takes as the `coqui_signin_rules` provider, which `coqui-access-signin-plugin` exports.
#![forbid(unsafe_code)]
#![warn(missing_docs)]

use std::fmt;
use std::net::IpAddr;

use coqui_builder::Describe;
use guatiao_builder::Described;

use coqui_access::guatiao::Provider;
use coqui_access::guatiao::library::ProviderError;
use coqui_access::{
    Factor, Limit, Offer, SignInContext, SignInDecision, SignInPolicy, Subject, clients, error,
    methods, purposes,
};
use coqui_net_acl::IpNet;

#[cfg(feature = "test-support")]
pub mod vectors;

/// Bounds on a rule set, each a [`Limit`] the caller can change or lift ([`Limit::UNBOUNDED`]).
///
/// They bound the work and memory a rule set costs, not who may sign in: the rule scan per
/// request, and the tables built once. By default at most 1 024 rules, 64 chains and 1 024
/// networks per rule, and 8 methods per chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Describe)]
#[map(deny_unknown_fields)]
#[non_exhaustive]
pub struct SignInLimits {
    /// Rules a set holds.
    #[map(default = Limit::at(1_024))]
    pub rules: Limit,
    /// Chains one rule allows.
    #[map(default = Limit::at(64))]
    pub chains: Limit,
    /// Methods one chain holds.
    #[map(default = Limit::at(8))]
    pub chain_methods: Limit,
    /// Networks one rule lists.
    #[map(default = Limit::at(1_024))]
    pub networks: Limit,
}

impl Default for SignInLimits {
    fn default() -> Self {
        Self {
            rules: Limit::at(1_024),
            chains: Limit::at(64),
            chain_methods: Limit::at(8),
            networks: Limit::at(1_024),
        }
    }
}

impl SignInLimits {
    /// No bound at all.
    pub const UNBOUNDED: Self = Self {
        rules: Limit::UNBOUNDED,
        chains: Limit::UNBOUNDED,
        chain_methods: Limit::UNBOUNDED,
        networks: Limit::UNBOUNDED,
    };

    /// These limits with `rules`.
    #[must_use]
    pub const fn rules(mut self, rules: Limit) -> Self {
        self.rules = rules;
        self
    }

    /// These limits with `chains`.
    #[must_use]
    pub const fn chains(mut self, chains: Limit) -> Self {
        self.chains = chains;
        self
    }

    /// These limits with `chain_methods`.
    #[must_use]
    pub const fn chain_methods(mut self, methods: Limit) -> Self {
        self.chain_methods = methods;
        self
    }

    /// These limits with `networks`.
    #[must_use]
    pub const fn networks(mut self, networks: Limit) -> Self {
        self.networks = networks;
        self
    }
}

/// One rule as data.
///
/// Every condition left out is true. A rule applies when all its conditions are true; one
/// that is false makes the rule not apply, and `subjects` is unknown until a step has named
/// someone.
#[derive(Debug, Clone, PartialEq, Eq, Describe)]
#[map(deny_unknown_fields)]
#[non_exhaustive]
pub struct RuleDef {
    /// A name for the decision's `rule`; the rule's position (from 0) when absent.
    pub name: Option<String>,
    /// The purposes it applies to, among `sign_in`, `resume` and `enrol`. Absent means
    /// `sign_in` and `resume`, so a rule for `enrol` is always written out.
    pub purposes: Option<Vec<String>>,
    /// Client networks (`10.0.0.0/8`, `2001:db8::/32`, a single address). An address that is
    /// not known, or not an IP address, is in none of them. `any` is refused: write no
    /// `networks` instead.
    #[map(default)]
    pub networks: Vec<String>,
    /// Kinds of client, among `browser`, `app` and `api`.
    #[map(default)]
    pub clients: Vec<String>,
    /// The consumer's own words for the service signed in to.
    #[map(default)]
    pub services: Vec<String>,
    /// Who it applies to, as `account:<name>`, `person:<id>` or `group:<name>`. Unknown
    /// until a step has named someone.
    #[map(default)]
    pub subjects: Vec<String>,
    /// The providers to ask for the methods this rule offers, in order; absent or empty is
    /// any provider that takes the method. A step counts as done only when one of these
    /// accepted it.
    #[map(default)]
    pub providers: Vec<String>,
    /// The chains it allows: each is methods that must all succeed and name one account.
    pub allow: Vec<Vec<String>>,
}

impl RuleDef {
    /// A rule that applies to every sign-in and allows `chains`.
    pub fn new<I, C>(chains: I) -> Self
    where
        I: IntoIterator<Item = C>,
        C: IntoIterator,
        C::Item: Into<String>,
    {
        Self {
            name: None,
            purposes: None,
            networks: Vec::new(),
            clients: Vec::new(),
            services: Vec::new(),
            subjects: Vec::new(),
            providers: Vec::new(),
            allow: chains
                .into_iter()
                .map(|chain| chain.into_iter().map(Into::into).collect())
                .collect(),
        }
    }

    /// This rule with a name.
    #[must_use]
    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// This rule applying to `purposes` only.
    #[must_use]
    pub fn purposes(mut self, purposes: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.purposes = Some(purposes.into_iter().map(Into::into).collect());
        self
    }

    /// This rule applying to clients inside `networks` only.
    #[must_use]
    pub fn networks(mut self, networks: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.networks = networks.into_iter().map(Into::into).collect();
        self
    }

    /// This rule applying to these kinds of client only.
    #[must_use]
    pub fn clients(mut self, clients: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.clients = clients.into_iter().map(Into::into).collect();
        self
    }

    /// This rule applying to these services only.
    #[must_use]
    pub fn services(mut self, services: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.services = services.into_iter().map(Into::into).collect();
        self
    }

    /// This rule applying to these subjects only.
    #[must_use]
    pub fn subjects(mut self, subjects: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.subjects = subjects.into_iter().map(Into::into).collect();
        self
    }

    /// This rule offering its methods from these providers only.
    #[must_use]
    pub fn providers(mut self, providers: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.providers = providers.into_iter().map(Into::into).collect();
        self
    }
}

/// A rule set as plain data: what [`SignInRules::new`] checks and compiles, and the
/// configuration of the `coqui_signin_rules` provider.
///
/// No rule means nothing is ever offered.
#[derive(Debug, Clone, Default, PartialEq, Eq, Describe)]
#[map(deny_unknown_fields)]
#[non_exhaustive]
pub struct SignInConfig {
    /// The rules, in the order they are tried.
    #[map(default)]
    pub rule: Vec<RuleDef>,
    /// Bounds on the set's size; [`SignInLimits::default`] when absent.
    #[map(default)]
    pub limits: SignInLimits,
}

impl SignInConfig {
    /// An empty set, which offers nothing.
    pub fn new() -> Self {
        Self::default()
    }
}

/// Builds [`SignInRules`]; rules are tried in the order they are added.
#[derive(Debug, Clone, Default)]
pub struct SignInRulesBuilder {
    config: SignInConfig,
}

impl SignInRulesBuilder {
    /// Adds a rule after the ones already added.
    #[must_use]
    pub fn rule(mut self, rule: RuleDef) -> Self {
        self.config.rule.push(rule);
        self
    }

    /// Bounds the set's size. Default [`SignInLimits::default`].
    #[must_use]
    pub fn limits(mut self, limits: SignInLimits) -> Self {
        self.config.limits = limits;
        self
    }

    /// Checks the rules and compiles them.
    ///
    /// # Errors
    ///
    /// As [`SignInRules::new`].
    pub fn build(self) -> Result<SignInRules, SignInRulesError> {
        SignInRules::new(self.config)
    }
}

/// The `coqui_signin_rules` provider: ordered rules over the sign-in's context.
///
/// Evaluation, per request:
///
/// - Each rule is true, false or unknown. One false condition makes it false; otherwise it
///   is unknown while `subjects` is set and nobody is named, and true.
/// - Nobody named: the chains of every unknown rule, and of the first true rule (where the
///   walk stops), are collected; the offer is their methods, minus
///   [`methods::NEEDS_ACCOUNT`].
/// - Someone named: the first true rule alone decides. The sign-in is complete when one of
///   its chains is within the steps done; otherwise the offer is the methods still missing
///   from the chains that hold every step done, and none such is a refusal.
/// - No rule true: nothing is offered.
/// - Steps done that name different people (`Factor::person`), or a person other than the
///   context's, are a refusal whatever the rules say.
#[derive(Clone, Provider)]
#[provider(
    SignInPolicy,
    id = "coqui_signin_rules",
    name = "Sign-in rules",
    config = Described<SignInConfig>,
    crate = ::coqui_access::guatiao
)]
pub struct SignInRules {
    rules: Vec<Compiled>,
}

#[derive(Clone)]
struct Compiled {
    name: String,
    purposes: Vec<String>,
    networks: Vec<IpNet>,
    clients: Vec<String>,
    services: Vec<String>,
    subjects: Vec<Subject>,
    providers: Vec<String>,
    chains: Vec<Vec<String>>,
}

/// The empty set, which offers nothing.
impl Default for SignInRules {
    fn default() -> Self {
        Self { rules: Vec::new() }
    }
}

impl fmt::Debug for SignInRules {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SignInRules")
            .field("rules", &self.rules.len())
            .finish()
    }
}

/// An invalid rule set. The message names the rule by position and never quotes the input.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct SignInRulesError(String);

impl fmt::Display for SignInRulesError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl core::error::Error for SignInRulesError {}

/// A rule set refused while building the provider is a bad configuration value.
impl From<SignInRulesError> for ProviderError {
    fn from(error: SignInRulesError) -> Self {
        error::bad_value(&error.0)
    }
}

impl TryFrom<Described<SignInConfig>> for SignInRules {
    type Error = SignInRulesError;

    fn try_from(config: Described<SignInConfig>) -> Result<Self, SignInRulesError> {
        Self::new(config.0)
    }
}

impl TryFrom<SignInConfig> for SignInRules {
    type Error = SignInRulesError;

    fn try_from(config: SignInConfig) -> Result<Self, SignInRulesError> {
        Self::new(config)
    }
}

impl SignInRules {
    /// Starts a rule set in code; add rules, then [`build`](SignInRulesBuilder::build).
    pub fn builder() -> SignInRulesBuilder {
        SignInRulesBuilder::default()
    }

    /// The rules of a deployment that has no policy: each of `methods` alone is enough, from
    /// anywhere, for `sign_in` and `resume`. Enrolment is then an administrator's only.
    ///
    /// # Errors
    ///
    /// When a method is not a valid word, or all of `methods` need an account named first.
    pub fn single_factor(
        methods: impl IntoIterator<Item = impl Into<String>>,
    ) -> Result<Self, SignInRulesError> {
        let chains: Vec<Vec<String>> = methods
            .into_iter()
            .map(|method| vec![method.into()])
            .collect();
        Self::builder().rule(RuleDef::new(chains)).build()
    }

    /// Checks `config` and compiles it.
    ///
    /// # Errors
    ///
    /// For a set past its [`SignInLimits`]; a rule with no chain, an empty chain, a method
    /// repeated in a chain or that is not a valid word, or a chain made only of
    /// [`methods::NEEDS_ACCOUNT`] methods (it could never start); a purpose or kind of
    /// client that is not one of the known words; a network or subject that does not parse;
    /// `any` among the networks; an empty `purposes`.
    pub fn new(config: SignInConfig) -> Result<Self, SignInRulesError> {
        let limits = config.limits;
        if !limits.rules.allows(config.rule.len()) {
            return Err(fail("the set has too many rules"));
        }
        let mut rules = Vec::with_capacity(config.rule.len());
        for (index, rule) in config.rule.into_iter().enumerate() {
            rules.push(compile(index, rule, &limits)?);
        }
        Ok(Self { rules })
    }

    fn decision(&self, context: &SignInContext) -> SignInDecision {
        // Steps that name different people are not one sign-in, whatever each would allow.
        let named = context
            .done
            .iter()
            .filter_map(|factor| factor.person.as_deref())
            .chain(context.person.as_ref().map(|person| person.id.as_str()));
        if let Some(first) = named.clone().next()
            && named.clone().any(|id| id != first)
        {
            return SignInDecision::default();
        }
        let address = context
            .address
            .as_deref()
            .and_then(|text| text.parse::<IpAddr>().ok());
        match &context.person {
            None => self.before_anyone(context, address),
            Some(person) => self.after_someone(context, address, person),
        }
    }

    fn before_anyone(&self, context: &SignInContext, address: Option<IpAddr>) -> SignInDecision {
        let mut offers = Offers::default();
        let mut decided = None;
        for rule in &self.rules {
            match rule.truth(context, address, None) {
                Truth::False => continue,
                Truth::Unknown => {}
                Truth::True => decided = Some(rule),
            }
            for chain in &rule.chains {
                for method in chain {
                    if !methods::NEEDS_ACCOUNT.contains(&method.as_str()) {
                        offers.add(method, &rule.providers);
                    }
                }
            }
            if decided.is_some() {
                break;
            }
        }
        SignInDecision {
            complete: false,
            methods: offers.into_offers(),
            rule: decided.map(|rule| rule.name.clone()),
            ..SignInDecision::default()
        }
    }

    fn after_someone(
        &self,
        context: &SignInContext,
        address: Option<IpAddr>,
        person: &coqui_access::Person,
    ) -> SignInDecision {
        let Some(rule) = self
            .rules
            .iter()
            .find(|rule| rule.truth(context, address, Some(person)) == Truth::True)
        else {
            return SignInDecision::default();
        };
        let done: Vec<&str> = context
            .done
            .iter()
            .filter(|factor| rule.counts(factor))
            .map(|factor| factor.method.as_str())
            .collect();
        let holds = |chain: &Vec<String>, step: &&str| chain.iter().any(|method| method == step);
        let complete = rule
            .chains
            .iter()
            .any(|chain| chain.iter().all(|method| done.contains(&method.as_str())));
        let mut offers = Offers::default();
        if !complete {
            for chain in rule
                .chains
                .iter()
                .filter(|chain| done.iter().all(|step| holds(chain, step)))
            {
                for method in chain
                    .iter()
                    .filter(|method| !done.contains(&method.as_str()))
                {
                    offers.add(method, &rule.providers);
                }
            }
        }
        SignInDecision {
            complete,
            methods: offers.into_offers(),
            rule: Some(rule.name.clone()),
            ..SignInDecision::default()
        }
    }
}

impl SignInPolicy for SignInRules {
    fn decide(&self, context: SignInContext) -> Result<SignInDecision, ProviderError> {
        Ok(self.decision(&context))
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Truth {
    True,
    False,
    Unknown,
}

impl Compiled {
    fn truth(
        &self,
        context: &SignInContext,
        address: Option<IpAddr>,
        person: Option<&coqui_access::Person>,
    ) -> Truth {
        let in_purposes = self.purposes.contains(&context.purpose);
        let in_networks = self.networks.is_empty()
            || address.is_some_and(|address| self.networks.iter().any(|net| net.contains(address)));
        let is_one_of = |allowed: &[String], word: &Option<String>| {
            allowed.is_empty() || word.as_ref().is_some_and(|word| allowed.contains(word))
        };
        if !in_purposes
            || !in_networks
            || !is_one_of(&self.clients, &context.client)
            || !is_one_of(&self.services, &context.service)
        {
            return Truth::False;
        }
        if self.subjects.is_empty() {
            return Truth::True;
        }
        match person {
            None => Truth::Unknown,
            Some(person) if self.subjects.iter().any(|subject| subject.matches(person)) => {
                Truth::True
            }
            Some(_) => Truth::False,
        }
    }

    /// Whether a step done counts toward this rule: a rule that names providers counts only
    /// a step one of them accepted.
    fn counts(&self, factor: &Factor) -> bool {
        self.providers.is_empty()
            || factor
                .provider
                .as_ref()
                .is_some_and(|provider| self.providers.contains(provider))
    }
}

/// The methods to offer, in the order first met, each with the providers to ask: none listed
/// is any.
#[derive(Default)]
struct Offers {
    list: Vec<(String, Option<Vec<String>>)>,
}

impl Offers {
    fn add(&mut self, method: &str, providers: &[String]) {
        let Some((_, kept)) = self.list.iter_mut().find(|(word, _)| word == method) else {
            let kept = (!providers.is_empty()).then(|| providers.to_vec());
            self.list.push((method.to_owned(), kept));
            return;
        };
        match kept {
            Some(kept) if !providers.is_empty() => {
                for provider in providers {
                    if !kept.contains(provider) {
                        kept.push(provider.clone());
                    }
                }
            }
            _ => *kept = None,
        }
    }

    fn into_offers(self) -> Vec<Offer> {
        self.list
            .into_iter()
            .map(|(method, providers)| Offer {
                method,
                providers: providers.unwrap_or_default(),
            })
            .collect()
    }
}

fn fail(message: &str) -> SignInRulesError {
    SignInRulesError(message.to_owned())
}

fn rule_fail(index: usize, message: &str) -> SignInRulesError {
    SignInRulesError(format!("rule {index}: {message}"))
}

fn compile(
    index: usize,
    rule: RuleDef,
    limits: &SignInLimits,
) -> Result<Compiled, SignInRulesError> {
    let name = match rule.name {
        Some(name) if !label(&name) => return Err(rule_fail(index, "the name is invalid")),
        Some(name) => name,
        None => index.to_string(),
    };
    let purposes = match rule.purposes {
        None => vec![purposes::SIGN_IN.to_owned(), purposes::RESUME.to_owned()],
        Some(given) if given.is_empty() => {
            return Err(rule_fail(
                index,
                "purposes is empty; leave it out for the default",
            ));
        }
        Some(given) => {
            let known = [
                purposes::SIGN_IN,
                purposes::RESUME,
                purposes::ENROL,
                purposes::ENROL_FIRST,
            ];
            if !given.iter().all(|word| known.contains(&word.as_str())) {
                return Err(rule_fail(
                    index,
                    "a purpose is not sign_in, resume, enrol or enrol_first",
                ));
            }
            given
        }
    };
    if !limits.networks.allows(rule.networks.len()) {
        return Err(rule_fail(index, "too many networks"));
    }
    let mut networks = Vec::with_capacity(rule.networks.len());
    for network in &rule.networks {
        match network.parse::<IpNet>() {
            Ok(IpNet::Any) => {
                return Err(rule_fail(
                    index,
                    "a network is `any`; write no networks instead",
                ));
            }
            Ok(network) => networks.push(network),
            Err(_) => {
                return Err(rule_fail(
                    index,
                    "a network is not an address or CIDR network",
                ));
            }
        }
    }
    let known = [clients::BROWSER, clients::APP, clients::API];
    if !rule
        .clients
        .iter()
        .all(|word| known.contains(&word.as_str()))
    {
        return Err(rule_fail(index, "a client is not browser, app or api"));
    }
    if !rule.services.iter().all(|word| text(word)) || !rule.providers.iter().all(|id| text(id)) {
        return Err(rule_fail(index, "a service or provider is invalid"));
    }
    let subjects = rule
        .subjects
        .iter()
        .map(|subject| subject.parse::<Subject>())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| {
            rule_fail(
                index,
                "a subject is not account:<name>, person:<id>, group:<name> (not empty, no control character, no leading or trailing whitespace) or any:signed-in",
            )
        })?;
    if rule.allow.is_empty() || !limits.chains.allows(rule.allow.len()) {
        return Err(rule_fail(
            index,
            "allow needs at least one chain, and at most the limit",
        ));
    }
    for chain in &rule.allow {
        if chain.is_empty() || !limits.chain_methods.allows(chain.len()) {
            return Err(rule_fail(index, "a chain is empty or too long"));
        }
        if !chain.iter().all(|method| word(method)) {
            return Err(rule_fail(index, "a method is not a lowercase word"));
        }
        if chain
            .iter()
            .enumerate()
            .any(|(at, method)| chain[..at].contains(method))
        {
            return Err(rule_fail(index, "a chain repeats a method"));
        }
        if chain
            .iter()
            .all(|method| methods::NEEDS_ACCOUNT.contains(&method.as_str()))
        {
            return Err(rule_fail(
                index,
                "a chain made only of methods that need an account named first can never start",
            ));
        }
    }
    Ok(Compiled {
        name,
        purposes,
        networks,
        clients: rule.clients,
        services: rule.services,
        subjects,
        providers: rule.providers,
        chains: rule.allow,
    })
}

/// A method word: lowercase letters, digits and underscores, at most 64 bytes.
fn word(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
}

/// A rule's name: not empty, at most 256 bytes, no control character.
fn label(value: &str) -> bool {
    !value.is_empty() && value.len() <= 256 && !value.chars().any(char::is_control)
}

/// A name or id: not empty, at most 256 bytes, no whitespace or control character.
fn text(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && !value.chars().any(|c| c.is_control() || c.is_whitespace())
}
