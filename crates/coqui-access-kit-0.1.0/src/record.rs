//! Bounds on the fields of a record a provider keeps.

use coqui_access::Limit;
use coqui_access::guatiao::{FromValue, Schema, ToValue};

use crate::table::check_field;

/// How large a record's fields may be, each a [`Limit`] the caller can change or lift.
///
/// By default at most 256 groups, 256 bytes in an id, a name or a group, 320 in an email
/// address and 1 024 in a display name. They bound memory and what a log line can carry, not
/// trust. Every field is also refused when empty or holding a control character
/// ([`check_field`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, FromValue, Schema, ToValue)]
#[map(crate = ::coqui_access::guatiao)]
#[non_exhaustive]
pub struct RecordLimits {
    /// Groups one record holds.
    #[map(default = Limit::at(256))]
    pub groups: Limit,
    /// Bytes in an id, a name or a group.
    #[map(default = Limit::at(256))]
    pub field_bytes: Limit,
    /// Bytes in an email address.
    #[map(default = Limit::at(320))]
    pub email_bytes: Limit,
    /// Bytes in a display name.
    #[map(default = Limit::at(1024))]
    pub display_name_bytes: Limit,
}

impl Default for RecordLimits {
    fn default() -> Self {
        Self::DEFAULT
    }
}

impl RecordLimits {
    /// The defaults: 256 groups, 256-byte fields, 320-byte email addresses, 1 024-byte
    /// display names.
    pub const DEFAULT: Self = Self {
        groups: Limit::at(256),
        field_bytes: Limit::at(256),
        email_bytes: Limit::at(320),
        display_name_bytes: Limit::at(1024),
    };

    /// No bound at all.
    pub const UNBOUNDED: Self = Self {
        groups: Limit::UNBOUNDED,
        field_bytes: Limit::UNBOUNDED,
        email_bytes: Limit::UNBOUNDED,
        display_name_bytes: Limit::UNBOUNDED,
    };

    /// These limits with `groups`.
    #[must_use]
    pub const fn groups(mut self, groups: Limit) -> Self {
        self.groups = groups;
        self
    }

    /// These limits with `field_bytes`.
    #[must_use]
    pub const fn field_bytes(mut self, bytes: Limit) -> Self {
        self.field_bytes = bytes;
        self
    }

    /// These limits with `email_bytes`.
    #[must_use]
    pub const fn email_bytes(mut self, bytes: Limit) -> Self {
        self.email_bytes = bytes;
        self
    }

    /// These limits with `display_name_bytes`.
    #[must_use]
    pub const fn display_name_bytes(mut self, bytes: Limit) -> Self {
        self.display_name_bytes = bytes;
        self
    }

    /// Whether `value` may be an id, a name or a group.
    pub fn allows_field(&self, value: &str) -> bool {
        check_field(value, self.field_bytes.or_max())
    }

    /// Whether a record may hold these groups: not too many, each a field.
    pub fn allows_groups(&self, groups: &[String]) -> bool {
        self.groups.allows(groups.len()) && groups.iter().all(|group| self.allows_field(group))
    }

    /// Whether `value` may be an email address.
    pub fn allows_email(&self, value: &str) -> bool {
        check_field(value, self.email_bytes.or_max())
    }

    /// Whether `value` may be a display name.
    pub fn allows_display_name(&self, value: &str) -> bool {
        check_field(value, self.display_name_bytes.or_max())
    }
}

#[cfg(test)]
mod tests {
    use coqui_access::guatiao::Map;

    use super::*;

    #[test]
    fn record_limits_default_bound_each_field_and_lift_with_unbounded() {
        let limits = RecordLimits::default();
        assert!(limits.allows_field(&"x".repeat(256)) && !limits.allows_field(&"x".repeat(257)));
        assert!(!limits.allows_field("") && !limits.allows_field("a\nb"));
        let many: Vec<String> = (0..257).map(|n| format!("g{n}")).collect();
        assert!(!limits.allows_groups(&many) && limits.allows_groups(&many[..256]));
        assert!(limits.allows_email(&"e".repeat(320)) && !limits.allows_email(&"e".repeat(321)));
        assert!(limits.allows_display_name(&"d".repeat(1024)));
        let lifted = RecordLimits::UNBOUNDED;
        assert!(lifted.allows_groups(&many) && lifted.allows_field(&"x".repeat(100_000)));
        assert!(!lifted.allows_field(""), "an empty field is never one");
        assert!(
            !RecordLimits::DEFAULT
                .groups(Limit::at(1))
                .allows_groups(&many[..2])
        );
    }

    #[test]
    fn record_limits_read_from_configuration_with_defaults() {
        assert_eq!(
            RecordLimits::from_value(&Map::new().into()).unwrap(),
            RecordLimits::DEFAULT
        );
        let mut tuned = Map::new();
        tuned.set("groups", 4u32);
        tuned.set("field_bytes", Limit::UNBOUNDED.to_value().unwrap());
        let read = RecordLimits::from_value(&tuned.into()).unwrap();
        assert_eq!(
            (read.groups, read.field_bytes),
            (Limit::at(4), Limit::UNBOUNDED)
        );
        assert_eq!(read.email_bytes, Limit::at(320));
    }
}
