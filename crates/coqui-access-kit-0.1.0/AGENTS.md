# `coqui-access-kit` — API header

Header-file-style reference for the `coqui-access-kit` crate: every public item with its
signature, contract and gotchas, so the crate can be used without reading its source. It
ships inside the package and is self-contained. Development documentation lives with the
source at <https://github.com/Boriken-Dev/coqui-access>.

| Feature | Adds | Default |
| --- | --- | --- |
| `file` | module `file`: `FileTable`, `Codec`, `Source`, private writes; `serde` and `guatiao-serde` with no format feature | no |
| `cache` | module `cache`: `Cached`, `CacheOptions`, `CacheSetting`; `moka` (`sync` only) | no |

MSRV Rust 1.89. A default build depends on `coqui-access` (for guatiao), `argon2`, `data-encoding`,
`getrandom`, `sha2` and `subtle`. No feature adds a format crate: a caller supplies its own codec. No
`unsafe` code. Public modules: `table`, `hash`, `secret`, `file` (feature `file`); the main
items of the first three are re-exported at the root. Table errors are `SourceError`; codecs
answer `CodecError`; helpers elsewhere return plain `String`s. None contains file contents or
secrets.

## Secrets (`coqui_access_kit::secret`)

```rust
pub const SECRET_CHARS: usize = 43;
pub fn generate(prefix: &str) -> Result<String, String>;
pub fn identifier() -> Result<String, String>;
pub fn hash(secret: &str, domain: Option<&str>) -> String;
pub fn same(a: &str, b: &str) -> bool;
pub fn is_hash(value: &str) -> bool;
```

- `generate` returns `prefix` plus 32 random bytes as 43 base64url characters. Errors only
  when the OS random source is unavailable.
- `identifier` returns 128 random bits as 32 lowercase hex characters.
- `hash` is SHA-256 as 64 lowercase hex characters. With a `domain` the digest covers the
  domain, a zero byte, then the secret, so a digest kept for one purpose cannot be presented
  for another.
- `same` compares in time independent of where the strings differ. `is_hash` checks the
  64-lowercase-hex shape.

## Picking an account (`coqui_access_kit::pick`)

```rust
pub const IP_FIELD: &str = "san.ip"; pub const DNS_IP_FIELD: &str = "san.dns.ip";
pub struct Pick { pub field: String, pub by: String, pub domain: Option<String>, pub ip_in_dns: bool }
    // non_exhaustive; guatiao derives, ip_in_dns defaulting to true
impl Pick { pub fn new(field, by) -> Self; pub fn domain(self, impl Into<String>) -> Self; pub fn ip_in_dns(self, bool) -> Self; }
pub enum Picked { Absent, Ambiguous, Found { by: String, value: String } }   // non_exhaustive
pub fn pick(fields: &BTreeMap<String, Vec<String>>, order: &[Pick]) -> Picked;
```

- `fields` is a credential's named fields, as coqui-peer-tls's `CertificateFields::to_map`
  writes them. Picks are tried in order: the first finding one distinct value decides
  (`Found { by, value }`, answered as `Lookup { by, value }`); one finding two different values
  is `Ambiguous`; none finding a value is `Absent`.
- `domain` keeps values ending `@<domain>` (ASCII case-insensitive) and yields the part before.
- `san.ip` also reads `san.dns.ip` (a DNS name that is an IP literal) unless `ip_in_dns` is
  false; addresses compare in one spelling (IPv6 compressed, IPv4-mapped as IPv4), so the
  same address in both kinds of name is one value.

## Record limits (`coqui_access_kit::record`; `RecordLimits` re-exported at the root)

```rust
pub struct RecordLimits { pub groups: Limit, pub field_bytes: Limit, pub email_bytes: Limit,
                          pub display_name_bytes: Limit }
    // non_exhaustive, Copy; guatiao derives (each key defaulting); Default = DEFAULT
impl RecordLimits {
    pub const DEFAULT: Self;     // 256 groups, 256-byte id/name/group, 320-byte email, 1 024-byte display name
    pub const UNBOUNDED: Self;
    pub const fn groups(self, Limit) -> Self; pub const fn field_bytes(self, Limit) -> Self;
    pub const fn email_bytes(self, Limit) -> Self; pub const fn display_name_bytes(self, Limit) -> Self;
    pub fn allows_field(&self, &str) -> bool; pub fn allows_groups(&self, &[String]) -> bool;
    pub fn allows_email(&self, &str) -> bool; pub fn allows_display_name(&self, &str) -> bool;
}
```

