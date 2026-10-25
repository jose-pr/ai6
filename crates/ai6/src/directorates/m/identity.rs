//! Validated identities crossing the daemon and agent-handler boundary.

use core::borrow::Borrow;
use core::fmt;
use core::str::FromStr;
use serde::{Deserialize, Serialize};

use guatiao::value::convert::{FromValue, MapError, ToValue};
use guatiao::{Alloc, Schema, Value, ValueError};

/// The largest accepted Mission or provider Session identity, in UTF-8 bytes.
pub const MAX_ID_BYTES: usize = 256;

/// A Mission or Session identifier is empty, over limit, or contains a control character.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct InvalidIdentity;

impl fmt::Display for InvalidIdentity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("identity must be non-empty, bounded, and contain no control characters")
    }
}

impl core::error::Error for InvalidIdentity {}

/// AI6's identifier for one unit of work; a Mission may group several Sessions.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Schema, serde::Serialize, serde::Deserialize,
)]
#[map(crate = ::guatiao, transparent)]
#[serde(try_from = "String", into = "String")]
pub struct MissionId(String);

/// A provider-issued identifier for one resumable agent operation.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Schema, serde::Serialize, serde::Deserialize,
)]
#[map(crate = ::guatiao, transparent)]
#[serde(try_from = "String", into = "String")]
pub struct SessionId(String);

/// Shared metadata for one host-issued device token; it never contains the bearer secret.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AccessTokenRecord {
    token_id: String,
    digest: String,
    person_id: String,
    issued_at: f64,
    expires_at: f64,
    revoked_at: Option<f64>,
}

impl AccessTokenRecord {
    /// Validates and stores one access-token record in the Registry format.
    pub fn try_new(
        token_id: impl Into<String>,
        digest: impl Into<String>,
        person_id: impl Into<String>,
        issued_at: f64,
        expires_at: f64,
        revoked_at: Option<f64>,
    ) -> Result<Self, InvalidIdentity> {
        let token_id = token_id.into();
        let digest = digest.into();
        let person_id = person_id.into();
        let canonical_id = uuid::Uuid::parse_str(&token_id).map_err(|_| InvalidIdentity)?;
        if canonical_id.to_string() != token_id
            || digest.len() != 64
            || !digest
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
            || person_id.is_empty()
            || person_id.len() > MAX_ID_BYTES
            || person_id.chars().any(char::is_control)
            || !issued_at.is_finite()
            || !expires_at.is_finite()
            || expires_at <= issued_at
            || revoked_at.is_some_and(|time| !time.is_finite())
        {
            return Err(InvalidIdentity);
        }
        Ok(Self {
            token_id,
            digest,
            person_id,
            issued_at,
            expires_at,
            revoked_at,
        })
    }

    /// Returns the public revocation identifier.
    pub fn token_id(&self) -> &str {
        &self.token_id
    }

    /// Returns the SHA-256 digest used by the shared Registry lookup index.
    pub fn digest(&self) -> &str {
        &self.digest
    }

    /// Returns the authoritative Directory person identifier.
    pub fn person_id(&self) -> &str {
        &self.person_id
    }

    /// Returns the token issue time as Unix seconds.
    pub fn issued_at(&self) -> f64 {
        self.issued_at
    }

    /// Returns the token expiry as Unix seconds.
    pub fn expires_at(&self) -> f64 {
        self.expires_at
    }

    /// Returns the revocation time, if revoked.
    pub fn revoked_at(&self) -> Option<f64> {
        self.revoked_at
    }

    /// Whether this token is valid at one Unix timestamp.
    pub fn active_at(&self, now: f64) -> bool {
        now.is_finite()
            && self.revoked_at.is_none()
            && self.issued_at <= now
            && now < self.expires_at
    }
}

impl fmt::Debug for AccessTokenRecord {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AccessTokenRecord")
            .field("token_id", &self.token_id)
            .field("digest", &"[REDACTED]")
            .field("person_id", &self.person_id)
            .field("issued_at", &self.issued_at)
            .field("expires_at", &self.expires_at)
            .field("revoked_at", &self.revoked_at)
            .finish()
    }
}

macro_rules! identity {
    ($type:ident) => {
        impl $type {
            /// Validates and stores an identifier.
            pub fn try_new(value: impl Into<String>) -> Result<Self, InvalidIdentity> {
                let value = value.into();
                if value.is_empty()
                    || value.len() > MAX_ID_BYTES
                    || value.chars().any(char::is_control)
                {
                    return Err(InvalidIdentity);
                }
                Ok(Self(value))
            }

            /// Returns the identifier text.
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl AsRef<str> for $type {
            fn as_ref(&self) -> &str {
                self.as_str()
            }
        }

        impl Borrow<str> for $type {
            fn borrow(&self) -> &str {
                self.as_str()
            }
        }

        impl fmt::Display for $type {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl FromStr for $type {
            type Err = InvalidIdentity;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                Self::try_new(value)
            }
        }

        impl TryFrom<String> for $type {
            type Error = InvalidIdentity;

            fn try_from(value: String) -> Result<Self, Self::Error> {
                Self::try_new(value)
            }
        }

        impl From<$type> for String {
            fn from(value: $type) -> Self {
                value.0
            }
        }

        impl ToValue for $type {
            fn to_value_in(&self, alloc: Alloc) -> Result<Value, ValueError> {
                self.0.to_value_in(alloc)
            }
        }

        impl FromValue for $type {
            const KINDS: guatiao::value::convert::Kinds = String::KINDS;

            fn from_value(value: &Value) -> Result<Self, MapError> {
                let text = String::from_value(value)?;
                Self::try_new(text).map_err(|_| MapError::bad_value("a valid identity"))
            }
        }
    };
}

identity!(MissionId);
identity!(SessionId);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identities_display_and_parse_without_changing_the_value() {
        let mission = MissionId::try_new("mission-42").unwrap();
        let session = SessionId::try_new("provider-session-9").unwrap();

        assert_eq!(mission.to_string().parse::<MissionId>().unwrap(), mission);
        assert_eq!(session.to_string().parse::<SessionId>().unwrap(), session);
        assert_eq!(mission.as_ref(), "mission-42");
        assert_eq!(session.as_ref(), "provider-session-9");
    }

    #[test]
    fn identity_constructors_reject_empty_unbounded_and_control_values() {
        assert!(MissionId::try_new("").is_err());
        assert!(SessionId::try_new("x".repeat(MAX_ID_BYTES + 1)).is_err());
        assert!(MissionId::try_new("mission\n42").is_err());
    }

    #[test]
    fn access_token_records_validate_times_and_redact_the_digest() {
        let record = AccessTokenRecord::try_new(
            "e747dbba-9a28-4463-a1ab-e31ed79d8991",
            "a".repeat(64),
            "person-007",
            100.0,
            200.0,
            None,
        )
        .unwrap();
        assert!(record.active_at(150.0));
        assert!(!record.active_at(200.0));
        assert!(!format!("{record:?}").contains(&"a".repeat(64)));
        assert!(
            AccessTokenRecord::try_new(
                "e747dbba-9a28-4463-a1ab-e31ed79d8991",
                "A".repeat(64),
                "person-007",
                100.0,
                200.0,
                None,
            )
            .is_err()
        );
        assert!(
            AccessTokenRecord::try_new(
                "e747dbba-9a28-4463-a1ab-e31ed79d8991",
                "a".repeat(64),
                "person-007",
                200.0,
                100.0,
                None,
            )
            .is_err()
        );
    }
}
