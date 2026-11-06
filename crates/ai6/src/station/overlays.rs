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
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use guatiao::Bytes;
use serde_json::Value as Data;

use crate::directorates::q::overlays::{
    Change, Dependency, Digest, Document, Environment, Facts, IssuedDigest, IssuedOverlay,
    MANIFEST, OverlayFrom, OverlayRecord, Phase, ReadOverlay, ScriptRun, SetupRecord, SetupRun,
    Source, SourceKind, TreeEntry, Weakening, check_requirements, composed_environment, context_of,
    default_on, digest_entries, effective, env_files_of, is_reserved_variable,
    is_sensitive_variable, issue_overlays, normalize_name, preparation, read_env_script_output,
    read_overlay, root_variable, rules_of, skill_description, skills_of, state_variable,
    tree_digest,
};
use crate::directorates::q::provision::RequiredProgram;
use crate::directorates::q::transport::kit::{ProcessInput, run_process};
use crate::directorates::section_00::agent::{OverlaySkill, SessionOverlays};
use crate::station::limits::OverlayLimits;

mod site;

pub(crate) use site::Site;
use site::{Found, Step, store_tree_files};

/// Environment variables by name.
type Variables = BTreeMap<String, String>;

/// The programs a site was found to have, by command.
type Tools = BTreeMap<String, Found>;

/// A weakening a delivery reports, naming its overlay.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Reported {
    pub(crate) overlay: String,
    pub(crate) weakening: Weakening,
}

/// What a Session is issued, checked and stored, before anything is placed where it runs.
pub(crate) struct Prepared {
    install_order: Vec<String>,
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
    /// What its commands get when its harness is elsewhere than they run (remote hands).
    pub(crate) theater_environment: Option<BTreeMap<String, String>>,
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

