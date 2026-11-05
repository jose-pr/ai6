//! Delivering overlays to a Session: the store a Station fetches them to, keyed by tree digest and
//! verified at every start, and what a Session is given from it. Nothing an overlay holds is ever
//! in the safe house, so no commit can hold it.
//!
//! Every call here blocks on the filesystem or on a program; call it from `spawn_blocking`.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use guatiao::Bytes;
use serde_json::Value as Data;

use crate::directorates::q::overlays::{
    Digest, Document, Environment, Facts, IssuedDigest, IssuedOverlay, MANIFEST, OverlayFrom,
    OverlayRecord, ReadOverlay, Source, SourceKind, TreeEntry, Weakening, composed_environment,
    context_of, default_on, digest_entries, effective, env_files_of, issue_overlays,
    normalize_name, read_overlay, root_variable, rules_of, skill_description, skills_of,
    tree_digest,
};
use crate::directorates::q::transport::kit::{ProcessInput, run_process};
use crate::directorates::section_00::agent::{OverlaySkill, SessionOverlays};
use crate::station::limits::OverlayLimits;

/// A weakening a delivery reports, naming its overlay.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Reported {
    pub(crate) overlay: String,
    pub(crate) weakening: Weakening,
}

/// What a Session is issued, checked and stored, before anything is placed where it runs.
pub(crate) struct Prepared {
    overlays: Vec<Ready>,
    limits: OverlayLimits,
    store: Store,
}

struct Ready {
    issued: IssuedOverlay,
    stored: Stored,
}

/// What a Session gets from its overlays.
#[derive(Debug, Default)]
pub(crate) struct Delivered {
    /// The Session's environment with the overlays' applied.
    pub(crate) environment: BTreeMap<String, String>,
    /// The rules, routing and context, as one piece of the initial context.
    pub(crate) context: Option<String>,
    /// What the harness is handed.
    pub(crate) overlays: SessionOverlays,
    /// The digests delivered, in composition order.
    pub(crate) digests: Vec<IssuedDigest>,
    /// What is reported each time they are issued.
    pub(crate) weakenings: Vec<Reported>,
}

/// One tree in the store.
struct Stored {
    digest: Digest,
    root: PathBuf,
    read: ReadOverlay,
}

/// The directory of a Station that holds overlays by digest, and how much of one it takes.
#[derive(Clone)]
pub(crate) struct Store {
    dir: PathBuf,
    base: Option<PathBuf>,
    limits: OverlayLimits,
}

/// Reads a `[secret.<name>]` for fetching a private source.
pub(crate) type SecretRead<'a> = &'a dyn Fn(&str) -> Result<zeroize::Zeroizing<String>, String>;

fn hex_of(digest: &Digest) -> String {
    digest.to_string()["sha256:".len()..].to_owned()
}

impl Store {
    /// The store under the Station's state directory; `base` is what a relative `path` in an
    /// `[overlay.<name>]` is read against.
    pub(crate) fn new(state_dir: &Path, base: Option<&Path>, limits: &OverlayLimits) -> Self {
        Self {
            dir: state_dir.join("overlays"),
            base: base.map(Path::to_path_buf),
            limits: limits.clone(),
        }
    }

    fn trees(&self) -> PathBuf {
        self.dir.join("trees")
    }

    fn tree_path(&self, digest: &Digest) -> PathBuf {
        self.trees().join(hex_of(digest))
    }

    /// The copy of `digest` in the store, read again: anything else in its place refuses, since
    /// what was verified when it was fetched has been changed since.
    fn verified(&self, identity: &str, digest: &Digest) -> Result<Stored, String> {
        let root = self.tree_path(digest);
        let tampered = |why: String| {
            format!(
                "overlay {identity:?}: the store's copy at {} {why}; it changed after it was \
                 fetched, so nothing issues it. Delete that directory to fetch it again",
                root.display()
            )
        };
        let read = read_overlay(&root, &self.limits).map_err(|why| tampered(why))?;
        if read.digest.digest != *digest {
            return Err(tampered(format!(
                "has the digest {}, not {digest}",
                read.digest.digest
            )));
        }
        Ok(Stored {
            digest: *digest,
            root,
            read,
        })
    }

    /// Fetches `record` unless the store holds its pin, and verifies what it holds. An overlay with
    /// no pin is read from its source every time, since nothing else says which bytes it means.
    fn fetch(
        &self,
        identity: &str,
        record: &OverlayRecord,
        secret: SecretRead<'_>,
    ) -> Result<Stored, String> {
        if let Some(pin) = record.pin()
            && self.tree_path(&pin).exists()
        {
            return self.verified(identity, &pin);
        }
        let fetched = self.materialize(identity, record, secret)?;
        let source = fetched.root();
        let read = read_overlay(source, &self.limits)
            .map_err(|why| format!("overlay {identity:?} cannot be read: {why}"))?;
        let digest = read.digest.digest;
        if let Some(pin) = record.pin()
            && pin != digest
        {
            return Err(format!(
                "overlay {identity:?}: its source has the digest {digest}, not its pin {pin}. \
                 Nothing overrides a pin: to accept these bytes, change it"
            ));
        }
        if !self.tree_path(&digest).exists() {
            self.install(source, &read, &digest)
                .map_err(|why| format!("overlay {identity:?} cannot be stored: {why}"))?;
        }
        self.verified(identity, &digest)
    }

