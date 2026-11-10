//! [`PrivateDir`]: the file methods of a spawner, on this machine, for a host that implements one.
//!
//! A provider's path is never trusted. A path is relative, with no `..` part, no `:` (a drive or a
//! stream), no part that ends in a dot or a space, and no part a Windows device answers to (`NUL`,
//! `COM1`, `CONIN$` ...); a link that leads out of the directory is refused; and what is opened is
//! checked after it is opened, so a link planted after the check is not followed out. A file with
//! another name for the same data (a hard link) is refused where the system says so (Unix).
//!
//! On Windows the granted directory's ACL is what keeps others out: `private` adds nothing there,
//! and a hard link is not seen.

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use std::fs::{File, Metadata, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use guatiao::Status;
use guatiao::library::ProviderError;

use crate::FileInfo;

/// The file methods in one directory.
#[derive(Debug, Clone)]
#[must_use]
pub struct PrivateDir {
    root: PathBuf,
}

fn outside() -> ProviderError {
    ProviderError::new(
        Status::GUATIAO_ERR_NOT_FOUND,
        "no such path in the granted directory",
    )
}

fn failed(what: &str) -> ProviderError {
    ProviderError::new(Status::GUATIAO_ERR_INTERNAL, what)
}

/// Whether `name` is one a Windows device answers to, with or without an extension.
fn is_device(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or(name).trim_end();
    let stem = stem.to_ascii_uppercase();
    matches!(
        stem.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$" | "CLOCK$"
    ) || ((stem.starts_with("COM") || stem.starts_with("LPT"))
        && stem.len() == 4
        && stem.as_bytes()[3].is_ascii_digit())
}

#[cfg(unix)]
fn linked(meta: &Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    meta.nlink() > 1
}

#[cfg(not(unix))]
const fn linked(_meta: &Metadata) -> bool {
    false
}

fn modified_ms(meta: &Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |since| {
            i64::try_from(since.as_millis()).unwrap_or(i64::MAX)
        })
}

