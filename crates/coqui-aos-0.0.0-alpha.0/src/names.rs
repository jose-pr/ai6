//! Overlay identities, relative paths, variable names and operating systems, as AOS defines
//! them (`spec/v0/package.md`, `manifest.md` "Fields", `environment.md`, `runtime.md`).

/// Path components that are never part of an overlay, compared exactly.
pub const EXCLUDED_COMPONENTS: [&str; 10] = [
    ".git",
    ".hg",
    ".svn",
    "__pycache__",
    ".mypy_cache",
    ".pytest_cache",
    ".ruff_cache",
    ".tox",
    ".venv",
    "node_modules",
];

/// Names an overlay identity may not take, whatever their ASCII case.
const RESERVED_NAMES: [&str; 6] = [
    "default",
    "overlay",
    "system",
    "user",
    "project",
    "project-root",
];

/// Whether `name` may be an overlay identity: an ASCII letter, then letters, digits, `_`, `.`
/// or `-`, ending in a letter or digit, and neither a reserved scope label nor a Windows
/// device name before its first `.`.
pub fn valid_overlay_name(name: &str) -> bool {
    if !overlay_name_shaped(name) {
        return false;
    }
    let lower = name.to_ascii_lowercase();
    let stem = lower.split('.').next().unwrap_or_default();
    !RESERVED_NAMES.contains(&lower.as_str()) && !is_device_stem(stem, false)
}

/// Whether `name` has an overlay name's shape, before the reserved names are refused.
pub(crate) fn overlay_name_shaped(name: &str) -> bool {
    let bytes = name.as_bytes();
    bytes.first().is_some_and(u8::is_ascii_alphabetic)
        && bytes.last().is_some_and(u8::is_ascii_alphanumeric)
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-'))
}

/// An identity compared as AOS compares it: ASCII lowercase, `_` and `.` as `-`.
pub fn normalize_name(name: &str) -> String {
    name.chars()
        .map(|c| match c {
            '_' | '.' => '-',
            other => other.to_ascii_lowercase(),
        })
        .collect()
}

/// `<NAME>_OVERLAY_ROOT` for an identity: its normalized form in upper case, every character
/// other than `A`-`Z` and `0`-`9` as `_`.
pub fn root_variable(identity: &str) -> String {
    let mut variable: String = normalize_name(identity)
        .chars()
        .map(|c| {
            let upper = c.to_ascii_uppercase();
            if upper.is_ascii_uppercase() || upper.is_ascii_digit() {
                upper
            } else {
                '_'
            }
        })
        .collect();
    variable.push_str("_OVERLAY_ROOT");
    variable
}

/// `<NAME>_OVERLAY_STATE` for an identity.
pub fn state_variable(identity: &str) -> String {
    let root = root_variable(identity);
    format!("{}STATE", &root[..root.len() - "ROOT".len()])
}

/// Whether `name` is a variable name: `[A-Za-z_][A-Za-z0-9_]*`.
pub fn is_variable_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    bytes
        .first()
        .is_some_and(|byte| byte.is_ascii_alphabetic() || *byte == b'_')
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
}

/// Whether a consumer owns `name`, so no overlay may set it: `AOS_*`, `*_OVERLAY_ROOT` and
/// `*_OVERLAY_STATE`, without regard to ASCII case.
pub fn is_reserved_variable(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    upper.starts_with("AOS_")
        || upper.ends_with("_OVERLAY_ROOT")
        || upper.ends_with("_OVERLAY_STATE")
}

/// Variables at least these names make sensitive (`spec/v0/security.md`).
const SENSITIVE_NAMES: [&str; 45] = [
    "PATH",
    "PATHEXT",
    "HOME",
    "USERPROFILE",
    "SHELL",
    "COMSPEC",
    "SYSTEMROOT",
    "BASH_ENV",
    "ENV",
    "ZDOTDIR",
    "PROMPT_COMMAND",
    "IFS",
    "EDITOR",
    "VISUAL",
    "PAGER",
    "LESSOPEN",
    "LESSCLOSE",
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
    "APPDATA",
    "LOCALAPPDATA",
    "TMPDIR",
    "TEMP",
    "TMP",
    "PSMODULEPATH",
    "CLASSPATH",
    "JAVA_TOOL_OPTIONS",
    "_JAVA_OPTIONS",
    "JDK_JAVA_OPTIONS",
    "PERL5OPT",
    "PERL5LIB",
    "RUBYOPT",
    "RUBYLIB",
    "RUSTFLAGS",
    "GOPROXY",
    "GONOSUMDB",
    "GONOSUMCHECK",
    "GOPRIVATE",
    "GOFLAGS",
    "SSH_ASKPASS",
    "SSLKEYLOGFILE",
    "REQUESTS_CA_BUNDLE",
    "CURL_CA_BUNDLE",
    "CLAUDE_CONFIG_DIR",
    "CODEX_HOME",
];