    fn records(&self) -> PathBuf {
        self.dir.join("records")
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
        let read = read_overlay(&root, &self.limits).map_err(tampered)?;
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
    fn plugin(&self, skills: &[(String, PathBuf)]) -> Result<(Digest, PathBuf), String> {
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
            return Ok((digest.digest, target));
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
        Ok((digest.digest, target))
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
    let install_order = issued.install_order;
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
        install_order,
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
    /// The handler provider.
    pub(crate) handler: &'a str,
    /// The directory it works in, which holds nothing of an overlay.
    pub(crate) working_directory: &'a str,
    /// What names the Session where it keeps things of its own.
    pub(crate) key: &'a str,
}

/// A Mission's ID as a directory name.
fn session_key(mission: &str) -> String {
    mission
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect::<String>()
        .trim_start_matches('.')
        .to_owned()
}

/// Removes what a Mission kept where it ran. A Mission that had none leaves nothing to remove.
pub(crate) fn forget_session(site: &Site, mission: &str) -> Result<(), String> {
    site.remove(&site.path(&format!("sessions/{}", session_key(mission))))
}

/// Sets `name` in `environment` back to what `session` holds, or takes it out.
fn restore(
    environment: &mut BTreeMap<String, String>,
    session: &BTreeMap<String, String>,
    name: &str,
) {
    let found = environment
        .keys()
        .find(|key| key.eq_ignore_ascii_case(name))
        .cloned();
    let Some(key) = found else {
        return;
    };
    match session
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
    {
        Some((_, value)) => {
            environment.insert(key, value.clone());
        }
        None => {
            environment.remove(&key);
        }
    }
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

    /// What a Session on `fit`'s site gets: its overlays' read-only copies there, what they need
    /// prepared outside its sandbox, their environment applied, and their text.
    pub(crate) fn deliver(
        &self,
        fit: &Fit,
        session: SessionFacts<'_>,
        keeper: &Keeper,
    ) -> Result<Delivered, String> {
        let site = &fit.site;
        let os = site.os().to_owned();
        let mut roots: BTreeMap<String, String> = BTreeMap::new();
        for ready in &self.overlays {
            let root = site
                .tree(
                    "trees",
                    &ready.stored.digest,
                    &ready.stored.root,
                    &ready.stored.read.digest.files,
                    &self.limits.tree(),
                )
                .map_err(|why| format!("overlay {:?}: {why}", ready.issued.identity))?;
            check_outside(&root, session.working_directory)?;
            roots.insert(ready.issued.identity.clone(), root);
        }
        let mut weakenings = Vec::new();
        if !site.protection_holds() {
            weakenings.extend(self.overlays.iter().map(|ready| Reported {
                overlay: ready.issued.identity.clone(),
                weakening: Weakening {
                    id: "read-only-not-enforced".to_owned(),
                    subject: site.name(),
                },
            }));
        }
        let empty = BTreeMap::new();
        let base = process_base(
            if fit.harness_here {
                &session.environment
            } else {
                &empty
            },
            &fit.path,
        );
        let mut packages: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
        if fit.commands {
            for identity in &self.install_order {
                let Some(ready) = self
                    .overlays
                    .iter()
                    .find(|ready| ready.issued.identity == *identity)
                else {
                    continue;
                };
                let session = if fit.harness_here {
                    &session.environment
                } else {
                    &empty
                };
                packages.insert(
                    identity.clone(),
                    self.prepare_one(ready, fit, &roots[identity], (session, &base), keeper)?,
                );
            }
        }
        let (harness_environment, theater_environment) = self.environment(
            fit,
            &session,
            &os,
            &roots,
            &packages,
            &base,
            &mut weakenings,
        )?;
        let context = self.context(&roots, session.handler, fit)?;
        let mut delivered = Delivered {
            environment: harness_environment,
            theater_environment,
            context,
            overlays: SessionOverlays {
                roots: if fit.harness_here {
                    roots.values().cloned().collect()
                } else {
                    Vec::new()
                },
                ..SessionOverlays::default()
            },
            digests: self.digests(),
            weakenings,
        };
        let skills = self.skills(
            session.handler,
            &roots,
            (&os, fit.harness_here),
            &mut delivered.weakenings,
        );
        let native: Vec<(String, PathBuf)> = self
            .overlays
            .iter()
            .flat_map(|ready| {
                let dir = ready.stored.root.join("skills");
                let wanted: Vec<(String, PathBuf)> = skills
                    .iter()
                    .filter(|skill| skill.native && skill.overlay == ready.issued.identity)
                    .map(|skill| (skill.name.clone(), dir.join(&skill.name)))
                    .collect();
                wanted
            })
            .collect();
        if !native.is_empty() {
            let (digest, here) = self.store.plugin(&native)?;
            let files = store_tree_files(&here, &self.limits.tree())?;
            let plugin = site.tree("plugins", &digest, &here, &files, &self.limits.tree())?;
            check_outside(&plugin, session.working_directory)?;
            delivered.overlays.plugin_dir = Some(plugin);
        }
        delivered.overlays.skills = skills;
        Ok(delivered)
    }

    /// The Session's environment with every overlay's applied in the order of application, and,
    /// when its harness is elsewhere than its commands, what its commands get instead.
    #[allow(
        clippy::too_many_arguments,
        reason = "one delivery's context, used once"
    )]
    fn environment(
        &self,
        fit: &Fit,
        session: &SessionFacts<'_>,
        os: &str,
        roots: &BTreeMap<String, String>,
        packages: &BTreeMap<String, BTreeMap<String, String>>,
        base: &BTreeMap<String, String>,
        weakenings: &mut Vec<Reported>,
    ) -> Result<(Variables, Option<Variables>), String> {
        let site = &fit.site;
        let windows = os == "windows";
        // What the overlays' environment starts from: a Theater's PATH when the Agent names none,
        // since a value here replaces the one there.
        let mut start = if fit.harness_here {
            session.environment.clone()
        } else {
            BTreeMap::new()
        };
        if path_of(&start).is_none() && !fit.path.is_empty() {
            start.insert("PATH".to_owned(), fit.path.clone());
        }
        let mut env = Environment::new(start.clone(), windows);
        let found: Vec<String> = INTERPRETER_COMMANDS
            .iter()
            .filter(|(name, _)| fit.commands && interpreter(name, &fit.tools).is_some())
            .map(|(name, _)| (*name).to_owned())
            .collect();
        let mut all_roots = BTreeMap::new();
        for ready in &self.overlays {
            let identity = &ready.issued.identity;
            let state = site.path(&format!("state/{identity}"));
            if !site.exists(&state)? {
                site.open_dir(&state)?;
                site.close_dir(&state)?;
            }
            env.set(&root_variable(identity), roots[identity].clone(), false);
            env.set(&state_variable(identity), state.clone(), false);
            all_roots.insert(root_variable(identity), roots[identity].clone());
            all_roots.insert(state_variable(identity), state);
        }
        for ready in &self.overlays {
            let identity = &ready.issued.identity;
            let state = site.path(&format!("state/{identity}"));
            let facts = Facts {
                os: os.to_owned(),
                root: roots[identity].clone(),
                identity: identity.clone(),
                state: Some(state.clone()),
                pinned: ready.issued.record.pin().is_some(),
                base_env: start.clone(),
                enforces_network: Some(false),
                env_scripts_enabled: Some(fit.commands),
                interpreters: Some(found.clone()),
                packages: packages.get(identity).cloned().unwrap_or_default(),
                tree: self.tree_of(ready)?,
                read_limits: self.limits.manifest(),
                ..Facts::default()
            };
            let manifest = &ready.issued.manifest;
            let applied = effective(manifest, &facts)
                .map_err(|refusal| format!("overlay {identity:?}: {refusal}"))?;
            weakenings.extend(
                applied
                    .weakenings
                    .iter()
                    .filter(|weakening| {
                        fit.commands || !RUN_WEAKENINGS.contains(&weakening.id.as_str())
                    })
                    .map(|weakening| Reported {
                        overlay: identity.clone(),
                        weakening: weakening.clone(),
                    }),
            );
            let mut ignored = Vec::new();
            env = composed_environment(&[(manifest, facts.clone())], env, &mut ignored)
                .map_err(|refusal| format!("overlay {identity:?}: {refusal}"))?;
            if fit.commands {
                for script in &applied.env_scripts {
                    // The overlay's own process: the base, every root, its own declarations.
                    let mut environment = process_environment(
                        &applied,
                        &start,
                        base,
                        (&roots[identity], &state, identity),
                    );
                    environment.extend(all_roots.clone());
                    self.run_env_script(
                        fit,
                        identity,
                        &roots[identity],
                        script,
                        &environment,
                        &mut env,
                        weakenings,
                    )?;
                }
            }
        }
        if fit.commands {
            self.add_tools(fit, session, roots, &mut env)?;
        }
        let environment: BTreeMap<String, String> = env.values().iter().cloned().collect();
        for (name, value) in &environment {
            if start.get(name) != Some(value) && name.to_ascii_uppercase().starts_with("AI6_") {
                return Err(format!(
                    "an overlay sets {name}, which is the Station's to set"
                ));
            }
        }
        if !fit.harness_here {
            // The harness runs on this Station and acts through the Theater's tools: the overlays'
            // environment is what those commands get, and it knows the roots by the same names.
            let mut harness = session.environment.clone();
            harness.extend(all_roots);
            let commands = fit.commands.then_some(environment);
            return Ok((harness, commands));
        }
        let mut environment = environment;
        if !fit.commands {
            // A Session that may not run commands gets nothing to run.
            for variable in ["PATH", "PYTHONPATH", "NODE_PATH"] {
                restore(&mut environment, &session.environment, variable);
            }
        } else if path_of(&session.environment).is_none()
            && path_of(&environment) == Some(fit.path.as_str())
        {
            restore(&mut environment, &session.environment, "PATH");
        }
        Ok((environment, None))
    }

