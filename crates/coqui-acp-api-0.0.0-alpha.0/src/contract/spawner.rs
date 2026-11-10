//! The checks of a spawner's file methods, for the host that implements one.
//!
//! The suite's other checks drive a provider. These drive the host's side of the kind: whatever
//! implements [`ProcessSpawner`] and hands a provider a private directory has to keep a provider's
//! paths inside it, since a provider is never trusted with a path.

use alloc::borrow::ToOwned;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use std::path::Path;

use guatiao::Secret;

use super::checks::{fresh_uuid, make_link};
use super::{Failure, Failures};
use crate::{ProcessSpawner, SecretBytes};

const NOT_FOUND: u32 = 10;
const BAD_VALUE: u32 = 7;
const UNSUPPORTED: u32 = 13;

type Outcome = Result<(), String>;

/// Holds `spawner` to the file methods' rules, in `root`, the directory it serves files in.
///
/// A spawner whose `describe` says `files` false must refuse all four methods
/// `GUATIAO_ERR_UNSUPPORTED`. One that says `files` true must write, read, describe and remove a
/// file (nested directories made on the way), refuse a file longer than `max_bytes`
/// `GUATIAO_ERR_BAD_VALUE`, and answer `GUATIAO_ERR_NOT_FOUND` for every path that leaves the
/// directory: an absolute one, one with a `..` part, an empty one, and one through a link that
/// leads out (made when the platform lets the suite make one). `root` must exist and be empty.
///
/// # Errors
///
/// Every rule it broke.
pub fn spawner_files(spawner: &dyn ProcessSpawner, root: &Path) -> Result<(), Failures> {
    let files = match spawner.describe() {
        Ok(info) => info.files,
        Err(e) => {
            return Err(Failures {
                provider: "the spawner".to_owned(),
                failures: alloc::vec![Failure {
                    check: "describes itself",
                    message: e.message().to_owned(),
                }],
            });
        }
    };
    let checks: Vec<(&'static str, Outcome)> = if files {
        alloc::vec![
            ("keeps a file", round_trip(spawner)),
            ("bounds a read", bounded(spawner)),
            ("keeps paths inside its directory", confined(spawner, root)),
        ]
    } else {
        alloc::vec![("refuses the file methods", refuses(spawner))]
    };
    let failures: Vec<Failure> = checks
        .into_iter()
        .filter_map(|(check, outcome)| outcome.err().map(|message| Failure { check, message }))
        .collect();
    if failures.is_empty() {
        Ok(())
    } else {
        Err(Failures {
            provider: "the spawner".to_owned(),
            failures,
        })
    }
}

fn status<T>(answer: &Result<T, guatiao::library::ProviderError>) -> Option<u32> {
    answer.as_ref().err().map(|e| e.status.into())
}

fn secret(bytes: &[u8]) -> Secret<SecretBytes> {
    Secret::new(SecretBytes::new(bytes.to_vec()))
}

fn refuses(spawner: &dyn ProcessSpawner) -> Outcome {
    let answers = [
        status(&spawner.write_file("a", secret(b"x"), false)),
        status(&spawner.read_file("a", 1)),
        status(&spawner.file_info("a")),
        status(&spawner.remove_file("a")),
    ];
    if answers.iter().all(|s| *s == Some(UNSUPPORTED)) {
        Ok(())
    } else {
        Err(format!(
            "a spawner with no files answered {answers:?}, not {UNSUPPORTED} each"
        ))
    }
}

