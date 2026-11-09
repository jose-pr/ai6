# `coqui-access-key` — API header

Header-file-style reference for the `coqui-access-key` crate: every public item with its
signature, contract and gotchas, so the crate can be used without reading its source. It
ships inside the package and is self-contained. Development documentation lives with the
source at <https://github.com/Boriken-Dev/coqui-access>.

MSRV Rust 1.89. No Cargo features. Depends on `coqui-access` and `coqui-access-kit`.
`KeyRecord` derives guatiao's `ToValue`, `FromValue` and `Schema`, so any table (a kit `FileTable` in the format the caller picks,
say) keeps it. No `unsafe` code.
It reads no file and no environment variable. The loadable library is the separate crate
`coqui-access-key-plugin`.

```rust
pub const DEFAULT_PREFIX: &str = "ck_";
pub const DEFAULT_MAX_KEYS: usize = 10_000;
pub struct KeyRecord { pub id, pub name, pub hash: String, pub email: Option<String>,
    pub groups: Vec<String>, pub disabled: bool, pub made_unix: Option<i64>,
    pub expires_unix: Option<i64> }       // non_exhaustive; Keyed by name; Debug redacts
impl KeyRecord {
    pub fn new(id: impl Into<String>, name: impl Into<String>, hash: impl Into<String>) -> Self;
    pub fn groups(self, ..) -> Self; pub fn email(self, ..) -> Self;
    pub fn disabled(self, bool) -> Self; pub fn expires_unix(self, i64) -> Self;
}
pub struct KeysBuilder;                   // Debug
impl KeysBuilder {
    pub fn table(self, table: Arc<dyn Table<KeyRecord>>) -> Self;   // default: MemoryTable
    pub fn prefix(self, prefix: impl Into<String>) -> Self;         // default "ck_"
    pub fn key_hash(self, hash: KeyHash) -> Self;                   // default KeyHash::Sha256
    pub fn max_keys(self, max: usize) -> Self;                      // default 10 000
    pub fn record_limits(self, limits: RecordLimits) -> Self;         // default RecordLimits::DEFAULT
    pub fn build(self) -> Result<Keys, SourceError>;
}
pub struct Keys;
impl Keys { pub fn builder() -> KeysBuilder; pub fn new_key(&self) -> Result<String, String>; }
impl Authenticator for Keys { /* takes::KEY */ }
impl Directory for Keys { /* by "id", "name", "email" */ }
impl Accounts for Keys { /* accounts, put, remove */ }
```

## Behaviour

- `build` refuses an empty or non-printable prefix and checks the table: at most `max_keys`
  records, unique names, ids and hashes, every `hash` a `sha256:<hex>` or `$argon2id$` string
  (`coqui_access_kit::is_key_hash`). A bare hex digest is not accepted.
- `authenticate` looks only at `secret` values that start with the prefix and runs
  `coqui_access_kit::verify_key` against every record, none stopping the others. Unknown key:
  `Unknown`; known but disabled or past `expires_unix`: `Invalid`; valid:
  `Lookup { by: "id", value: id }`. Argon2id records cost one verification each per attempt.
- `lookup` finds taken (enabled, unexpired) records only.
- `put` never accepts a secret: a key is made here. A new account gets a key and a random id;
  an existing account with no `groups`, `disabled` or `expires_unix` said is given another key
  (the old one becomes `Unknown`); otherwise the fields change and the key stays. New keys are
  kept with the builder `key_hash`; existing records keep theirs. The key is answered once in
  `PutOutcome::secret`. A read-only table answers `GUATIAO_ERR_UNSUPPORTED`.

## Errors

A table that cannot be read at call time is `GUATIAO_ERR_INTERNAL`; a refused change is
`GUATIAO_ERR_BAD_VALUE`. Messages never contain keys or hashes.