    /// What code in an overlay runs with: `AGENTS_PYTHON`, and for `cmds/` the pinned `dotagents`
    /// with a home of its own, empty, so nothing of the Station's reaches it.
    fn add_tools(
        &self,
        fit: &Fit,
        session: &SessionFacts<'_>,
        roots: &BTreeMap<String, String>,
        env: &mut Environment,
    ) -> Result<(), String> {
        let site = &fit.site;
        let os = site.os();
        let separator = if os == "windows" { ";" } else { ":" };
        let uses_python = self.overlays.iter().any(|ready| {
            holds_python_code(ready).is_some()
                || ready
                    .stored
                    .read
                    .tree()
                    .contains("lib/python/requirements.txt")
        });
        if uses_python && let Some(python) = interpreter("python", &fit.tools) {
            env.set("AGENTS_PYTHON", python.path.clone(), false);
        }
        let with_commands: Vec<String> = self
            .overlays
            .iter()
            .filter(|ready| {
                ready
                    .stored
                    .read
                    .tree()
                    .paths()
                    .any(|path| path.starts_with("cmds/"))
            })
            .map(|ready| under(&roots[&ready.issued.identity], "cmds", os))
            .collect();
        if with_commands.is_empty() {
            return Ok(());
        }
        let tool_bin = self.place_dotagents(fit)?;
        let home = site.path(&format!("sessions/{}/agents", session_key(session.key)));
        site.empty_dir(&home)?;
        env.set("AGENTS_HOME", home, false);
        env.set("AGENTS_CMDS_PATH", with_commands.join(separator), false);
        let path = env.get("PATH").unwrap_or_default().to_owned();
        let joined = if path.is_empty() {
            tool_bin
        } else {
            format!("{tool_bin}{separator}{path}")
        };
        env.set("PATH", joined, false);
        Ok(())
    }