- Every check is also `check_field`'s: not empty, no control character. The bounds are
  resources, not trust: lifting one is not reported.

## Caches (`coqui_access_kit::cache`, feature `cache`; re-exported at the root)

```rust
pub const DEFAULT_TTL: Duration = 5 s; pub const DEFAULT_NEGATIVE_TTL: Duration = 1 s;
pub const DEFAULT_MAX_ENTRIES: u64 = 10_000;
pub struct CacheOptions { pub ttl: Duration, pub negative_ttl: Duration, pub max_entries: u64 }
    // non_exhaustive, Copy; guatiao derives (durations as seconds, each key defaulting)
impl CacheOptions {
    pub const fn off() -> Self;                         // keeps nothing
    pub const fn ttl(self, Duration) -> Self; pub const fn negative_ttl(self, Duration) -> Self;
    pub const fn max_entries(self, u64) -> Self;
    pub fn exceeds_default(&self) -> bool;              // a ttl above the defaults: report it
}
impl Default for CacheOptions;                          // 5 s, 1 s, 10 000
pub enum CacheSetting { Enabled(bool), Tuned(CacheOptions) }   // untagged: true | false | {..}; Default Enabled(true)
impl CacheSetting { pub fn options(&self) -> CacheOptions; }
pub struct Cached<T: ?Sized>;                           // Debug: options and counts only
impl<T: ?Sized> Cached<T> {
    pub fn new(inner: Arc<T>, options: CacheOptions) -> Self;
    pub fn inner(&self) -> &Arc<T>; pub fn options(&self) -> CacheOptions;
    pub fn invalidate(&self, lookup: &Lookup); pub fn invalidate_all(&self);
}
impl<T: Directory + ?Sized> Directory for Cached<T>;    // keyed by (by, value)
impl<T: Authorizer + ?Sized> Authorizer for Cached<T>;  // keyed by person id, name, sorted groups, action, resource, ancestors
impl<T: Accounts + ?Sized> Accounts for Cached<T>;      // delegates; put/remove clear everything kept
```

- A found person and a grant are kept `ttl`; "not found" (`None`) and a denial
  `negative_ttl`; errors never. At most `max_entries` of each kind. Not an `Authenticator`:
  a credential is always checked at the source.
- Revocation window: after a change at the source, a removed account or a withdrawn
  permission can still pass a cached lookup or decision for up to `ttl`, and a new one can be
  refused for up to `negative_ttl`, unless `invalidate`/`invalidate_all` is called.
- No background thread: moka expires entries as the cache is used.

## Files (`coqui_access_kit::file`, feature `file`)

```rust
pub const DEFAULT_MAX_BYTES: usize = 1024 * 1024;
pub struct CodecError;                  // non_exhaustive, Copy, Error; CodecError::new()
pub trait Codec: Send + Sync {
    fn name(&self) -> &'static str;                                  // "TOML", for errors
    fn presentation(&self) -> guatiao_serde::Presentation;           // default Presentation::new()
    fn write<D: Serialize + ?Sized>(&self, document: &D) -> Result<String, CodecError>;
    fn read<'de, S: DeserializeSeed<'de>>(&self, text: &'de str, seed: S) -> Result<S::Value, CodecError>;
}
pub struct FileTable<T, C>;             // Debug (path, section, format name)
impl<T, C> FileTable<T, C> {
    pub fn new(path: impl Into<PathBuf>, section: impl Into<String>, codec: C) -> Self;
    pub fn max_bytes(self, max: usize) -> Self;                      // default DEFAULT_MAX_BYTES
    pub fn path(&self) -> &Path;
}
impl<T: Keyed + Clone + Send + Walk + FromWalk, C: Codec> Table<T> for FileTable<T, C>;
pub struct Source<F> { pub file: Option<FileSource<F>> }             // guatiao derives
pub struct FileSource<F> { pub path: String, pub format: F }         // guatiao derives; format required
impl<F> Source<F> {
    pub fn file(path: impl Into<String>, format: F) -> Self;
    pub fn sibling(&self, suffix: &str) -> Self where F: Clone;      // "<path>.<suffix>", same format
    pub fn open<T>(&self, section: &str) -> Result<Arc<dyn Table<T>>, SourceError>
        where F: Codec + Clone + 'static, T: Keyed + Clone + Send + Walk + FromWalk + 'static;
}
pub struct Stamp;                       // Debug, Clone, Copy, PartialEq, Eq
pub fn stamp(path: &Path) -> Result<Option<Stamp>, String>;
pub fn read_bounded(path: &Path, max_bytes: usize) -> Result<Option<String>, String>;
pub fn write_private(path: &Path, bytes: &[u8]) -> Result<(), String>;
```

