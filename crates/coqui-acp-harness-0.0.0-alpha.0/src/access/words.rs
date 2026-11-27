//! The one table that turns an [`Access`] into each harness's own words.
//!
//! The launch of a translator, the arguments and variables a host gives a program it starts, and the text a host reports
//! about what became of each setting all read [`Access::translate`], so none can say something the others do not.

use alloc::vec::Vec;
use serde_json::{Map, Value};

use super::{Access, Approvals, Sandbox};

/// A harness an [`Access`] is turned into the words of: the AOS id it goes by and the table that says each setting's words.
///
/// A harness's crate defines its own as a constant, `Target::new("id", translate)`, with `translate` a `const fn` of the
/// [`Access`] making a [`Translation`] out of [`Part`]s.
#[derive(Clone, Copy, Debug)]
pub struct Target {
    harness: &'static str,
    table: fn(Access) -> Translation,
}

impl PartialEq for Target {
    fn eq(&self, other: &Self) -> bool {
        self.harness == other.harness
    }
}

impl Eq for Target {}

impl Target {
    /// The target of the AOS harness `harness`, whose words `table` says.
    #[must_use]
    pub const fn new(harness: &'static str, table: fn(Access) -> Translation) -> Self {
        Self { harness, table }
    }

    /// The AOS harness id the target goes by.
    #[must_use]
    pub const fn harness(&self) -> &'static str {
        self.harness
    }

    /// Whether the table is well formed for every access: a part the target cannot express passes nothing, a part it takes
    /// says what became of it, and the JSON a part adds is an object.
    ///
    /// # Errors
    ///
    /// The first access and part found wrong, and how.
    pub fn well_formed(&self) -> Result<(), String> {
        for access in Access::every() {
            for part in access.translate(self).parts() {
                if part.said.is_err()
                    && (part.word.is_some()
                        || !part.args.is_empty()
                        || !part.env.is_empty()
                        || part.json.is_some())
                {
                    return Err(format!(
                        "{} {access:?}: a part it cannot express passes {part:?}",
                        self.harness
                    ));
                }
                if part.said == Ok("") {
                    return Err(format!("{} {access:?}: a part says nothing", self.harness));
                }
                if let Some(json) = part.json
                    && !serde_json::from_str::<Value>(json).is_ok_and(|value| value.is_object())
                {
                    return Err(format!("{} {access:?}: {json} is no object", self.harness));
                }
            }
        }
        Ok(())
    }
}

/// What one setting of an access became for a target.
///
/// A part whose `said` is an error passes nothing: its other fields are empty.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Part {
    /// The target's own word for it, where it has one: a sandbox profile, an approval policy or a permission mode.
    pub word: Option<&'static str>,
    /// Arguments it adds to the program's command line, after the arguments the program is started with.
    pub args: &'static [&'static str],
    /// Variables it sets in the program's environment.
    pub env: &'static [(&'static str, &'static str)],
    /// A JSON object it adds to what the target takes as JSON: a settings argument, a session's `_meta`.
    pub json: Option<&'static str>,
    /// What became of it in words, or why the target cannot express it. Both are one clause, without a full stop.
    pub said: Result<&'static str, &'static str>,
}

impl Part {
    /// A part that adds nothing and says nothing.
    pub const NOTHING: Self = Self {
        word: None,
        args: &[],
        env: &[],
        json: None,
        said: Ok(""),
    };

    /// A part that is the target's own `word` for the setting, said as `said`.
    #[must_use]
    pub const fn word(word: &'static str, said: &'static str) -> Self {
        Self {
            word: Some(word),
            said: Ok(said),
            ..Self::NOTHING
        }
    }

    /// A part that adds nothing and says `said`.
    #[must_use]
    pub const fn said(said: &'static str) -> Self {
        Self {
            said: Ok(said),
            ..Self::NOTHING
        }
    }

    /// A part the target cannot express, for the reason `why`.
    #[must_use]
    pub const fn cannot(why: &'static str) -> Self {
        Self {
            said: Err(why),
            ..Self::NOTHING
        }
    }

    /// A part that adds the JSON object `json`, said as `said`.
    #[must_use]
    pub const fn json(json: &'static str, said: &'static str) -> Self {
        Self {
            json: Some(json),
            said: Ok(said),
            ..Self::NOTHING
        }
    }

    /// A part that adds `args` to the command line, said as `said`.
    #[must_use]
    pub const fn args(args: &'static [&'static str], said: &'static str) -> Self {
        Self {
            args,
            said: Ok(said),
            ..Self::NOTHING
        }
    }

    /// A part that sets `env` in the environment, said as `said`.
    #[must_use]
    pub const fn env(env: &'static [(&'static str, &'static str)], said: &'static str) -> Self {
        Self {
            env,
            said: Ok(said),
            ..Self::NOTHING
        }
    }

    /// Whether the target takes it.
    #[must_use]
    pub const fn applies(&self) -> bool {
        self.said.is_ok()
    }
}

