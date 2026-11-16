# `coqui-access-totp` — API header

Header-file-style reference for the `coqui-access-totp` crate: every public item with its
signature, contract and gotchas, so the crate can be used without reading its source. It
ships inside the package and is self-contained. Development documentation lives with the
source at <https://github.com/Boriken-Dev/coqui-access>.

One-time codes (TOTP, RFC 6238 over RFC 4226) as a second factor, with enrolment, recovery codes
and reset through the `Accounts` kind. MSRV Rust 1.89. No Cargo features. Depends on
`coqui-access`, `coqui-access-kit`, `coqui-builder` (`Leaf`), `hmac`, `sha1`, `sha2`, `subtle`,
`getrandom`, `data-encoding` and `percent-encoding`. The records derive guatiao's `ToValue`,
`FromValue` and `Schema`, so any table keeps them. No `unsafe` code. It reads no file and no
environment variable. The loadable library is the separate crate `coqui-access-totp-plugin` (id
`coqui_access_totp`).

```rust
pub const DEFAULT_MAX_RECORDS: usize = 10_000;  pub const DEFAULT_DIGITS: u32 = 6;
pub const DEFAULT_PERIOD: u32 = 30;  pub const DEFAULT_SKEW: u32 = 1;
pub const DEFAULT_RECOVERY_CODES: u32 = 10;  pub const DEFAULT_ENROL_SECONDS: u32 = 600;
pub enum Algorithm { Sha1 /* default */, Sha256, Sha512 }   // coqui_builder::Leaf: "sha1" | "sha256" | "sha512"; as_str()
pub struct TotpRecord { pub name, pub secret: String, pub label: Option<String>, pub algorithm: String,
    pub digits: u32, pub period: u32, pub last_step: Option<i64>, pub codes: Vec<String>,
    pub disabled: bool, pub made_unix: Option<i64>, pub failures: u32,
    pub locked_until_unix: Option<i64> }                   // non_exhaustive; Keyed by name; Debug redacts
                                                           // failures/locked_until_unix default, so old files read
pub struct Lockout { pub after: u32, pub first_seconds: u32, pub longest_seconds: u32 }   // non_exhaustive, Copy
impl Lockout { pub const DEFAULT: Self /* 5, 30, 3600 */; pub const OFF: Self; pub const fn new(after, first_seconds, longest_seconds) -> Self; }
pub struct PendingTotp { pub name, pub secret: String, pub label: Option<String>, pub algorithm: String,
    pub digits: u32, pub period: u32, pub codes: Vec<String>, pub created_unix: i64 }   // as above
pub struct TotpBuilder;                                      // Debug
impl TotpBuilder {
    pub fn table(self, Arc<dyn Table<TotpRecord>>) -> Self;     // default MemoryTable
    pub fn pending(self, Arc<dyn Table<PendingTotp>>) -> Self;  // default MemoryTable
    pub fn issuer(self, impl Into<String>) -> Self;             // default none
    pub fn algorithm(self, Algorithm) -> Self;                  // default Sha1
    pub fn digits(self, u32) -> Self;                           // 6..=8, default 6
    pub fn period(self, seconds: u32) -> Self;                  // 1..=3600, default 30
    pub fn skew(self, steps: u32) -> Self;                      // 0..=10, default 1
    pub fn recovery_codes(self, count: u32) -> Self;            // 0..=64, default 10
    pub fn enrol_seconds(self, seconds: u32) -> Self;           // >= 1, default 600
    pub fn max_records(self, usize) -> Self;                    // default 10 000, each table
    pub fn record_limits(self, RecordLimits) -> Self;
    pub fn lockout(self, Lockout) -> Self;                      // default Lockout::DEFAULT; OFF lifts it
    pub fn clock(self, impl Fn() -> i64 + Send + Sync + 'static) -> Self;   // default the system's
    pub fn build(self) -> Result<TotpAccounts, SourceError>;
}
pub struct TotpAccounts;                                     // Debug: settings only
impl TotpAccounts { pub fn builder() -> TotpBuilder; }
impl Authenticator for TotpAccounts { /* takes::OTP */ }
impl Accounts for TotpAccounts { /* accounts, put, remove, waiting */ }
```

