//! [`Environment`]: the variables a harness and what it starts run with, which may be credentials.

use alloc::collections::BTreeMap;
use alloc::collections::btree_map;
use alloc::string::String;
use core::fmt;

use guatiao::{FromValue, Schema, ToValue, Wipe};

/// Environment variables for what a harness runs. They may be credentials: a model crossing the
/// kind holds them as `Secret<Environment>`, whose schema marks them sensitive and whose `Debug`
/// prints `[redacted]`.
///
/// `Debug` here names the variables and never shows a value. Wiping it overwrites every name and
/// value with zeros, as [`guatiao::Secret`] does when it drops one.
#[derive(ToValue, FromValue, Schema, Clone, Default, PartialEq, Eq)]
#[map(crate = ::guatiao, transparent)]
pub struct Environment(BTreeMap<String, String>);

impl Environment {
    /// No variables.
    #[must_use]
    pub const fn new() -> Self {
        Self(BTreeMap::new())
    }

    /// Sets `name` to `value`, answering the value it replaces.
    pub fn insert(&mut self, name: impl Into<String>, value: impl Into<String>) -> Option<String> {
        self.0.insert(name.into(), value.into())
    }

    /// The value of `name`.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&str> {
        self.0.get(name).map(String::as_str)
    }

    /// Every variable, by name.
    pub fn iter(&self) -> btree_map::Iter<'_, String, String> {
        self.0.iter()
    }

    /// The names, without the values.
    pub fn names(&self) -> btree_map::Keys<'_, String, String> {
        self.0.keys()
    }

    /// How many variables there are.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether there are none.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl From<BTreeMap<String, String>> for Environment {
    fn from(variables: BTreeMap<String, String>) -> Self {
        Self(variables)
    }
}

impl<K: Into<String>, V: Into<String>> FromIterator<(K, V)> for Environment {
    fn from_iter<I: IntoIterator<Item = (K, V)>>(iter: I) -> Self {
        Self(
            iter.into_iter()
                .map(|(name, value)| (name.into(), value.into()))
                .collect(),
        )
    }
}

impl<'a> IntoIterator for &'a Environment {
    type Item = (&'a String, &'a String);
    type IntoIter = btree_map::Iter<'a, String, String>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

/// Names only: a value may be a credential.
impl fmt::Debug for Environment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_set().entries(self.0.keys()).finish()
    }
}

impl Wipe for Environment {
    fn wipe(&mut self) {
        for (mut name, mut value) in core::mem::take(&mut self.0) {
            name.wipe();
            value.wipe();
        }
    }
}
