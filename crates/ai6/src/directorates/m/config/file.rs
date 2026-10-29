//! A directory of family files: [`FileProvider`].

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use super::{
    ConfigDefinitions, ConfigProvider, ConfigRecord, ConfigRevision, ConfigScope, ConfigSnapshot,
    Layer, assemble, check_placement, merged,
};
use crate::bootstrap::format::{self, EXTENSIONS, Format};
use crate::directorates::m::roster::{RosterError, RosterErrorKind, error, error_of};

/// The directory of a Station's own family files under the provider's root.
const STATIONS_DIR: &str = "stations";

/// Configuration in a directory, one file per family ([`ConfigRecord::file`]), each with the
/// extension `cfg`, `toml`, `json`, `yaml` or `yml` ([`format`](crate::bootstrap::format)),
/// except `unex.cfg`, which is Universal Exports' bootstrap file. A Station's own records
/// are in `stations/<id>/` beside them, for the families that take them. A missing file
/// holds no records; two files for one family make the configuration invalid.
///
/// Its revision is a digest of every family file of the view, global and the Station's own,
/// with their relative paths, so asking for changes reads them but parses nothing until one
/// differs. The files are edited by hand or through version control, so the provider takes no
/// [`put`](ConfigProvider::put).
#[derive(Debug, Clone)]
pub struct FileProvider {
    root: PathBuf,
}

/// The family files of one directory: each family file's stem, the path found, if any.
type Found = BTreeMap<&'static str, PathBuf>;

impl FileProvider {
    /// The configuration in `root`. Nothing is read until the provider is asked.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// The configuration directory.
    pub fn root(&self) -> &Path {
        &self.root
    }

    fn scope_dir(&self, scope: &ConfigScope) -> Result<PathBuf, RosterError> {
        match scope {
            ConfigScope::Global => Ok(self.root.clone()),
            ConfigScope::Station(id) if ConfigScope::is_plain(id) => {
                Ok(self.root.join(STATIONS_DIR).join(id))
            }
            ConfigScope::Station(id) => Err(error(format!(
                "station:{id} cannot name a directory of family files"
            ))),
        }
    }

    /// The family files present in `dir`.
    fn found(dir: &Path) -> Result<Found, RosterError> {
        let mut found = Found::new();
        let stems: std::collections::BTreeSet<&'static str> = ConfigRecord::ALL
            .into_iter()
            .map(ConfigRecord::file)
            .collect();
        for stem in stems {
            let mut present = Vec::new();
            for extension in EXTENSIONS {
                let name = format!("{stem}.{extension}");
                // A bootstrap file beside the family files is not one of them.
                if name == crate::bootstrap::FILE_NAME || name == crate::bootstrap::UNEX_FILE_NAME {
                    continue;
                }
                let path = dir.join(name);
                match fs::metadata(&path) {
                    Ok(metadata) if metadata.is_file() => present.push(path),
                    Ok(_) => {}
                    Err(failure) if failure.kind() == io::ErrorKind::NotFound => {}
                    Err(failure) => return Err(unavailable(&path, &failure)),
                }
            }
            match present.len() {
                0 => {}
                1 => {
                    found.insert(stem, present.remove(0));
                }
                _ => {
                    let names: Vec<String> = present
                        .iter()
                        .map(|path| path.display().to_string())
                        .collect();
                    return Err(error(format!(
                        "{} are the same family; keep one of them",
                        names.join(" and ")
                    )));
                }
            }
        }
        Ok(found)
    }

    /// The scopes a view reads: global, and `station`'s own when it names one.
    fn view_scopes(station: &str) -> Vec<ConfigScope> {
        let mut scopes = vec![ConfigScope::Global];
        if !station.is_empty() {
            scopes.push(ConfigScope::of_station(station));
        }
        scopes
    }

