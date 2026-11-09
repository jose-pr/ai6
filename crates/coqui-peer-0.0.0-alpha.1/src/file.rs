//! Whole-file writes that a crash or a concurrent writer cannot leave half done: the contents
//! go to a temporary file beside the target, are synced, and only then take the target's name.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Who may read a file written here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub enum Access {
    /// What the process's defaults give a new file.
    #[default]
    Default,
    /// The owner alone: mode `0600` on Unix. On Windows the file inherits its directory's
    /// access control list, as with `Default`.
    OwnerOnly,
}

/// Writes `contents` to `path` unless a file is there, atomically: a hard link from a synced
/// temporary file, which fails when `path` exists, so an existing file is never replaced and
/// of two concurrent writers exactly one wins. Parent directories are created.
///
/// # Errors
///
/// `AlreadyExists` when `path` exists; otherwise the filesystem's error. The temporary file
/// is removed either way.
pub fn write_new(path: &Path, contents: &[u8], access: Access) -> io::Result<()> {
    let temporary = written(path, contents, access)?;
    let result = retried(|| fs::hard_link(&temporary, path));
    let _ = fs::remove_file(&temporary);
    result
}

/// Writes `contents` to `path`, replacing any file there, atomically: a rename of a synced
/// temporary file, so a reader sees the old contents or the new, never part of either. Parent
/// directories are created.
///
/// # Errors
///
/// The filesystem's error; the temporary file is removed then.
pub fn replace(path: &Path, contents: &[u8], access: Access) -> io::Result<()> {
    let temporary = written(path, contents, access)?;
    let result = retried(|| fs::rename(&temporary, path));
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

/// Runs `operation`, and on Windows runs it again while it fails with `PermissionDenied`, for
/// up to about a second: a file another handle holds open without delete sharing (a scanner
/// that opens a file just written, a concurrent rename of it) cannot be renamed, linked or
/// removed meanwhile, and the call fails with `ERROR_ACCESS_DENIED`.
pub(crate) fn retried(mut operation: impl FnMut() -> io::Result<()>) -> io::Result<()> {
    let mut wait = core::time::Duration::from_millis(2);
    loop {
        match operation() {
            Err(error)
                if cfg!(windows)
                    && error.kind() == io::ErrorKind::PermissionDenied
                    && wait <= core::time::Duration::from_millis(512) =>
            {
                std::thread::sleep(wait);
                wait *= 2;
            }
            result => return result,
        }
    }
}

/// A synced temporary file beside `path` holding `contents`.
fn written(path: &Path, contents: &[u8], access: Access) -> io::Result<PathBuf> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?;
    if !parent.as_os_str().is_empty() {
        fs::create_dir_all(parent)?;
    }
    let temporary = temporary_path(path);
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    if access == Access::OwnerOnly {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    #[cfg(not(unix))]
    let _ = access;
    let mut file = options.open(&temporary)?;
    let result = file.write_all(contents).and_then(|()| file.sync_all());
    drop(file);
    match result {
        Ok(()) => Ok(temporary),
        Err(error) => {
            let _ = fs::remove_file(&temporary);
            Err(error)
        }
    }
}

/// A name beside `path` that no other call of this process uses, and that another process is
/// unlikely to: the process id, the time and a counter.
pub(crate) fn temporary_path(path: &Path) -> PathBuf {
    static NEXT: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
    let count = NEXT.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".{}.{}.{}.tmp", std::process::id(), nonce, count));
    path.with_file_name(name)
}
