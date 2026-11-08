//! What is generated from the declarations and kept in the repository: the reference tables of
//! the narrative docs, between `<!-- schema:SECTION:begin -->` and its end, and Outer Office's
//! copy of `[oo]` ([`OO_DART`]). `cargo run -p ai6 --example settings_reference --features cli`
//! writes them, and `-- --check` fails when one differs.

use std::fmt::Write as _;
use std::path::Path;

use serde_json::Value;

use super::{fields, schema};

/// The docs holding generated tables, from the repository's root.
pub const DOCS: [&str; 6] = [
    "docs/configuration.md",
    "docs/operations.md",
    "docs/assistant.md",
    "docs/compartments.md",
    "docs/review.md",
    "docs/watches.md",
];

/// What Outer Office reads of `[oo]`, from the repository's root: the schema, the form and each
/// default as a constant, since a default is written in a `const` constructor.
pub const OO_DART: &str = "clients/outer-office/lib/settings/oo_settings.g.dart";

const KIB: u64 = 1024;
const MIB: u64 = 1024 * 1024;
const GIB: u64 = 1024 * 1024 * 1024;

/// `1234567` as `1,234,567`.
fn with_commas(digits: &str) -> String {
    let (whole, rest) = digits.split_once('.').unwrap_or((digits, ""));
    let mut out = String::new();
    for (at, digit) in whole.chars().enumerate() {
        if at > 0 && (whole.len() - at).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    if !rest.is_empty() && rest != "0" {
        out.push('.');
        out.push_str(rest);
    }
    out
}

fn bytes(count: u64) -> String {
    for (size, unit) in [(GIB, "GiB"), (MIB, "MiB"), (KIB, "KiB")] {
        if count >= size && count.is_multiple_of(size) {
            return format!("{} {unit}", count / size);
        }
    }
    with_commas(&count.to_string())
}

/// A number of seconds; a whole number of days says so.
fn seconds(count: f64) -> String {
    const DAY: f64 = 86_400.0;
    let number = with_commas(&count.to_string());
    let days = count / DAY;
    if days >= 1.0 && days.fract() == 0.0 {
        let word = if days == 1.0 {
            "a day".to_owned()
        } else {
            format!("{days} days")
        };
        format!("{number} ({word})")
    } else {
        number
    }
}

/// The format a field declares, its own or its kind's.
fn format_of(field: &Value) -> Option<&str> {
    field.get("format").and_then(Value::as_str).or_else(|| {
        field
            .get("anyOf")?
            .as_array()?
            .iter()
            .find_map(|arm| arm.get("format").and_then(Value::as_str))
    })
}

/// A schema's `default` as a reader wants it: `256 KiB`, `1,024`, `none`, `` `word` ``.
fn shown(value: &Value, format: Option<&str>) -> String {
    match value {
        Value::Null => "none".to_owned(),
        Value::Bool(flag) => format!("`{flag}`"),
        Value::Number(number) => match (format, number.as_u64()) {
            (Some("bytes"), Some(count)) => bytes(count),
            (Some("seconds"), _) => seconds(number.as_f64().unwrap_or_default()),
            _ => with_commas(&number.to_string()),
        },
        Value::String(word) if word == "none" => "none".to_owned(),
        Value::String(word) => format!("`{word}`"),
        Value::Array(list) if list.is_empty() => "none".to_owned(),
        Value::Array(list) => list
            .iter()
            .map(|item| shown(item, format))
            .collect::<Vec<_>>()
            .join(", "),
        Value::Object(map) if map.values().all(Value::is_null) => "none".to_owned(),
        Value::Object(_) => format!("`{value}`"),
    }
}

/// An option has no schema default, so its doc comment ends `Default: …`. A short one fills the
/// column and leaves the sentence; a long one stays in the sentence, the column saying where.
fn default_and_meaning(field: &Value, description: &str) -> (String, String) {
    if let Some(default) = field.get("default") {
        return (shown(default, format_of(field)), description.to_owned());
    }
    let Some(at) = description.find("Default: ") else {
        return ("none".to_owned(), description.to_owned());
    };
    let (before, sentence) = description.split_at(at);
    let said = sentence["Default: ".len()..].trim_end_matches('.');
    let (head, tail) = said.split_once("; ").unwrap_or((said, ""));
    if head.len() > 24 {
        return ("see meaning".to_owned(), description.to_owned());
    }
    let mut meaning = before.trim_end().to_owned();
    if !tail.is_empty() {
        let mut letters = tail.chars();
        let first = letters.next().map(|c| c.to_ascii_uppercase());
        meaning.push(' ');
        meaning.extend(first);
        meaning.extend(letters);
        meaning.push('.');
    }
    let default = serde_json::from_str::<Value>(head)
        .ok()
        .filter(|value| value.is_number() || value.is_boolean())
        .map_or_else(|| head.to_owned(), |value| shown(&value, format_of(field)));
    (default, meaning.trim().to_owned())
}

/// The table of a section's own keys; a member that is a table of its own has its own table.
///
/// # Errors
///
/// When `section` has no keys.
pub fn table(section: &str) -> Result<String, String> {
    let node = schema(section).ok_or_else(|| format!("no section {section:?}"))?;
    let members = fields(&node).ok_or_else(|| format!("{section:?} is not a table"))?;
    let mut out = String::from("| Key | Default | Meaning |\n| --- | --- | --- |\n");
    for (key, field) in members {
        if fields(field).is_some() {
            continue;
        }
        let description = field
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        let title = field
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let (default, meaning) = default_and_meaning(field, &description);
        let plain = meaning.trim_end_matches('.');
        let meaning = if plain.eq_ignore_ascii_case(title) || title.is_empty() {
            meaning.clone()
        } else if plain.is_empty() {
            format!("**{title}.**")
        } else {
            format!("**{title}.** {meaning}")
        };
        let meaning = meaning.replace('|', "\\|");
        let _ = writeln!(out, "| `{key}` | {default} | {meaning} |");
    }
    Ok(out)
}

/// `text` with the table between each marker pair written anew.
///
/// # Errors
///
/// A marker with no end, or naming no section.
pub fn rewritten(text: &str) -> Result<String, String> {
    const OPEN: &str = "<!-- schema:";
    const BEGIN: &str = ":begin -->\n";
    let mut out = String::new();
    let mut rest = text;
    while let Some(at) = rest.find(OPEN) {
        let after = &rest[at + OPEN.len()..];
        let Some(name_end) = after.find(BEGIN) else {
            return Err(format!(
                "a schema marker with no begin: {}",
                &after[..after.len().min(40)]
            ));
        };
        let section = &after[..name_end];
        let body = &after[name_end + BEGIN.len()..];
        let end = format!("<!-- schema:{section}:end -->");
        let Some(stop) = body.find(&end) else {
            return Err(format!("{section}: no end marker"));
        };
        out.push_str(&rest[..at]);
        let _ = write!(out, "{OPEN}{section}{BEGIN}{}", table(section)?);
        out.push_str(&end);
        rest = &body[stop + end.len()..];
    }
    out.push_str(rest);
    Ok(out)
}

/// `min_width` in the table `chat` as `chatMinWidth`.
fn dart_name(table: &str, key: &str) -> String {
    let mut name = String::new();
    for (at, word) in table.split('_').chain(key.split('_')).enumerate() {
        if at == 0 {
            name.push_str(word);
        } else {
            let mut letters = word.chars();
            name.extend(letters.next().map(|first| first.to_ascii_uppercase()));
            name.push_str(letters.as_str());
        }
    }
    name
}

/// A schema's default as a Dart constant: a number of seconds is a `Duration`, as the app holds it.
fn dart_constant(field: &Value) -> Option<String> {
    let default = field.get("default")?;
    if format_of(field) == Some("seconds") {
        let micros = (default.as_f64()? * 1_000_000.0).round() as u64;
        return Some(if micros.is_multiple_of(1_000_000) {
            format!("Duration(seconds: {})", micros / 1_000_000)
        } else if micros.is_multiple_of(1_000) {
            format!("Duration(milliseconds: {})", micros / 1_000)
        } else {
            format!("Duration(microseconds: {micros})")
        });
    }
    match default {
        Value::Number(number) if field["type"] == "number" => {
            Some(format!("{:?}", number.as_f64().unwrap_or_default()))
        }
        Value::Number(number) => Some(number.to_string()),
        Value::String(word) => Some(format!("'{word}'")),
        _ => None,
    }
}

/// The app's generated file, from `[oo]`'s declaration.
fn oo_dart() -> Result<String, String> {
    let form =
        crate::directorates::m::records::oo_settings_form().map_err(|error| error.to_string())?;
    let json = serde_json::to_string_pretty(&form).map_err(|error| error.to_string())?;
    if json.contains("'''") {
        return Err("[oo]'s declaration holds three quotes in a row".to_owned());
    }
    let schema = schema("oo").ok_or("no section \"oo\"")?;
    let tables = fields(&schema).ok_or("[oo] is not a table")?;
    let mut out = String::from(
        "// Generated from `OoSettings` in crates/ai6/src/bootstrap/oo.rs; a test there fails when it\n\
         // differs. To write it: cargo run -p ai6 --features cli --example settings_reference\n\n\
         /// `[oo]`'s defaults, by table and key.\n\
         abstract final class OoDefaults {\n",
    );
    for (table, node) in tables {
        for (key, field) in fields(node).ok_or_else(|| format!("[oo.{table}] is not a table"))? {
            if let Some(constant) = dart_constant(field) {
                let _ = writeln!(
                    out,
                    "  static const {} = {constant};",
                    dart_name(table, key)
                );
            }
        }
    }
    let _ = write!(
        out,
        "}}\n\n/// `[oo]`'s schema and form, as `ai6 config schema oo` states them.\n\
         const ooSettingsJson = r'''\n{json}\n''';\n"
    );
    Ok(out)
}

/// One generated file: its path from the repository's root and what it should hold.
fn generated(root: &Path) -> Result<Vec<(String, String)>, String> {
    let mut files = vec![(OO_DART.to_owned(), oo_dart()?)];
    for doc in DOCS {
        let text =
            std::fs::read_to_string(root.join(doc)).map_err(|error| format!("{doc}: {error}"))?;
        files.push((
            doc.to_owned(),
            rewritten(&text).map_err(|why| format!("{doc}: {why}"))?,
        ));
    }
    Ok(files)
}

/// Writes every generated file under `root`, the repository's root.
///
/// # Errors
///
/// When a file cannot be read or written, or a marker names no section.
pub fn write(root: &Path) -> Result<(), String> {
    for (path, content) in generated(root)? {
        std::fs::write(root.join(&path), content).map_err(|error| format!("{path}: {error}"))?;
    }
    Ok(())
}

/// The generated files under `root` that differ from their declarations.
///
/// # Errors
///
/// When a file cannot be read, or a marker names no section.
pub fn stale(root: &Path) -> Result<Vec<String>, String> {
    let mut differing = Vec::new();
    for (path, content) in generated(root)? {
        // A copy of the repository may hold no app.
        let found = std::fs::read_to_string(root.join(&path)).unwrap_or_default();
        if found != content {
            differing.push(path);
        }
    }
    Ok(differing)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repository() -> &'static Path {
        Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
    }

    #[test]
    fn what_is_generated_is_what_the_declarations_say() {
        assert_eq!(stale(repository()), Ok(Vec::<String>::new()));
    }

    /// A copy of the generated files under a new root, with `change` applied to one.
    fn copied(doc: &str, change: impl FnOnce(String) -> String) -> tempfile::TempDir {
        let copy = tempfile::tempdir().unwrap();
        for file in DOCS.into_iter().chain([OO_DART]) {
            let to = copy.path().join(file);
            std::fs::create_dir_all(to.parent().unwrap()).unwrap();
            std::fs::copy(repository().join(file), &to).unwrap();
        }
        let path = copy.path().join(doc);
        let text = std::fs::read_to_string(&path).unwrap();
        std::fs::write(&path, change(text)).unwrap();
        copy
    }

    #[test]
    fn a_changed_default_in_a_copy_is_found_and_written_back() {
        let row = "| `check_in` | 15 |";
        let copy = copied("docs/configuration.md", |text| {
            assert!(text.contains(row), "the row this test changes");
            text.replace(row, "| `check_in` | 16 |")
        });
        assert_eq!(
            stale(copy.path()),
            Ok(vec!["docs/configuration.md".to_owned()])
        );
        write(copy.path()).unwrap();
        assert_eq!(stale(copy.path()), Ok(Vec::<String>::new()));
        let text = std::fs::read_to_string(copy.path().join("docs/configuration.md")).unwrap();
        assert!(text.contains(row));
    }

    #[test]
    fn a_stale_copy_of_oo_is_found_and_says_each_default() {
        let copy = copied(OO_DART, |text| {
            let row = "static const chatWidth = 400.0;";
            assert!(text.contains(row), "the row this test changes");
            text.replace(row, "static const chatWidth = 401.0;")
        });
        assert_eq!(stale(copy.path()), Ok(vec![OO_DART.to_owned()]));
        let dart = oo_dart().unwrap();
        for row in [
            "static const chatMinWidth = 320.0;",
            "static const chatTransitionMs = 200;",
            "static const chatConnectTimeout = Duration(seconds: 10);",
            "static const routesTextBytes = 128;",
            "static const refreshSessions = Duration(seconds: 3);",
            "static const controlsPoll = Duration(milliseconds: 500);",
            "static const pagesSearch = 24;",
            "static const terminalScrollbackLines = 10000;",
            "static const limitsMissionPromptChars = 65536;",
            "static const assetsCdn = 'none';",
        ] {
            assert!(dart.contains(row), "{row}\n{dart}");
        }
    }

    #[test]
    fn a_marker_naming_no_section_is_refused() {
        let copy = copied("docs/operations.md", |text| {
            format!("{text}\n<!-- schema:no.such:begin -->\n<!-- schema:no.such:end -->\n")
        });
        assert!(stale(copy.path()).unwrap_err().contains("no.such"));
        let unended = copied("docs/operations.md", |text| {
            format!("{text}\n<!-- schema:provider:begin -->\n")
        });
        assert!(stale(unended.path()).unwrap_err().contains("no end marker"));
    }

    #[test]
    fn a_table_says_each_default_as_a_reader_wants_it() {
        let shown_in = |section: &str, row: &str| {
            let table = table(section).unwrap();
            assert!(table.contains(row), "{row}\n{table}");
        };
        shown_in("registry.limits.accounts", "| `record_bytes` | 256 KiB |");
        shown_in("registry.limits.accounts", "| `claims` | 1,024 |");
        shown_in("record.station.scheduler", "| `interval` | 0.5 |");
        shown_in(
            "record.station.accounts",
            "| `limit_horizon` | 691,200 (8 days) |",
        );
        shown_in("record.station", "| `stall_timeout` | 300 |");
        shown_in("record.station", "| `max_runtime` | none |");
        shown_in("record.station", "| `history` | 4,096 |");
        shown_in("record.station", "| `normalize_digits` | `false` |");
        shown_in(
            "record.station",
            "| `safe_house_cache_age` | 604,800 (7 days) |",
        );
        shown_in("provider", "| `password` | none |");
        shown_in("record.unex", "| `assistant` | `true` |");
        shown_in("record.unex", "| `assistant_agent` | `moneypenny` |");
        shown_in("record.operation.issues", "| `create` | `propose` |");
        let station = table("record.station").unwrap();
        assert!(
            !station.contains("`scheduler`"),
            "a nested table has its own: {station}"
        );
    }

    #[test]
    fn numbers_read_with_their_units() {
        assert_eq!(with_commas("1"), "1");
        assert_eq!(with_commas("1234"), "1,234");
        assert_eq!(with_commas("1234567.0"), "1,234,567");
        assert_eq!(with_commas("0.25"), "0.25");
        assert_eq!(bytes(16_384), "16 KiB");
        assert_eq!(bytes(2 * GIB), "2 GiB");
        assert_eq!(bytes(1_500), "1,500");
        assert_eq!(bytes(32), "32");
    }
}
