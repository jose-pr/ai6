//! Building blocks shared by access providers, independent of any plugin host.
//!
//! | Module | Purpose |
//! | --- | --- |
//! | [`mod@table`] | [`Table`], the typed source a provider reads its records from and writes them to; [`MemoryTable`] built in code; [`Reloading`], a cache that follows a table's changes. |
//! | [`mod@hash`] | Use these with your own data: [`KeyHash`], [`verify_key`], [`PasswordHasher`], [`verify_password`], [`generate_key`]; self-describing `sha256:` and `$argon2id$` strings. |
//! | [`mod@pick`] | [`pick::pick`]: which account a credential's named fields (a certificate's SANs, subject, fingerprint) name. |
//! | [`mod@secret`] | Generate a prefixed 256-bit secret, hash it with optional domain separation, compare in constant time. |
//! | `file` | Feature `file`: atomic private (`0600`) writes, bounded reads, change stamps, and `FileTable`, a table kept in one file in any serde format the caller supplies. |
//!
//! The kit names no format. A table kept in a file takes the caller's serde codec; a SQL
//! database or an HTTP API is a [`Table`] the caller implements. Nothing here reads a file or
//! an environment variable unless it is asked to.
//!
//! Secret helpers report errors as plain `String`s that never contain secrets; tables fail
//! with [`SourceError`].

#![forbid(unsafe_code)]
#![warn(missing_docs)]
#![warn(missing_debug_implementations)]
#![cfg_attr(docsrs, feature(doc_cfg))]

#[cfg(feature = "cache")]
#[cfg_attr(docsrs, doc(cfg(feature = "cache")))]
pub mod cache;
#[cfg(feature = "file")]
#[cfg_attr(docsrs, doc(cfg(feature = "file")))]
pub mod file;
pub mod hash;
pub mod pick;
pub mod record;
pub mod secret;
pub mod table;

#[cfg(feature = "cache")]
pub use cache::{CacheOptions, CacheSetting, Cached};
pub use hash::{
    Argon2Params, DEFAULT_MAX_PASSWORD_BYTES, KeyHash, PasswordHasher, generate_key, is_key_hash,
    is_password_hash, verify_key, verify_password,
};
pub use record::RecordLimits;
pub use table::{
    Keyed, MemoryTable, Reloading, Revision, Snapshot, SourceError, Table, check_field, check_keys,
    unix_now,
};