    /// Where `record`'s bytes are, on this host.
    fn materialize(
        &self,
        identity: &str,
        record: &OverlayRecord,
        secret: SecretRead<'_>,
    ) -> Result<Fetched, String> {
        match record.from() {
            OverlayFrom::Path(path) => self.local_path(identity, path, None),
            OverlayFrom::Source(source) => match source.kind {
                SourceKind::Directory => self.local_path(
                    identity,
                    Path::new(&source.location),
                    source.path.as_deref(),
                ),
                SourceKind::Git => self.clone_source(identity, source, record, secret),
                SourceKind::Url => Err(format!(
                    "overlay {identity:?}: {} is a plain URL, which a Station does not fetch; name a \
                     Git repository or a directory",
                    source.shown()
                )),
            },
        }
    }

    fn local_path(
        &self,
        identity: &str,
        path: &Path,
        inner: Option<&str>,
    ) -> Result<Fetched, String> {
        let absolute = if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.base
                .as_ref()
                .map(|base| base.join(path))
                .ok_or_else(|| {
                    format!(
                        "overlay {identity:?}: {} is relative and this Station has no \
                         configuration directory",
                        path.display()
                    )
                })?
        };
        Ok(Fetched::Directory(match inner {
            Some(inner) => absolute.join(inner),
            None => absolute,
        }))
    }

    /// The blobs of `source` at its ref, written out with no checkout filter: a digest is over the
    /// bytes as published (`spec/v0/integrity.md`), and a link or submodule in the tree refuses.
    fn clone_source(
        &self,
        identity: &str,
        source: &Source,
        record: &OverlayRecord,
        secret: SecretRead<'_>,
    ) -> Result<Fetched, String> {
        let refuse = |why: String| format!("overlay {identity:?} from {}: {why}", source.shown());
        let scratch = self.scratch().map_err(|why| refuse(why.to_string()))?;
        let git = Git::new(self.limits.fetch_timeout.0, source, record, secret).map_err(refuse)?;
        let repository = scratch.path().join("repository");
        let tree = scratch.path().join("tree");
        fs::create_dir_all(&tree).map_err(|why| refuse(why.to_string()))?;
        let out = git
            .fetch(&repository, source)
            .and_then(|commit| {
                let spec = match &source.path {
                    Some(path) => format!("{commit}:{path}"),
                    None => format!("{commit}^{{tree}}"),
                };
                git.write_blobs(&repository, &spec, &tree, &self.limits)
            })
            .map_err(refuse);
        out.map(|()| Fetched::Scratch {
            _held: scratch,
            tree,
        })
    }

    fn scratch(&self) -> io::Result<Scratch> {
        Scratch::new(&self.dir)
    }

    /// Copies `read`'s files from `source` into the store under `digest`, read-only, appearing
    /// there only when whole.
    fn install(&self, source: &Path, read: &ReadOverlay, digest: &Digest) -> Result<(), String> {
        let scratch = self.scratch().map_err(|why| why.to_string())?;
        let staged = scratch.path().join("tree");
        for path in read.digest.files.keys() {
            let target = staged.join(path);
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent).map_err(|why| why.to_string())?;
            }
            fs::copy(source.join(path), &target).map_err(|why| format!("{path}: {why}"))?;
        }
        self.place(&staged, &self.tree_path(digest))
    }

    /// Moves the finished tree `staged` to `target` and makes it read-only. A tree another start
    /// placed first is as good as this one, since both have the digest in their name.
    fn place(&self, staged: &Path, target: &Path) -> Result<(), String> {
        let parent = target.parent().ok_or("no parent")?;
        fs::create_dir_all(parent).map_err(|why| why.to_string())?;
        match fs::rename(staged, target) {
            Ok(()) => protect(target)
                .map_err(|why| format!("{} could not be made read-only: {why}", target.display())),
            Err(_) if target.is_dir() => Ok(()),
            Err(why) => Err(format!("{}: {why}", target.display())),
        }
    }

    /// A read-only directory laying out `skills` as a harness plugin, kept by the digest of what it
    /// holds, so Sessions given the same skills share one.
    fn plugin(&self, skills: &[(String, PathBuf)]) -> Result<PathBuf, String> {
        let mut files: BTreeMap<String, Vec<u8>> = BTreeMap::new();
        files.insert(
            ".claude-plugin/plugin.json".to_owned(),
            serde_json::json!({
                "name": crate::directorates::m::assistant::SKILLS_PLUGIN,
                "description": "The skills of the overlays AI6 issued to this Session",
            })
            .to_string()
            .into_bytes(),
        );
        for (name, dir) in skills {
            collect_files(dir, &format!("skills/{name}"), &mut files)
                .map_err(|why| format!("skill {name:?}: {why}"))?;
        }
        let entries: Vec<(Vec<Vec<u8>>, TreeEntry)> = files
            .iter()
            .map(|(path, bytes)| {
                (
                    path.split('/')
                        .map(|part| part.as_bytes().to_vec())
                        .collect(),
                    TreeEntry::File(bytes.clone()),
                )
            })
            .collect();
        let digest =
            digest_entries(entries.iter().map(|(parts, entry)| {
                (parts.iter().map(Vec::as_slice).collect::<Vec<_>>(), entry)
            }))
            .map_err(|why| why.to_string())?;
        let target = self.dir.join("plugins").join(hex_of(&digest.digest));
        if target.is_dir() {
            let found = tree_digest(&target, &self.limits.tree()).map_err(|why| why.to_string())?;
            if found.digest != digest.digest {
                return Err(format!(
                    "the plugin at {} changed after it was made. Delete that directory to make it \
                     again",
                    target.display()
                ));
            }
            return Ok(target);
        }
        let scratch = self.scratch().map_err(|why| why.to_string())?;
        let staged = scratch.path().join("tree");
        for (path, bytes) in &files {
            let file = staged.join(path);
            if let Some(parent) = file.parent() {
                fs::create_dir_all(parent).map_err(|why| why.to_string())?;
            }
            fs::write(&file, bytes).map_err(|why| why.to_string())?;
        }
        self.place(&staged, &target)?;
        Ok(target)
    }
}

