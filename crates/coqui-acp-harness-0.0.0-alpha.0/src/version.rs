//! A harness's version, against the versions a driver was tested with.

use alloc::format;
use alloc::string::String;

/// The warning to show for a harness at `found` when a driver was tested with `tested`, or `None` when it was.
/// Nothing is refused: a version not tested is said, never silently taken or rejected.
///
/// A release counts as tested when one tested version has its `major.minor`, since a harness's point releases come
/// weekly and a warning on each would be noise nobody reads. A version that is not dotted numbers is compared whole.
#[must_use]
pub fn untested(harness: &str, found: &str, tested: &[&str]) -> Option<String> {
    let found = found.trim();
    let series = series(found);
    if tested
        .iter()
        .any(|known| *known == found || (series.is_some() && series == self::series(known)))
    {
        return None;
    }
    let list = tested.join(", ");
    Some(if found.is_empty() {
        format!("{harness} did not say its version; this driver was tested with {list}")
    } else {
        format!(
            "{harness} {found} was not tested with this driver, which was tested with {list}; it may not work as described"
        )
    })
}

/// The `major.minor` of a version such as `2.1.318` or `v0.164.0-rc1`, when it starts with two numbers.
fn series(version: &str) -> Option<(u64, u64)> {
    let mut parts = version.trim_start_matches('v').split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?;
    let digits = minor
        .find(|c: char| !c.is_ascii_digit())
        .map_or(minor, |end| &minor[..end]);
    Some((major, digits.parse().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_version_not_tested_is_said_and_a_tested_one_is_not() {
        assert_eq!(untested("Claude Code", "2.1.318", &["2.1.318"]), None);
        let warning = untested("Claude Code", "2.1.400", &["2.1.318"]);
        assert_eq!(warning, None, "a point release is the tested series");
        let warning = untested("Claude Code", "2.2.0", &["2.1.318"]).unwrap();
        assert!(
            warning.contains("2.2.0") && warning.contains("2.1.318"),
            "{warning}"
        );
        assert!(
            untested("Codex", "", &["0.164.0"])
                .unwrap()
                .contains("did not say")
        );
    }

    #[test]
    fn point_releases_of_a_tested_series_are_not_said_and_other_series_are() {
        let tested = ["0.164.0", "0.166.0"];
        for quiet in ["0.164.0", "0.164.9", "v0.166.2", "0.166.0-rc1"] {
            assert_eq!(untested("Codex", quiet, &tested), None, "{quiet}");
        }
        for said in ["0.165.0", "1.164.0", "0.16.4", "nightly", "0"] {
            assert!(untested("Codex", said, &tested).is_some(), "{said}");
        }
        assert_eq!(untested("Tool", "nightly", &["nightly"]), None);
    }
}
