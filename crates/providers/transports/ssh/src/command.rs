//! The command line the transport sends to the remote shell.
//!
//! SSH carries a single command string, so the argument vector is quoted for a POSIX shell:
//! every element is wrapped in single quotes and nothing is ever re-split by the remote side.

use std::collections::BTreeMap;

/// Quotes `word` so a POSIX shell reads it back as exactly one word.
pub(crate) fn quote(word: &str) -> String {
    let mut quoted = String::with_capacity(word.len() + 2);
    quoted.push('\'');
    for ch in word.chars() {
        if ch == '\'' {
            quoted.push_str("'\\''");
        } else {
            quoted.push(ch);
        }
    }
    quoted.push('\'');
    quoted
}

/// `cd <cwd> && exec env <K=V>... <argv>...`, leaving out what the spec does not set.
pub(crate) fn command_line(
    argv: &[String],
    cwd: Option<&str>,
    env: Option<&BTreeMap<String, String>>,
) -> Result<String, String> {
    if argv.is_empty() {
        return Err("argv is empty".to_owned());
    }
    if argv.iter().any(|word| word.contains('\0')) {
        return Err("argv holds a NUL byte".to_owned());
    }
    let mut line = String::new();
    if let Some(cwd) = cwd {
        line.push_str("cd ");
        line.push_str(&quote(cwd));
        line.push_str(" && ");
    }
    line.push_str("exec ");
    if let Some(env) = env.filter(|env| !env.is_empty()) {
        line.push_str("env");
        for (name, value) in env {
            let valid = !name.is_empty()
                && !name.contains('=')
                && !name.contains('\0')
                && !name.starts_with(|c: char| c.is_ascii_digit())
                && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
            if !valid {
                return Err(format!("{name:?} is not an environment variable name"));
            }
            line.push(' ');
            line.push_str(&quote(&format!("{name}={value}")));
        }
        line.push(' ');
    }
    let words: Vec<String> = argv.iter().map(|word| quote(word)).collect();
    line.push_str(&words.join(" "));
    Ok(line)
}

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
    fn words_survive_the_shell_unchanged() {
        assert_eq!(quote("plain"), "'plain'");
        assert_eq!(quote("it's"), "'it'\\''s'");
        assert_eq!(quote("a b; rm -rf *"), "'a b; rm -rf *'");
        assert_eq!(quote(""), "''");
    }

    #[test]
    fn the_command_line_carries_cwd_and_environment() {
        let argv = ["echo".to_owned(), "hi there".to_owned()];
        assert_eq!(
            command_line(&argv, None, None).unwrap(),
            "exec 'echo' 'hi there'"
        );
        let env = BTreeMap::from([("A".to_owned(), "x y".to_owned())]);
        assert_eq!(
            command_line(&argv, Some("/srv/it's"), Some(&env)).unwrap(),
            "cd '/srv/it'\\''s' && exec env 'A=x y' 'echo' 'hi there'"
        );
    }

    #[test]
    fn invalid_specs_are_refused_before_anything_is_sent() {
        assert!(command_line(&[], None, None).is_err());
        assert!(command_line(&["a\0".to_owned()], None, None).is_err());
        for name in ["", "A=B", "1A", "A-B", "A B"] {
            let env = BTreeMap::from([(name.to_owned(), "v".to_owned())]);
            assert!(command_line(&["x".to_owned()], None, Some(&env)).is_err());
        }
    }

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