    /// The pinned `dotagents.pyz` on the site and a program that runs it under `AGENTS_PYTHON`, by
    /// the file's digest, answering the directory that program is in.
    fn place_dotagents(&self, fit: &Fit) -> Result<String, String> {
        let site = &fit.site;
        let source = self
            .limits
            .dotagents_pyz
            .as_deref()
            .ok_or("station.overlays.dotagents_pyz names no dotagents.pyz")?;
        let bytes = fs::read(source)
            .map_err(|why| format!("station.overlays.dotagents_pyz {source}: {why}"))?;
        let digest = Digest::of(&bytes);
        match self.limits.dotagents_pin.as_deref() {
            Some(pin) if pin.eq_ignore_ascii_case(&digest.to_string()) => {}
            Some(pin) => {
                return Err(format!(
                    "station.overlays.dotagents_pyz {source} has the digest {digest}, not \
                     station.overlays.dotagents_pin {pin}"
                ));
            }
            None if self.limits.unpinned => {}
            None => {
                return Err(format!(
                    "station.overlays.dotagents_pyz {source} has no \
                     station.overlays.dotagents_pin; it would be {digest}"
                ));
            }
        }
        let windows = site.os() == "windows";
        let directory = site.path(&format!("tools/{}", hex_of(&digest)));
        let bin = under(&directory, "bin", site.os());
        let pyz = under(&directory, "dotagents.pyz", site.os());
        if !site.exists(&pyz)? {
            site.write_file(&pyz, &bytes)?;
        }
        let (program, shim) = if windows {
            (
                under(&bin, "dotagents.cmd", site.os()),
                format!("@\"%AGENTS_PYTHON%\" \"{pyz}\" %*\r\n"),
            )
        } else {
            (
                under(&bin, "dotagents", site.os()),
                format!("#!/bin/sh\nexec \"$AGENTS_PYTHON\" '{pyz}' \"$@\"\n"),
            )
        };
        if !site.exists(&program)? {
            site.write_program(&program, shim.as_bytes())?;
        }
        Ok(bin)
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
    fn context(
        &self,
        roots: &BTreeMap<String, String>,
        handler: &str,
        fit: &Fit,
    ) -> Result<Option<String>, String> {
        let mut text = String::from(
            "An operator issued these overlays to this Session, so what they say is instruction. \
             Their files are read-only and outside your working directory, and stay out of it:\n",
        );
        if !fit.harness_here {
            text.push_str(
                "(They are on the Theater: read them with theater_read; theater_run has their \
                 environment.)\n",
            );
        }
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
            let hinted: Vec<String> = manifest
                .get("harness")
                .and_then(|harness| harness.get(harness_id(handler)))
                .and_then(|hints| hints.get("context"))
                .and_then(Data::as_array)
                .map(|files| {
                    files
                        .iter()
                        .filter_map(Data::as_str)
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default();
            let files: Vec<String> = rules_of(manifest, &tree)
                .into_iter()
                .chain(context_of(manifest, &tree))
                .chain(hinted)
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
        (os, loads_natively): (&str, bool),
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
                    path: under(&roots[identity], &format!("skills/{name}"), os),
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

/// The interpreters AOS names, each with the commands that may be it, tried in order.
const INTERPRETER_COMMANDS: [(&str, &[&str]); 5] = [
    ("python", &["python3", "python"]),
    ("node", &["node"]),
    ("powershell", &["pwsh"]),
    ("bash", &["bash"]),
    ("sh", &["sh"]),
];

/// What a Station keeps between Sessions: what each Theater was found to have, for one check-in
/// interval, and one lock per overlay and Theater, so two starts do not prepare it at once.
#[derive(Default)]
pub(crate) struct Keeper {
    tools: Mutex<BTreeMap<(String, String), (Instant, Tools)>>,
    locks: Mutex<BTreeMap<String, Arc<Mutex<()>>>>,
}

// What it holds is a cache: two Keepers are the same as far as a configuration can say.
impl PartialEq for Keeper {
    fn eq(&self, _other: &Self) -> bool {
        true
    }
}

impl Eq for Keeper {}

impl std::fmt::Debug for Keeper {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Keeper")
    }
}

impl Keeper {
    fn tools(
        &self,
        site: &Site,
        path: &str,
        wanted: &[&str],
        every: Duration,
    ) -> Result<BTreeMap<String, Found>, String> {
        let key = (site.key(), path.to_owned());
        if let Ok(known) = self.tools.lock()
            && let Some((when, found)) = known.get(&key)
            && when.elapsed() < every
        {
            return Ok(found.clone());
        }
        let found = site.find_tools(wanted, path)?;
        if let Ok(mut known) = self.tools.lock() {
            known.insert(key, (Instant::now(), found.clone()));
        }
        Ok(found)
    }

    fn lock(&self, key: String) -> Arc<Mutex<()>> {
        self.locks
            .lock()
            .map(|mut locks| locks.entry(key).or_default().clone())
            .unwrap_or_default()
    }
}

/// The Python found where a Session runs, or what takes its place: an interpreter's command is
/// whichever is first on the `PATH` and answers.
fn interpreter<'a>(name: &str, tools: &'a BTreeMap<String, Found>) -> Option<&'a Found> {
    INTERPRETER_COMMANDS
        .iter()
        .find(|(known, _)| *known == name)
        .and_then(|(_, commands)| commands.iter().find_map(|command| tools.get(*command)))
        .filter(|found| name != "python" || found.version.contains("Python 3"))
}

/// A site's checked fit for a Session: where its overlays go and what is there to prepare them.
pub(crate) struct Fit {
    site: Site,
    tools: BTreeMap<String, Found>,
    path: String,
    commands: bool,
    harness_here: bool,
}

/// What an overlay holds that needs Python where it runs, beyond what AOS's own reading finds: an
/// importable library, or commands. A program in `bin/` says what it needs itself.
fn holds_python_code(ready: &Ready) -> Option<&'static str> {
    let tree = ready.stored.read.tree();
    let under = |prefix: &str| tree.paths().any(|path| path.starts_with(prefix));
    if under("cmds/") {
        Some("its cmds/")
    } else if default_on(&ready.issued.manifest, "lib") && under("lib/python/") {
        Some("its lib/python/")
    } else {
        None
    }
}

impl Prepared {
    /// Whether `site` has what this Session's overlays need. A Session that may not run commands
    /// gets only their text and needs nothing there. Looks at the site once per `every`.
    ///
    /// # Errors
    ///
    /// The refusal a Mission waits with, naming what is missing and which overlay needs it.
    pub(crate) fn fit(
        &self,
        site: Site,
        environment: &BTreeMap<String, String>,
        commands: bool,
        harness_here: bool,
        (keeper, every): (&Keeper, Duration),
    ) -> Result<Fit, String> {
        // The harness's own PATH says nothing of a Theater it acts on through tools.
        let path = match path_of(environment).filter(|_| harness_here) {
            Some(path) => path.to_owned(),
            None => site.login_path()?,
        };
        let mut fit = Fit {
            site,
            tools: BTreeMap::new(),
            path,
            commands,
            harness_here,
        };
        let os = fit.site.os().to_owned();
        let empty = BTreeMap::new();
        let session = if harness_here { environment } else { &empty };
        for ready in &self.overlays {
            let facts = self.plain_facts(ready, &os, session)?;
            let applied = effective(&ready.issued.manifest, &facts)
                .map_err(|refusal| format!("overlay {:?}: {refusal}", ready.issued.identity))?;
            if !applied.supported {
                return Err(format!(
                    "overlay {:?} does not support {os} (its runtime.os), which {} runs",
                    ready.issued.identity,
                    fit.site.name()
                ));
            }
        }
        if !commands {
            return Ok(fit);
        }
        let mut wanted: BTreeSet<String> = INTERPRETER_COMMANDS
            .iter()
            .flat_map(|(_, commands)| commands.iter().map(|command| (*command).to_owned()))
            .collect();
        wanted.insert("npm".to_owned());
        for ready in &self.overlays {
            let facts = self.plain_facts(ready, &os, session)?;
            let applied = effective(&ready.issued.manifest, &facts)
                .map_err(|refusal| format!("overlay {:?}: {refusal}", ready.issued.identity))?;
            wanted.extend(applied.needs.commands);
        }
        let names: Vec<&str> = wanted.iter().map(String::as_str).collect();
        fit.tools = keeper.tools(&fit.site, &fit.path, &names, every)?;
        for ready in &self.overlays {
            self.check_needs(ready, &fit, &os, session)?;
        }
        if self.overlays.iter().any(|ready| {
            ready
                .stored
                .read
                .tree()
                .paths()
                .any(|path| path.starts_with("cmds/"))
        }) && self.limits.dotagents_pyz.is_none()
        {
            return Err(
                "an issued overlay holds cmds/, which run under dotagents, and \
                 station.overlays.dotagents_pyz names none"
                    .to_owned(),
            );
        }
        Ok(fit)
    }

