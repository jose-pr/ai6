# `coqui-access` — API header

Header-file-style reference for the `coqui-access` crate: every public item with its
signature, contract and gotchas, so the crate can be used without reading its source. It
ships inside the package and is self-contained. Development documentation lives with the
source at <https://github.com/Boriken-Dev/coqui-access>.

MSRV Rust 1.89. No Cargo features. A default build depends on `guatiao` with its `provider`
feature and on `coqui-builder` (for `Limit`). The crate holds no `unsafe` code. Everything is re-exported at the root, plus
`guatiao`, `error` and `takes`.

## Kinds

All methods are synchronous; a host calls them from a blocking thread. A kind grows by
appending methods with default bodies after the required ones. The floor hash of a kind covers
every method, optional ones too, so appending to a kind that exists changes its hash and
providers built before are refused: a kind is extended by a new kind, and a model by an
`Option` field.

```rust
#[guatiao::kind(name = "coqui_authenticator")]   // floor hash 0xc9379b44
pub trait Authenticator: Send + Sync {
    fn authenticate(&self, attempt: Attempt) -> Result<Verdict, ProviderError>;
    fn describe(&self) -> Result<Description, ProviderError>; // default: Description::default()
}

#[guatiao::kind(name = "coqui_directory")]       // floor hash 0x8f7a4451
pub trait Directory: Send + Sync {
    fn lookup(&self, lookup: Lookup) -> Result<Option<Person>, ProviderError>;
    fn describe(&self) -> Result<Description, ProviderError>; // default: Description::default()
}

#[guatiao::kind(name = "coqui_authorizer")]      // floor hash 0xa08d4555
pub trait Authorizer: Send + Sync {
    fn authorize(&self, request: AuthorizationRequest) -> Result<AuthorizationDecision, ProviderError>;
}

#[guatiao::kind(name = "coqui_accounts")]        // floor hash 0x2468f527
pub trait Accounts: Send + Sync {
    fn accounts(&self) -> Result<Vec<AccountInfo>, ProviderError>;
    fn put(&self, account: AccountPut) -> Result<PutOutcome, ProviderError>;
    fn remove(&self, name: String) -> Result<(), ProviderError>;
    fn waiting(&self) -> Result<Vec<Waiting>, ProviderError>; // default: empty
}

#[guatiao::kind(name = "coqui_sign_in_policy")]  // floor hash 0xd5850cd8
pub trait SignInPolicy: Send + Sync {
    fn decide(&self, context: SignInContext) -> Result<SignInDecision, ProviderError>;
}

#[guatiao::kind(name = "coqui_challenger")]     // floor hash 0x8aa14a59
pub trait Challenger: Send + Sync {
    fn challenge(&self, request: ChallengeRequest) -> Result<Challenge, ProviderError>;
}
```

- `authenticate` returns `Err` only when it could not find out; a host treats that as "could
  not decide", never as a denial. `Verdict::Unknown` means "not mine, ask the next
  authenticator"; `Invalid` means "mine, and refused".
- `lookup` returns `None` for an unknown or disabled account.
- `put` makes an account or changes the one of that name; a field not said is left as it is.
  A provider that makes secrets itself answers with the one it made, once.
- `remove` returns `GUATIAO_ERR_NOT_FOUND` when no account has that name.
- `decide` returns `Err` only when it could not find out; a consumer then offers nothing and
  refuses, never allowing. The contract a consumer follows is the narrative site's sign-in page.
