# `coqui-access-signin` — API header

Header-file-style reference for the `coqui-access-signin` crate: every public item with its
signature, contract and gotchas, so the crate can be used without reading its source. It
ships inside the package and is self-contained. Development documentation lives with the
source at <https://github.com/Boriken-Dev/coqui-access>.

MSRV Rust 1.89. Feature `test-support` adds the module `vectors` (see below). Depends on
`coqui-access`, `coqui-builder`, `coqui-net-acl` and `guatiao-builder`: no format crate, no
`unsafe` code. Everything is at the crate root. `SignInRules` is the Guatiao provider
`coqui_signin_rules` (`#[derive(Provider)]`, kind `SignInPolicy`, configured with
`guatiao_builder::Described<SignInConfig>`); `coqui-access-signin-plugin` (a `cdylib`) is its one
`providers!` line. The contract a consumer follows is the narrative site's sign-in page.

```rust
pub struct SignInLimits { pub rules: Limit, pub chains: Limit, pub chain_methods: Limit,
                          pub networks: Limit }
    // non_exhaustive, Copy; coqui_builder::Describe (each key defaulting, a bound is a count or `none`);
    // Default = 1 024, 64, 8, 1 024 (chains and networks are per rule, methods per chain)
impl SignInLimits {
    pub const UNBOUNDED: Self;
    pub const fn rules(self, Limit) -> Self; pub const fn chains(self, Limit) -> Self;
    pub const fn chain_methods(self, Limit) -> Self; pub const fn networks(self, Limit) -> Self;
}
pub struct RuleDef { pub name: Option<String>, pub purposes: Option<Vec<String>>,
    pub networks: Vec<String>, pub clients: Vec<String>, pub services: Vec<String>,
    pub subjects: Vec<String>, pub providers: Vec<String>, pub allow: Vec<Vec<String>> }
    // non_exhaustive; coqui_builder::Describe, deny_unknown_fields; only `allow` is required
impl RuleDef {
    pub fn new<I, C>(chains: I) -> Self where I: IntoIterator<Item = C>, C: IntoIterator, C::Item: Into<String>;
    pub fn name(self, impl Into<String>) -> Self; pub fn purposes(self, ..) -> Self;
    pub fn networks(self, ..) -> Self; pub fn clients(self, ..) -> Self; pub fn services(self, ..) -> Self;
    pub fn subjects(self, ..) -> Self; pub fn providers(self, ..) -> Self;
}
pub struct SignInConfig { pub rule: Vec<RuleDef>, pub limits: SignInLimits }
    // non_exhaustive, Default (no rule); Describe, deny_unknown_fields; SignInConfig::new()
pub struct SignInRulesBuilder;                     // Clone, Default
impl SignInRulesBuilder {
    pub fn rule(self, rule: RuleDef) -> Self;      // rules are tried in the order added
    pub fn limits(self, limits: SignInLimits) -> Self;
    pub fn build(self) -> Result<SignInRules, SignInRulesError>;
}
pub struct SignInRules;                            // Clone, Default (no rule), Debug (a count only)
impl SignInRules {
    pub fn builder() -> SignInRulesBuilder;
    pub fn new(config: SignInConfig) -> Result<Self, SignInRulesError>;
    pub fn single_factor(methods: impl IntoIterator<Item = impl Into<String>>) -> Result<Self, SignInRulesError>;
}
impl SignInPolicy for SignInRules { /* decide: never Err */ }
impl TryFrom<SignInConfig> for SignInRules { type Error = SignInRulesError; }   // = new
pub struct SignInRulesError;                       // non_exhaustive, Display, Error; names the rule by position, never quotes input
impl From<SignInRulesError> for ProviderError;     // GUATIAO_ERR_BAD_VALUE
```

## Evaluation

- **Steps that name different people are refused first.** If the `person` of two `done` factors
  differ, or one differs from the context's `person.id`, the answer is an empty offer and
  `complete` false whatever the rules say. A factor with no `person` is not held against it.

- A rule is **false** when its `purposes` (default `sign_in` and `resume`) lack the context's
  purpose, or `networks` do not contain its `address`, or `clients` or `services` are set and the
  context's is absent or not listed. Otherwise it is **unknown** while `subjects` is set and
  `person` is `None`, and **true** otherwise. A false condition wins over an unknown one.
- An `address` that is `None`, is not an IP address (a port, a zone, text) or is not in any listed
  network matches no `networks` condition. An IPv4-mapped IPv6 address is the IPv4 address.
- **`person` is `None`:** walk the rules in order; a false rule is skipped, an unknown one has its
  chains collected, the first true one has its chains collected and the walk stops. The offer is the
  methods of every collected chain, once each in the order met, minus
  `coqui_access::methods::NEEDS_ACCOUNT`; `complete` is false; `rule` is the true rule's name (none if
  only unknown rules or none applied).
- **`person` is set:** the first true rule alone decides. A `done` step counts when the rule names no
  `providers`, or the step's `provider` is listed (a step with no provider then does not count).
  `complete` is true when every method of one chain is among the counted steps (more steps do not
  undo it). Otherwise the offer is the methods missing from the chains that hold every counted step;
  no such chain, or no true rule, is an empty offer and `complete` false, a refusal.
- An `Offer`'s `providers` is the rule's `providers`; a method offered by several rules asks the union
  of their lists, or any provider when one of them lists none.
- `rule` is the rule's `name`, else its position from 0 as text. `limits` is always empty. `time_unix`
  and `attributes` are not read.
- No rule offers nothing: an empty set denies. A policy that has none configured is the consumer's
  `SignInRules::single_factor(methods)`.

## Building

`new` and `build` refuse: a set past its `limits`; a rule with no chain, an empty chain, or too many
chains or methods; a method that is not 1 to 64 of `a-z`, `0-9`, `_`; a method repeated in a chain; a
chain made only of `NEEDS_ACCOUNT` methods; a purpose other than `sign_in`, `resume`, `enrol`, `enrol_first`, or an
empty `purposes`; a client other than `browser`, `app`, `api`; a network that is not an address or
CIDR network, or is `any`; a subject that is not `account:`, `person:` or `group:`; a service,
provider or rule name that is empty, over 256 bytes, or holds whitespace (a name may hold spaces) or a
control character. Method words beyond those of `coqui_access::methods` are allowed; a misspelt one
makes its chain unsatisfiable.

## Provider (`coqui_signin_rules`)

Id `coqui_signin_rules`, name `Sign-in rules`, kind `SignInPolicy`, config `Described<SignInConfig>`.
Config: `rule = [ { name?, purposes?, networks?, clients?, services?, subjects?, providers?, allow } ]`
and `limits = { rules, chains, chain_methods, networks }`. An invalid set is `GUATIAO_ERR_BAD_VALUE`
at build time.

## `vectors` (feature `test-support`)

`vectors::POLICY` and `vectors::FLOWS` are the shared JSON vectors as `&'static str`: `POLICY` cases
`{ name, config, context, expect: { methods, complete } }` run through any implementation of the
policy; `FLOWS` scripts a consumer's sign-in steps. Their shape is in the narrative site's sign-in
page.