    /// The digest of every family file of `station`'s view.
    fn digest(&self, station: &str) -> Result<ConfigRevision, RosterError> {
        let mut digest = Sha256::new();
        for scope in Self::view_scopes(station) {
            for path in Self::found(&self.scope_dir(&scope)?)?.values() {
                let bytes = fs::read(path).map_err(|failure| unavailable(path, &failure))?;
                let relative = path.strip_prefix(&self.root).unwrap_or(path);
                let name = relative.to_string_lossy();
                digest.update((name.len() as u64).to_le_bytes());
                digest.update(name.as_bytes());
                digest.update((bytes.len() as u64).to_le_bytes());
                digest.update(&bytes);
            }
        }
        let digest = digest.finalize();
        let mut number = [0; 8];
        number.copy_from_slice(&digest[..8]);
        Ok(ConfigRevision::new(u64::from_le_bytes(number)))
    }

    /// The records of every family file at `scope`.
    fn layer(&self, scope: &ConfigScope) -> Result<Layer, RosterError> {
        let mut layer = Layer::new();
        for (stem, path) in Self::found(&self.scope_dir(scope)?)? {
            read_family(stem, &path, scope, &mut layer)?;
        }
        Ok(layer)
    }
}

fn unavailable(path: &Path, failure: &io::Error) -> RosterError {
    error_of(
        RosterErrorKind::Unavailable,
        format!("{}: {failure}", path.display()),
    )
}

/// Reads the family file `path` (stem `stem`) into `layer`. A whole-file family's record is
/// the document; `plugins` is `[[library]]` entries keyed by their `file`; otherwise each
/// top-level table names a family the file holds: a table family's entries are its records,
/// a singleton's table is its one record.
fn read_family(
    stem: &str,
    path: &Path,
    scope: &ConfigScope,
    layer: &mut Layer,
) -> Result<(), RosterError> {
    let in_file = |reason: &dyn std::fmt::Display| error(format!("{}: {reason}", path.display()));
    let format = Format::for_path(path).map_err(|reason| in_file(&reason))?;
    let text = fs::read_to_string(path).map_err(|failure| unavailable(path, &failure))?;
    let document: serde_json::Value =
        format::from_str(format, &text).map_err(|reason| in_file(&reason))?;
    let document = match document {
        serde_json::Value::Object(document) => document,
        // An empty YAML document is null.
        serde_json::Value::Null => serde_json::Map::new(),
        _ => return Err(in_file(&"the document is not a table")),
    };
    if let Some(record) = ConfigRecord::ALL
        .into_iter()
        .find(|record| record.file() == stem && record.is_whole_file())
    {
        check_placement(record, scope, ConfigRecord::SINGLETON_KEY, &path.display())?;
        layer.entry(record).or_default().insert(
            ConfigRecord::SINGLETON_KEY.to_owned(),
            serde_json::Value::Object(document),
        );
        return Ok(());
    }
    if stem == ConfigRecord::Plugin.file() {
        let records = layer.entry(ConfigRecord::Plugin).or_default();
        for (name, value) in document {
            let serde_json::Value::Array(libraries) = value.clone() else {
                return Err(in_file(&format!("unknown key `{name}`")));
            };
            if name != "library" {
                return Err(in_file(&format!("unknown key `{name}`")));
            }
            for library in libraries {
                let serde_json::Value::Object(mut library) = library else {
                    return Err(in_file(&"a [[library]] entry is not a table"));
                };
                let Some(serde_json::Value::String(file)) = library.remove("file") else {
                    return Err(in_file(&"a [[library]] entry names no file"));
                };
                check_placement(ConfigRecord::Plugin, scope, &file, &path.display())?;
                if records
                    .insert(file.clone(), serde_json::Value::Object(library))
                    .is_some()
                {
                    return Err(in_file(&format!("library {file:?} is given twice")));
                }
            }
        }
        return Ok(());
    }
    for (name, value) in document {
        let record = ConfigRecord::from_name(&name)
            .filter(|record| record.file() == stem)
            .ok_or_else(|| in_file(&format!("unknown table `{name}`")))?;
        let records = layer.entry(record).or_default();
        if record.is_singleton() {
            if !value.is_object() {
                return Err(in_file(&format!("`{name}` is not a table")));
            }
            check_placement(record, scope, ConfigRecord::SINGLETON_KEY, &path.display())?;
            records.insert(ConfigRecord::SINGLETON_KEY.to_owned(), value);
            continue;
        }
        let serde_json::Value::Object(entries) = value else {
            return Err(in_file(&format!("`{name}` is not a table")));
        };
        for (key, definition) in entries {
            check_placement(record, scope, &key, &path.display())?;
            records.insert(key, definition);
        }
    }
    Ok(())
}