/// Every regular file under `dir`, by `prefix/<relative path>`, the excluded names left out as a
/// tree digest leaves them.
fn collect_files(
    dir: &Path,
    prefix: &str,
    files: &mut BTreeMap<String, Vec<u8>>,
) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = format!("{prefix}/{name}");
        if !crate::directorates::q::overlays::valid_relative_path(&path) {
            continue;
        }
        if entry.file_type()?.is_dir() {
            collect_files(&entry.path(), &path, files)?;
        } else {
            files.insert(path, fs::read(entry.path())?);
        }
    }
    Ok(())
}

/// A directory under the store removed when dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new(store: &Path) -> io::Result<Self> {
        let path = store
            .join("scratch")
            .join(uuid::Uuid::new_v4().simple().to_string());
        fs::create_dir_all(&path)?;
        Ok(Self(path))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = release(&self.0);
    }
}

/// Where a fetched overlay's bytes are until they are stored.
enum Fetched {
    Directory(PathBuf),
    Scratch {
        /// Held so its directory lasts as long as this does.
        _held: Scratch,
        tree: PathBuf,
    },
}

impl Fetched {
    fn root(&self) -> &Path {
        match self {
            Self::Directory(tree) | Self::Scratch { tree, .. } => tree,
        }
    }
}

/// `git` for one source, with what it reads of the Station kept to what it needs.
struct Git {
    timeout: Duration,
    environment: BTreeMap<String, String>,
}

const GIT_OUTPUT_BYTES: u64 = 1024 * 1024;

impl Git {
    fn new(
        timeout: Duration,
        source: &Source,
        record: &OverlayRecord,
        secret: SecretRead<'_>,
    ) -> Result<Self, String> {
        let mut environment: BTreeMap<String, String> = std::env::vars()
            .filter(|(name, _)| {
                [
                    "PATH",
                    "HOME",
                    "USERPROFILE",
                    "SYSTEMROOT",
                    "TEMP",
                    "TMP",
                    "TMPDIR",
                    "SSH_AUTH_SOCK",
                    "LANG",
                ]
                .iter()
                .any(|kept| name.eq_ignore_ascii_case(kept))
            })
            .collect();
        environment.insert("GIT_TERMINAL_PROMPT".to_owned(), "0".to_owned());
        environment.insert(
            "GIT_ALLOW_PROTOCOL".to_owned(),
            "https:http:ssh:git:file".to_owned(),
        );
        if let Some(name) = record.secrets().first() {
            let http = ["http://", "https://"]
                .iter()
                .any(|scheme| source.location.starts_with(scheme));
            if !http {
                return Err(format!(
                    "secret {name:?} is passed over HTTPS, which this location does not use"
                ));
            }
            use base64::Engine;
            let token = secret(name)?;
            let basic = base64::engine::general_purpose::STANDARD
                .encode(format!("x-access-token:{}", token.as_str()));
            // In the environment, so it is on no command line.
            environment.insert("GIT_CONFIG_COUNT".to_owned(), "1".to_owned());
            environment.insert("GIT_CONFIG_KEY_0".to_owned(), "http.extraHeader".to_owned());
            environment.insert(
                "GIT_CONFIG_VALUE_0".to_owned(),
                format!("Authorization: Basic {basic}"),
            );
        }
        Ok(Self {
            timeout,
            environment,
        })
    }

    fn run(
        &self,
        repository: &Path,
        arguments: &[&str],
        stdin: Option<&[u8]>,
        max_output: u64,
    ) -> Result<Vec<u8>, String> {
        let mut command = Command::new("git");
        command
            .env_clear()
            .envs(&self.environment)
            .arg("-C")
            .arg(repository)
            .args(arguments);
        let output = run_process(
            &mut command,
            &ProcessInput {
                stdin,
                timeout_ms: u64::try_from(self.timeout.as_millis()).unwrap_or(u64::MAX),
                max_output,
            },
            Duration::from_millis(1000),
        )
        .map_err(|error| crate::directorates::q::transport::describe(&error))?;
        if output.timed_out {
            return Err(format!(
                "git did not finish within {} seconds (station.overlays.fetch_timeout)",
                self.timeout.as_secs()
            ));
        }
        if output.truncated {
            return Err("git's output passed what an overlay may hold".to_owned());
        }
        if output.code != Some(0) {
            return Err(format!(
                "git {} failed: {}",
                arguments.first().unwrap_or(&""),
                String::from_utf8_lossy(&output.stderr.0)
                    .trim()
                    .lines()
                    .last()
                    .unwrap_or("no message")
            ));
        }
        let Bytes(bytes) = output.stdout;
        Ok(bytes)
    }

