//! A bound a caller can set or lift.

use core::fmt;

use coqui_builder::Leaf;
use coqui_builder::describe::LeafError;

/// At most a count, or no bound at all.
///
/// Libraries here state their resource bounds (entries, rules, bytes) as `Limit`s with a
/// default the caller can change or lift. As configuration it is a number, or `none` for no
/// bound. A document's `null` is a key that is not given, so it reads the default, not "no
/// bound".
///
/// ```
/// use coqui_access::Limit;
///
/// assert!(Limit::at(2).allows(2) && !Limit::at(2).allows(3));
/// assert!(Limit::UNBOUNDED.allows(usize::MAX));
/// ```
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Limit(Option<u32>);

/// The text of a limit is its count, or `none`; `Display` stays the human wording.
impl Leaf for Limit {
    const TYPE: &'static str = "Limit";

    fn to_text(&self) -> String {
        match self.0 {
            Some(most) => most.to_string(),
            None => "none".to_owned(),
        }
    }

    fn from_text(text: &str) -> Result<Self, LeafError> {
        if text == "none" {
            return Ok(Self::UNBOUNDED);
        }
        text.parse().map(Self::at).map_err(|_| LeafError::Invalid {
            type_name: Self::TYPE,
        })
    }
}

impl Limit {
    /// No bound.
    pub const UNBOUNDED: Self = Self(None);

    /// At most `count`.
    pub const fn at(count: u32) -> Self {
        Self(Some(count))
    }

    /// The count, or `None` for no bound.
    pub const fn get(self) -> Option<u32> {
        self.0
    }

    /// Whether `count` is within the bound.
    pub fn allows(self, count: usize) -> bool {
        self.0
            .is_none_or(|most| usize::try_from(most).is_ok_and(|most| count <= most))
    }

    /// The bound as a `usize`, `usize::MAX` when there is none: for an API that takes a
    /// plain maximum.
    pub fn or_max(self) -> usize {
        self.0.map_or(usize::MAX, |most| {
            usize::try_from(most).unwrap_or(usize::MAX)
        })
    }
}

impl From<u32> for Limit {
    fn from(count: u32) -> Self {
        Self::at(count)
    }
}

impl From<Option<u32>> for Limit {
    fn from(count: Option<u32>) -> Self {
        Self(count)
    }
}

impl fmt::Debug for Limit {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            Some(most) => write!(formatter, "Limit({most})"),
            None => formatter.write_str("Limit(unbounded)"),
        }
    }
}

impl fmt::Display for Limit {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            Some(most) => write!(formatter, "at most {most}"),
            None => formatter.write_str("unbounded"),
        }
    }
}

#[cfg(test)]
mod tests {
    use coqui_builder::{Describe, Partial};

    use super::*;

    #[derive(Debug, PartialEq, Describe)]
    struct Bounded {
        #[map(default = Limit::at(256))]
        entries: Limit,
    }

    fn read(key: &str, text: &str) -> Limit {
        let mut partial = Partial::<Bounded>::new();
        partial.set(key, text).unwrap();
        partial.build().unwrap().entries
    }

    #[test]
    fn a_limit_defaults_reads_none_as_unbounded_and_a_count_as_itself() {
        assert_eq!(
            Partial::<Bounded>::new().build().unwrap().entries,
            Limit::at(256)
        );
        assert_eq!(read("entries", "none"), Limit::UNBOUNDED);
        assert_eq!(read("entries", "3"), Limit::at(3));
        let mut partial = Partial::<Bounded>::new();
        for text in ["null", "unbounded", "-1", "1.5", ""] {
            assert!(partial.set("entries", text).is_err(), "{text}");
        }
        coqui_builder::describe::assert_round_trip(&[
            Limit::UNBOUNDED,
            Limit::at(0),
            Limit::at(u32::MAX),
        ]);
        assert!(Limit::UNBOUNDED.allows(usize::MAX));
        assert!(Limit::at(0).allows(0) && !Limit::at(0).allows(1));
        assert_eq!(Limit::UNBOUNDED.or_max(), usize::MAX);
        assert_eq!(Limit::at(7).or_max(), 7);
        assert_eq!(Limit::from(Some(4)).get(), Some(4));
    }
}
