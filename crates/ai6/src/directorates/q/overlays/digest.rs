//! The tree digest an overlay is pinned by (`spec/v0/integrity.md`): one line per member,
//! `path NUL sha256:<hex> LF`, sorted by path bytes, and the SHA-256 of those lines.

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io::{self, Read};
use std::path::Path;

use sha2::{Digest as _, Sha256};

use super::names::{EXCLUDED_COMPONENTS, valid_relative_path};
use crate::bounds::Limit;

/// A SHA-256 digest, written `sha256:` and 64 hexadecimal digits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Digest([u8; 32]);

impl Digest {
    /// The digest of `bytes`.
    pub fn of(bytes: &[u8]) -> Self {
        Self(Sha256::digest(bytes).into())
    }

    /// Reads `sha256:<hex>`, the hexadecimal digits in either case.
    ///
    /// # Errors
    ///
    /// When the prefix is not `sha256:` or there are not exactly 64 hexadecimal digits.
    pub fn parse(text: &str) -> Result<Self, String> {
        let hex = text
            .strip_prefix("sha256:")
            .filter(|hex| hex.len() == 64 && hex.bytes().all(|byte| byte.is_ascii_hexdigit()))
            .ok_or("a digest is sha256: and 64 hexadecimal digits")?;
        let mut bytes = [0u8; 32];
        for (index, byte) in bytes.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&hex[index * 2..index * 2 + 2], 16)
                .map_err(|_| "a digest is sha256: and 64 hexadecimal digits")?;
        }
        Ok(Self(bytes))
    }

    /// The 32 bytes.
    pub fn bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Display for Digest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("sha256:")?;
        for byte in self.0 {
            write!(formatter, "{byte:02x}")?;
        }
        Ok(())
    }
}

impl std::str::FromStr for Digest {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, String> {
        Self::parse(text)
    }
}

/// Why a tree has no digest.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DigestErrorKind {
    /// A symbolic link, junction or other reparse point.
    Symlink,
    /// Something that is neither a regular file nor a directory.
    SpecialFile,
    /// A name that is not UTF-8.
    NonUtf8Name,
    /// A member whose path is not a relative path.
    InvalidName,
    /// Two members equal after ASCII lowercasing.
    CaseCollision,
    /// A limit on what is read was passed.
    TooLarge,
    /// The tree could not be read.
    Io,
}

impl DigestErrorKind {
    /// The AOS error class: `symlink`, `special-file`, `non-utf8-name`, `invalid-name` or
    /// `case-collision`; `too-large` and `io` are this reader's own.
    pub const fn class(self) -> &'static str {
        match self {
            Self::Symlink => "symlink",
            Self::SpecialFile => "special-file",
            Self::NonUtf8Name => "non-utf8-name",
            Self::InvalidName => "invalid-name",
            Self::CaseCollision => "case-collision",
            Self::TooLarge => "too-large",
            Self::Io => "io",
        }
    }
}

/// A tree whose digest cannot be computed, naming the entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DigestError {
    /// Why.
    pub kind: DigestErrorKind,
    /// The entry's path from the root, or the limit's name; never file content.
    pub path: String,
}

impl fmt::Display for DigestError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.kind.class(), self.path)
    }
}

impl std::error::Error for DigestError {}

fn refused(kind: DigestErrorKind, path: impl Into<String>) -> DigestError {
    DigestError {
        kind,
        path: path.into(),
    }
}

/// One entry of a tree, for [`digest_entries`].
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TreeEntry {
    /// A regular file and its bytes.
    File(Vec<u8>),
    /// A directory.
    Directory,
    /// A symbolic link, junction or other reparse point.
    Symlink,
    /// A device, pipe, socket or anything else.
    Special,
}

/// A tree's digest and the file it was computed from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeDigest {
    /// The tree digest.
    pub digest: Digest,
    /// Each member's digest, by its path.
    pub files: BTreeMap<String, Digest>,
}