    /// Fetches `source`'s ref into a new bare repository and names its commit. A server that will
    /// not hand out one commit by name gets a whole fetch.
    fn fetch(&self, repository: &Path, source: &Source) -> Result<String, String> {
        fs::create_dir_all(repository).map_err(|why| why.to_string())?;
        self.run(
            repository,
            &["init", "--quiet", "--bare"],
            None,
            GIT_OUTPUT_BYTES,
        )?;
        let wanted = source.reference.as_deref().unwrap_or("HEAD");
        let shallow = self.run(
            repository,
            &[
                "fetch",
                "--quiet",
                "--depth=1",
                "--no-tags",
                "--",
                &source.location,
                wanted,
            ],
            None,
            GIT_OUTPUT_BYTES,
        );
        let commit = match shallow {
            Ok(_) => "FETCH_HEAD".to_owned(),
            Err(_) => {
                self.run(
                    repository,
                    &[
                        "fetch",
                        "--quiet",
                        "--",
                        &source.location,
                        "+refs/*:refs/fetched/*",
                    ],
                    None,
                    GIT_OUTPUT_BYTES,
                )?;
                wanted.to_owned()
            }
        };
        let resolved = self.run(
            repository,
            &["rev-parse", "--verify", &format!("{commit}^{{commit}}")],
            None,
            GIT_OUTPUT_BYTES,
        )?;
        Ok(String::from_utf8_lossy(&resolved).trim().to_owned())
    }

    /// Writes every blob under the tree `spec` names into `target`, refusing a link or submodule.
    fn write_blobs(
        &self,
        repository: &Path,
        spec: &str,
        target: &Path,
        limits: &OverlayLimits,
    ) -> Result<(), String> {
        let listing = self.run(
            repository,
            &["ls-tree", "-r", "-z", spec],
            None,
            GIT_OUTPUT_BYTES.max((limits.files.or_max() as u64).saturating_mul(4096)),
        )?;
        let mut names = Vec::new();
        let mut wanted = String::new();
        for entry in listing.split(|byte| *byte == 0).filter(|e| !e.is_empty()) {
            let entry = String::from_utf8(entry.to_vec())
                .map_err(|_| "a path that is not UTF-8".to_owned())?;
            let (meta, path) = entry.split_once('\t').ok_or("an unreadable tree entry")?;
            let fields: Vec<&str> = meta.split(' ').collect();
            let [mode, kind, object] = fields.as_slice() else {
                return Err("an unreadable tree entry".to_owned());
            };
            if *mode == "120000" {
                return Err(format!("symlink: {path}"));
            }
            if *kind != "blob" {
                return Err(format!("submodule: {path}"));
            }
            if !limits.files.allows(names.len() as u64 + 1) {
                return Err("too many files (station.overlays.files)".to_owned());
            }
            names.push(path.to_owned());
            let _ = writeln!(wanted, "{object}");
        }
        // Each object's header is less than this; the limit is the overlay's own.
        let room = (limits.bytes.or_max() as u64)
            .saturating_add((names.len() as u64 + 1).saturating_mul(128));
        let batch = self.run(
            repository,
            &["cat-file", "--batch"],
            Some(wanted.as_bytes()),
            room,
        )?;
        let mut rest = batch.as_slice();
        for name in names {
            let newline = rest
                .iter()
                .position(|byte| *byte == b'\n')
                .ok_or("git's batch ended early")?;
            let header = String::from_utf8_lossy(&rest[..newline]).into_owned();
            let size: usize = header
                .rsplit(' ')
                .next()
                .and_then(|size| size.parse().ok())
                .ok_or("git's batch has an unreadable header")?;
            let start = newline + 1;
            let end = start + size;
            let bytes = rest.get(start..end).ok_or("git's batch ended early")?;
            let file = target.join(&name);
            // A path git holds that leaves the tree is refused by the digest of what is left.
            if !crate::directorates::q::overlays::valid_relative_path(&name) {
                rest = rest.get(end + 1..).unwrap_or_default();
                continue;
            }
            if let Some(parent) = file.parent() {
                fs::create_dir_all(parent).map_err(|why| why.to_string())?;
            }
            fs::write(&file, bytes).map_err(|why| format!("{name}: {why}"))?;
            rest = rest.get(end + 1..).unwrap_or_default();
        }
        Ok(())
    }
}

/// Makes `root` read-only for this user, the owner included, so a process of the Session cannot
/// write it without first changing the permissions it was given.
#[cfg(unix)]
pub(crate) fn protect(root: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            protect(&path)?;
        } else {
            let mode = fs::metadata(&path)?.permissions().mode();
            let executable = mode & 0o111 != 0 || runs_by_itself(root, &path);
            fs::set_permissions(
                &path,
                fs::Permissions::from_mode(if executable { 0o555 } else { 0o444 }),
            )?;
        }
    }
    fs::set_permissions(root, fs::Permissions::from_mode(0o555))
}