- `stamp` identifies a writing of a file by modification time, length and (Unix) inode;
  `None` when the file is absent.
- `read_bounded` returns `None` for a missing file; errors for unreadable, longer than
  `max_bytes`, or non-UTF-8 content.
- `write_private` writes `bytes` to a fresh file beside `path` (created new, `0600` on Unix),
  syncs, and renames over `path`. The directory must exist.
- `FileTable` writes `{ <section>: [record, ..] }` through `C::write` and reads it back
  through `C::read` with a seed of its own; each record crosses with guatiao-serde's
  `Walked` and `from_deserializer`, so `T` needs `#[derive(ToValue, FromValue)]` (which also
  stream). Every read compares the file's stamp, so another process's write is a new
  revision. A missing file is empty. Over `max_bytes` (read, or about to be written), an
  unknown top-level key, a refused record or a document the codec refuses is a `SourceError`
  in the kit's own words (`file has an unknown top-level key`, `a record in the file is
  invalid`, `file is not a valid <name> table`): a codec's error, which may quote the
  document, is never shown. Atomic within one process; across processes the last writer wins.
- A codec reads with `seed.deserialize(..)` and should refuse trailing text (serde_json's
  `Deserializer::end`). Through a self-describing format's `deserialize_any` a byte string is
  only what the codec's `Presentation` spells; the providers' records hold no bytes.
- `Source::open` is `Invalid` for no kind or a blank path, and reads nothing yet.

## Tables (`coqui_access_kit::table`)

```rust
pub enum SourceError { ReadOnly, Invalid(String), Unavailable(String) }   // non_exhaustive, Error
impl SourceError {
    pub fn invalid(reason: impl Into<String>) -> Self; pub fn unavailable(reason: impl Into<String>) -> Self;
    pub fn reading(&self, table: &str) -> ProviderError;   // always GUATIAO_ERR_INTERNAL, "<table>: <reason>"
    pub fn writing(&self, table: &str) -> ProviderError;   // ReadOnly UNSUPPORTED, Invalid BAD_VALUE, else INTERNAL
}
pub struct Revision;                    // Copy, Eq, Ord, Default; Revision::new(u64), .get()
pub trait Keyed { fn key(&self) -> Cow<'_, str>; }   // borrowed when held as text, owned when written from a value
pub struct Snapshot<T> { pub revision: Revision, pub records: Vec<T> }     // non_exhaustive; Snapshot::new
pub trait Table<T>: Send + Sync {
    fn load(&self) -> Result<Snapshot<T>, SourceError>;
    fn changes_since(&self, since: Revision) -> Result<Option<Snapshot<T>>, SourceError>;  // default: load + compare
    fn put(&self, record: T) -> Result<(), SourceError>;        // default Err(ReadOnly); same key replaces
    fn remove(&self, key: &str) -> Result<bool, SourceError>;   // default Err(ReadOnly); true when it was there
}
pub struct MemoryTable<T>;              // Table<T> for T: Keyed + Clone + Send
impl<T> MemoryTable<T> { pub fn new() -> Self; pub fn from_records(impl IntoIterator<Item = T>) -> Self; }
pub struct Reloading<T>;                // Debug
impl<T: Keyed + Clone> Reloading<T> {
    pub fn new(table: Arc<dyn Table<T>>, validate: impl Fn(&[T]) -> Result<(), SourceError> + Send + Sync + 'static) -> Result<Self, SourceError>;
    pub fn read<R>(&self, read: impl FnOnce(&[T]) -> R) -> Result<R, SourceError>;
    pub fn put_with<R>(&self, make: impl FnOnce(&[T]) -> Result<(T, R), SourceError>) -> Result<R, SourceError>;
    pub fn remove(&self, key: &str) -> Result<bool, SourceError>;
}
pub fn check_keys<T: Keyed>(records: &[T], max: usize) -> Result<(), SourceError>;
pub fn check_field(value: &str, max_bytes: usize) -> bool;   // non-empty, <= max_bytes, no control characters
pub fn unix_now() -> i64;                                    // whole seconds since the epoch; 0 before 1970
```

- A `Table` is any source: memory, a file with a codec, SQL, an HTTP API. The caller
  implements it for the storage it picks; the kit supplies no codec. A table that implements
  only `load` is a read-only source.
- `MemoryTable` revision counts its changes; `from_records` keeps the last of a repeated key.
- `Reloading` calls `changes_since` on every use, so it must be cheap. Every snapshot is
  checked with `validate`; a table that has become unreadable or invalid is an error, never a
  stale answer. `put_with` validates the records as they would be with the new one before
  putting it, so a refused change leaves the table alone. It is atomic within one process only.
- `check_keys` refuses more than `max` records or a repeated key; providers call it first.
- A provider maps a table error with `reading` where it only reads (a table it cannot read
  means "could not find out", never "no") and with `writing` where it puts, removes or opens
  a table from configuration.

## Hashing (`coqui_access_kit::hash`) - use these with your own data

```rust
pub const DEFAULT_MAX_PASSWORD_BYTES: Limit = Limit::at(4096);   // coqui_access::Limit
pub struct Argon2Params;                // Copy, FromValue, ToValue, Schema; Default = 19456 KiB, 2 passes, 1 lane
impl Argon2Params { pub fn new(memory_kib: u32, iterations: u32, parallelism: u32) -> Result<Self, String>;
                    pub fn memory_kib(&self) -> u32; pub fn iterations(&self) -> u32; pub fn parallelism(&self) -> u32; }
pub enum KeyHash { Sha256 /* default */, Argon2id(Argon2Params) }          // non_exhaustive
// FromValue/ToValue/Schema, tagged: { "algorithm": "sha256" } | { "algorithm": "argon2id", <Argon2Params keys> }
impl KeyHash { pub fn argon2id() -> Self; pub fn hash(&self, secret: &str) -> Result<String, String>; }
pub fn verify_key(stored: &str, presented: &[u8]) -> bool;
pub fn is_key_hash(stored: &str) -> bool;
pub fn generate_key(prefix: &str) -> Result<String, String>;
pub struct PasswordHasher;              // Clone; Default = default costs
impl PasswordHasher {
    pub fn new(params: Argon2Params) -> Result<Self, String>;
    pub fn params(&self) -> Argon2Params;
    pub fn max_password_bytes(self, most: Limit) -> Self;            // default DEFAULT_MAX_PASSWORD_BYTES
    pub fn hash(&self, password: &str) -> Result<String, String>;
    pub fn verify(&self, stored: Option<&str>, presented: &str) -> bool;
}
pub fn verify_password(stored: &str, presented: &str) -> bool;
pub fn is_password_hash(stored: &str) -> bool;
```

- Stored strings name their algorithm: `sha256:<64 lowercase hex>` or `$argon2id$v=19$...`
  (PHC). A table may mix both. A bare digest or any other prefix never verifies.
- `KeyHash::hash` is `Err` only for Argon2id (RNG or parameters). `verify_key` branches only on
  the stored prefix; SHA-256 digests are compared in constant time and Argon2id runs in full.
  Argon2id keys cost one verification per Argon2id record they are tried against.
- `Argon2Params::new` rejects what Argon2 refuses (memory under 8 KiB per lane, zero counts).
  Stored hashes verify with the costs they carry.
- As configuration `Argon2Params` is `{ memory_kib, iterations, parallelism }`, every key
  optional with the default above (the schema says so). Like every configuration a host
  validates against its schema first, reading ignores a key it does not know.
  `FromValue` checks the combination: one Argon2 refuses is `MapError::BadValue` with
  `argon2 parameters are out of range`, the words `new` returns.
- `PasswordHasher::new` makes a dummy hash with the same costs; `verify(None, ..)` does the
  same work and answers `false`, so an unknown name costs what a wrong password does.
  `hash` rejects empty and over-long passwords; `verify_password` ignores them.