impl TreeDigest {
    /// The lines the digest is the SHA-256 of, in order.
    pub fn lines(&self) -> Vec<Vec<u8>> {
        self.files
            .iter()
            .map(|(path, digest)| {
                let mut line = path.as_bytes().to_vec();
                line.push(0);
                line.extend_from_slice(digest.to_string().as_bytes());
                line.push(b'\n');
                line
            })
            .collect()
    }
}

/// Whether a path's components put it outside the overlay: an excluded name, or a final
/// component other than a directory ending in `.pyc`.
fn excluded(components: &[&[u8]], directory: bool) -> bool {
    components.iter().any(|component| {
        EXCLUDED_COMPONENTS
            .iter()
            .any(|name| name.as_bytes() == *component)
    }) || (!directory
        && components
            .last()
            .is_some_and(|last| last.ends_with(b".pyc")))
}

fn shown(components: &[&[u8]]) -> String {
    components
        .iter()
        .map(|component| String::from_utf8_lossy(component).into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

/// The digest of a tree given as entries, each by its path components as stored: the order
/// of entries does not matter.
///
/// # Errors
///
/// A [`DigestError`] for a link, a special file, a name that is not UTF-8 or not a relative
/// path, or two members equal under ASCII case, unless the entry is excluded.
pub fn digest_entries<'a, I>(entries: I) -> Result<TreeDigest, DigestError>
where
    I: IntoIterator<Item = (Vec<&'a [u8]>, &'a TreeEntry)>,
{
    let mut files = BTreeMap::new();
    for (components, entry) in entries {
        let directory = matches!(entry, TreeEntry::Directory);
        if directory || excluded(&components, directory) {
            continue;
        }
        let bytes = match entry {
            TreeEntry::File(bytes) => bytes,
            TreeEntry::Symlink => {
                return Err(refused(DigestErrorKind::Symlink, shown(&components)));
            }
            _ => return Err(refused(DigestErrorKind::SpecialFile, shown(&components))),
        };
        let path = member_path(&components)?;
        files.insert(path, Digest::of(bytes));
    }
    check_case(&files)?;
    Ok(finish(files))
}

/// The path of a member from its components, refused when it is not a relative path.
fn member_path(components: &[&[u8]]) -> Result<String, DigestError> {
    let mut names = Vec::with_capacity(components.len());
    for component in components {
        names.push(
            std::str::from_utf8(component)
                .map_err(|_| refused(DigestErrorKind::NonUtf8Name, shown(components)))?,
        );
    }
    let path = names.join("/");
    if !valid_relative_path(&path) {
        return Err(refused(DigestErrorKind::InvalidName, shown(components)));
    }
    Ok(path)
}

fn check_case(files: &BTreeMap<String, Digest>) -> Result<(), DigestError> {
    let mut folded = BTreeMap::new();
    for path in files.keys() {
        if let Some(other) = folded.insert(path.to_ascii_lowercase(), path)
            && other != path
        {
            return Err(refused(DigestErrorKind::CaseCollision, path.clone()));
        }
    }
    Ok(())
}

fn finish(files: BTreeMap<String, Digest>) -> TreeDigest {
    let mut text = Vec::new();
    for (path, digest) in &files {
        text.extend_from_slice(path.as_bytes());
        text.push(0);
        text.extend_from_slice(digest.to_string().as_bytes());
        text.push(b'\n');
    }
    TreeDigest {
        digest: Digest::of(&text),
        files,
    }
}

/// How much of a tree a walk reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TreeLimits {
    /// Members of the tree.
    pub files: Limit,
    /// Bytes of every member together.
    pub bytes: Limit,
    /// Bytes of one member.
    pub file_bytes: Limit,
}

/// The digest of the directory `root`, read without following any link.
///
/// # Errors
///
/// A [`DigestError`] as [`digest_entries`] refuses, `io` when an entry cannot be read, and
/// `too-large` naming the limit passed.
pub fn tree_digest(root: &Path, limits: &TreeLimits) -> Result<TreeDigest, DigestError> {
    let metadata = fs::symlink_metadata(root)
        .map_err(|error| refused(DigestErrorKind::Io, error.to_string()))?;
    if !metadata.is_dir() || is_reparse_point(&metadata) {
        return Err(refused(
            DigestErrorKind::Io,
            "the root is not a directory (a link is never followed)",
        ));
    }
    let mut walk = Walk {
        limits,
        files: BTreeMap::new(),
        bytes: 0,
    };
    walk.directory(root, &mut Vec::new())?;
    check_case(&walk.files)?;
    Ok(finish(walk.files))
}

struct Walk<'a> {
    limits: &'a TreeLimits,
    files: BTreeMap<String, Digest>,
    bytes: u64,
}