fn round_trip(spawner: &dyn ProcessSpawner) -> Outcome {
    let path = "nested/dir/credentials.json";
    let content = b"a file of the account";
    spawner
        .write_file(path, secret(content), true)
        .map_err(|e| format!("write_file: {}", e.message()))?;
    let read = spawner
        .read_file(path, 1 << 10)
        .map_err(|e| format!("read_file: {}", e.message()))?;
    if read.expose().as_slice() != content {
        return Err("read_file answered other bytes than were written".to_owned());
    }
    let info = spawner
        .file_info(path)
        .map_err(|e| format!("file_info: {}", e.message()))?
        .ok_or("file_info found no file that was written")?;
    if info.len != content.len() as u64 || info.modified_ms <= 0 {
        return Err(format!("file_info says {info:?}"));
    }
    spawner
        .write_file(path, secret(b"replaced"), false)
        .map_err(|e| format!("write_file over a file: {}", e.message()))?;
    let read = spawner
        .read_file(path, 1 << 10)
        .map_err(|e| format!("read_file: {}", e.message()))?;
    if read.expose().as_slice() != b"replaced" {
        return Err("a file written over kept its old content".to_owned());
    }
    spawner
        .remove_file(path)
        .map_err(|e| format!("remove_file: {}", e.message()))?;
    if spawner
        .file_info(path)
        .map_err(|e| format!("file_info: {}", e.message()))?
        .is_some()
    {
        return Err("file_info found a file that was removed".to_owned());
    }
    spawner
        .remove_file(path)
        .map_err(|e| format!("removing a file that is not there: {}", e.message()))?;
    let missing = spawner.read_file(path, 1 << 10);
    if status(&missing) != Some(NOT_FOUND) {
        return Err(format!(
            "reading a file that is not there answered {:?}, not {NOT_FOUND}",
            status(&missing)
        ));
    }
    Ok(())
}

fn bounded(spawner: &dyn ProcessSpawner) -> Outcome {
    spawner
        .write_file("bounded", secret(b"0123456789"), false)
        .map_err(|e| format!("write_file: {}", e.message()))?;
    let long = spawner.read_file("bounded", 5);
    let whole = spawner.read_file("bounded", 10);
    let _ = spawner.remove_file("bounded");
    if status(&long) != Some(BAD_VALUE) {
        return Err(format!(
            "reading 10 bytes into max_bytes 5 answered {:?}, not {BAD_VALUE}",
            status(&long)
        ));
    }
    if whole.is_err() {
        return Err("reading a file of exactly max_bytes was refused".to_owned());
    }
    Ok(())
}

