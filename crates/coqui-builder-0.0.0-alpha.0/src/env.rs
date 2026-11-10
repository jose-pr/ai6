// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! [`Env`]: environment variables as a [`Source`].
//!
//! A key path maps to `PREFIX_A__B`: each segment upper-cased with `-` as `_`, `__` between
//! segments, a list position as digits, and an open map's key as it is found. A variable
//! holds text, so a number, a boolean or a duration is parsed by its leaf. An
//! empty variable is unset.

use alloc::string::String;
use alloc::string::ToString;
use alloc::vec::Vec;

use crate::describe::LeafInfo;
use crate::path::{KeyPath, Segment};
use crate::source::{Entries, Source, SourceError};

/// The variables of one prefix.
#[derive(Clone)]
pub struct Env {
    prefix: String,
    vars: Vec<(String, String)>,
    /// Which variables a leaf was read from; those are never reported as unknown.
    used: Vec<bool>,
}

/// Shows the prefix and how many variables were taken, never their values: they may be secrets.
impl core::fmt::Debug for Env {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Env")
            .field("prefix", &self.prefix)
            .field("variables", &self.vars.len())
            .finish()
    }
}

/// With `zeroize`, the values are wiped when the `Env` drops. The process environment itself
/// is not touched.
#[cfg(feature = "zeroize")]
impl Drop for Env {
    fn drop(&mut self) {
        for (_, value) in &mut self.vars {
            zeroize::Zeroize::zeroize(value);
        }
    }
}

impl Env {
    /// The variables of this process whose name starts with `prefix` and an underscore.
    ///
    /// The environment is read now, once; this crate never sets a variable.
    pub fn new(prefix: &str) -> Env {
        Env::from_vars(prefix, std::env::vars())
    }

    /// The variables of `vars` whose name starts with `prefix` and an underscore. A test
    /// passes its variables here instead of setting any.
    pub fn from_vars<K, V>(prefix: &str, vars: impl IntoIterator<Item = (K, V)>) -> Env
    where
        K: Into<String>,
        V: Into<String>,
    {
        let lead = alloc::format!("{prefix}_");
        let vars: Vec<(String, String)> = vars
            .into_iter()
            .map(|(k, v)| (k.into(), v.into()))
            .filter(|(k, _)| k.starts_with(&lead))
            .collect();
        Env {
            prefix: lead,
            used: alloc::vec![false; vars.len()],
            vars,
        }
    }

    /// The variable that holds the leaf at `at`.
    pub fn name(&self, at: &KeyPath) -> String {
        let mut out = self.prefix.clone();
        out.push_str(&join(at.segments().iter().map(segment_name)));
        out
    }
}

fn segment_name(segment: &Segment) -> String {
    match segment {
        Segment::Index(at) => alloc::format!("{at}"),
        Segment::Key(key) => key
            .chars()
            .map(|c| {
                if c == '-' {
                    '_'
                } else {
                    c.to_ascii_uppercase()
                }
            })
            .collect(),
    }
}

fn join(parts: impl Iterator<Item = String>) -> String {
    let mut out = String::new();
    for (at, part) in parts.enumerate() {
        if at > 0 {
            out.push_str("__");
        }
        out.push_str(&part);
    }
    out
}

impl Source for Env {
    fn value(&mut self, at: &KeyPath, _leaf: &LeafInfo) -> Result<Option<String>, SourceError> {
        let name = self.name(at);
        let Some(found) = self
            .vars
            .iter()
            .position(|(k, _)| k.eq_ignore_ascii_case(&name))
        else {
            return Ok(None);
        };
        self.used[found] = true;
        Ok(Some(self.vars[found].1.clone()).filter(|v| !v.is_empty()))
    }

    /// The segments that follow `at`, as found: an open map's keys, or the positions of a list
    /// written in digits.
    fn entries(&mut self, at: &KeyPath) -> Result<Option<Entries>, SourceError> {
        let lead = alloc::format!("{}__", self.name(at));
        let mut found: Vec<String> = Vec::new();
        for (name, _) in &self.vars {
            if !name
                .get(..lead.len())
                .is_some_and(|head| head.eq_ignore_ascii_case(&lead))
            {
                continue;
            }
            let next = name[lead.len()..].split("__").next().unwrap_or_default();
            if !next.is_empty() && !found.iter().any(|f| f == next) {
                found.push(next.into());
            }
        }
        if found.is_empty() {
            return Ok(None);
        }
        Ok(Some(Entries::Keys(found)))
    }

    /// A variable of the prefix whose path the description does not know, tried as upper-cased
    /// names read in lower case, with `_` as `-`, and as found (an open map's key).
    fn unknown(&mut self, known: &dyn Fn(&KeyPath) -> bool) -> Vec<KeyPath> {
        let mut out = Vec::new();
        for (at, (name, _)) in self.vars.iter().enumerate() {
            if self.used[at] {
                continue;
            }
            let rest = &name[self.prefix.len()..];
            let parts: Vec<&str> = rest.split("__").collect();
            let candidate =
                |map: fn(&str) -> Segment| -> KeyPath { parts.iter().map(|p| map(p)).collect() };
            let lower = |p: &str| segment_of(p, p.to_ascii_lowercase());
            let dashed = |p: &str| segment_of(p, p.to_ascii_lowercase().replace('_', "-"));
            let found = |p: &str| segment_of(p, p.into());
            let paths = [candidate(lower), candidate(dashed), candidate(found)];
            if parts.iter().any(|p| p.is_empty()) || !paths.iter().any(known) {
                out.push(paths[0].clone());
            }
        }
        out
    }
}

fn segment_of(original: &str, text: String) -> Segment {
    match original.parse::<usize>() {
        Ok(at) if at.to_string() == original => Segment::Index(at),
        _ => Segment::Key(text),
    }
}