impl Walk<'_> {
    fn directory(&mut self, path: &Path, components: &mut Vec<Vec<u8>>) -> Result<(), DigestError> {
        let io = |error: io::Error| refused(DigestErrorKind::Io, error.to_string());
        let mut entries = fs::read_dir(path)
            .map_err(io)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(io)?;
        entries.sort_by_key(fs::DirEntry::file_name);
        for entry in entries {
            components.push(os_bytes(&entry.file_name()));
            let result = self.entry(&entry.path(), components);
            components.pop();
            result?;
        }
        Ok(())
    }

    fn entry(&mut self, path: &Path, components: &mut Vec<Vec<u8>>) -> Result<(), DigestError> {
        let metadata = fs::symlink_metadata(path)
            .map_err(|error| refused(DigestErrorKind::Io, error.to_string()))?;
        let directory = metadata.is_dir() && !is_reparse_point(&metadata);
        let borrowed: Vec<&[u8]> = components.iter().map(Vec::as_slice).collect();
        if excluded(&borrowed, directory) {
            return Ok(());
        }
        if metadata.file_type().is_symlink() || is_reparse_point(&metadata) {
            return Err(refused(DigestErrorKind::Symlink, shown(&borrowed)));
        }
        if directory {
            return self.directory(path, components);
        }
        if !metadata.is_file() {
            return Err(refused(DigestErrorKind::SpecialFile, shown(&borrowed)));
        }
        let member = member_path(&borrowed)?;
        if !self.limits.files.allows(self.files.len() + 1) {
            return Err(refused(
                DigestErrorKind::TooLarge,
                format!("more than {} files", self.limits.files),
            ));
        }
        let mut bytes = Vec::new();
        let most = self
            .limits
            .file_bytes
            .get()
            .map_or(u64::MAX, |most| most.saturating_add(1));
        fs::File::open(path)
            .and_then(|file| file.take(most).read_to_end(&mut bytes))
            .map_err(|error| refused(DigestErrorKind::Io, format!("{member}: {error}")))?;
        if !self.limits.file_bytes.allows(bytes.len()) {
            return Err(refused(
                DigestErrorKind::TooLarge,
                format!("{member} is larger than {} bytes", self.limits.file_bytes),
            ));
        }
        self.bytes = self.bytes.saturating_add(bytes.len() as u64);
        if !self.limits.bytes.allows(self.bytes) {
            return Err(refused(
                DigestErrorKind::TooLarge,
                format!("more than {} bytes", self.limits.bytes),
            ));
        }
        self.files.insert(member, Digest::of(&bytes));
        Ok(())
    }
}

#[cfg(windows)]
fn is_reparse_point(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    // FILE_ATTRIBUTE_REPARSE_POINT: symbolic links and junctions alike.
    metadata.file_attributes() & 0x400 != 0
}

#[cfg(not(windows))]
fn is_reparse_point(_metadata: &fs::Metadata) -> bool {
    false
}

#[cfg(unix)]
fn os_bytes(name: &std::ffi::OsStr) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    name.as_bytes().to_vec()
}

#[cfg(not(unix))]
fn os_bytes(name: &std::ffi::OsStr) -> Vec<u8> {
    // A name that is not Unicode keeps a byte no UTF-8 has, so it is refused as such.
    name.to_str()
        .map_or_else(|| vec![0xff], |name| name.as_bytes().to_vec())
}