/// Whether a file should be executable though its source could not say (a fetch through Git's
/// blobs, a copy from Windows): it opens with `#!`.
#[cfg(unix)]
fn runs_by_itself(_root: &Path, path: &Path) -> bool {
    use std::io::Read as _;
    let mut head = [0u8; 2];
    fs::File::open(path)
        .and_then(|mut file| file.read_exact(&mut head))
        .is_ok()
        && &head == b"#!"
}

/// Windows has no mode bits for a directory, so an inherited deny for everyone takes writing,
/// adding, changing attributes and deleting away from the tree: the read-only attribute alone
/// leaves new files possible.
#[cfg(windows)]
pub(crate) fn protect(root: &Path) -> io::Result<()> {
    let status = icacls()
        .arg(root)
        .args(["/deny", "*S-1-1-0:(OI)(CI)(WD,AD,WEA,WA,DC,DE)", "/Q"])
        .output()?;
    if status.status.success() {
        Ok(())
    } else {
        Err(io::Error::other(
            String::from_utf8_lossy(&status.stdout).trim().to_owned(),
        ))
    }
}

#[cfg(windows)]
fn icacls() -> Command {
    let system = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
    Command::new(Path::new(&system).join("System32").join("icacls.exe"))
}

#[cfg(not(any(unix, windows)))]
pub(crate) fn protect(_root: &Path) -> io::Result<()> {
    Ok(())
}

/// Takes `protect` off `path` and removes it.
#[cfg(unix)]
pub(crate) fn release(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fn open(path: &Path) -> io::Result<()> {
        let metadata = fs::symlink_metadata(path)?;
        if metadata.is_dir() {
            fs::set_permissions(path, fs::Permissions::from_mode(0o755))?;
            for entry in fs::read_dir(path)? {
                open(&entry?.path())?;
            }
        }
        Ok(())
    }
    open(path)?;
    fs::remove_dir_all(path)
}

/// Takes `protect` off `path` and removes it.
#[cfg(windows)]
pub(crate) fn release(path: &Path) -> io::Result<()> {
    let _ = icacls()
        .arg(path)
        .args(["/reset", "/T", "/C", "/Q"])
        .output()?;
    // Git's object files are read-only, which stops a removal here.
    fn writable(path: &Path) -> io::Result<()> {
        let metadata = fs::symlink_metadata(path)?;
        if metadata.is_dir() {
            for entry in fs::read_dir(path)? {
                writable(&entry?.path())?;
            }
        } else {
            let mut permissions = metadata.permissions();
            #[allow(
                clippy::permissions_set_readonly_false,
                reason = "Windows has no modes"
            )]
            permissions.set_readonly(false);
            fs::set_permissions(path, permissions)?;
        }
        Ok(())
    }
    writable(path)?;
    fs::remove_dir_all(path)
}

#[cfg(not(any(unix, windows)))]
pub(crate) fn release(path: &Path) -> io::Result<()> {
    fs::remove_dir_all(path)
}

/// What a Station asks for one Session.
pub(crate) struct Request<'a> {
    /// The loadout's list.
    pub(crate) loadout: &'a [String],
    /// The Operation's list.
    pub(crate) operation: &'a [String],
    /// The `[overlay.<name>]` records, by identity.
    pub(crate) records: &'a BTreeMap<String, OverlayRecord>,
    /// `[overlays]`.
    pub(crate) limits: &'a OverlayLimits,
    /// The Station's state directory.
    pub(crate) state_dir: &'a Path,
    /// What a relative `path` is read against.
    pub(crate) base: Option<&'a Path>,
}

/// What `request` issues, each overlay fetched and verified and every skill checked, or why it
/// cannot be issued. `None` when there is nothing to issue: the Session is as it was.
pub(crate) fn prepare(
    request: &Request<'_>,
    secret: SecretRead<'_>,
) -> Result<Option<Prepared>, String> {
    if request.loadout.is_empty() && request.operation.is_empty() {
        return Ok(None);
    }
    let store = Store::new(request.state_dir, request.base, request.limits);
    let mut stored: BTreeMap<String, Stored> = BTreeMap::new();
    let mut pending: Vec<String> = request
        .loadout
        .iter()
        .chain(request.operation)
        .map(|name| normalize_name(name))
        .collect();
    while let Some(identity) = pending.pop() {
        if stored.contains_key(&identity) {
            continue;
        }
        // A name no record has is `issue_overlays`' to refuse, with who asked for it.
        let Some(record) = request.records.get(&identity) else {
            continue;
        };
        if record.pin().is_none() && !request.limits.unpinned {
            continue;
        }
        let found = store.fetch(&identity, record, secret)?;
        for required in found
            .read
            .manifest
            .get("requires")
            .and_then(Data::as_array)
            .map_or(&[][..], Vec::as_slice)
        {
            if let Some(name) = required.as_str() {
                pending.push(normalize_name(name));
            }
        }
        stored.insert(identity, found);
    }
    let manifests: BTreeMap<String, Document> = request
        .records
        .keys()
        .map(|identity| {
            (
                identity.clone(),
                stored
                    .get(identity)
                    .map_or_else(Document::new, |found| found.read.manifest.clone()),
            )
        })
        .collect();
    let issued = issue_overlays(
        request.loadout,
        request.operation,
        request.records,
        &manifests,
        request.limits.unpinned,
    )
    .map_err(|refusal| refusal.to_string())?;
    let mut overlays = Vec::new();
    for issued in issued.overlays {
        let found = stored
            .remove(&issued.identity)
            .ok_or_else(|| format!("overlay {:?} was not fetched", issued.identity))?;
        check_usable(&issued.identity, &found.read)?;
        overlays.push(Ready {
            issued,
            stored: found,
        });
    }
    Ok(Some(Prepared {
        overlays,
        limits: request.limits.clone(),
        store,
    }))
}

