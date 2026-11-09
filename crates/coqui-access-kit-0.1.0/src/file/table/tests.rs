use std::path::Path;

use coqui_access::guatiao::{FromValue, Schema, ToValue};
use serde::de::DeserializeSeed;

use super::*;
use crate::file::Source;

#[derive(Debug, Clone, PartialEq, FromValue, Schema, ToValue)]
#[map(crate = ::coqui_access::guatiao, deny_unknown_fields)]
struct Item {
    name: String,
    note: Option<String>,
    #[map(default, skip_serializing_if = Vec::is_empty)]
    tags: Vec<String>,
    #[map(default)]
    count: i64,
}

impl Keyed for Item {
    fn key(&self) -> std::borrow::Cow<'_, str> {
        std::borrow::Cow::Borrowed(&self.name)
    }
}

fn item(name: &str) -> Item {
    Item {
        name: name.to_owned(),
        note: None,
        tags: Vec::new(),
        count: 0,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, FromValue, Schema, ToValue)]
#[map(crate = ::coqui_access::guatiao, rename_all = "lowercase")]
enum Format {
    Toml,
    Json,
}

impl Codec for Format {
    fn name(&self) -> &'static str {
        match self {
            Self::Toml => "TOML",
            Self::Json => "JSON",
        }
    }

    fn write<D: Serialize + ?Sized>(&self, document: &D) -> Result<String, CodecError> {
        match self {
            Self::Toml => toml::to_string(document).map_err(|_| CodecError::new()),
            Self::Json => serde_json::to_string_pretty(document).map_err(|_| CodecError::new()),
        }
    }

    fn read<'de, S: DeserializeSeed<'de>>(
        &self,
        text: &'de str,
        seed: S,
    ) -> Result<S::Value, CodecError> {
        match self {
            Self::Toml => {
                let deserializer =
                    toml::Deserializer::parse(text).map_err(|_| CodecError::new())?;
                seed.deserialize(deserializer)
                    .map_err(|_| CodecError::new())
            }
            Self::Json => {
                let mut deserializer = serde_json::Deserializer::from_str(text);
                let read = seed
                    .deserialize(&mut deserializer)
                    .map_err(|_| CodecError::new())?;
                deserializer.end().map_err(|_| CodecError::new())?;
                Ok(read)
            }
        }
    }
}

const FORMATS: [Format; 2] = [Format::Toml, Format::Json];

fn table(path: &Path, format: Format) -> FileTable<Item, Format> {
    FileTable::new(path, "items", format)
}

#[test]
fn records_round_trip_through_each_codec() {
    for format in FORMATS {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("items");
        let items = table(&path, format);
        assert!(items.load().unwrap().records.is_empty());
        let full = Item {
            note: Some("a \"quoted\" note".to_owned()),
            tags: vec!["x".to_owned(), "y".to_owned()],
            count: -3,
            ..item("a")
        };
        items.put(full.clone()).unwrap();
        items.put(item("b")).unwrap();
        let other = table(&path, format);
        assert_eq!(
            other.load().unwrap().records,
            [full, item("b")],
            "{format:?}"
        );
    }
}

#[test]
fn each_codec_writes_its_own_format() {
    let directory = tempfile::tempdir().unwrap();
    let toml_path = directory.path().join("items.toml");
    table(&toml_path, Format::Toml).put(item("a")).unwrap();
    let text = std::fs::read_to_string(&toml_path).unwrap();
    assert!(
        text.contains("[[items]]") && text.contains("name = \"a\""),
        "{text}"
    );
    let json_path = directory.path().join("items.json");
    table(&json_path, Format::Json).put(item("a")).unwrap();
    let text = std::fs::read_to_string(&json_path).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(parsed["items"][0]["name"], "a");
}

#[test]
fn a_missing_file_is_empty_and_puts_replace_by_key() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("items.toml");
    let items = table(&path, Format::Toml);
    items.put(item("a")).unwrap();
    items.put(item("b")).unwrap();
    items
        .put(Item {
            note: Some("n".into()),
            ..item("a")
        })
        .unwrap();
    let records = items.load().unwrap().records;
    assert_eq!(records.len(), 2);
    assert_eq!(records[0].note.as_deref(), Some("n"));
    assert!(items.remove("b").unwrap());
    assert!(!items.remove("b").unwrap());
}

