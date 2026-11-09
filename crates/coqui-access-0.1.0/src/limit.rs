//! A bound a caller can set or lift.

use core::fmt;

use guatiao::{FromValue, Schema, ToValue};

/// At most a count, or no bound at all.
///
/// Libraries here state their resource bounds (entries, rules, bytes) as `Limit`s with a
/// default the caller can change or lift. As configuration it is a number, or `null` for no
/// bound; a struct field of this type with a default keeps `null` when written, so a lifted
/// bound reads back lifted.
///
/// ```
/// use coqui_access::Limit;
///
/// assert!(Limit::at(2).allows(2) && !Limit::at(2).allows(3));
/// assert!(Limit::UNBOUNDED.allows(usize::MAX));
/// ```
#[derive(Clone, Copy, PartialEq, Eq, Hash, FromValue, Schema, ToValue)]
#[map(crate = ::guatiao, transparent)]
pub struct Limit(Option<u32>);

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
    use guatiao::Map;

    use super::*;

    #[derive(Debug, PartialEq, FromValue, Schema, ToValue)]
    #[map(crate = ::guatiao)]
    struct Bounded {
        #[map(default = Limit::at(256))]
        entries: Limit,
    }

    #[test]
    fn a_limit_defaults_reads_null_as_unbounded_and_writes_it_back() {
        assert_eq!(
            Bounded::from_value(&Map::new().into()).unwrap().entries,
            Limit::at(256)
        );
        for entries in [Limit::UNBOUNDED, Limit::at(3)] {
            let written = Bounded { entries }.to_value().unwrap();
            assert_eq!(Bounded::from_value(&written).unwrap().entries, entries);
        }
        assert!(Limit::UNBOUNDED.allows(usize::MAX));
        assert!(Limit::at(0).allows(0) && !Limit::at(0).allows(1));
        assert_eq!(Limit::UNBOUNDED.or_max(), usize::MAX);
        assert_eq!(Limit::at(7).or_max(), 7);
        assert_eq!(Limit::from(Some(4)).get(), Some(4));
    }
}