- `challenge` makes what a method needs before it can be answered (a passkey's challenge): `UNSUPPORTED` for a method it does not
  challenge, `NOT_FOUND` for a named account it cannot challenge, `error::busy()` when the source holds too many already. The
  answer goes through `Authenticator::authenticate` with `Attempt::challenge` set to `Challenge::challenge`. A host builds one
  instance per kind, so a challenge one instance issued must be found by the other: keep it in a table both read, never in memory.
- A provider offers the kinds it implements: `#[provider(Authenticator, Directory, Accounts,
  crate = ::coqui_access::guatiao)]`.
- `KIND_PREFIX` is `"coqui_"`: a host scanning libraries keeps those that declare a kind
  beginning with it.

## Models

All models derive Guatiao `ToValue`, `FromValue` and `Schema`. A model grows by `Option`
fields. Unknown keys are skipped when reading.

| Model | Fields (required first) |
| --- | --- |
| `Attempt` | `name?`, `secret?`, `headers: Vec<(String, String)>`, `peer?`, `secure?`, `certificate?`, `certificates?: Vec<Bytes>` (DER, leaf first), `method?` (the `methods` word the credential is presented as; `None` is the provider's default), `challenge?` (what a `Challenger` issued for this sign-in; `Debug` shows presence only) |
| `Lookup` | `by`, `value`, `claims?` — `by` is `id`, `name`, `email`, `filter` (an LDAP filter) or `claims` (the authenticator names no account; a matcher decides from `claims`); a provider may add more |
| `Claims` | `values: BTreeMap<String, Vec<String>>` (empty is not written), `certificate?: Bytes` (the leaf as DER); `get(name) -> &[String]`; `Debug` shows counts |
| `Verdict` | enum tagged by `kind`: `Unknown`, `Invalid`, `Valid(Lookup)` |
| `Person` | `id`, `name`, `groups`, `attributes: BTreeMap<String, Value>`, `display_name?`, `email?` |
| `AuthorizationRequest` | `person`, `action`, `resource`, `ancestors: Vec<String>` (the resources `resource` sits under, nearest first; empty by default and then not written) |
| `AuthorizationDecision` | `allowed`, `limits: BTreeMap<String, Value>` |
| `Description` | `name?`, `takes?` (one of `takes::PASSWORD`, `KEY`, `REQUEST`, `OTP`, `PASSKEY`) |
| `AccountInfo` | `id`, `name`, `groups`, `disabled?`, `made_unix?`, `expires_unix?`, `reset?` (an administrator reset the account; only the one-time code provider keeps it), `unlocked_unix?` (when an administrator last cleared its failure counts), `credentials?: Vec<CredentialInfo>` (an account that holds several, a passkey's) |
| `AccountPut` | `name`, `secret?`, `groups?`, `disabled?`, `expires_unix?`, `display_name?` (the label an enrolment shows; providers keeping none ignore it), `reset` (bool, default false, not written when false: an administrator's reset, the provider takes the account's factor away and keeps a mark; a provider with none refuses it as unsupported), `unlock` (bool, same: clears the failure count and any delay and notes when; refused as unsupported by a provider that counts no failures), `proved_current` (bool, default false, not written when false: the caller checked the account's current secret in this request; the one-time code provider refuses a rotation without it, the others ignore it) |
| `ChallengeRequest` | `method?` (`None` is the provider's own), `name?` (the account an earlier step named; `None` asks for any, a usernameless sign-in), `peer?` (so a provider can bound what one source holds); `Debug` shows the method only |
| `Challenge` | `challenge` (the key the consumer keeps and returns in `Attempt::challenge`; base64url for a passkey), `options` (the method's wire text, for a passkey the JSON for `navigator.credentials.get`, passed on unread), `expires_unix`; `Debug` shows the expiry only |
| `CredentialInfo` | `id`, `label?`, `made_unix?`, `used_unix?`, `synced?`, `disabled?`; `Debug` hides the id and label |
| `PutOutcome` | `secret?`, `codes?: Vec<String>` (one-time recovery codes, said once) |
| `Waiting` | `shown`, `peer?`, `seen_unix?` |
| `Factor` | `method`, `provider?` (the id that accepted the step), `person?` (the `Person::id` it named; `Debug` hides it) |
| `SignInContext` | `purpose`, `done: Vec<Factor>`, `address?` (the client's IP as text, no port; `None` when unknown), `client?` (from the endpoint the request arrived on, never from what the client says; a consumer that does that sets `attributes[attributes::CLIENT_VERIFIED]` to `true` with `with_verified_client()`, and `client_is_verified()` reads it, so a policy may treat any other `client` as a claim), `service?`, `time_unix?`, `person?`, `attributes` (empty is not written) |
| `Offer` | `method`, `providers: Vec<String>` (empty is any provider taking the method; empty is not written) |
| `SignInDecision` | `complete`, `methods: Vec<Offer>`, `rule?`, `limits` (empty is not written) |

`headers` are ordered pairs; a header sent twice appears twice; match names case-insensitively.
`Attempt::certificate` is a fingerprint the transport verified the peer holds the key of, in
the canonical text `coqui-peer-tls` writes (`sha256:<hex>`); `Attempt::certificates` is that
certificate and the intermediates the client sent, as DER, leaf first. `Debug` shows only
their count. `Debug` of `Attempt`, `Lookup`, `Person`, `AccountPut`, `PutOutcome` and
`SignInContext` shows no secret, code, address or identity value (an `Attempt`'s `method` and a
context's `purpose`, `client` and `service` are words and are shown).

## Claims

What an authenticator obtained, in full, for a matcher (`coqui_access_kit::matcher`) to tie to a
directory account; the credential's claim is never the account or its groups. The names an
authenticator that reads a certificate uses: `subject.dn`, `subject.cn` and the other
`subject.<attribute>` (`o`, `ou`, `c`, `st`, `l`, `dc`, `uid`, `email`, `serialnumber`), `issuer.dn`
and `issuer.<attribute>`, `serial.hex`, `serial.reversed` (the same bytes last first), `san.upn`,
`san.email`, `san.dns`, `san.uri`, `san.ip`, `fingerprint` and `spki` (both `sha256:<hex>`),
`not_before` and `not_after` (Unix seconds); a name it has nothing for is absent. A password
authenticator names the account in `by`/`value` and has no claims.

## Words (`methods`, `purposes`, `clients`)

```rust
pub mod methods { pub const PASSWORD, CERTIFICATE, PROXY, KEY, TOTP, RECOVERY_CODE, PASSKEY: &str;
                  pub const NEEDS_ACCOUNT: &[&str]; }     // [TOTP, RECOVERY_CODE]
pub mod purposes { pub const SIGN_IN, RESUME, ENROL: &str; }   // "sign_in" | "resume" | "enrol"
pub mod clients  { pub const BROWSER, APP, API: &str; }         // "browser" | "app" | "api"
pub mod attributes { pub const CLIENT_VERIFIED: &str; }          // "client_verified"
pub mod takes    { /* PASSWORD, KEY, REQUEST, */ pub const OTP, PASSKEY: &str; }   // "otp", "passkey"
```

- A method in `NEEDS_ACCOUNT` checks a code against an account an earlier step already named, so
  nothing offers it before one has. A policy may use words of its own beside these.

## Subjects

```rust
#[non_exhaustive]
pub enum Subject { Account(String), Person(String), Group(String), AnySignedIn }  // Clone, Eq, Ord, Hash
impl Subject {
    pub fn kind(&self) -> &'static str;          // "account" | "person" | "group" | "any"
    pub fn value(&self) -> &str;                 // "signed-in" for AnySignedIn
    pub fn matches(&self, person: &Person) -> bool;
}
impl FromStr for Subject { type Err = SubjectError; }   // "account:<name>", "person:<id>", "group:<name>", "any:signed-in"
impl Display for Subject;                               // the same text
pub struct SubjectError;                                // non_exhaustive, Error
```

- `Account` matches `Person::name`, `Person` matches `Person::id`, `Group` any of
  `Person::groups`. `Account` and `Person` compare exact strings. `Group` compares by Unicode
  lowercase (`char::to_lowercase`, not full case folding), so `group:Domain Users` matches a
  person whose groups hold `domain users` (SSSD reports Active Directory groups in lowercase),
  and two POSIX groups that differ only in case are one. `==` on a `Subject` stays exact.
  `AnySignedIn` matches every person whose
  `Person::id` is not empty, so an anonymous caller never matches; it grants nothing by itself.
- `any:signed-in` is the only reserved `any:` name: every other text starting `any:` is a
  `SubjectError` (`group:any:signed-in` is still a group, since the kind comes first).
  `Debug` shows `Subject(any:signed-in)` whole, since it names no one. Everything that reads a
  `Subject` (roles, ACL principals, sign-in rule subjects) accepts it.
- The value after the first colon may hold colons and inner spaces (`group:Domain Users`); empty,
  holding a control character, or starting or ending with whitespace is
  refused. Policies add their own length and character rules.
- `Debug` shows the kind only: the value is an identity.

## Limits

```rust
pub struct Limit;                       // Copy, Eq, Hash, Debug, Display; coqui_builder::Leaf (text: N or `none`)
impl Limit {
    pub const UNBOUNDED: Limit;
    pub const fn at(count: u32) -> Limit;
    pub const fn get(self) -> Option<u32>;
    pub fn allows(self, count: usize) -> bool;   // count <= the bound, or no bound
    pub fn or_max(self) -> usize;                // usize::MAX when unbounded
}
impl From<u32> for Limit; impl From<Option<u32>> for Limit;
```

- The resource bounds of the crates here (roles, rules, record fields, password bytes) are
  `Limit`s with defaults a caller changes or lifts; none is a security property.
- As configuration a `Limit` is a count or `none` (no bound): `max_chain = 16`, `max_chain = "none"`.
  A struct field of this type with `#[map(default = Limit::at(n))]` reads an absent key as `n`.
  A document's `null` is a key that is not given, so it reads that default, never "no bound";
  `Display` stays the human wording (`at most 5`, `unbounded`).

## Errors (`coqui_access::error`)

`bad_value`, `not_found`, `unsupported`, `internal`: each `fn(&str) -> ProviderError` with the
matching Guatiao status. Messages never hold credentials. `busy() -> ProviderError` is what a provider answers when it will not take another pending ceremony from a source (`bad_value` with the message `BUSY`; `is_busy(&ProviderError) -> bool`). `ambiguous() -> ProviderError` is what a
directory answers when one lookup finds more than one account (`bad_value` with the message
`AMBIGUOUS`); `is_ambiguous(&ProviderError) -> bool` tells it from any other refusal, and a matcher
refuses the sign-in on it.

## Gotchas

- Do not rename a model type or a kind trait: the hash is taken over type names as written.
  `tests/contract.rs` pins each hash and each model's required keys; `kinds.json` is the same
  contract as data.
- `Person::id` is what `AccountInfo::id` and a `Lookup { by: "id" }` mean.