    /// Why the site lacks what `ready` needs on it.
    fn check_needs(
        &self,
        ready: &Ready,
        fit: &Fit,
        os: &str,
        session: &BTreeMap<String, String>,
    ) -> Result<(), String> {
        let identity = &ready.issued.identity;
        let site = fit.site.name();
        let mut facts = self.plain_facts(ready, os, session)?;
        facts.interpreters = Some(
            INTERPRETER_COMMANDS
                .iter()
                .filter(|(name, _)| interpreter(name, &fit.tools).is_some())
                .map(|(name, _)| (*name).to_owned())
                .collect(),
        );
        let applied = effective(&ready.issued.manifest, &facts)
            .map_err(|refusal| format!("overlay {identity:?}: {refusal}"))?;
        let mut needed: Vec<(String, String)> = applied
            .needs
            .interpreters
            .iter()
            .map(|name| (name.clone(), "what it declares".to_owned()))
            .collect();
        if let Some(why) = holds_python_code(ready) {
            needed.push(("python".to_owned(), why.to_owned()));
        }
        for (name, why) in needed {
            if interpreter(&name, &fit.tools).is_none() {
                let commands = INTERPRETER_COMMANDS
                    .iter()
                    .find(|(known, _)| *known == name)
                    .map_or_else(|| name.clone(), |(_, commands)| commands.join(" or "));
                return Err(format!(
                    "{site} has no {commands}, which overlay {identity:?} needs for {why}"
                ));
            }
        }
        for command in &applied.needs.commands {
            if !fit.tools.contains_key(command) {
                return Err(format!(
                    "{site} has no {command}, which overlay {identity:?} needs (its \
                     runtime.commands or dependency file)"
                ));
            }
        }
        for (name, variant) in [("python", &applied.python), ("node", &applied.node)] {
            let Some(minimum) = variant
                .as_ref()
                .and_then(|variant| variant.get("version"))
                .and_then(Data::as_str)
            else {
                continue;
            };
            let Some(found) = interpreter(name, &fit.tools) else {
                continue;
            };
            let refusal = RequiredProgram::new(name)
                .and_then(|program| program.at_least(minimum))
                .ok()
                .and_then(|program| program.refuses(&found.version));
            if let Some(why) = refusal {
                return Err(format!(
                    "{site}: {name} {why}, and overlay {identity:?} needs {minimum}"
                ));
            }
        }
        Ok(())
    }

    /// What is known of an overlay before anything is placed for it.
    fn plain_facts(
        &self,
        ready: &Ready,
        os: &str,
        session: &BTreeMap<String, String>,
    ) -> Result<Facts, String> {
        Ok(Facts {
            os: os.to_owned(),
            root: "/".to_owned(),
            identity: ready.issued.identity.clone(),
            state: Some("/".to_owned()),
            pinned: ready.issued.record.pin().is_some(),
            base_env: session.clone(),
            enforces_network: Some(false),
            tree: self.tree_of(ready)?,
            read_limits: self.limits.manifest(),
            ..Facts::default()
        })
    }

