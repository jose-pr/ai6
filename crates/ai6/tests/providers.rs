//! Configuration providers: the example configuration in every format, and a SQLite copy.

use std::path::{Path, PathBuf};

use ai6::bootstrap::format::{self, Format};
use ai6::{ConfigProvider, ConfigSnapshot, FileProvider};

fn examples() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/config")
}

/// The example family files, each as a JSON value, by stem.
fn example_documents() -> Vec<(String, serde_json::Value)> {
    let mut documents = Vec::new();
    for entry in std::fs::read_dir(examples()).unwrap() {
        let path = entry.unwrap().path();
        let Ok(format) = Format::for_path(&path) else {
            continue;
        };
        let stem = path.file_stem().unwrap().to_string_lossy().into_owned();
        if !["operations", "agents", "covers", "theaters"].contains(&stem.as_str()) {
            continue;
        }
        let text = std::fs::read_to_string(&path).unwrap();
        documents.push((stem, format::from_str(format, &text).unwrap()));
    }
    documents
}

/// What a snapshot says, for comparing two: the roster and the settings as they print.
fn shape(snapshot: &ConfigSnapshot) -> String {
    format!("{:?}\n{:?}", snapshot.roster, snapshot.settings)
}

/// The example configuration rewritten with every family file in `extension`'s format.
fn rewritten(extension: &str, header: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let format = Format::for_extension(extension).unwrap();
    for (stem, document) in example_documents() {
        let text = format::to_string(format, &document).unwrap();
        std::fs::write(
            dir.path().join(format!("{stem}.{extension}")),
            format!("{header}{text}"),
        )
        .unwrap();
    }
    dir
}

#[test]
fn the_example_configuration_reads_alike_in_every_format() {
    let expected = FileProvider::new(examples()).load("").unwrap();
    assert!(expected.roster.operation("G").is_ok(), "the examples load");
    for extension in ["toml", "json", "yaml", "yml", "cfg"] {
        let dir = rewritten(extension, "");
        let read = FileProvider::new(dir.path()).load("").unwrap();
        assert_eq!(shape(&read), shape(&expected), "{extension}");
    }
}

#[test]
#[ignore = "guatiao text::cfg"]
fn a_cfg_family_file_reads_the_format_its_first_line_names() {
    let expected = FileProvider::new(examples()).load("").unwrap();
    let dir = tempfile::tempdir().unwrap();
    for (stem, document) in example_documents() {
        let text = format::to_string(Format::Yaml, &document).unwrap();
        std::fs::write(
            dir.path().join(format!("{stem}.cfg")),
            format!("#!yaml\n{text}"),
        )
        .unwrap();
    }
    let read = FileProvider::new(dir.path()).load("").unwrap();
    assert_eq!(shape(&read), shape(&expected));
}

#[cfg(feature = "sqlite")]
#[test]
fn a_sqlite_provider_holding_the_example_records_reads_alike() {
    use ai6::{ConfigRecord, ConfigScope, SqlProvider};

    let file = FileProvider::new(examples());
    let expected = file.load("").unwrap();
    let dir = tempfile::tempdir().unwrap();
    let sql =
        SqlProvider::open(&format!("sqlite:{}", dir.path().join("ai6.db").display())).unwrap();
    // Roles first: an Agent's roles must exist when it is checked.
    let order = [
        ConfigRecord::Role,
        ConfigRecord::Cover,
        ConfigRecord::Theater,
        ConfigRecord::Assistant,
        ConfigRecord::Agent,
        ConfigRecord::Operation,
    ];
    for record in order {
        let records = file
            .definitions(record, &ConfigScope::Global)
            .unwrap()
            .records;
        for (key, definition) in records {
            sql.put(record, &ConfigScope::Global, &key, &definition)
                .unwrap_or_else(|error| panic!("{record} {key}: {error}"));
        }
    }
    assert_eq!(shape(&sql.load("").unwrap()), shape(&expected));
}
