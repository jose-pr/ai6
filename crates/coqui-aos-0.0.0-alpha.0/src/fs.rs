//! The filesystem the reader reads through, which the caller supplies, so the core does no I/O
//! of its own and builds for targets that have no files.

use std::ffi::OsString;
use std::io;
use std::path::Path;

/// What a path is, without following a link.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum FileKind {
    /// A regular file.
    File,
    /// A directory.
    Directory,
    /// A symbolic link, junction or other reparse point.
    Symlink,
    /// A device, pipe, socket or anything else.
    Other,
}

/// What a path is and how many bytes it holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Metadata {
    /// What the path is.
    pub kind: FileKind,
    /// Its size in bytes; 0 for anything but a regular file.
    pub len: u64,
}

impl Metadata {
    /// The metadata of something that is `kind` and `len` bytes long.
    #[must_use]
    pub const fn new(kind: FileKind, len: u64) -> Self {
        Self { kind, len }
    }
}

/// Reads files, lists directories and says what a path is. Every path is as the caller built it
/// from the root it handed the reader, and a link is never followed.
pub trait FileSystem {
    /// What `path` is, **without following a link**: a link is [`FileKind::Symlink`].
    ///
    /// # Errors
    ///
    /// The `io::Error` of a path that cannot be inspected.
    fn metadata(&self, path: &Path) -> io::Result<Metadata>;

    /// The first `max` bytes of the file at `path`, fewer when it is shorter.
    ///
    /// # Errors
    ///
    /// The `io::Error` of a file that cannot be read.
    fn read(&self, path: &Path, max: u64) -> io::Result<Vec<u8>>;

    /// The names in the directory at `path`, in any order.
    ///
    /// # Errors
    ///
    /// The `io::Error` of a directory that cannot be listed.
    fn list(&self, path: &Path) -> io::Result<Vec<OsString>>;
}

impl<F: FileSystem + ?Sized> FileSystem for &F {
    fn metadata(&self, path: &Path) -> io::Result<Metadata> {
        (**self).metadata(path)
    }

    fn read(&self, path: &Path, max: u64) -> io::Result<Vec<u8>> {
        (**self).read(path, max)
    }

    fn list(&self, path: &Path) -> io::Result<Vec<OsString>> {
        (**self).list(path)
    }
}

/// The operating system's files, through `std::fs`.
#[cfg(feature = "fs")]
#[derive(Clone, Copy, Debug, Default)]
pub struct StdFs;

#[cfg(feature = "fs")]
impl FileSystem for StdFs {
    fn metadata(&self, path: &Path) -> io::Result<Metadata> {
        let metadata = std::fs::symlink_metadata(path)?;
        let kind = if metadata.file_type().is_symlink() || is_reparse_point(&metadata) {
            FileKind::Symlink
        } else if metadata.is_dir() {
            FileKind::Directory
        } else if metadata.is_file() {
            FileKind::File
        } else {
            FileKind::Other
        };
        Ok(Metadata::new(
            kind,
            if kind == FileKind::File {
                metadata.len()
            } else {
                0
            },
        ))
    }

    fn read(&self, path: &Path, max: u64) -> io::Result<Vec<u8>> {
        use io::Read as _;
        let mut bytes = Vec::new();
        std::fs::File::open(path)?
            .take(max)
            .read_to_end(&mut bytes)?;
        Ok(bytes)
    }

    fn list(&self, path: &Path) -> io::Result<Vec<OsString>> {
        std::fs::read_dir(path)?
            .map(|entry| entry.map(|entry| entry.file_name()))
            .collect()
    }
}

#[cfg(all(feature = "fs", windows))]
fn is_reparse_point(metadata: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    // FILE_ATTRIBUTE_REPARSE_POINT: symbolic links and junctions alike.
    metadata.file_attributes() & 0x400 != 0
}

#[cfg(all(feature = "fs", not(windows)))]
fn is_reparse_point(_metadata: &std::fs::Metadata) -> bool {
    false
}

/// A name as bytes, for the digest's rules on names: valid UTF-8 where the name is Unicode, and
/// a byte no UTF-8 holds where it is not.
pub(crate) fn name_bytes(name: &std::ffi::OsStr) -> Vec<u8> {
    name.to_str()
        .map_or_else(|| vec![0xff], |name| name.as_bytes().to_vec())
}
