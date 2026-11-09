# `coqui-access` — API header

Header-file-style reference for the `coqui-access` crate: every public item with its
signature, contract and gotchas, so the crate can be used without reading its source. It
ships inside the package and is self-contained. Development documentation lives with the
source at <https://github.com/Boriken-Dev/coqui-access>.

MSRV Rust 1.89. No Cargo features. A default build depends on `guatiao` with its `provider`
feature only. The crate holds no `unsafe` code. Everything is re-exported at the root, plus
`guatiao`, `error` and `takes`.

## Kinds

All methods are synchronous; a host calls them from a blocking thread. A kind grows by
appending methods with default bodies after the required ones. The hash of a kind covers its
required methods only.

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
```

- `authenticate` returns `Err` only when it could not find out; a host treats that as "could
  not decide", never as a denial. `Verdict::Unknown` means "not mine, ask the next
  authenticator"; `Invalid` means "mine, and refused".
- `lookup` returns `None` for an unknown or disabled account.
- `put` makes an account or changes the one of that name; a field not said is left as it is.
  A provider that makes secrets itself answers with the one it made, once.
- `remove` returns `GUATIAO_ERR_NOT_FOUND` when no account has that name.
- A provider offers the kinds it implements: `#[provider(Authenticator, Directory, Accounts,
  crate = ::coqui_access::guatiao)]`.
- `KIND_PREFIX` is `"coqui_"`: a host scanning libraries keeps those that declare a kind
  beginning with it.

## Models

All models derive Guatiao `ToValue`, `FromValue` and `Schema`. A model grows by `Option`
fields. Unknown keys are skipped when reading.

| Model | Fields (required first) |
| --- | --- |
| `Attempt` | `name?`, `secret?`, `headers: Vec<(String, String)>`, `peer?`, `secure?`, `certificate?`, `certificates?: Vec<Bytes>` (DER, leaf first) |
| `Lookup` | `by`, `value` — `by` is `id`, `name` or `email` (a provider may add more) |
| `Verdict` | enum tagged by `kind`: `Unknown`, `Invalid`, `Valid(Lookup)` |
| `Person` | `id`, `name`, `groups`, `attributes: BTreeMap<String, Value>`, `display_name?`, `email?` |
| `AuthorizationRequest` | `person`, `action`, `resource`, `ancestors: Vec<String>` (the resources `resource` sits under, nearest first; empty by default and then not written) |
| `AuthorizationDecision` | `allowed`, `limits: BTreeMap<String, Value>` |
| `Description` | `name?`, `takes?` (one of `takes::PASSWORD`, `KEY`, `REQUEST`) |
| `AccountInfo` | `id`, `name`, `groups`, `disabled?`, `made_unix?`, `expires_unix?` |
| `AccountPut` | `name`, `secret?`, `groups?`, `disabled?`, `expires_unix?` |
| `PutOutcome` | `secret?` |
| `Waiting` | `shown`, `peer?`, `seen_unix?` |

`headers` are ordered pairs; a header sent twice appears twice; match names case-insensitively.
`Attempt::certificate` is a fingerprint the transport verified the peer holds the key of, in
the canonical text `coqui-peer-tls` writes (`sha256:<hex>`); `Attempt::certificates` is that
certificate and the intermediates the client sent, as DER, leaf first. `Debug` shows only
their count. `Debug` of `Attempt`, `Lookup`,
`Person`, `AccountPut` and `PutOutcome` shows no secret and no identity value.

## Subjects

```rust
#[non_exhaustive]
pub enum Subject { Account(String), Person(String), Group(String) }  // Clone, Eq, Ord, Hash
impl Subject {
    pub fn kind(&self) -> &'static str;          // "account" | "person" | "group"
    pub fn value(&self) -> &str;
    pub fn matches(&self, person: &Person) -> bool;
}
impl FromStr for Subject { type Err = SubjectError; }   // "account:<name>", "person:<id>", "group:<name>"
impl Display for Subject;                               // the same text
pub struct SubjectError;                                // non_exhaustive, Error
```

- `Account` matches `Person::name`, `Person` matches `Person::id`, `Group` any of
  `Person::groups`; all compare exact strings.
- The value after the first colon may hold colons; empty or holding a control character is
  refused. Policies add their own length and character rules.
- `Debug` shows the kind only: the value is an identity.

## Limits

```rust
pub struct Limit;                       // Copy, Eq, Hash, Debug, Display; guatiao derives (transparent)
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
- As configuration a `Limit` is a number or `null` (no bound). A struct field of this type
  with `#[map(default = Limit::at(n))]` reads an absent key as `n` and writes `null` for
  unbounded, so a lifted bound survives a round trip (a bare `Option` field could not).

## Errors (`coqui_access::error`)

`bad_value`, `not_found`, `unsupported`, `internal`: each `fn(&str) -> ProviderError` with the
matching Guatiao status. Messages never hold credentials.

## Gotchas

- Do not rename a model type or a kind trait: the hash is taken over type names as written.
  `tests/contract.rs` pins each hash and each model's required keys; `kinds.json` is the same
  contract as data.
- `Person::id` is what `AccountInfo::id` and a `Lookup { by: "id" }` mean.
