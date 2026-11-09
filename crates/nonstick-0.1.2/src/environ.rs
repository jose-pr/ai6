//! Traits for managing the environment of a PAM handle.
//!
//! PAM modules can set environment variables to apply to a user session.
//! This module manages the interface for doing all of that.

use std::ffi::{OsStr, OsString};

/// A read-only key/value map for environment variables, as [`OsString`]s.
pub trait EnvironMap<'a> {
    /// Gets the environment variable of the given key.
    fn get(&self, key: impl AsRef<OsStr>) -> Option<OsString>;

    /// Iterates over the key/value pairs of the environment.
    fn iter(&self) -> impl Iterator<Item = (OsString, OsString)>;
}

/// A read/write key/value map for environment variables as [`OsString`]s.
///
/// This is a limited subset of what [`HashMap`](std::collections::HashMap)
/// can do. Notably, we do *not* support mutable iteration.
pub trait EnvironMapMut<'a>: EnvironMap<'a> {
    /// Sets the environment variable with the given key,
    /// returning the old one if present.
    fn insert(&mut self, key: impl AsRef<OsStr>, val: impl AsRef<OsStr>) -> Option<OsString>;

    /// Removes the environment variable from the map, returning its old value
    /// if present.
    fn remove(&mut self, key: impl AsRef<OsStr>) -> Option<OsString>;
}