const SENSITIVE_PREFIXES: [&str; 14] = [
    "LD_",
    "DYLD_",
    "GIT_",
    "PYTHON",
    "NODE_",
    "NPM_CONFIG_",
    "PIP_",
    "UV_",
    "CARGO_",
    "RUSTC_",
    "DOTNET_",
    "COR_",
    "CORECLR_",
    "SSL_",
];

/// Whether setting `name` changes how programs find code, trust certificates or reach the
/// network, so an overlay setting it reports the non-default setting `env-sensitive`.
pub fn is_sensitive_variable(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    SENSITIVE_NAMES.contains(&upper.as_str())
        || SENSITIVE_PREFIXES
            .iter()
            .any(|prefix| upper.starts_with(prefix))
        || upper.ends_with("_PROXY")
        || upper.ends_with("_BASE_URL")
}

/// The list variables path additions exist for: adding to them is no non-default setting.
pub(crate) fn is_path_variable(name: &str) -> bool {
    matches!(
        name.to_ascii_uppercase().as_str(),
        "PATH" | "PYTHONPATH" | "NODE_PATH"
    )
}

/// Characters no component of a relative path holds.
fn refused_in_component(c: char) -> bool {
    matches!(c, '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|')
        || matches!(c, '\u{0}'..='\u{1f}' | '\u{7f}'..='\u{9f}')
        || matches!(
            c,
            '\u{200c}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{206f}' | '\u{feff}'
        )
}

/// `con`, `prn`, `aux`, `nul`, `com`/`lpt` and a digit, and with `console`, `conin$` and
/// `conout$` too; `stem` is in ASCII lowercase.
fn is_device_stem(stem: &str, console: bool) -> bool {
    if matches!(stem, "con" | "prn" | "aux" | "nul") {
        return true;
    }
    if console && matches!(stem, "conin$" | "conout$") {
        return true;
    }
    let digits: &[char] = if console {
        &[
            '0', '1', '2', '3', '4', '5', '6', '7', '8', '9', '\u{b9}', '\u{b2}', '\u{b3}',
        ]
    } else {
        &['1', '2', '3', '4', '5', '6', '7', '8', '9']
    };
    ["com", "lpt"].iter().any(|prefix| {
        stem.strip_prefix(prefix).is_some_and(|rest| {
            let mut chars = rest.chars();
            chars.next().is_some_and(|c| digits.contains(&c)) && chars.next().is_none()
        })
    })
}

/// Whether one component of a relative path is acceptable.
fn valid_component(component: &str) -> bool {
    if component.is_empty()
        || component.ends_with('.')
        || component.ends_with(' ')
        || component.chars().any(refused_in_component)
    {
        return false;
    }
    let lower = component.to_ascii_lowercase();
    if EXCLUDED_COMPONENTS.contains(&lower.as_str()) || lower.ends_with(".pyc") {
        return false;
    }
    let stem = lower.split('.').next().unwrap_or_default();
    !is_device_stem(stem, true)
}

/// Whether `path` is a relative path as the manifest defines one: `/`-separated components
/// that are not empty, do not end in `.` or a space, hold no character Windows refuses or
/// Unicode ignores, are no excluded or device name in any ASCII case and no `.pyc` entry, and
/// no `~` followed by a digit anywhere.
pub fn valid_relative_path(path: &str) -> bool {
    !path.is_empty()
        && !path
            .as_bytes()
            .windows(2)
            .any(|pair| pair[0] == b'~' && pair[1].is_ascii_digit())
        && path.split('/').all(valid_component)
}

/// Whether `name` names an operating system in an `os` list: `[a-z][a-z0-9-]*`.
pub(crate) fn valid_os_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    bytes.first().is_some_and(u8::is_ascii_lowercase)
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-')
}

/// The systems `unix` matches.
const UNIX_NAMES: [&str; 5] = ["macos", "linux", "freebsd", "openbsd", "netbsd"];

/// Whether an `os` list matches `current`; `None` (no list) matches every OS.
pub fn os_matches(names: Option<&[String]>, current: &str) -> bool {
    match names {
        None => true,
        Some(names) => names.iter().any(|name| {
            name == current
                || (name == "unix" && current != "windows" && UNIX_NAMES.contains(&current))
        }),
    }
}

/// The OS name AOS gives the system this program runs on.
pub fn current_os() -> &'static str {
    match std::env::consts::OS {
        "windows" => "windows",
        "macos" => "macos",
        "linux" | "android" => "linux",
        "freebsd" => "freebsd",
        "openbsd" => "openbsd",
        "netbsd" => "netbsd",
        other => other,
    }
}

/// Whether `name` is a skill or tool name: `[A-Za-z0-9][A-Za-z0-9_.-]*`.
pub(crate) fn valid_tool_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    bytes.first().is_some_and(u8::is_ascii_alphanumeric)
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-'))
}
