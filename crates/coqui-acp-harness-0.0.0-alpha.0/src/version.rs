//! A harness's version, against the versions a driver was tested with.

use alloc::format;
use alloc::string::{String, ToString};
use std::collections::HashMap;
use std::ffi::OsStr;
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex, PoisonError};
use std::time::SystemTime;

use crate::process::locate;

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

/// What `<program> --version` said, by scope, the file the program is, and when that file was last changed.
type Probed = Mutex<HashMap<(String, PathBuf, SystemTime), String>>;

static PROBED: LazyLock<Probed> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// `probe`'s answer for `program`, asked once for every version of the file in this process rather than once for each provider
/// instance. The program is the file this process finds under that name (or the path given); a file changed since is asked
/// again, as is any program this process cannot find, and an unanswered probe (`None`) is never kept.
///
/// `scope` is [`WarmPolicy::scope`](crate::WarmPolicy::scope): a program started through a spawner that runs it elsewhere is
/// not the file here of the same name.
pub fn cached(
    scope: &str,
    program: &OsStr,
    probe: impl FnOnce() -> Option<String>,
) -> Option<String> {
    let key = locate(program).and_then(|path| {
        let changed = std::fs::metadata(&path)
            .and_then(|meta| meta.modified())
            .ok()?;
        Some((scope.to_string(), path, changed))
    });
    let Some(key) = key else {
        return probe();
    };
    if let Some(known) = PROBED
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(&key)
    {
        return Some(known.clone());
    }
    let answer = probe()?;
    PROBED
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(key, answer.clone());
    Some(answer)
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
    use core::sync::atomic::{AtomicUsize, Ordering};
    use core::time::Duration;

    #[test]
    fn a_probe_is_asked_once_for_a_version_of_a_file_and_again_when_it_changes() {
        let dir = std::env::temp_dir().join(format!("coqui-version-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let program = dir.join("agent");
        std::fs::write(&program, b"x").unwrap();
        let asked = AtomicUsize::new(0);
        let count = &asked;
        let probe = |answer: Option<&'static str>| {
            move || {
                count.fetch_add(1, Ordering::SeqCst);
                answer.map(str::to_string)
            }
        };
        let name = program.as_os_str();
        assert_eq!(cached("", name, probe(None)), None);
        assert_eq!(cached("", name, probe(Some("1.0"))).as_deref(), Some("1.0"));
        assert_eq!(
            cached("", name, probe(Some("other"))).as_deref(),
            Some("1.0")
        );
        assert_eq!(
            asked.load(Ordering::SeqCst),
            2,
            "an answer is kept, a silence is not"
        );
        assert_eq!(
            cached("far", name, probe(Some("2.0"))).as_deref(),
            Some("2.0"),
            "another scope is another program"
        );
        let file = std::fs::File::options().write(true).open(&program).unwrap();
        file.set_modified(SystemTime::now() + Duration::from_secs(5))
            .unwrap();
        drop(file);
        assert_eq!(cached("", name, probe(Some("1.1"))).as_deref(), Some("1.1"));
        let missing = dir.join("missing");
        assert_eq!(
            cached("", missing.as_os_str(), probe(Some("a"))).as_deref(),
            Some("a")
        );
        assert_eq!(
            cached("", missing.as_os_str(), probe(Some("b"))).as_deref(),
            Some("b"),
            "a program that is not a file is asked every time"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

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