/// Why the overlay may not be issued: its manifest does not apply or a skill fails its check.
fn check_usable(identity: &str, read: &ReadOverlay) -> Result<(), String> {
    if !read.usable() {
        if let Some(issue) = read
            .issues
            .iter()
            .find(|issue| !issue.at.starts_with("/about"))
        {
            return Err(format!(
                "overlay {identity:?}: {MANIFEST} {}: {}",
                issue.at, issue.code
            ));
        }
        if let Some((name, Err(why))) = read.skills.iter().find(|(_, check)| check.is_err()) {
            return Err(format!("overlay {identity:?}: skill {name:?}: {why}"));
        }
    }
    Ok(())
}

/// The harness id AOS hints are written for, from the handler provider's.
fn harness_id(handler: &str) -> &str {
    match handler {
        "claude" => "claude-code",
        other => other,
    }
}

/// What a Session is, as far as delivery cares.
pub(crate) struct SessionFacts<'a> {
    /// The Session's environment before its overlays'.
    pub(crate) environment: BTreeMap<String, String>,
    /// Whether its clearance runs commands: a `desk` Session gets the text and no `bin/`.
    pub(crate) commands: bool,
    /// The handler provider.
    pub(crate) handler: &'a str,
    /// The directory it works in, which holds nothing of an overlay.
    pub(crate) working_directory: &'a str,
}

impl Prepared {
    /// The digests issued, in composition order.
    pub(crate) fn digests(&self) -> Vec<IssuedDigest> {
        self.overlays
            .iter()
            .map(|ready| IssuedDigest {
                overlay: ready.issued.identity.clone(),
                digest: ready.stored.digest.to_string(),
            })
            .collect()
    }

    /// What a Session on this Station's own host gets: its overlays' read-only copies in the
    /// store, their environment applied, and their text.
    pub(crate) fn deliver_here(&self, session: SessionFacts<'_>) -> Result<Delivered, String> {
        let roots: BTreeMap<String, String> = self
            .overlays
            .iter()
            .map(|ready| {
                (
                    ready.issued.identity.clone(),
                    ready.stored.root.to_string_lossy().into_owned(),
                )
            })
            .collect();
        for root in roots.values() {
            check_outside(root, session.working_directory)?;
        }
        let os = crate::directorates::q::overlays::current_os();
        let mut weakenings = Vec::new();
        let environment = self.environment(&session, os, &roots, &mut weakenings)?;
        let context = self.context(&roots)?;
        let mut delivered = Delivered {
            environment,
            context,
            overlays: SessionOverlays {
                roots: roots.values().cloned().collect(),
                ..SessionOverlays::default()
            },
            digests: self.digests(),
            weakenings,
        };
        let skills = self.skills(session.handler, &roots, true, &mut delivered.weakenings);
        let native: Vec<(String, PathBuf)> = skills
            .iter()
            .filter(|skill| skill.native)
            .map(|skill| (skill.name.clone(), PathBuf::from(&skill.path)))
            .collect();
        if !native.is_empty() {
            let plugin = self.store.plugin(&native)?;
            let plugin = plugin.to_string_lossy().into_owned();
            check_outside(&plugin, session.working_directory)?;
            delivered.overlays.plugin_dir = Some(plugin);
        }
        delivered.overlays.skills = skills;
        Ok(delivered)
    }

