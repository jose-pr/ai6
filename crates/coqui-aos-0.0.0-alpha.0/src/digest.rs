//! The tree digest an overlay is pinned by (`spec/v0/integrity.md`): one line per member,
//! `path NUL sha256:<hex> LF`, sorted by path bytes, and the SHA-256 of those lines.

use alloc::collections::BTreeMap;
use core::fmt;
use std::path::Path;

use sha2::{Digest as _, Sha256};

use crate::defaults::OverlayTree;
use crate::error::{Class, Error};
use crate::fs::{FileKind, FileSystem, name_bytes};
use crate::limits::Settings;
use crate::names::{EXCLUDED_COMPONENTS, valid_relative_path};

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
    pub fn parse(text: &str) -> Result<Self, Error> {
        let hex = text
            .strip_prefix("sha256:")
            .filter(|hex| hex.len() == 64 && hex.bytes().all(|byte| byte.is_ascii_hexdigit()))
            .ok_or_else(|| {
                Error::new(
                    Class::InvalidDigest,
                    "a digest is sha256: and 64 hexadecimal digits",
                )
            })?;
        let mut bytes = [0u8; 32];
        for (index, byte) in bytes.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&hex[index * 2..index * 2 + 2], 16).map_err(|_| {
                Error::new(
                    Class::InvalidDigest,
                    "a digest is sha256: and 64 hexadecimal digits",
                )
            })?;
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

impl core::str::FromStr for Digest {
    type Err = Error;

    fn from_str(text: &str) -> Result<Self, Error> {
        Self::parse(text)
    }
}

fn refused(class: Class, path: impl Into<String>) -> Error {
    Error::new(class, path)
}

/// A `too-large` refusal of the limit `setting`.
fn over(setting: &'static str, subject: impl Into<String>) -> Error {
    Error::new(Class::TooLarge, subject).over(setting)
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
#[non_exhaustive]
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
/// A [`Error`] for a link, a special file, a name that is not UTF-8 or not a relative
/// path, or two members equal under ASCII case, unless the entry is excluded.
pub fn digest_entries<'a, I>(entries: I) -> Result<TreeDigest, Error>
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
                return Err(refused(Class::Symlink, shown(&components)));
            }
            _ => return Err(refused(Class::SpecialFile, shown(&components))),
        };
        let path = member_path(&components)?;
        if files.insert(path.clone(), Digest::of(bytes)).is_some() {
            return Err(refused(Class::DuplicateMember, path));
        }
    }
    check_case(&files)?;
    Ok(finish(files))
}

/// The path of a member from its components, refused when it is not a relative path.
fn member_path(components: &[&[u8]]) -> Result<String, Error> {
    let mut names = Vec::with_capacity(components.len());
    for component in components {
        names.push(
            core::str::from_utf8(component)
                .map_err(|_| refused(Class::NonUtf8Name, shown(components)))?,
        );
    }
    let path = names.join("/");
    if !valid_relative_path(&path) {
        return Err(refused(Class::InvalidName, shown(components)));
    }
    Ok(path)
}