impl ConfigProvider for FileProvider {
    fn load(&self, station: &str) -> Result<ConfigSnapshot, RosterError> {
        // The revision is taken before the files are parsed, so a file changed in between
        // shows up as a change on the next poll rather than being missed.
        let revision = self.digest(station)?;
        let global = self.layer(&ConfigScope::Global)?;
        let view = if station.is_empty() {
            global
        } else {
            merged(&global, &self.layer(&ConfigScope::of_station(station))?)
        };
        assemble(revision, &view, Some(&self.root), &self.root.display())
    }

    fn changes_since(
        &self,
        station: &str,
        since: ConfigRevision,
    ) -> Result<Option<ConfigSnapshot>, RosterError> {
        if self.digest(station)? == since {
            return Ok(None);
        }
        self.load(station).map(Some)
    }

    fn revision(&self, station: &str) -> Result<ConfigRevision, RosterError> {
        self.digest(station)
    }

    fn scopes(&self) -> Result<Vec<ConfigScope>, RosterError> {
        let mut scopes = vec![ConfigScope::Global];
        let stations = self.root.join(STATIONS_DIR);
        let entries = match fs::read_dir(&stations) {
            Ok(entries) => entries,
            Err(failure) if failure.kind() == io::ErrorKind::NotFound => return Ok(scopes),
            Err(failure) => return Err(unavailable(&stations, &failure)),
        };
        for entry in entries {
            let entry = entry.map_err(|failure| unavailable(&stations, &failure))?;
            if entry.path().is_dir()
                && let Some(id) = entry.file_name().to_str()
                && ConfigScope::is_plain(id)
            {
                scopes.push(ConfigScope::of_station(id));
            }
        }
        scopes.sort();
        Ok(scopes)
    }

