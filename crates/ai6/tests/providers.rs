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
        // Family files only: the bootstrap files beside them are not.
        if !ai6::ConfigRecord::ALL
            .iter()
            .any(|record| record.file() == stem)
            || path.extension().is_some_and(|extension| extension == "cfg")
        {
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
    assert!(expected.settings.theaters.find("build-box").is_some());
    assert_eq!(expected.settings.station.max_concurrent_missions, Some(4));
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
    use ai6::{ConfigScope, SqlProvider};

    let file = FileProvider::new(examples());
    let expected = file.load("").unwrap();
    let dir = tempfile::tempdir().unwrap();
    let sql =
        SqlProvider::open(&format!("sqlite:{}", dir.path().join("ai6.db").display())).unwrap();
    // Roles first: an Agent's roles must exist when it is checked.
    let order = ai6::directorates::m::config::IMPORT_ORDER;
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

/// Writes one file of every family into `dir`.
fn write_every_family(dir: &Path) {
    let files = [
        ("operations.toml", "[operation.G]\nroster = [\"007\"]\n"),
        (
            "agents.toml",
            "[role.case-officer]\npurpose = \"Runs a Mission.\"\npolicy = \"Report back.\"\n\
             [agent.007]\nhandler = \"mock-agent\"\nroles = [\"case-officer\"]\n\
             [assistant]\nallow_elevated = true\n",
        ),
        (
            "covers.toml",
            "[cover.house]\ndescription = \"House style.\"\n",
        ),
        (
            "theaters.json",
            r#"{"theater": {"box": {"access": {"ssh": {"host": "b.example"}}}}}"#,
        ),
        (
            "loadouts.yaml",
            "loadout:\n  \"007\":\n    secrets: [forge-token]\nsecret:\n  forge-token:\n    env: FORGE_TOKEN\n",
        ),
        (
            "cables.toml",
            "max_fanout = 40\n[[grants]]\nagent = \"007\"\naction = \"send\"\nscope = \"station\"\nname = \"G\"\n",
        ),
        (
            "plugins.toml",
            &format!(
                "[[library]]\nfile = \"ai6_claude.dll\"\npin = \"sha256:{}\"\n",
                "0".repeat(64)
            ),
        ),
        (
            "station.toml",
            "max_concurrent_missions = 2\nstall_timeout = 120\nlabels = { gpu = \"yes\" }\n",
        ),
        ("tls.toml", "profile = \"intermediate\"\n"),
        (
            "operation_access.toml",
            "[operation_access.G.role.reviewer]
actions = [\"mission.read\", \"approval.decide\"]
\n             [[operation_access.G.rule]]
subject = \"group:qa\"
scope = \"operation:G\"
role = \"reviewer\"
",
        ),
        (
            "access.toml",
            "[role.viewer]\nactions = [\"mission.read\"]\n\
             [[rule]]\nsubject = \"group:ops\"\nscope = \"operation:G\"\nrole = \"viewer\"\n",
        ),
        (
            "unex.toml",
            "directory = \"coqui_access_password\"\ndirectory_config = { path = \"accounts.toml\" }\n\
             authenticator = \"coqui_access_password\"\nassistant = false\n",
        ),
    ];
    for (name, text) in files {
        std::fs::write(dir.join(name), text).unwrap();
    }
}

#[test]
fn every_family_reads_from_a_directory() {
    let dir = tempfile::tempdir().unwrap();
    write_every_family(dir.path());
    let snapshot = FileProvider::new(dir.path()).load("").unwrap();
    let settings = &snapshot.settings;
    assert!(settings.theaters.find("box").is_some());
    assert_eq!(settings.loadouts.secrets().len(), 1);
    assert_eq!(settings.station.max_concurrent_missions, Some(2));
    assert_eq!(settings.pins.as_ref().unwrap().libraries.len(), 1);
    assert!(settings.tls.is_some() && settings.access.is_some());
    assert!(settings.access_policy().is_ok());
    let reviewers = &settings.operation_access["G"];
    assert_eq!(
        reviewers.roles["reviewer"].actions,
        ["mission.read", "approval.decide"]
    );
    assert_eq!(reviewers.rules[0].subject, "group:qa");
    assert!(!settings.unex.assistant);
    assert_eq!(
        settings.unex.directory.as_deref(),
        Some("coqui_access_password")
    );
    assert!(snapshot.roster.assistant().allow_elevated());
}

#[test]
fn a_station_record_of_one_station_leaves_the_others_at_the_global_one() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("station.toml"),
        "max_concurrent_missions = 3\n",
    )
    .unwrap();
    let own = dir.path().join("stations").join("a");
    std::fs::create_dir_all(&own).unwrap();
    std::fs::write(own.join("station.cfg"), "max_concurrent_missions = 7\n").unwrap();
    let provider = FileProvider::new(dir.path());
    let capacity = |station: &str| {
        provider
            .load(station)
            .unwrap()
            .settings
            .station
            .max_concurrent_missions
    };
    assert_eq!(capacity("a"), Some(7));
    assert_eq!(capacity("b"), Some(3));
    std::fs::write(
        dir.path().join("station.toml"),
        "max_concurrent_missions = 0\n",
    )
    .unwrap();
    assert!(provider.load("b").is_err(), "a capacity of 0 is refused");
}

#[cfg(feature = "sqlite")]
#[test]
fn every_family_reads_alike_from_a_directory_and_sqlite() {
    use ai6::directorates::m::config::IMPORT_ORDER;
    use ai6::{ConfigScope, SqlProvider};

    let dir = tempfile::tempdir().unwrap();
    write_every_family(dir.path());
    let file = FileProvider::new(dir.path());
    let db = tempfile::tempdir().unwrap();
    let sql = SqlProvider::open(&format!("sqlite:{}", db.path().join("ai6.db").display())).unwrap();
    let mut records = Vec::new();
    for record in IMPORT_ORDER {
        for (key, definition) in file
            .definitions(record, &ConfigScope::Global)
            .unwrap()
            .records
        {
            records.push((record, key, definition));
        }
    }
    sql.put_all(&ConfigScope::Global, &records).unwrap();
    // Skill paths and the like resolve beside a directory; these records have none.
    assert_eq!(
        format!("{:?}", sql.load("").unwrap().settings),
        format!("{:?}", file.load("").unwrap().settings)
    );
    assert_eq!(
        shape(&sql.load("").unwrap()),
        shape(&file.load("").unwrap())
    );

    let refused = sql
        .put_all(
            &ConfigScope::Global,
            &[(
                ai6::ConfigRecord::Station,
                "settings".to_owned(),
                serde_json::json!({"roles": []}),
            )],
        )
        .unwrap_err();
    assert!(refused.to_string().contains("station.roles"), "{refused}");
}
