//! A harness's version, against the versions a driver was tested with.

use alloc::format;
use alloc::string::String;

/// The warning to show for a harness at `found` when a driver was tested with `tested`, or `None` when it was.
/// Nothing is refused: a version not tested is said, never silently taken or rejected.
#[must_use]
pub fn untested(harness: &str, found: &str, tested: &[&str]) -> Option<String> {
    let found = found.trim();
    if tested.contains(&found) {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_version_not_tested_is_said_and_a_tested_one_is_not() {
        assert_eq!(untested("Claude Code", "2.1.318", &["2.1.318"]), None);
        let warning = untested("Claude Code", "2.1.400", &["2.1.318"]).unwrap();
        assert!(
            warning.contains("2.1.400") && warning.contains("2.1.318"),
            "{warning}"
        );
        assert!(
            untested("Codex", "", &["0.164.0"])
                .unwrap()
                .contains("did not say")
        );
    }
}
