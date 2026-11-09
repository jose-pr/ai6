//! Private files: written whole and atomically, read within a bound, told apart by stamp;
//! and [`FileTable`], a [`Table`](crate::Table) kept in one of them in a serde format the
//! caller supplies through a [`Codec`].
//!
//! Needs the `file` feature.

mod source;
mod table;

pub use source::{FileSource, Source};
pub use table::{Codec, CodecError, DEFAULT_MAX_BYTES, FileTable};

use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// Which writing of a file is there: told apart from any other by when it was written, how
/// long it is and, on Unix, which inode it is (every writing here is a new file put in the
/// old one's place, and a coarse clock alone cannot tell two apart).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stamp {
    modified: Option<SystemTime>,
    length: u64,
    inode: u64,
}

/// The stamp of the file at `path`; `None` when there is no such file.
///
/// # Errors
///
/// Returns an error when the file's metadata cannot be read for another reason.
pub fn stamp(path: &Path) -> Result<Option<Stamp>, String> {
    match fs::metadata(path) {
        Ok(metadata) => {
            #[cfg(unix)]
            let inode = std::os::unix::fs::MetadataExt::ino(&metadata);
            #[cfg(not(unix))]
            let inode = 0;
            Ok(Some(Stamp {
                modified: metadata.modified().ok(),
                length: metadata.len(),
                inode,
            }))
        }
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(_) => Err("file metadata is unavailable".to_owned()),
    }
}

/// Reads the UTF-8 text of the file at `path`, refusing more than `max_bytes`. `None` when
/// there is no such file: nothing has been kept yet.
///
/// # Errors
///
/// Returns an error, without the path, when the file is unreadable, too long or not UTF-8.
pub fn read_bounded(path: &Path, max_bytes: usize) -> Result<Option<String>, String> {
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err("file cannot be read".to_owned()),
    };
    let mut bytes = Vec::new();
    file.take(max_bytes as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "file cannot be read".to_owned())?;
    if bytes.len() > max_bytes {
        return Err(format!("file exceeds the {max_bytes} byte limit"));
    }
    String::from_utf8(bytes)
        .map(Some)
        .map_err(|_| "file is not UTF-8".to_owned())
}

/// Writes `bytes` as the file at `path`: beside it first and then in its place, so it is
/// never there half written, readable by its owner alone (`0600` on Unix), and never
/// written through a link or into a file someone else can read.
///
/// # Errors
///
/// Returns an error when the file cannot be created, written or put in place.
pub fn write_private(path: &Path, bytes: &[u8]) -> Result<(), String> {
    static WRITTEN: AtomicU64 = AtomicU64::new(0);
    let nth = WRITTEN.fetch_add(1, Ordering::Relaxed);
    let moment = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos());
    let mut beside = path.as_os_str().to_owned();
    beside.push(format!(".{}.{moment:x}.{nth}.new", std::process::id()));
    let beside = PathBuf::from(beside);
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
        .open(&beside)
        .and_then(|mut file| {
            file.write_all(bytes)?;
            file.sync_all()
        })
        .and_then(|()| fs::rename(&beside, path))
        .map_err(|error| {
            let _ = fs::remove_file(&beside);
            format!("{}: could not be written: {error}", path.display())
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_are_written_whole_private_and_read_back_within_a_bound() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("kept.dat");
        assert_eq!(read_bounded(&file, 100), Ok(None));
        assert_eq!(stamp(&file), Ok(None));
        write_private(&file, b"first").unwrap();
        write_private(&file, b"second, and longer").unwrap();
        assert_eq!(
            read_bounded(&file, 100),
            Ok(Some("second, and longer".to_owned()))
        );
        assert!(read_bounded(&file, 5).is_err());
        assert!(stamp(&file).unwrap().is_some());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&file).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
            for _ in 0..20 {
                write_private(&file, b"one").unwrap();
                let first = stamp(&file).unwrap();
                write_private(&file, b"two").unwrap();
                assert_ne!(stamp(&file).unwrap(), first);
            }
        }
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
        let nowhere = directory.path().join("missing").join("kept.dat");
        assert!(write_private(&nowhere, b"x").is_err());
    }

    #[test]
    fn invalid_utf8_is_refused() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("bad");
        fs::write(&file, [0xff, 0xfe]).unwrap();
        assert!(read_bounded(&file, 10).is_err());
    }
}
