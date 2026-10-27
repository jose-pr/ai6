//! Whether the Station runs elevated: as root on Unix, or on Windows with an administrator's
//! elevated token (a high or system integrity level).

use std::io;

/// Whether this process runs elevated. A caller deciding whether to run something only an
/// unprivileged process should treat an error as elevated.
///
/// Unix: the effective user is root. Windows: `whoami /groups` (from the system directory,
/// not the `PATH`) lists the High (`S-1-16-12288`) or System (`S-1-16-16384`) mandatory
/// level.
///
/// # Errors
///
/// The check could not be made: `whoami` did not run or answered nothing usable, or the
/// platform has no check.
pub fn is_elevated() -> io::Result<bool> {
    imp::is_elevated()
}

#[cfg(unix)]
mod imp {
    pub(super) fn is_elevated() -> std::io::Result<bool> {
        Ok(rustix::process::geteuid().is_root())
    }
}

#[cfg(windows)]
mod imp {
    use std::io;
    use std::path::PathBuf;
    use std::process::{Command, Stdio};

    /// The mandatory labels of an elevated token: High and System.
    const ELEVATED_LEVELS: [&str; 2] = ["S-1-16-12288", "S-1-16-16384"];

    pub(super) fn is_elevated() -> io::Result<bool> {
        let system = std::env::var_os("SystemRoot")
            .map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from);
        let output = Command::new(system.join("System32").join("whoami.exe"))
            .args(["/groups", "/fo", "csv", "/nh"])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()?;
        if !output.status.success() {
            return Err(io::Error::other("whoami /groups failed"));
        }
        elevated_in(&String::from_utf8_lossy(&output.stdout))
    }

    /// Whether `groups`, `whoami /groups /fo csv /nh` output, holds an elevated level.
    pub(super) fn elevated_in(groups: &str) -> io::Result<bool> {
        if !groups.contains("S-1-16-") {
            return Err(io::Error::other("whoami /groups listed no mandatory level"));
        }
        Ok(ELEVATED_LEVELS
            .iter()
            .any(|level| groups.contains(&format!("\"{level}\""))))
    }
}

#[cfg(not(any(unix, windows)))]
mod imp {
    pub(super) fn is_elevated() -> std::io::Result<bool> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "this platform has no elevation check",
        ))
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_check_answers_on_this_platform() {
        assert!(super::is_elevated().is_ok());
    }

    #[cfg(windows)]
    #[test]
    fn a_high_or_system_level_is_elevated_and_none_is_an_error() {
        use super::imp::elevated_in;
        let medium = "\"Everyone\",\"Well-known group\",\"S-1-1-0\",\"Mandatory group\"\r\n\
                      \"Mandatory Label\\Medium Mandatory Level\",\"Label\",\"S-1-16-8192\",\"\"\r\n";
        assert!(!elevated_in(medium).unwrap());
        let high = medium
            .replace("Medium", "High")
            .replace("S-1-16-8192", "S-1-16-12288");
        assert!(elevated_in(&high).unwrap());
        let system = medium.replace("S-1-16-8192", "S-1-16-16384");
        assert!(elevated_in(&system).unwrap());
        assert!(elevated_in("\"Everyone\",\"Well-known group\",\"S-1-1-0\"").is_err());
        assert!(
            !elevated_in(&medium.replace("S-1-16-8192", "S-1-16-122880")).unwrap(),
            "a longer SID is not the High level"
        );
    }
}