fn check_case(files: &BTreeMap<String, Digest>) -> Result<(), Error> {
    let mut folded = BTreeMap::new();
    for path in files.keys() {
        if let Some(other) = folded.insert(path.to_ascii_lowercase(), path)
            && other != path
        {
            return Err(refused(Class::CaseCollision, path.clone()));
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

/// The digest of the directory `root` on `files`, read without following any link, within the
/// limits `settings` holds.
///
/// # Errors
///
/// An [`Error`] as [`digest_entries`] refuses, `io` when an entry cannot be read, and `too-large`
/// naming the limit passed.
pub fn tree_digest(
    files: &dyn FileSystem,
    root: &Path,
    settings: &Settings,
) -> Result<TreeDigest, Error> {
    Ok(walk_tree(files, root, settings, false)?.0)
}

/// The tree under `root` on `files`, read whole: every member's bytes, held in an
/// [`OverlayTree`], and the digest of the same walk. A link is never followed, and the limits of
/// `settings` bound what is read. Holds up to `tree_bytes` in memory.
///
/// # Errors
///
/// As [`tree_digest`].
pub fn read_tree(
    files: &dyn FileSystem,
    root: &Path,
    settings: &Settings,
) -> Result<(OverlayTree, TreeDigest), Error> {
    let (digest, contents) = walk_tree(files, root, settings, true)?;
    let mut tree = OverlayTree::from_paths(digest.files.keys().cloned());
    for (path, bytes) in contents {
        tree = tree.with_bytes(&path, bytes);
    }
    Ok((tree, digest))
}

fn walk_tree(
    files: &dyn FileSystem,
    root: &Path,
    settings: &Settings,
    keep: bool,
) -> Result<(TreeDigest, BTreeMap<String, Vec<u8>>), Error> {
    let metadata = files
        .metadata(root)
        .map_err(|_| refused(Class::Io, "the filesystem"))?;
    if metadata.kind != FileKind::Directory {
        return Err(refused(
            Class::Io,
            "the root is not a directory (a link is never followed)",
        ));
    }
    let mut walk = Walk {
        files,
        settings,
        found: BTreeMap::new(),
        contents: keep.then(BTreeMap::new),
        bytes: 0,
        entries: 0,
    };
    walk.directory(root, &mut Vec::new(), 1)?;
    check_case(&walk.found)?;
    Ok((finish(walk.found), walk.contents.unwrap_or_default()))
}

struct Walk<'a> {
    files: &'a dyn FileSystem,
    settings: &'a Settings,
    found: BTreeMap<String, Digest>,
    contents: Option<BTreeMap<String, Vec<u8>>>,
    bytes: u64,
    entries: usize,
}

impl Walk<'_> {
    fn directory(
        &mut self,
        path: &Path,
        components: &mut Vec<Vec<u8>>,
        depth: usize,
    ) -> Result<(), Error> {
        if !self.settings.tree_depth.allows(depth) {
            return Err(over(
                "tree_depth",
                format!(
                    "directories nested deeper than {}",
                    self.settings.tree_depth
                ),
            ));
        }
        let mut names = self
            .files
            .list(path)
            .map_err(|_| refused(Class::Io, "the filesystem"))?;
        names.sort_by_key(|name| name_bytes(name));
        for name in names {
            let bytes = name_bytes(&name);
            if bytes.is_empty()
                || bytes == b"."
                || bytes == b".."
                || bytes.iter().any(|byte| matches!(byte, b'/' | b'\\' | 0))
            {
                return Err(refused(Class::InvalidName, shown(&[bytes.as_slice()])));
            }
            components.push(bytes);
            let result = self.entry(&path.join(&name), components, depth);
            components.pop();
            result?;
        }
        Ok(())
    }

    fn entry(
        &mut self,
        path: &Path,
        components: &mut Vec<Vec<u8>>,
        depth: usize,
    ) -> Result<(), Error> {
        let metadata = self
            .files
            .metadata(path)
            .map_err(|_| refused(Class::Io, "the filesystem"))?;
        let directory = metadata.kind == FileKind::Directory;
        let borrowed: Vec<&[u8]> = components.iter().map(Vec::as_slice).collect();
        if excluded(&borrowed, directory) {
            return Ok(());
        }
        match metadata.kind {
            FileKind::Symlink => return Err(refused(Class::Symlink, shown(&borrowed))),
            FileKind::Directory => {
                self.entries += 1;
                if !self.settings.tree_files.allows(self.entries) {
                    return Err(over(
                        "tree_files",
                        format!(
                            "more than {} files and directories",
                            self.settings.tree_files
                        ),
                    ));
                }
                return self.directory(path, components, depth + 1);
            }
            FileKind::File => {}
            _ => return Err(refused(Class::SpecialFile, shown(&borrowed))),
        }
        let member = member_path(&borrowed)?;
        self.entries += 1;
        if !self.settings.tree_files.allows(self.entries) {
            return Err(over(
                "tree_files",
                format!(
                    "more than {} files and directories",
                    self.settings.tree_files
                ),
            ));
        }
        let most = self
            .settings
            .file_bytes
            .get()
            .map_or(u64::MAX, |most| most.saturating_add(1));
        let bytes = self
            .files
            .read(path, most)
            .map_err(|_| refused(Class::Io, member.as_str()))?;
        if !self.settings.file_bytes.allows(bytes.len()) {
            return Err(over(
                "file_bytes",
                format!("{member} is larger than {} bytes", self.settings.file_bytes),
            ));
        }
        self.bytes = self.bytes.saturating_add(bytes.len() as u64);
        if !self.settings.tree_bytes.allows(self.bytes) {
            return Err(over(
                "tree_bytes",
                format!("more than {} bytes", self.settings.tree_bytes),
            ));
        }
        self.found.insert(member.clone(), Digest::of(&bytes));
        if let Some(contents) = &mut self.contents {
            contents.insert(member, bytes);
        }
        Ok(())
    }
}

/// Refuses entries past the tree limits of `settings` before any is hashed: for trees a caller
/// hands over rather than the core walks.
///
/// # Errors
///
/// `too-large` naming the limit passed.
pub(crate) fn check_entries(
    entries: &[(Vec<&[u8]>, TreeEntry)],
    settings: &Settings,
) -> Result<(), Error> {
    if !settings.tree_files.allows(entries.len()) {
        return Err(over(
            "tree_files",
            format!("more than {} files and directories", settings.tree_files),
        ));
    }
    let mut total = 0u64;
    for (_, entry) in entries {
        if let TreeEntry::File(bytes) = entry {
            if !settings.file_bytes.allows(bytes.len()) {
                return Err(over(
                    "file_bytes",
                    format!("a file larger than {} bytes", settings.file_bytes),
                ));
            }
            total = total.saturating_add(bytes.len() as u64);
        }
    }
    if !settings.tree_bytes.allows(total) {
        return Err(over(
            "tree_bytes",
            format!("more than {} bytes", settings.tree_bytes),
        ));
    }
    Ok(())
}