## Verifying

- `authenticate` needs `Attempt::name` and `secret`, else `Unknown`. `name` is the account key: the
  `Person::id` an earlier step named. `Attempt::method` is absent or `totp` for a time-based code,
  `recovery_code` for a recovery code; any other method is `Unknown`. An account not enrolled is
  `Unknown`; a disabled account, a wrong code, a code of the wrong length or with non-digits, and a
  step already used are `Invalid`. A success is `Valid(Lookup { by: "id", value: name })`.
- A time-based code is accepted for the present step and `skew` steps either side. The newest
  matching step later than `last_step` wins, and is written through the table **before** the answer;
  a write that fails is an error (`GUATIAO_ERR_INTERNAL`), never a success that could be replayed.
  The code is judged against the record the table read under its lock (`Table::update`), so two
  requests with one code cannot both succeed, in this process or another over the same file (a
  `FileTable`; another kind of table is as atomic as it is).
- **Lockout.** After `after` wrong codes in a row (a wrong recovery code, or a step already used,
  counts) the account answers `Invalid` without checking until `first_seconds` have passed, doubling
  with each further wrong code up to `longest_seconds`; a refusal during the delay is not counted; a
  right code resets it; `put` with `disabled` (either value) resets it. `Lockout::OFF` (`after` 0)
  keeps no count and writes nothing on a wrong code. The delay lets anyone who can name an account keep
  its owner out for up to `longest_seconds` at a time: the consumer's per-address throttle limits that.
- Every step in range is compared in constant time, and every recovery code is compared, none
  stopping the others.
- A recovery code is `XXXXX-XXXXX` (letters and digits, no `I`, `O`, `0`, `1`), compared without the
  hyphen, spaces or case. Only SHA-256 digests (taken under a domain of their own) are kept; a used
  code is removed before the answer.
- The provider does **not** throttle. A code is one in a million, so the consumer's throttle and its
  pending sign-in's try cap bound guessing; the consumer must count a failed code.

## Enrolling (`Accounts::put`)

- No `secret` and no `disabled`: begins an enrolment for `name`, replacing one already waiting. 20
  random bytes, `recovery_codes` recovery codes; answers `PutOutcome { secret: the otpauth link,
  codes }` once. Nothing is a factor yet. The link is
  `otpauth://totp/[ISSUER:]LABEL?secret=BASE32[&issuer=ISSUER]&algorithm=SHA1&digits=6&period=30`, the
  label `AccountPut::display_name` else `name`, percent-encoded.
- `secret` a code from the app: confirms the enrolment waiting, within `enrol_seconds`. The pending
  entry becomes the account, replacing any the account had (rotation: the old secret works until
  this moment). The confirming code is spent. A wrong code is `GUATIAO_ERR_BAD_VALUE` and leaves the
  enrolment; an expired enrolment is `GUATIAO_ERR_BAD_VALUE` and is removed; none waiting is
  `GUATIAO_ERR_NOT_FOUND`.
- `disabled` and no `secret`: disables or enables the enrolled account (`GUATIAO_ERR_NOT_FOUND` if
  none). `groups` and `expires_unix` are refused.
- `remove(name)`: deletes the account and any enrolment waiting (an administrator's reset);
  `GUATIAO_ERR_NOT_FOUND` when neither existed. `accounts()` lists enrolled accounts (`id` = `name`);
  `waiting()` lists unconfirmed enrolments, `shown` being the name.
- Who may call `put` and `remove` (a person under an `enrol` decision, or an administrator) is the
  consumer's.

## Gotchas

- The secret is kept **readable** (a time-based code needs it) in `TotpRecord::secret` and
  `PendingTotp::secret`. A file table written through `coqui-access-kit` is private to its owner (`0600`);
  keep any other table where only the service can read it. Recovery codes are digests only, and a
  50-bit code is guessable offline if the table leaks, so treat the table as holding the factor.
- No secret, code, link or name appears in a `Debug` output or an error message.
- The plugin keeps unconfirmed enrolments in a second file, `<path>.pending`.