fn confined(spawner: &dyn ProcessSpawner, root: &Path) -> Outcome {
    let mut paths = alloc::vec![
        "../outside".to_owned(),
        "a/../../outside".to_owned(),
        "..".to_owned(),
        String::new(),
        "/etc/outside".to_owned(),
        "\\outside".to_owned(),
        "C:\\outside".to_owned(),
        "C:outside".to_owned(),
        "a\0b".to_owned(),
        "NUL".to_owned(),
        "nul.txt".to_owned(),
        "COM1".to_owned(),
        "dir/CONIN$".to_owned(),
        "auth.json:stream".to_owned(),
        "x/C:y".to_owned(),
        "trailing.".to_owned(),
    ];
    let base = std::fs::canonicalize(root).map_err(|e| format!("no directory: {e}"))?;
    let outside = base.with_file_name(format!("outside-{}", fresh_uuid()));
    std::fs::create_dir_all(&outside).map_err(|e| format!("cannot make {outside:?}: {e}"))?;
    let link = base.join("escape");
    let linked = make_link(&outside, &link);
    if linked {
        paths.push("escape/leaked".to_owned());
        paths.push("escape".to_owned());
    }
    // A second name for a file outside: seen and refused where the system tells (Unix).
    let other = outside.join("other");
    let hard = base.join("hard");
    let hard_linked = cfg!(unix)
        && std::fs::write(&other, b"x").is_ok()
        && std::fs::hard_link(&other, &hard).is_ok();
    if hard_linked {
        paths.push("hard".to_owned());
    }
    let mut broken = Vec::new();
    for path in &paths {
        let answers = [
            (
                "write_file",
                status(&spawner.write_file(path, secret(b"x"), false)),
            ),
            ("read_file", status(&spawner.read_file(path, 8))),
            ("file_info", status(&spawner.file_info(path))),
            ("remove_file", status(&spawner.remove_file(path))),
        ];
        for (method, answer) in answers {
            if answer != Some(NOT_FOUND) {
                broken.push(format!("{method}({path:?}) answered {answer:?}"));
            }
        }
    }
    let left = std::fs::read_dir(&outside).map_or(0, Iterator::count) - usize::from(hard_linked);
    if hard_linked {
        let _ = std::fs::remove_file(&hard);
    }
    if linked {
        let _ = std::fs::remove_dir(&link).or_else(|_| std::fs::remove_file(&link));
    }
    let _ = std::fs::remove_dir_all(&outside);
    if left != 0 {
        broken.push("a file was written outside the directory".to_owned());
    }
    if broken.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "not {NOT_FOUND} for a path outside the directory: {}",
            broken.join("; ")
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::host;

    #[test]
    fn the_suites_own_spawner_keeps_files_in_its_directory() {
        let dir = std::env::temp_dir().join(format!("acp-spawner-files-{}", fresh_uuid()));
        std::fs::create_dir_all(&dir).expect("a directory");
        let (_, spawner, _) = host::spawner_in(Some(&dir));
        let outcome = spawner_files(&*spawner, &dir);
        let _ = std::fs::remove_dir_all(&dir);
        if let Err(failures) = outcome {
            panic!("{failures}");
        }
    }

    #[test]
    fn a_spawner_without_files_refuses_the_four_methods() {
        let dir = std::env::temp_dir();
        let (_, spawner, _) = host::spawner_in(None);
        if let Err(failures) = spawner_files(&*spawner, &dir) {
            panic!("{failures}");
        }
    }

    #[test]
    fn a_spawner_that_takes_a_path_out_of_its_directory_is_named() {
        struct Careless(std::path::PathBuf);

        impl ProcessSpawner for Careless {
            fn describe(&self) -> Result<crate::SpawnerInfo, guatiao::library::ProviderError> {
                let mut info = crate::SpawnerInfo::new("linux", "x86_64", true);
                info.files = true;
                Ok(info)
            }

            fn spawn(
                &self,
                _spec: crate::SpawnSpec,
            ) -> Result<
                guatiao::library::Object<dyn crate::SpawnedProcess>,
                guatiao::library::ProviderError,
            > {
                Err(guatiao::library::ProviderError::new(
                    guatiao::Status::GUATIAO_ERR_UNSUPPORTED,
                    "no",
                ))
            }

            fn write_file(
                &self,
                path: &str,
                bytes: Secret<SecretBytes>,
                _private: bool,
            ) -> Result<(), guatiao::library::ProviderError> {
                // It trusts a relative path, `..` included; an absolute path it only pretends to write,
                // so the test never writes outside its own directory.
                if path.contains(':') || path.starts_with(['/', '\\']) {
                    return Ok(());
                }
                let full = self.0.join(path);
                if let Some(parent) = full.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                std::fs::write(full, bytes.expose().as_slice()).map_err(|_| {
                    guatiao::library::ProviderError::new(guatiao::Status::GUATIAO_ERR_INTERNAL, "x")
                })
            }

            fn read_file(
                &self,
                path: &str,
                _max_bytes: u64,
            ) -> Result<Secret<SecretBytes>, guatiao::library::ProviderError> {
                std::fs::read(self.0.join(path))
                    .map(|b| Secret::new(SecretBytes::new(b)))
                    .map_err(|_| {
                        guatiao::library::ProviderError::new(
                            guatiao::Status::GUATIAO_ERR_NOT_FOUND,
                            "x",
                        )
                    })
            }

            fn file_info(
                &self,
                _path: &str,
            ) -> Result<Option<crate::FileInfo>, guatiao::library::ProviderError> {
                Ok(None)
            }

            fn remove_file(&self, _path: &str) -> Result<(), guatiao::library::ProviderError> {
                Ok(())
            }
        }

        let base = std::env::temp_dir().join(format!("acp-spawner-careless-{}", fresh_uuid()));
        let dir = base.join("root");
        std::fs::create_dir_all(&dir).expect("a directory");
        let failures = spawner_files(&Careless(dir.clone()), &dir)
            .expect_err("a spawner that trusts a path is caught");
        let _ = std::fs::remove_dir_all(&base);
        let said = failures.to_string();
        assert!(
            said.contains("keeps a file") || said.contains("inside its directory"),
            "{said}"
        );
    }
}