    /// Installs an overlay's dependencies and runs its setup where it is due, per AOS's phases, and
    /// answers its packages directories. A preparation stops at the first step that fails, and a
    /// digest that has not been prepared is not issued.
    fn prepare_one(
        &self,
        ready: &Ready,
        fit: &Fit,
        root: &str,
        (session, base): (&BTreeMap<String, String>, &BTreeMap<String, String>),
        keeper: &Keeper,
    ) -> Result<BTreeMap<String, String>, String> {
        let identity = &ready.issued.identity;
        let digest = ready.stored.digest.to_string();
        let site = &fit.site;
        let mut session = session.clone();
        if path_of(&session).is_none() && !fit.path.is_empty() {
            session.insert("PATH".to_owned(), fit.path.clone());
        }
        let session = &session;
        let lock = keeper.lock(format!("{}/{identity}", site.key()));
        let _held = lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let state = site.path(&format!("state/{identity}"));
        let hex = hex_of(&ready.stored.digest);
        let mut facts = self.plain_facts(ready, site.os(), session)?;
        facts.root = root.to_owned();
        facts.state = Some(state.clone());
        facts.interpreters = Some(
            INTERPRETER_COMMANDS
                .iter()
                .filter(|(name, _)| interpreter(name, &fit.tools).is_some())
                .map(|(name, _)| (*name).to_owned())
                .collect(),
        );
        let applied = effective(&ready.issued.manifest, &facts)
            .map_err(|refusal| format!("overlay {identity:?}: {refusal}"))?;
        let record_path = self
            .store
            .records()
            .join(site.key())
            .join(format!("{identity}.json"));
        let mut record: Option<SetupRecord> = fs::read(&record_path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok());
        // A Theater rebuilt since keeps no environment or state: its record says nothing of it.
        let envs = |runtime: &str| site.path(&format!("envs/{hex}/{runtime}"));
        let mut gone = !site.exists(&state)?;
        for dependency in &applied.dependencies {
            if record.as_ref().is_some_and(|r| r.digest == digest) {
                gone |= !site.exists(&envs(&dependency.runtime))?;
            }
        }
        if gone {
            record = None;
        }
        let plan = preparation(
            Change::Activate,
            record.as_ref(),
            Some(&digest),
            applied.setup.as_ref(),
            &applied.dependencies,
            None,
        );
        if !site.exists(&state)? {
            site.open_dir(&state)?;
            site.close_dir(&state)?;
        }
        let environment = process_environment(&applied, session, base, (root, &state, identity));
        let mut packages = BTreeMap::new();
        for step in &plan.steps {
            let done = match step.split(':').collect::<Vec<_>>().as_slice() {
                ["empty-state"] => site.empty_dir(&state),
                ["deps", "python"] => {
                    let dependency = applied
                        .dependencies
                        .iter()
                        .find(|dependency| dependency.runtime == "python");
                    self.install_python(ready, fit, root, &envs("python"), dependency, &environment)
                }
                ["deps", "node"] => {
                    let dependency = applied
                        .dependencies
                        .iter()
                        .find(|dependency| dependency.runtime == "node");
                    self.install_node(ready, fit, root, &envs("node"), dependency, &environment)
                }
                ["setup"] => applied.setup.as_ref().map_or(Ok(()), |setup| {
                    let phase = plan.phase.map_or("sync", Phase::name);
                    self.run_setup(ready, fit, root, &state, setup, phase, &environment)
                }),
                ["record"] => {
                    let write = || -> io::Result<()> {
                        if let Some(parent) = record_path.parent() {
                            fs::create_dir_all(parent)?;
                        }
                        fs::write(
                            &record_path,
                            serde_json::to_vec_pretty(&plan.record).map_err(io::Error::other)?,
                        )
                    };
                    write().map_err(|why| format!("its setup record: {why}"))
                }
                _ => Ok(()),
            };
            if let Err(why) = done {
                let failed = step.split(':').next().unwrap_or_default();
                let after = preparation(
                    Change::Activate,
                    record.as_ref(),
                    Some(&digest),
                    applied.setup.as_ref(),
                    &applied.dependencies,
                    Some(failed),
                );
                if !after.activates {
                    return Err(format!(
                        "overlay {identity:?} was not prepared on {}: {step}: {why}",
                        site.name()
                    ));
                }
            }
        }
        site.close_dir(&state)?;
        for dependency in &applied.dependencies {
            let directory = envs(&dependency.runtime);
            packages.insert(
                dependency.runtime.clone(),
                match dependency.runtime.as_str() {
                    "python" => self.python_packages(fit, &directory, base)?,
                    _ => format!(
                        "{}{}node_modules",
                        directory,
                        if site.os() == "windows" { "\\" } else { "/" }
                    ),
                },
            );
        }
        Ok(packages)
    }

    /// `python -m venv` and pip, in hash-checking mode for a lock, from an environment of this
    /// overlay's digest alone (`docs/agents.md`).
    fn install_python(
        &self,
        ready: &Ready,
        fit: &Fit,
        root: &str,
        directory: &str,
        dependency: Option<&Dependency>,
        environment: &BTreeMap<String, String>,
    ) -> Result<(), String> {
        let Some(dependency) = dependency else {
            return Ok(());
        };
        let site = &fit.site;
        let text = fs::read_to_string(ready.stored.root.join(&dependency.file))
            .map_err(|why| format!("{}: {why}", dependency.file))?;
        check_requirements(&text, dependency.lock.is_some())
            .map_err(|why| format!("{}: {why}", dependency.file))?;
        let python = interpreter("python", &fit.tools).ok_or("no Python")?;
        site.empty_dir(directory)?;
        let limit = Some(self.limits.deps_timeout.0);
        let run = |argv: Vec<String>| -> Result<(), String> {
            let finished = site.run(&Step {
                argv,
                cwd: Some(root),
                env: environment,
                limit,
                max_output: self.limits.output_bytes.or_max() as u64,
            })?;
            if finished.succeeded() {
                Ok(())
            } else {
                Err(finished.failure())
            }
        };
        run(vec![
            python.path.clone(),
            "-m".to_owned(),
            "venv".to_owned(),
            directory.to_owned(),
        ])?;
        let mut pip = vec![
            venv_python(directory, site.os()),
            "-m".to_owned(),
            "pip".to_owned(),
            "install".to_owned(),
            "--no-input".to_owned(),
            "--disable-pip-version-check".to_owned(),
        ];
        if dependency.lock.is_some() {
            pip.extend(["--require-hashes", "--no-deps", "--only-binary=:all:"].map(str::to_owned));
        }
        pip.push("-r".to_owned());
        pip.push(under(root, &dependency.file, site.os()));
        run(pip)?;
        site.close_dir(directory)
    }