impl PrivateDir {
    /// Serves the directory `root`, which must exist.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// The directory.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    fn base(&self) -> Result<PathBuf, ProviderError> {
        std::fs::canonicalize(&self.root).map_err(|_| outside())
    }

    /// `path` under the root, by its text alone and by the nearest directory that exists.
    fn locate(&self, path: &str) -> Result<PathBuf, ProviderError> {
        if path.is_empty()
            || path.contains(['\0', ':'])
            || path.starts_with(['/', '\\'])
            || Path::new(path).is_absolute()
        {
            return Err(outside());
        }
        let mut full = self.root.clone();
        let mut parts = 0;
        for part in path.split(['/', '\\']) {
            match part {
                "" | "." => {}
                ".." => return Err(outside()),
                part if part.ends_with(['.', ' ']) || is_device(part) => return Err(outside()),
                part => {
                    full.push(part);
                    parts += 1;
                }
            }
        }
        if parts == 0 {
            return Err(outside());
        }
        let base = self.base()?;
        let mut nearest = full.clone();
        while !nearest.exists() {
            if !nearest.pop() {
                return Err(outside());
            }
        }
        let nearest = std::fs::canonicalize(&nearest).map_err(|_| outside())?;
        if nearest.starts_with(&base) {
            Ok(full)
        } else {
            Err(outside())
        }
    }

    /// Checks, after `full` was opened as `meta`, that it still is a file inside the root: the path
    /// now resolves inside it and, where the system tells, to the very file that was opened.
    fn opened(&self, full: &Path, meta: &Metadata) -> Result<(), ProviderError> {
        if !meta.is_file() || linked(meta) {
            return Err(outside());
        }
        let canonical = std::fs::canonicalize(full).map_err(|_| outside())?;
        if !canonical.starts_with(self.base()?) {
            return Err(outside());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let now = std::fs::metadata(&canonical).map_err(|_| outside())?;
            if now.dev() != meta.dev() || now.ino() != meta.ino() {
                return Err(outside());
            }
        }
        Ok(())
    }

    /// Writes `bytes` to `path`, making directories on the way and replacing a file there. On Unix
    /// the file is made in a temporary name beside it, owner-only when `private`, and renamed into
    /// place, so there is no moment it is readable by others holding the bytes.
    ///
    /// # Errors
    ///
    /// `GUATIAO_ERR_NOT_FOUND` for a path the rules refuse.
    pub fn write(&self, path: &str, bytes: &[u8], private: bool) -> Result<(), ProviderError> {
        let full = self.locate(path)?;
        let parent = full.parent().ok_or_else(outside)?;
        std::fs::create_dir_all(parent).map_err(|_| failed("cannot make the directory"))?;
        let base = self.base()?;
        if !std::fs::canonicalize(parent)
            .map_err(|_| outside())?
            .starts_with(&base)
        {
            return Err(outside());
        }
        match std::fs::symlink_metadata(&full) {
            Ok(meta) if !meta.is_file() || linked(&meta) => return Err(outside()),
            _ => {}
        }
        self.put(&full, parent, &base, bytes, private)
    }

    #[cfg(unix)]
    fn put(
        &self,
        full: &Path,
        parent: &Path,
        base: &Path,
        bytes: &[u8],
        private: bool,
    ) -> Result<(), ProviderError> {
        use core::sync::atomic::{AtomicU64, Ordering};
        use std::os::unix::fs::OpenOptionsExt;
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let temp = parent.join(format!(
            ".{}-{}.tmp",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        if private {
            options.mode(0o600);
        }
        let mut file = options
            .open(&temp)
            .map_err(|_| failed("cannot write the file"))?;
        let done = (|| {
            let inside = std::fs::canonicalize(&temp).map_err(|_| outside())?;
            if !inside.starts_with(base) {
                return Err(outside());
            }
            file.write_all(bytes)
                .map_err(|_| failed("cannot write the file"))?;
            std::fs::rename(&temp, full).map_err(|_| failed("cannot write the file"))
        })();
        if done.is_err() {
            let _ = std::fs::remove_file(&temp);
        }
        done
    }

    #[cfg(not(unix))]
    fn put(
        &self,
        full: &Path,
        _parent: &Path,
        _base: &Path,
        bytes: &[u8],
        _private: bool,
    ) -> Result<(), ProviderError> {
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(full)
            .map_err(|_| failed("cannot write the file"))?;
        let meta = file
            .metadata()
            .map_err(|_| failed("cannot write the file"))?;
        self.opened(full, &meta)?;
        file.set_len(0)
            .map_err(|_| failed("cannot write the file"))?;
        file.write_all(bytes)
            .map_err(|_| failed("cannot write the file"))
    }

    /// The content of `path`, at most `max_bytes`.
    ///
    /// # Errors
    ///
    /// `GUATIAO_ERR_NOT_FOUND` for a path the rules refuse, a file that is not there and one that
    /// is not a regular file; `GUATIAO_ERR_BAD_VALUE` for one longer than `max_bytes`.
    pub fn read(&self, path: &str, max_bytes: u64) -> Result<Vec<u8>, ProviderError> {
        let full = self.locate(path)?;
        // A pipe or a device is not opened: opening one can wait for ever.
        match std::fs::symlink_metadata(&full) {
            Ok(meta) if meta.is_file() => {}
            _ => return Err(outside()),
        }
        let file = File::open(&full).map_err(|_| outside())?;
        let meta = file.metadata().map_err(|_| outside())?;
        self.opened(&full, &meta)?;
        let mut bytes = Vec::new();
        file.take(max_bytes.saturating_add(1))
            .read_to_end(&mut bytes)
            .map_err(|_| failed("cannot read the file"))?;
        if bytes.len() as u64 > max_bytes {
            return Err(ProviderError::new(
                Status::GUATIAO_ERR_BAD_VALUE,
                "the file is longer than max_bytes",
            ));
        }
        Ok(bytes)
    }

    /// How long `path` is and when it last changed; `None` when it is not there.
    ///
    /// # Errors
    ///
    /// `GUATIAO_ERR_NOT_FOUND` for a path the rules refuse.
    pub fn info(&self, path: &str) -> Result<Option<FileInfo>, ProviderError> {
        let full = self.locate(path)?;
        let meta = match std::fs::symlink_metadata(&full) {
            Ok(meta) => meta,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(outside()),
        };
        if meta.file_type().is_symlink() || linked(&meta) {
            return Err(outside());
        }
        if !meta.is_file() {
            return Ok(None);
        }
        if !std::fs::canonicalize(&full)
            .map_err(|_| outside())?
            .starts_with(self.base()?)
        {
            return Err(outside());
        }
        Ok(Some(FileInfo::new(meta.len(), modified_ms(&meta))))
    }

    /// Removes `path`; a file that is not there is no error.
    ///
    /// # Errors
    ///
    /// `GUATIAO_ERR_NOT_FOUND` for a path the rules refuse.
    pub fn remove(&self, path: &str) -> Result<(), ProviderError> {
        let full = self.locate(path)?;
        match std::fs::symlink_metadata(&full) {
            Ok(meta) if meta.is_file() && !linked(&meta) => std::fs::remove_file(&full)
                .or_else(|e| {
                    if e.kind() == std::io::ErrorKind::NotFound {
                        Ok(())
                    } else {
                        Err(e)
                    }
                })
                .map_err(|e| failed(&e.to_string())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            _ => Err(outside()),
        }
    }
}

/// The file's text, for a caller that keeps credentials as text.
impl PrivateDir {
    /// [`read`](Self::read), as text with anything that is not UTF-8 replaced.
    ///
    /// # Errors
    ///
    /// As [`read`](Self::read).
    pub fn read_text(&self, path: &str, max_bytes: u64) -> Result<String, ProviderError> {
        self.read(path, max_bytes)
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir() -> PathBuf {
        static COUNTER: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "acp-private-dir-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, core::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).expect("a directory");
        dir
    }

    fn status<T>(answer: Result<T, ProviderError>) -> Option<u32> {
        answer.err().map(|e| e.status.into())
    }

    #[test]
    fn a_file_is_written_read_described_and_removed() {
        let root = dir();
        let files = PrivateDir::new(&root);
        files
            .write("a/b/c.json", b"content", true)
            .expect("written");
        assert_eq!(files.read("a/b/c.json", 100).expect("read"), b"content");
        files
            .write("a/b/c.json", b"replaced", false)
            .expect("replaced");
        assert_eq!(
            files.read_text("a/b/c.json", 100).expect("read"),
            "replaced"
        );
        let info = files.info("a/b/c.json").expect("described").expect("there");
        assert_eq!(info.len, 8);
        assert_eq!(status(files.read("a/b/c.json", 7)), Some(7));
        files.remove("a/b/c.json").expect("removed");
        assert!(files.info("a/b/c.json").expect("described").is_none());
        files
            .remove("a/b/c.json")
            .expect("removing nothing is no error");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn names_a_system_would_read_otherwise_are_refused() {
        let root = dir();
        let files = PrivateDir::new(&root);
        for path in [
            "",
            "..",
            "../x",
            "a/../../x",
            "/x",
            "\\x",
            "C:\\x",
            "C:x",
            "a\0b",
            "NUL",
            "nul.txt",
            "COM1",
            "lpt9.log",
            "dir/CONIN$",
            "auth.json:stream",
            "x/C:y",
            "trailing.",
            "space ",
        ] {
            assert_eq!(status(files.write(path, b"x", false)), Some(10), "{path:?}");
            assert_eq!(status(files.read(path, 8)), Some(10), "{path:?}");
            assert_eq!(status(files.info(path)), Some(10), "{path:?}");
            assert_eq!(status(files.remove(path)), Some(10), "{path:?}");
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[cfg(unix)]
    #[test]
    fn a_private_file_is_never_readable_by_others() {
        use std::os::unix::fs::PermissionsExt;
        let root = dir();
        let files = PrivateDir::new(&root);
        files.write("secret", b"x", true).expect("written");
        let mode = std::fs::metadata(root.join("secret"))
            .expect("there")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
        // Replaced, it is private again, and no temporary file is left.
        files.write("secret", b"y", true).expect("replaced");
        assert_eq!(
            std::fs::metadata(root.join("secret"))
                .expect("there")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert_eq!(std::fs::read_dir(&root).expect("listed").count(), 1);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[cfg(unix)]
    #[test]
    fn a_second_name_for_a_file_a_link_and_a_pipe_are_refused() {
        let root = dir();
        let outside = root.with_file_name(format!("{}-outside", root.display()));
        std::fs::create_dir_all(&outside).expect("a directory");
        std::fs::write(outside.join("target"), "other").expect("written");
        std::fs::hard_link(outside.join("target"), root.join("hard")).expect("linked");
        std::os::unix::fs::symlink(outside.join("target"), root.join("soft")).expect("linked");
        std::os::unix::fs::symlink(&outside, root.join("dir")).expect("linked");
        let made = std::process::Command::new("mkfifo")
            .arg(root.join("pipe"))
            .status()
            .is_ok_and(|s| s.success());
        let files = PrivateDir::new(&root);
        let mut refused = vec!["hard", "soft", "dir/target", "dir/new"];
        if made {
            refused.push("pipe");
        }
        for path in refused {
            assert_eq!(status(files.read(path, 8)), Some(10), "read {path}");
            assert_eq!(
                status(files.write(path, b"x", true)),
                Some(10),
                "write {path}"
            );
            assert_eq!(status(files.remove(path)), Some(10), "remove {path}");
        }
        assert_eq!(
            std::fs::read_to_string(outside.join("target")).expect("read"),
            "other",
            "a file outside was written through"
        );
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&outside);
    }
}
