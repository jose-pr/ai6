//! The value types of limits and durations in settings: [`Limit`] (a count or a size that may
//! be unbounded), [`Seconds`] (a duration) and [`TimeLimit`] (a duration that may be
//! unbounded), and [`looser`], which names the settings looser than their defaults.
//!
//! An unbounded value is written `null`, or the string `"none"` in a format with no null
//! (TOML). A duration is a number of seconds, whole or fractional, finite and not negative.

use std::fmt;
use std::time::Duration;

use serde::de::{self, Deserializer, Visitor};
use serde::{Deserialize, Serialize, Serializer};

/// A count or a number of bytes, or no bound at all.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Limit(Option<u64>);

impl Limit {
    /// No bound.
    pub const UNBOUNDED: Self = Self(None);

    /// A bound of `n`.
    pub const fn at(n: u64) -> Self {
        Self(Some(n))
    }

    /// The bound, `None` when there is none.
    pub const fn get(self) -> Option<u64> {
        self.0
    }

    /// Whether `n` is within the bound: at most the bound, or anything when there is none.
    /// A value that does not fit a `u64` is outside every bound.
    pub fn allows<N: TryInto<u64>>(self, n: N) -> bool {
        match (self.0, n.try_into()) {
            (None, _) => true,
            (Some(bound), Ok(n)) => n <= bound,
            (Some(_), Err(_)) => false,
        }
    }

    /// The bound as a `usize`, for slicing and truncating: `usize::MAX` when there is none
    /// or it does not fit.
    pub fn or_max(self) -> usize {
        self.0
            .and_then(|bound| usize::try_from(bound).ok())
            .unwrap_or(usize::MAX)
    }
}

impl fmt::Display for Limit {
    /// The number, or `none`.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            Some(bound) => write!(formatter, "{bound}"),
            None => formatter.write_str("none"),
        }
    }
}

impl Serialize for Limit {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self.0 {
            Some(bound) => serializer.serialize_u64(bound),
            None => serializer.serialize_none(),
        }
    }
}

impl<'de> Deserialize<'de> for Limit {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct LimitVisitor;

        impl<'de> Visitor<'de> for LimitVisitor {
            type Value = Limit;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a whole number, 0 or more, or null or \"none\" for no bound")
            }

            fn visit_u64<E: de::Error>(self, value: u64) -> Result<Limit, E> {
                Ok(Limit::at(value))
            }

            fn visit_i64<E: de::Error>(self, value: i64) -> Result<Limit, E> {
                u64::try_from(value)
                    .map(Limit::at)
                    .map_err(|_| E::custom("a limit must be 0 or more"))
            }

            fn visit_str<E: de::Error>(self, value: &str) -> Result<Limit, E> {
                if value == "none" {
                    Ok(Limit::UNBOUNDED)
                } else {
                    Err(E::invalid_value(de::Unexpected::Str(value), &self))
                }
            }

            fn visit_unit<E: de::Error>(self) -> Result<Limit, E> {
                Ok(Limit::UNBOUNDED)
            }

            fn visit_none<E: de::Error>(self) -> Result<Limit, E> {
                Ok(Limit::UNBOUNDED)
            }

            fn visit_some<D: Deserializer<'de>>(self, inner: D) -> Result<Limit, D::Error> {
                inner.deserialize_any(self)
            }
        }

        deserializer.deserialize_any(LimitVisitor)
    }
}

/// A duration read and written as a number of seconds, whole or fractional, finite and not
/// negative.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Seconds(pub Duration);

impl Seconds {
    /// `seconds` whole seconds.
    pub const fn new(seconds: u64) -> Self {
        Self(Duration::from_secs(seconds))
    }
}

impl Serialize for Seconds {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_f64(self.0.as_secs_f64())
    }
}

impl<'de> Deserialize<'de> for Seconds {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let seconds = f64::deserialize(deserializer)?;
        seconds_from(seconds).map(Self).map_err(de::Error::custom)
    }
}

fn seconds_from(seconds: f64) -> Result<Duration, &'static str> {
    Duration::try_from_secs_f64(seconds).map_err(|_| "seconds must be a finite number, 0 or more")
}