    /// `npm ci` with a lock and `npm install` without, from copies of the two files alone, so no
    /// `.npmrc` of the overlay configures npm, and with no package's install script.
    fn install_node(
        &self,
        ready: &Ready,
        fit: &Fit,
        root: &str,
        directory: &str,
        dependency: Option<&Dependency>,
        environment: &BTreeMap<String, String>,
    ) -> Result<(), String> {
        let Some(dependency) = dependency else {
            return Ok(());
        };
        let _ = ready;
        let site = &fit.site;
        let npm = fit.tools.get("npm").ok_or("no npm")?;
        site.empty_dir(directory)?;
        let separator = if site.os() == "windows" { "\\" } else { "/" };
        site.copy_file(
            &under(root, &dependency.file, site.os()),
            &format!("{directory}{separator}package.json"),
        )?;
        if let Some(lock) = &dependency.lock {
            site.copy_file(
                &under(root, lock, site.os()),
                &format!("{directory}{separator}package-lock.json"),
            )?;
        }
        let verb = if dependency.lock.is_some() {
            "ci"
        } else {
            "install"
        };
        let finished = site.run(&Step {
            argv: vec![
                npm.path.clone(),
                verb.to_owned(),
                "--ignore-scripts".to_owned(),
                "--no-audit".to_owned(),
                "--no-fund".to_owned(),
            ],
            cwd: Some(directory),
            env: environment,
            limit: Some(self.limits.deps_timeout.0),
            max_output: self.limits.output_bytes.or_max() as u64,
        })?;
        if !finished.succeeded() {
            return Err(finished.failure());
        }
        site.close_dir(directory)
    }

    fn python_packages(
        &self,
        fit: &Fit,
        directory: &str,
        base: &BTreeMap<String, String>,
    ) -> Result<String, String> {
        let finished = fit.site.run(&Step {
            argv: vec![
                venv_python(directory, fit.site.os()),
                "-c".to_owned(),
                "import site; print(site.getsitepackages()[-1])".to_owned(),
            ],
            cwd: None,
            env: base,
            limit: Some(Duration::from_secs(60)),
            max_output: 64 * 1024,
        })?;
        if !finished.succeeded() {
            return Err(format!("its packages directory: {}", finished.failure()));
        }
        Ok(String::from_utf8_lossy(&finished.stdout).trim().to_owned())
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "a step's whole context, used once"
    )]
    fn run_setup(
        &self,
        ready: &Ready,
        fit: &Fit,
        root: &str,
        state: &str,
        setup: &SetupRun,
        phase: &str,
        environment: &BTreeMap<String, String>,
    ) -> Result<(), String> {
        let _ = ready;
        let site = &fit.site;
        let script = under(root, &setup.path, site.os());
        let mut arguments = vec![phase.to_owned()];
        arguments.extend(setup.args.iter().cloned());
        let argv = interpreter_argv(&setup.interpreter, &script, &arguments, &fit.tools)?;
        let mut env = environment.clone();
        env.insert("AOS_SETUP_PHASE".to_owned(), phase.to_owned());
        site.open_dir(state)?;
        let finished = site.run(&Step {
            argv,
            cwd: Some(root),
            env: &env,
            limit: (setup.timeout > 0).then(|| Duration::from_secs(setup.timeout)),
            max_output: self.limits.output_bytes.or_max() as u64,
        });
        let closed = site.close_dir(state);
        let finished = finished?;
        closed?;
        if finished.succeeded() {
            Ok(())
        } else {
            Err(finished.failure())
        }
    }

    /// Runs an env script as a step does, and sets what it printed: an environment value, never a
    /// variable that is the consumer's or the Station's.
    #[allow(
        clippy::too_many_arguments,
        reason = "a step's whole context, used once"
    )]
    fn run_env_script(
        &self,
        fit: &Fit,
        identity: &str,
        root: &str,
        script: &ScriptRun,
        environment: &BTreeMap<String, String>,
        env: &mut Environment,
        weakenings: &mut Vec<Reported>,
    ) -> Result<(), String> {
        let refuse = |why: String| {
            format!(
                "overlay {identity:?}: its env script {}: {why}",
                script.path
            )
        };
        let path = under(root, &script.path, fit.site.os());
        let argv = interpreter_argv(&script.interpreter, &path, &script.args, &fit.tools)
            .map_err(refuse)?;
        let finished = fit
            .site
            .run(&Step {
                argv,
                cwd: Some(root),
                env: environment,
                limit: (script.timeout > 0).then(|| Duration::from_secs(script.timeout)),
                max_output: self.limits.output_bytes.or_max() as u64,
            })
            .map_err(refuse)?;
        if !finished.succeeded() {
            return Err(refuse(finished.failure()));
        }
        let values =
            read_env_script_output(&finished.stdout).map_err(|why| refuse(why.to_string()))?;
        for (name, value) in values {
            if is_reserved_variable(&name) {
                return Err(refuse(format!("it sets {name}, which is reserved")));
            }
            let Some(value) = value else {
                env.unset(&name);
                continue;
            };
            if is_sensitive_variable(&name) {
                weakenings.push(Reported {
                    overlay: identity.to_owned(),
                    weakening: Weakening {
                        id: "env-sensitive".to_owned(),
                        subject: name.clone(),
                    },
                });
            }
            env.set(&name, value, false);
        }
        Ok(())
    }
}