    /// The family's records at `scope` as JSON; the files themselves are edited by hand, so
    /// the directory takes no [`put`](ConfigProvider::put).
    fn definitions(
        &self,
        record: ConfigRecord,
        scope: &ConfigScope,
    ) -> Result<ConfigDefinitions, RosterError> {
        let station = match scope {
            ConfigScope::Global => "",
            ConfigScope::Station(id) => id,
        };
        let revision = self.digest(station)?;
        let records = self
            .layer(scope)?
            .remove(&record)
            .unwrap_or_default()
            .into_iter()
            .collect();
        Ok(ConfigDefinitions {
            revision,
            records,
            writable: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, name: &str, text: &str) {
        fs::create_dir_all(dir).unwrap();
        fs::write(dir.join(name), text).unwrap();
    }

    #[test]
    fn a_config_directory_changes_when_a_family_file_does() {
        let root = tempfile::tempdir().unwrap();
        let provider = FileProvider::new(root.path());
        let empty = provider.load("").unwrap();
        assert_eq!(empty.roster.operations().count(), 0);
        assert!(
            provider
                .changes_since("", empty.revision)
                .unwrap()
                .is_none()
        );

        write(root.path(), "operations.toml", "[operation.G]\n");
        let changed = provider.changes_since("", empty.revision).unwrap().unwrap();
        assert_eq!(changed.roster.operation("G").unwrap().id, "G");
        assert!(
            provider
                .changes_since("", changed.revision)
                .unwrap()
                .is_none()
        );

        // A file of no family is not a change.
        write(root.path(), "notes.toml", "");
        assert!(
            provider
                .changes_since("", changed.revision)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn a_config_directory_lists_its_tables_and_takes_no_changes() {
        let root = tempfile::tempdir().unwrap();
        let provider = FileProvider::new(root.path());
        write(
            root.path(),
            "agents.toml",
            "[agent.007]\nhandler = \"mock-agent\"\npriority = 5\n\
             [role.case-officer]\npurpose = \"Runs it.\"\npolicy = \"Report.\"\n\
             [assistant]\nallow_elevated = true\n[assistant.shared.ops]\nrepo = \"/srv/ops.git\"\n",
        );
        let global = ConfigScope::Global;
        assert_eq!(
            provider
                .definitions(ConfigRecord::Assistant, &global)
                .unwrap()
                .records,
            [(
                ConfigRecord::SINGLETON_KEY.to_owned(),
                serde_json::json!({"allow_elevated": true, "shared": {"ops": {"repo": "/srv/ops.git"}}})
            )]
        );
        let agents = provider.definitions(ConfigRecord::Agent, &global).unwrap();
        assert_eq!(agents.revision, provider.revision("").unwrap());
        assert!(!agents.writable);
        assert_eq!(
            agents.records,
            [(
                "007".to_owned(),
                serde_json::json!({"handler": "mock-agent", "priority": 5})
            )]
        );
        let refused = provider
            .put(ConfigRecord::Agent, &global, "008", &serde_json::json!({}))
            .unwrap_err();
        assert_eq!(refused.kind(), RosterErrorKind::ReadOnly);
    }

    #[test]
    fn an_invalid_or_unresolvable_directory_is_an_error_not_a_roster() {
        let root = tempfile::tempdir().unwrap();
        let provider = FileProvider::new(root.path());
        let good = provider.load("").unwrap();
        write(
            root.path(),
            "operations.toml",
            "[operation.G]\non_blown = \"shrug\"\n",
        );
        let refused = provider.changes_since("", good.revision).unwrap_err();
        assert!(refused.to_string().contains("on_blown"), "{refused}");

        write(root.path(), "operations.toml", "");
        write(
            root.path(),
            "agents.toml",
            "[agent.008]\nextends = \"nobody\"\n",
        );
        let refused = provider.load("").unwrap_err();
        assert_eq!(refused.kind(), RosterErrorKind::Invalid);
        assert!(refused.to_string().contains("nobody"), "{refused}");

        write(root.path(), "agents.toml", "[agents.008]\n");
        let refused = provider.load("").unwrap_err();
        assert!(refused.to_string().contains("`agents`"), "{refused}");
    }

    #[test]
    fn two_files_of_one_family_are_invalid_and_name_both() {
        let root = tempfile::tempdir().unwrap();
        write(root.path(), "agents.toml", "");
        write(root.path(), "agents.json", "{}");
        let refused = FileProvider::new(root.path()).load("").unwrap_err();
        assert_eq!(refused.kind(), RosterErrorKind::Invalid);
        let text = refused.to_string();
        assert!(
            text.contains("agents.toml") && text.contains("agents.json"),
            "{text}"
        );
    }

    #[test]
    fn a_station_reads_its_own_records_over_the_global_ones() {
        let root = tempfile::tempdir().unwrap();
        write(
            root.path(),
            "theaters.toml",
            "[theater.box]\naccess = { ssh = { host = \"global.example\" } }\n",
        );
        let own = root.path().join("stations").join("a");
        write(
            &own,
            "theaters.yaml",
            "theater:\n  box:\n    access:\n      ssh:\n        host: a.example\n  extra:\n    access:\n      ssh:\n        host: extra.example\n",
        );
        let provider = FileProvider::new(root.path());
        let host = |station: &str, id: &str| {
            let snapshot = provider.load(station).unwrap();
            snapshot
                .settings
                .theaters
                .find(id)
                .map(|theater| format!("{:?}", theater.access))
        };
        assert!(host("a", "box").unwrap().contains("a.example"));
        assert!(host("a", "extra").is_some());
        assert!(host("b", "box").unwrap().contains("global.example"));
        assert!(host("b", "extra").is_none());
        assert_ne!(
            provider.revision("a").unwrap(),
            provider.revision("b").unwrap(),
            "a Station's own files are part of its revision"
        );

        write(
            &own,
            "agents.toml",
            "[agent.007]\nhandler = \"mock-agent\"\n",
        );
        let refused = provider.load("a").unwrap_err();
        assert!(refused.to_string().contains("global"), "{refused}");
        assert!(
            provider.load("b").is_ok(),
            "another Station is not affected"
        );
    }
}