/// A duration that may be unbounded: a number of seconds, or null or `"none"` for none.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TimeLimit(Option<Duration>);

impl TimeLimit {
    /// No bound.
    pub const UNBOUNDED: Self = Self(None);

    /// A bound of `duration`.
    pub const fn at(duration: Duration) -> Self {
        Self(Some(duration))
    }

    /// A bound of `seconds` whole seconds.
    pub const fn secs(seconds: u64) -> Self {
        Self(Some(Duration::from_secs(seconds)))
    }

    /// The bound, `None` when there is none.
    pub const fn get(self) -> Option<Duration> {
        self.0
    }

    /// Whether `duration` is within the bound.
    pub fn allows(self, duration: Duration) -> bool {
        self.0.is_none_or(|bound| duration <= bound)
    }

    /// Whether `seconds`, such as a difference of Unix times, is within the bound.
    pub fn allows_secs(self, seconds: f64) -> bool {
        self.0.is_none_or(|bound| seconds <= bound.as_secs_f64())
    }

    /// The bound in seconds, `f64::INFINITY` when there is none.
    pub fn as_secs_f64(self) -> f64 {
        self.0.map_or(f64::INFINITY, |bound| bound.as_secs_f64())
    }
}

impl fmt::Display for TimeLimit {
    /// The number of seconds, or `none`.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            Some(bound) => write!(formatter, "{} s", bound.as_secs_f64()),
            None => formatter.write_str("none"),
        }
    }
}

impl Serialize for TimeLimit {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self.0 {
            Some(bound) => serializer.serialize_f64(bound.as_secs_f64()),
            None => serializer.serialize_none(),
        }
    }
}

impl<'de> Deserialize<'de> for TimeLimit {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct TimeLimitVisitor;

        impl<'de> Visitor<'de> for TimeLimitVisitor {
            type Value = TimeLimit;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a number of seconds, 0 or more, or null or \"none\"")
            }

            fn visit_u64<E: de::Error>(self, value: u64) -> Result<TimeLimit, E> {
                Ok(TimeLimit::secs(value))
            }

            fn visit_i64<E: de::Error>(self, value: i64) -> Result<TimeLimit, E> {
                self.visit_f64(value as f64)
            }

            fn visit_f64<E: de::Error>(self, value: f64) -> Result<TimeLimit, E> {
                seconds_from(value).map(TimeLimit::at).map_err(E::custom)
            }

            fn visit_str<E: de::Error>(self, value: &str) -> Result<TimeLimit, E> {
                if value == "none" {
                    Ok(TimeLimit::UNBOUNDED)
                } else {
                    Err(E::invalid_value(de::Unexpected::Str(value), &self))
                }
            }

            fn visit_unit<E: de::Error>(self) -> Result<TimeLimit, E> {
                Ok(TimeLimit::UNBOUNDED)
            }

            fn visit_none<E: de::Error>(self) -> Result<TimeLimit, E> {
                Ok(TimeLimit::UNBOUNDED)
            }

            fn visit_some<D: Deserializer<'de>>(self, inner: D) -> Result<TimeLimit, D::Error> {
                inner.deserialize_any(self)
            }
        }

        deserializer.deserialize_any(TimeLimitVisitor)
    }
}

/// One line per setting in `current` looser than in `default`, both the serialized form of
/// the same settings struct: `"PATH VALUE is looser than the default DEFAULT"`, `PATH` being
/// `prefix` and the keys down to the setting, joined by dots.
///
/// A number is looser when it is larger, and no bound (`null`) is looser than any number;
/// for a key named in `smaller_is_looser` (by its own name or its whole path) a smaller number
/// is looser. Values that are not numbers or `null` are not compared.
pub fn looser(
    prefix: &str,
    current: &serde_json::Value,
    default: &serde_json::Value,
    smaller_is_looser: &[&str],
) -> Vec<String> {
    let mut lines = Vec::new();
    walk(prefix, "", current, default, smaller_is_looser, &mut lines);
    lines
}