/// `relative`, `/`-separated in an overlay, under `root` on `os`.
fn under(root: &str, relative: &str, os: &str) -> String {
    let separator = if os == "windows" { "\\" } else { "/" };
    format!(
        "{}{separator}{}",
        root.trim_end_matches(['/', '\\']),
        relative.replace('/', separator)
    )
}

fn venv_python(directory: &str, os: &str) -> String {
    if os == "windows" {
        format!("{directory}\\Scripts\\python.exe")
    } else {
        format!("{directory}/bin/python")
    }
}

/// How AOS runs a script under `interpreter`; arguments are a list, never read by a shell.
fn interpreter_argv(
    interpreter_name: &str,
    script: &str,
    arguments: &[String],
    tools: &BTreeMap<String, Found>,
) -> Result<Vec<String>, String> {
    let found = interpreter(interpreter_name, tools)
        .ok_or_else(|| format!("no {interpreter_name} to run it"))?;
    let mut argv = vec![found.path.clone()];
    if interpreter_name == "powershell" {
        argv.extend(["-NoLogo", "-NoProfile", "-NonInteractive", "-File"].map(str::to_owned));
    }
    argv.push(script.to_owned());
    argv.extend(arguments.iter().cloned());
    Ok(argv)
}

/// The variables of a Session's environment an overlay's own process keeps: what a host needs to
/// start one and where a package manager finds its index. No credential a Session is issued is here.
const PROCESS_VARIABLES: [&str; 23] = [
    "PATH",
    "PATHEXT",
    "HOME",
    "USERPROFILE",
    "APPDATA",
    "LOCALAPPDATA",
    "SYSTEMROOT",
    "SYSTEMDRIVE",
    "WINDIR",
    "COMSPEC",
    "TEMP",
    "TMP",
    "TMPDIR",
    "LANG",
    "LC_ALL",
    "LC_CTYPE",
    "TZ",
    "TERM",
    "SSL_CERT_FILE",
    "PIP_INDEX_URL",
    "PIP_CERT",
    "NPM_CONFIG_REGISTRY",
    "NODE_EXTRA_CA_CERTS",
];

/// The base environment of an overlay's own processes, from a Session's.
fn process_base(session: &BTreeMap<String, String>, path: &str) -> BTreeMap<String, String> {
    let mut base: BTreeMap<String, String> = session
        .iter()
        .filter(|(name, _)| {
            PROCESS_VARIABLES
                .iter()
                .any(|kept| name.eq_ignore_ascii_case(kept))
        })
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect();
    if !base.keys().any(|name| name.eq_ignore_ascii_case("PATH")) && !path.is_empty() {
        base.insert("PATH".to_owned(), path.to_owned());
    }
    #[cfg(windows)]
    for name in ["SYSTEMROOT", "TEMP", "TMP", "PATHEXT", "COMSPEC"] {
        if !base.keys().any(|known| known.eq_ignore_ascii_case(name))
            && let Some(value) = std::env::var_os(name)
        {
            base.insert(name.to_owned(), value.to_string_lossy().into_owned());
        }
    }
    base
}

/// What one overlay's process gets (`spec/v0/runtime.md#what-a-step-receives`): the base, this
/// overlay's own declarations and the `AOS_` variables.
fn process_environment(
    applied: &crate::directorates::q::overlays::Effective,
    session: &BTreeMap<String, String>,
    base: &BTreeMap<String, String>,
    (root, state, identity): (&str, &str, &str),
) -> BTreeMap<String, String> {
    let mut environment: BTreeMap<String, String> = base.clone();
    // Substituted against the Session's, but only what this overlay itself declared is passed on.
    environment.extend(
        applied
            .env
            .values()
            .iter()
            .filter(|(name, value)| session.get(name) != Some(value))
            .cloned(),
    );
    environment.insert("AOS_OVERLAY_ROOT".to_owned(), root.to_owned());
    environment.insert("AOS_OVERLAY_NAME".to_owned(), normalize_name(identity));
    environment.insert("AOS_OVERLAY_STATE".to_owned(), state.to_owned());
    environment
}

/// Weakenings that say something ran, which do not hold for a Session given only text.
const RUN_WEAKENINGS: [&str; 7] = [
    "deps-network",
    "unlocked-dependencies",
    "setup-network",
    "network-not-enforced",
    "env-script",
    "no-timeout",
    "script-not-run",
];

/// The value of `PATH` in `environment`, under any case.
fn path_of(environment: &BTreeMap<String, String>) -> Option<&str> {
    environment
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("PATH"))
        .map(|(_, value)| value.as_str())
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
    let within = |path: &str, directory: &str| {
        let directory = directory.trim_end_matches(['/', '\\']);
        path.strip_prefix(directory)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with(['/', '\\']))
    };
    if within(root, working_directory) {
        return Err(format!(
            "an overlay's copy at {root} is inside the working directory {working_directory}, \
             where a commit could hold it"
        ));
    }
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