    fn environment(
        &self,
        session: &SessionFacts<'_>,
        os: &str,
        roots: &BTreeMap<String, String>,
        weakenings: &mut Vec<Reported>,
    ) -> Result<BTreeMap<String, String>, String> {
        let windows = os == "windows";
        let mut base = session.environment.clone();
        // A harness has only the environment it is given: with no PATH there, an addition must
        // keep where such a process looks.
        if !base.keys().any(|name| name.eq_ignore_ascii_case("PATH")) {
            base.insert(
                "PATH".to_owned(),
                crate::directorates::section_00::programs::NO_PATH_SEARCH.to_owned(),
            );
        }
        let mut facts: Vec<(&Document, Facts)> = Vec::new();
        for ready in &self.overlays {
            let identity = &ready.issued.identity;
            let tree = self.tree_of(ready)?;
            let one = Facts {
                os: os.to_owned(),
                root: roots[identity].clone(),
                identity: identity.clone(),
                pinned: ready.issued.record.pin().is_some(),
                base_env: base.clone(),
                enforces_network: Some(false),
                env_scripts_enabled: Some(false),
                interpreters: Some(Vec::new()),
                tree,
                read_limits: self.limits.manifest(),
                ..Facts::default()
            };
            let manifest = &ready.issued.manifest;
            let applied = effective(manifest, &one)
                .map_err(|refusal| format!("overlay {identity:?}: {refusal}"))?;
            if !applied.supported {
                return Err(format!(
                    "overlay {identity:?} does not support {os} (its runtime.os)"
                ));
            }
            weakenings.extend(applied.weakenings.into_iter().map(|weakening| Reported {
                overlay: identity.clone(),
                weakening,
            }));
            facts.push((manifest, one));
        }
        let mut composed = Vec::new();
        let env = composed_environment(
            &facts,
            Environment::new(base.clone(), windows),
            &mut composed,
        )
        .map_err(|refusal| format!("the overlays' environment: {refusal}"))?;
        let mut environment: BTreeMap<String, String> = env.values().iter().cloned().collect();
        for (name, value) in &environment {
            let same = base.get(name) == Some(value);
            if !same && name.to_ascii_uppercase().starts_with("AI6_") {
                return Err(format!(
                    "an overlay sets {name}, which is the Station's to set"
                ));
            }
        }
        if !session.commands {
            // A Session that may not run commands gets nothing to run.
            let path = environment
                .keys()
                .find(|name| name.eq_ignore_ascii_case("PATH"))
                .cloned();
            if let Some(path) = path {
                let before = session
                    .environment
                    .iter()
                    .find(|(name, _)| name.eq_ignore_ascii_case("PATH"));
                match before {
                    Some((_, value)) => {
                        environment.insert(path, value.clone());
                    }
                    None => {
                        environment.remove(&path);
                    }
                }
            }
        } else if !session
            .environment
            .keys()
            .any(|name| name.eq_ignore_ascii_case("PATH"))
            && environment.get("PATH").map(String::as_str)
                == Some(crate::directorates::section_00::programs::NO_PATH_SEARCH)
        {
            environment.remove("PATH");
        }
        Ok(environment)
    }

    /// The overlay's files, with the text of what the environment reads.
    fn tree_of(
        &self,
        ready: &Ready,
    ) -> Result<crate::directorates::q::overlays::OverlayTree, String> {
        let mut tree = ready.stored.read.tree();
        let mut wanted: Vec<String> = env_files_of(&ready.issued.manifest, &tree)
            .iter()
            .filter_map(|entry| match entry {
                Data::String(path) => Some(path.clone()),
                Data::Object(map) => map.get("path").and_then(Data::as_str).map(str::to_owned),
                _ => None,
            })
            .collect();
        if default_on(&ready.issued.manifest, "deps") {
            wanted.push("lib/python/requirements.txt".to_owned());
        }
        for path in wanted {
            if !tree.contains(&path) {
                continue;
            }
            let text = fs::read_to_string(ready.stored.root.join(&path))
                .map_err(|why| format!("overlay {:?}: {path}: {why}", ready.issued.identity))?;
            tree = tree.with_text(&path, text);
        }
        Ok(tree)
    }

    /// The rules, routing and context of every overlay in composition order, each file marked, or
    /// the refusal naming `[overlays] context_bytes` when they do not fit.
    fn context(&self, roots: &BTreeMap<String, String>) -> Result<Option<String>, String> {
        let mut text = String::from(
            "An operator issued these overlays to this Session, so what they say is instruction. \
             Their files are read-only and outside your working directory, and stay out of it:\n",
        );
        for ready in &self.overlays {
            let identity = &ready.issued.identity;
            let _ = writeln!(
                text,
                "- `{identity}`: {} (`${}`)",
                roots[identity],
                root_variable(identity)
            );
        }
        for ready in &self.overlays {
            let identity = &ready.issued.identity;
            let manifest = &ready.issued.manifest;
            let tree = ready.stored.read.tree();
            let routing: Vec<&str> = manifest
                .get("routing")
                .and_then(Data::as_array)
                .map(|lines| lines.iter().filter_map(Data::as_str).collect())
                .unwrap_or_default();
            let files: Vec<String> = rules_of(manifest, &tree)
                .into_iter()
                .chain(context_of(manifest, &tree))
                .collect();
            if routing.is_empty() && files.is_empty() {
                continue;
            }
            let _ = writeln!(text, "\n# Overlay `{identity}`");
            if !routing.is_empty() {
                let _ = writeln!(text, "\nRead the file named when the task matches:");
                for line in routing {
                    let _ = writeln!(text, "{}", line.trim());
                }
            }
            for file in files {
                let body = fs::read_to_string(ready.stored.root.join(&file))
                    .map_err(|why| format!("overlay {identity:?}: {file}: {why}"))?;
                let _ = writeln!(text, "\nFrom `{file}`:\n{}", without_front_matter(&body));
            }
        }
        if !self.limits.context_bytes.allows(text.len() as u64) {
            return Err(format!(
                "the issued overlays' rules, routing and context hold {} bytes, past \
                 station.overlays.context_bytes ({})",
                text.len(),
                self.limits.context_bytes
            ));
        }
        Ok(Some(text))
    }