fn walk(
    path: &str,
    key: &str,
    current: &serde_json::Value,
    default: &serde_json::Value,
    smaller_is_looser: &[&str],
    lines: &mut Vec<String>,
) {
    use serde_json::Value;
    match (current, default) {
        (Value::Object(current), Value::Object(default)) => {
            for (key, value) in current {
                if let Some(default) = default.get(key) {
                    let path = if path.is_empty() {
                        key.clone()
                    } else {
                        format!("{path}.{key}")
                    };
                    walk(&path, key, value, default, smaller_is_looser, lines);
                }
            }
        }
        (Value::Null, Value::Number(_)) => {
            lines.push(format!("{path} none is looser than the default {default}"));
        }
        (Value::Number(value), Value::Number(bound)) => {
            let (Some(value_f), Some(bound_f)) = (value.as_f64(), bound.as_f64()) else {
                return;
            };
            let inverted = smaller_is_looser
                .iter()
                .any(|name| *name == key || *name == path);
            let is_looser = if inverted {
                value_f < bound_f
            } else {
                value_f > bound_f
            };
            if is_looser {
                lines.push(format!("{path} {value} is looser than the default {bound}"));
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, PartialEq, Deserialize, Serialize)]
    struct Holder {
        limit: Limit,
        time: TimeLimit,
    }

    #[test]
    fn a_limit_reads_a_number_null_or_none_in_every_format() {
        let toml: Holder = toml::from_str("limit = 3\ntime = 1.5").unwrap();
        assert_eq!(toml.limit, Limit::at(3));
        assert_eq!(toml.time, TimeLimit::at(Duration::from_millis(1500)));
        let none: Holder = toml::from_str("limit = \"none\"\ntime = \"none\"").unwrap();
        assert_eq!(none.limit, Limit::UNBOUNDED);
        assert_eq!(none.time, TimeLimit::UNBOUNDED);
        let json: Holder = serde_json::from_str(r#"{"limit": null, "time": 2}"#).unwrap();
        assert_eq!(json.limit, Limit::UNBOUNDED);
        assert_eq!(json.time, TimeLimit::secs(2));
        let yaml: Holder = serde_saphyr::from_str("limit: 7\ntime: ~\n").unwrap();
        assert_eq!(yaml.limit, Limit::at(7));
        assert_eq!(yaml.time, TimeLimit::UNBOUNDED);
        for refused in [
            r#"{"limit": -1, "time": 1}"#,
            r#"{"limit": "many", "time": 1}"#,
            r#"{"limit": 1.5, "time": 1}"#,
            r#"{"limit": 1, "time": -1}"#,
        ] {
            assert!(
                serde_json::from_str::<Holder>(refused).is_err(),
                "{refused}"
            );
        }
        assert_eq!(
            serde_json::to_string(&Holder {
                limit: Limit::UNBOUNDED,
                time: TimeLimit::secs(3)
            })
            .unwrap(),
            r#"{"limit":null,"time":3.0}"#
        );
    }

    #[test]
    fn a_limit_allows_up_to_its_bound() {
        assert!(Limit::at(2).allows(2_usize));
        assert!(!Limit::at(2).allows(3_u64));
        assert!(Limit::UNBOUNDED.allows(u64::MAX));
        assert!(!Limit::at(2).allows(-1_i64));
        assert_eq!(Limit::at(5).or_max(), 5);
        assert_eq!(Limit::UNBOUNDED.or_max(), usize::MAX);
        assert!(TimeLimit::secs(1).allows(Duration::from_secs(1)));
        assert!(!TimeLimit::secs(1).allows_secs(1.5));
        assert!(TimeLimit::UNBOUNDED.allows_secs(f64::MAX));
    }

    #[test]
    fn looser_names_each_setting_past_its_default() {
        let default = serde_json::json!({"a": {"count": 5, "min": 4, "off": 1}, "b": 2});
        let current = serde_json::json!({"a": {"count": 6, "min": 3, "off": null}, "b": 1});
        let lines = looser("x", &current, &default, &["min"]);
        assert_eq!(
            lines,
            [
                "x.a.count 6 is looser than the default 5",
                "x.a.min 3 is looser than the default 4",
                "x.a.off none is looser than the default 1",
            ]
        );
        assert!(looser("x", &default, &default, &[]).is_empty());
    }
}