#[test]
fn a_change_made_by_another_table_over_the_file_is_a_new_revision() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("items.toml");
    let one = table(&path, Format::Toml);
    let other = table(&path, Format::Toml);
    let first = one.load().unwrap();
    assert_eq!(one.changes_since(first.revision), Ok(None));
    other.put(item("a")).unwrap();
    let second = one.changes_since(first.revision).unwrap().unwrap();
    assert_eq!(second.records, [item("a")]);
    assert_eq!(one.changes_since(second.revision), Ok(None));
}

#[test]
fn invalid_unknown_and_oversized_files_are_errors_that_quote_nothing() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("items");
    for (format, bad, reason) in [
        (
            Format::Toml,
            "surprise = 1",
            "file has an unknown top-level key",
        ),
        (
            Format::Toml,
            "items = [ { name = 'a', unknown = 'secret-x' } ]",
            "a record in the file is invalid",
        ),
        (
            Format::Toml,
            "items = [ { note = 'secret-x' } ]",
            "a record in the file is invalid",
        ),
        (
            Format::Toml,
            "items = [ 'secret-x",
            "file is not a valid TOML table",
        ),
        (
            Format::Toml,
            "items = 'secret-x'",
            "file is not a valid TOML table",
        ),
        (
            Format::Json,
            "{\"items\": [], \"secret-x\": 1}",
            "file has an unknown top-level key",
        ),
        (
            Format::Json,
            "{\"items\": []} \"secret-x\"",
            "file is not a valid JSON table",
        ),
        (
            Format::Json,
            "[\"secret-x\"]",
            "file is not a valid JSON table",
        ),
    ] {
        std::fs::write(&path, bad).unwrap();
        let error = table(&path, format).load().unwrap_err();
        assert_eq!(error, SourceError::invalid(reason), "{bad}");
        assert!(!error.to_string().contains("secret-x"));
        assert!(!format!("{error:?}").contains("secret-x"));
    }
    std::fs::write(&path, vec![b' '; DEFAULT_MAX_BYTES + 1]).unwrap();
    assert!(table(&path, Format::Toml).load().is_err());
    std::fs::write(&path, "items = []").unwrap();
    assert!(
        table(&path, Format::Toml)
            .load()
            .unwrap()
            .records
            .is_empty()
    );
}

#[test]
fn the_size_limit_is_an_option_for_reading_and_writing() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("items.toml");
    let small = table(&path, Format::Toml).max_bytes(40);
    small.put(item("a")).unwrap();
    assert_eq!(
        small.put(item("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb")),
        Err(SourceError::invalid("table would exceed its size limit"))
    );
    table(&path, Format::Toml)
        .put(item("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"))
        .unwrap();
    assert!(matches!(small.load(), Err(SourceError::Unavailable(_))));
}

#[test]
fn a_source_names_one_kind_and_a_sibling_beside_it() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("items.toml");
    let named = Source::file(path.to_str().unwrap(), Format::Toml);
    let opened = named.open::<Item>("items").unwrap();
    opened.put(item("a")).unwrap();
    assert!(path.exists());
    assert!(
        Source::<Format> { file: None }
            .open::<Item>("items")
            .is_err()
    );
    assert!(
        Source::file(" ", Format::Json)
            .open::<Item>("items")
            .is_err()
    );
    let beside = named.sibling("waiting").file.unwrap();
    assert!(beside.path.ends_with("items.toml.waiting"));
    assert_eq!(beside.format, Format::Toml);
}

#[test]
fn a_source_reads_from_configuration_and_needs_a_format() {
    let written = Source::file("items.json", Format::Json).to_value().unwrap();
    assert_eq!(
        Source::<Format>::from_value(&written).unwrap(),
        Source::file("items.json", Format::Json)
    );
    let read = |text: &str| {
        let value =
            guatiao_serde::from_serde(&mut serde_json::Deserializer::from_str(text)).unwrap();
        Source::<Format>::from_value(&value)
    };
    assert!(read(r#"{"file": {"path": "items.json", "format": "json"}}"#).is_ok());
    assert!(read(r#"{"file": {"path": "items.json"}}"#).is_err());
    assert!(read(r#"{"file": {"path": "items.json", "format": "yaml"}}"#).is_err());
}
