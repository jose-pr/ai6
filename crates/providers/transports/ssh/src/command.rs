//! What the transport reads out of a host's answers. The command line it sends is the kit's
//! `command_line`: every word single-quoted for the remote POSIX shell.

/// `(os, arch)` from the output of `uname -sm`, named as Rust names them.
pub(crate) fn parse_uname(output: &str) -> (Option<String>, Option<String>) {
    let mut words = output.split_whitespace();
    let os = words
        .next()
        .map(|name| match name.to_ascii_lowercase().as_str() {
            "darwin" => "macos".to_owned(),
            other => other.to_owned(),
        });
    let arch = words.next().map(|name| match name {
        "arm64" | "aarch64" => "aarch64".to_owned(),
        "amd64" | "x86_64" => "x86_64".to_owned(),
        "i386" | "i686" | "i586" => "x86".to_owned(),
        other => other.to_owned(),
    });
    (os, arch)
}

/// The signal number for an SSH signal name; `None` for one without a stable number.
pub(crate) fn signal_number(name: &str) -> Option<i64> {
    Some(match name {
        "HUP" => 1,
        "INT" => 2,
        "QUIT" => 3,
        "ILL" => 4,
        "ABRT" => 6,
        "FPE" => 8,
        "KILL" => 9,
        "USR1" => 10,
        "SEGV" => 11,
        "PIPE" => 13,
        "ALRM" => 14,
        "TERM" => 15,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uname_output_is_named_as_rust_names_it() {
        assert_eq!(
            parse_uname("Linux aarch64\n"),
            (Some("linux".to_owned()), Some("aarch64".to_owned()))
        );
        assert_eq!(
            parse_uname("Darwin arm64\n"),
            (Some("macos".to_owned()), Some("aarch64".to_owned()))
        );
        assert_eq!(
            parse_uname("FreeBSD amd64"),
            (Some("freebsd".to_owned()), Some("x86_64".to_owned()))
        );
        assert_eq!(parse_uname(""), (None, None));
    }
}