    /// Every skill of every overlay, once by name without regard to ASCII case, a later overlay's
    /// over an earlier one's (which is reported), in composition order.
    fn skills(
        &self,
        handler: &str,
        roots: &BTreeMap<String, String>,
        loads_natively: bool,
        weakenings: &mut Vec<Reported>,
    ) -> Vec<OverlaySkill> {
        let mut chosen: BTreeMap<String, OverlaySkill> = BTreeMap::new();
        let mut order = Vec::new();
        for ready in &self.overlays {
            let identity = &ready.issued.identity;
            let manifest = &ready.issued.manifest;
            let tree = ready.stored.read.tree();
            let native = loads_natively
                && manifest
                    .get("harness")
                    .and_then(|harness| harness.get(harness_id(handler)))
                    .and_then(|hints| hints.get("skills"))
                    .and_then(Data::as_bool)
                    .unwrap_or(true);
            for name in skills_of(manifest, &tree) {
                let dir = ready.stored.root.join("skills").join(&name);
                let key = name.to_ascii_lowercase();
                let skill = OverlaySkill {
                    description: skill_description(&dir).unwrap_or_default(),
                    path: format!(
                        "{}/skills/{name}",
                        roots[identity].trim_end_matches(['/', '\\'])
                    ),
                    overlay: identity.clone(),
                    name,
                    native,
                };
                if let Some(earlier) = chosen.insert(key.clone(), skill) {
                    weakenings.push(Reported {
                        overlay: identity.clone(),
                        weakening: Weakening {
                            id: "skill-collision".to_owned(),
                            subject: format!("{} of {}", earlier.name, earlier.overlay),
                        },
                    });
                } else {
                    order.push(key);
                }
            }
        }
        order
            .into_iter()
            .filter_map(|key| chosen.remove(&key))
            .collect()
    }
}

/// The text after a leading front matter block: an optional byte-order mark, a first line that is
/// `---`, up to the next that is `---` or `...`. Without that closing line there is none.
fn without_front_matter(text: &str) -> &str {
    let body = text.strip_prefix('\u{feff}').unwrap_or(text);
    let Some(rest) = body
        .strip_prefix("---\r\n")
        .or_else(|| body.strip_prefix("---\n"))
    else {
        return text;
    };
    let mut at = 0;
    for line in rest.split_inclusive('\n') {
        at += line.len();
        if matches!(line.trim_end_matches(['\r', '\n']), "---" | "...") {
            return &rest[at..];
        }
    }
    text
}

/// Refuses a copy of an overlay inside the working directory, where a commit could hold it.
fn check_outside(root: &str, working_directory: &str) -> Result<(), String> {
    let canonical = |path: &str| fs::canonicalize(path).unwrap_or_else(|_| PathBuf::from(path));
    if canonical(root).starts_with(canonical(working_directory)) {
        return Err(format!(
            "an overlay's copy at {root} is inside the working directory {working_directory}, \
             where a commit could hold it"
        ));
    }
    Ok(())
}

/// The digest list `after` differs from `before` in, in words, or `None` when they are the same.
pub(crate) fn change_between(before: &[IssuedDigest], after: &[IssuedDigest]) -> Option<String> {
    let map = |list: &[IssuedDigest]| -> BTreeMap<String, String> {
        list.iter()
            .map(|issued| (issued.overlay.clone(), issued.digest.clone()))
            .collect()
    };
    let (before, after) = (map(before), map(after));
    let names: BTreeSet<&String> = before.keys().chain(after.keys()).collect();
    let changes: Vec<String> = names
        .into_iter()
        .filter_map(|name| match (before.get(name), after.get(name)) {
            (Some(old), Some(new)) if old != new => Some(format!("{name}: {old} -> {new}")),
            (None, Some(new)) => Some(format!("{name}: added at {new}")),
            (Some(old), None) => Some(format!("{name}: {old} removed")),
            _ => None,
        })
        .collect();
    (!changes.is_empty()).then(|| changes.join("; "))
}

#[cfg(test)]
pub(crate) mod testing {
    use super::*;

    /// An overlay directory with `files`, by path, under `root/name`.
    pub(crate) fn overlay_dir(root: &Path, name: &str, files: &[(&str, &str)]) -> PathBuf {
        let dir = root.join(name);
        for (path, text) in files {
            let file = dir.join(path);
            fs::create_dir_all(file.parent().unwrap()).unwrap();
            fs::write(file, text).unwrap();
        }
        dir
    }

    /// The pin of the overlay directory `dir`.
    pub(crate) fn pin_of(dir: &Path) -> Digest {
        read_overlay(dir, &OverlayLimits::default())
            .unwrap()
            .digest
            .digest
    }

    /// Takes a stored tree's protection off, as its owner can.
    pub(crate) fn open_tree(path: &Path) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
            if path.is_dir() {
                for entry in fs::read_dir(path).unwrap() {
                    open_tree(&entry.unwrap().path());
                }
            }
        }
        #[cfg(windows)]
        {
            let done = icacls()
                .arg(path)
                .args(["/reset", "/T", "/C", "/Q"])
                .status()
                .unwrap();
            assert!(done.success());
        }
    }

    /// Removes a test's store, whose trees are read-only.
    pub(crate) fn release_store(state_dir: &Path) {
        let store = state_dir.join("overlays");
        if store.exists() {
            release(&store).unwrap();
        }
    }
}

#[cfg(test)]
#[path = "overlays_tests.rs"]
mod tests;