/// An [`Access`] in a target's words, part by part.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Translation {
    /// How much a command may reach.
    pub sandbox: Part,
    /// Whether a command may use the network.
    pub network: Part,
    /// What is asked first.
    pub approvals: Part,
}

impl Translation {
    /// A translation of the three settings.
    #[must_use]
    pub const fn new(sandbox: Part, network: Part, approvals: Part) -> Self {
        Self {
            sandbox,
            network,
            approvals,
        }
    }

    /// The three parts, sandbox, network and approvals.
    #[must_use]
    pub fn parts(&self) -> [&Part; 3] {
        [&self.sandbox, &self.network, &self.approvals]
    }

    /// The target's mode for the access, where it has one: the sandbox's word (`plan` when read-only), else the approvals'.
    #[must_use]
    pub const fn mode(&self) -> Option<&'static str> {
        match self.sandbox.word {
            Some(word) => Some(word),
            None => self.approvals.word,
        }
    }

    /// The arguments to add to the program's command line.
    #[must_use]
    pub fn args(&self) -> Vec<&'static str> {
        self.parts()
            .iter()
            .flat_map(|part| part.args.iter().copied())
            .collect()
    }

    /// The variables to set in the program's environment.
    #[must_use]
    pub fn env(&self) -> Vec<(&'static str, &'static str)> {
        self.parts()
            .iter()
            .flat_map(|part| part.env.iter().copied())
            .collect()
    }

    /// The JSON object the parts add up to, each key set by the last part that names it; none when no part adds one.
    #[must_use]
    pub fn json(&self) -> Option<Map<String, Value>> {
        let mut merged: Option<Map<String, Value>> = None;
        for part in self.parts() {
            let Some(Value::Object(object)) = part
                .json
                .and_then(|text| serde_json::from_str::<Value>(text).ok())
            else {
                continue;
            };
            merged.get_or_insert_with(Map::new).extend(object);
        }
        merged
    }
}

impl Access {
    /// This access in `target`'s words. What a harness cannot express is a part that says why and passes nothing.
    #[must_use]
    pub fn translate(self, target: &Target) -> Translation {
        (target.table)(self)
    }

    /// Every access the three settings can make.
    #[must_use]
    pub fn every() -> Vec<Self> {
        let mut all = Vec::new();
        for sandbox in [Sandbox::Unrestricted, Sandbox::Workspace, Sandbox::ReadOnly] {
            for network in [true, false] {
                for approvals in [Approvals::Ask, Approvals::Edits, Approvals::Never] {
                    let mut access = Self::STANDARD;
                    access.sandbox = sandbox;
                    access.network = network;
                    access.approvals = approvals;
                    all.push(access);
                }
            }
        }
        all
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const fn words(access: Access) -> Translation {
        let sandbox = match access.sandbox {
            Sandbox::ReadOnly => Part::cannot("it has no read-only mode"),
            _ => Part::args(&["--open"], "its --open flag"),
        };
        Translation::new(sandbox, Part::NOTHING, Part::word("ask", "its ask word"))
    }

    const STAND_IN: Target = Target::new("stand-in", words);

    #[test]
    fn a_target_is_its_id_and_its_table() {
        assert_eq!(STAND_IN.harness(), "stand-in");
        let open = Access::STANDARD.translate(&STAND_IN);
        assert_eq!(open.args(), ["--open"]);
        assert_eq!(open.mode(), Some("ask"));
        let mut access = Access::STANDARD;
        access.sandbox = Sandbox::ReadOnly;
        assert!(!access.translate(&STAND_IN).sandbox.applies());
    }

    #[test]
    fn a_table_that_passes_something_it_cannot_express_is_not_well_formed() {
        const fn fine(_: Access) -> Translation {
            Translation::new(Part::said("a"), Part::said("b"), Part::said("c"))
        }
        const fn bad(_: Access) -> Translation {
            let mut cannot = Part::cannot("no");
            cannot.args = &["--x"];
            Translation::new(cannot, Part::said("b"), Part::said("c"))
        }
        const fn silent(_: Access) -> Translation {
            Translation::new(Part::said(""), Part::said("b"), Part::said("c"))
        }
        const fn not_an_object(_: Access) -> Translation {
            Translation::new(
                Part::json("[1]", "a list"),
                Part::said("b"),
                Part::said("c"),
            )
        }
        assert_eq!(Target::new("fine", fine).well_formed(), Ok(()));
        assert!(Target::new("bad", bad).well_formed().is_err());
        assert!(Target::new("silent", silent).well_formed().is_err());
        assert!(Target::new("list", not_an_object).well_formed().is_err());
    }

    #[test]
    fn every_access_is_listed_once() {
        let all = Access::every();
        assert_eq!(all.len(), 18);
        assert_eq!(all[0], {
            let mut first = Access::STANDARD;
            first.sandbox = Sandbox::Unrestricted;
            first
        });
    }
}
