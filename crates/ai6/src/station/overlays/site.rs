//! Where a Session's overlays go: this Station's own host, or a Theater reached through its
//! transport. A Theater is reached with POSIX shell scripts and holds one copy of a tree per
//! digest, which every start hashes again against what the Station's store holds.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

use guatiao::Bytes;

use crate::directorates::q::overlays::{Digest, TreeLimits, tree_digest, valid_relative_path};
use crate::directorates::q::transport::kit::{ProcessInput, run_process};
use crate::directorates::q::transport::{Output, RunSpec, TransportClient, WriteSpec, describe};
use crate::directorates::section_00::programs;

#[cfg(windows)]
use super::icacls;
use super::{hex_of, protect, release};

/// Where a Theater keeps overlays: relative to the transport's default directory, the login
/// user's home over SSH.
const THEATER_ROOT: &str = ".ai6/overlays";

/// Makes the root and prints where it is, absolute, and the user it runs as.
const HOME: &str = r#"mkdir -p "$1" && cd "$1" && pwd -P && id -u
"#;

/// Prints the login environment's `PATH`.
const LOGIN_PATH: &str = r#"printf '%s' "$PATH"
"#;

/// Lists a tree's regular files with their SHA-256, one `HEX  ./PATH` a line, or names the first
/// entry that is neither a file nor a directory. Exits 44 when there is no tree.
const HASH_TREE: &str = r#"cd "$1" 2>/dev/null || exit 44
ai6_other=$(find . ! -type d ! -type f -print | head -n 1)
if [ -n "$ai6_other" ]; then
  printf 'other\t%s\n' "$ai6_other"
  exit 0
fi
if command -v sha256sum >/dev/null 2>&1; then
  find . -type f -exec sha256sum {} +
elif command -v shasum >/dev/null 2>&1; then
  find . -type f -exec shasum -a 256 {} +
elif command -v sha256 >/dev/null 2>&1; then
  find . -type f -exec sha256 -r {} +
elif command -v openssl >/dev/null 2>&1; then
  find . -type f -exec openssl dgst -sha256 -r {} +
else
  echo 'no program here prints a SHA-256 (sha256sum, shasum, sha256, openssl)' >&2
  exit 45
fi
"#;

/// Makes the tree in `$1` read-only, the files named on standard input executable, and moves it to
/// `$2`. One already there is as good, since both have the digest in their name.
const PLACE_TREE: &str = r#"set -e
cd "$1"
find . -type f -exec chmod 444 {} +
while IFS= read -r ai6_file; do chmod 555 "$ai6_file"; done
find . -mindepth 1 -type d -exec chmod 555 {} +
cd /
mkdir -p "${2%/*}"
if [ -e "$2" ]; then
  chmod -R u+w "$1"
  rm -rf "$1"
else
  mv "$1" "$2"
  chmod 555 "$2"
fi
rmdir "${1%/*}" 2>/dev/null || true
"#;

/// Removes a tree a `PLACE_TREE` or a `CLOSE` made read-only.
const REMOVE: &str = r#"[ -e "$1" ] || exit 0
chmod -R u+w "$1"
rm -rf "$1"
"#;

/// Makes `$1` an empty directory, whatever was there.
const EMPTY: &str = r#"set -e
if [ -e "$1" ]; then
  chmod -R u+w "$1"
  rm -rf "$1"
fi
mkdir -p "$1"
"#;

/// Makes `$1` a directory this user can write, keeping what is in it.
const OPEN: &str = r#"set -e
mkdir -p "$1"
chmod -R u+w "$1"
"#;

/// Makes a directory read-only for this user, for what a Session must not plant in it.
const CLOSE: &str = r#"[ -e "$1" ] || exit 0
chmod -R a-w "$1"
"#;

/// Says whether `$1` exists.
const EXISTS: &str = r#"[ -e "$1" ]
"#;

/// Writes standard input to the executable `$1`.
const WRITE_PROGRAM: &str = r#"set -e
mkdir -p "${1%/*}"
cat > "$1"
chmod 555 "$1"
"#;

/// Copies `$1` to `$2`.
const COPY: &str = r#"set -e
mkdir -p "${2%/*}"
cp "$1" "$2"
"#;

/// Finds each program named, on the `PATH` it is given: `NAME TAB PATH TAB VERSION`.
const FIND_TOOLS: &str = r#"for ai6_name in "$@"; do
  ai6_path=$(command -v "$ai6_name" 2>/dev/null) || continue
  ai6_version=$("$ai6_path" --version </dev/null 2>&1 | head -n 1)
  printf '%s\t%s\t%s\n' "$ai6_name" "$ai6_path" "$ai6_version"
done
exit 0
"#;

/// Runs a program in a process group of its own, so what it starts ends with it. `$1` is the
/// seconds it gets, 0 for no limit, and the rest is the program. `setsid` makes the group; where
/// there is none the shell's job control does. A watcher sent `TERM` ends its `sleep`, which would
/// otherwise hold the output open.
const IN_A_GROUP: &str = r#"ai6_limit=$1
shift
if command -v setsid >/dev/null 2>&1; then
  setsid "$@" </dev/null &
else
  set -m 2>/dev/null
  "$@" </dev/null &
fi
ai6_pid=$!
ai6_watch=
if [ "$ai6_limit" -gt 0 ]; then
  (
    trap 'kill "$ai6_sleep" 2>/dev/null; exit 0' TERM
    sleep "$ai6_limit" &
    ai6_sleep=$!
    wait "$ai6_sleep"
    kill -TERM -- "-$ai6_pid" 2>/dev/null || kill -TERM "$ai6_pid" 2>/dev/null
    sleep 2
    kill -KILL -- "-$ai6_pid" 2>/dev/null
  ) >/dev/null 2>&1 &
  ai6_watch=$!
fi
wait "$ai6_pid"
ai6_code=$?
[ -n "$ai6_watch" ] && kill "$ai6_watch" 2>/dev/null
kill -TERM -- "-$ai6_pid" 2>/dev/null
exit "$ai6_code"
"#;

/// One program a preparation runs.
pub(super) struct Step<'a> {
    /// The program and its arguments, never read by a shell.
    pub(super) argv: Vec<String>,
    /// Its working directory.
    pub(super) cwd: Option<&'a str>,
    /// Its environment.
    pub(super) env: &'a BTreeMap<String, String>,
    /// How long it gets; `None` for no limit.
    pub(super) limit: Option<Duration>,
    /// Bytes of each output stream kept.
    pub(super) max_output: u64,
}

/// What a finished [`Step`] left.
pub(super) struct Finished {
    pub(super) code: Option<i64>,
    pub(super) timed_out: bool,
    pub(super) stdout: Vec<u8>,
    pub(super) stderr: Vec<u8>,
}

impl Finished {
    /// The exit status and the last line of standard error, for a message.
    pub(super) fn failure(&self) -> String {
        let status = if self.timed_out {
            "stopped at its time limit".to_owned()
        } else {
            self.code.map_or_else(
                || "no exit status".to_owned(),
                |code| format!("exit {code}"),
            )
        };
        match String::from_utf8_lossy(&self.stderr).trim().lines().last() {
            Some(line) => format!("{status}: {line}"),
            None => status,
        }
    }

    pub(super) fn succeeded(&self) -> bool {
        self.code == Some(0) && !self.timed_out
    }
}

/// A program found where a Session runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Found {
    /// Its absolute path.
    pub(crate) path: String,
    /// The first line it printed for `--version`.
    pub(crate) version: String,
}

/// Where overlays are put.
pub(crate) enum Site {
    /// The Station's own host.
    Here {
        /// The store's directory.
        dir: PathBuf,
    },
    /// A Theater, reached through its transport.
    Theater {
        /// Its ID, for messages.
        id: String,
        /// Its transport.
        client: Arc<TransportClient>,
        /// Its operating system, as AOS names it.
        os: String,
        /// The absolute directory that holds its overlays.
        root: String,
        /// Whether the transport writes files itself.
        files: bool,
        /// Whether it runs as root, whom file modes do not stop.
        root_user: bool,
    },
}

impl Site {
    /// A Theater reached through `client`, its overlay directory made if need be.
    pub(crate) fn theater(id: &str, client: Arc<TransportClient>) -> Result<Self, String> {
        let capabilities = client.capabilities().map_err(|error| describe(&error))?;
        if !capabilities.run {
            return Err(format!(
                "Theater {id:?} cannot run a command, so nothing can be placed on it"
            ));
        }
        let os = capabilities.os.clone().unwrap_or_default();
        if os == "windows" {
            return Err(format!(
                "overlays reach a Theater through a POSIX shell, and {id:?} runs Windows"
            ));
        }
        let shell = |script: &str, args: &[&str]| shell_argv(script, args);
        let output = client
            .run(&RunSpec {
                argv: shell(HOME, &[THEATER_ROOT]),
                timeout_ms: 60_000,
                ..RunSpec::default()
            })
            .map_err(|error| format!("Theater {id:?}: {}", describe(&error)))?;
        let printed = String::from_utf8_lossy(&output.stdout.0).into_owned();
        let mut lines = printed.lines();
        let root = lines.next().unwrap_or_default().trim().to_owned();
        let root_user = lines.next().is_some_and(|uid| uid.trim() == "0");
        if output.code != Some(0) || !root.starts_with('/') {
            return Err(format!(
                "Theater {id:?}: its overlay directory could not be made: {}",
                String::from_utf8_lossy(&output.stderr.0).trim()
            ));
        }
        Ok(Self::Theater {
            id: id.to_owned(),
            client,
            os: if os.is_empty() {
                "linux".to_owned()
            } else {
                os
            },
            root,
            files: capabilities.files,
            root_user,
        })
    }

    /// Whether a process here is stopped by the protection a copy has. An elevated Windows token gets
    /// past a deny entry for what asks to back up (Git Bash wrote a new file through one), and root
    /// past file modes; a limited token is stopped.
    pub(super) fn protection_holds(&self) -> bool {
        match self {
            Self::Here { .. } => !crate::station::elevation::is_elevated().unwrap_or(false),
            Self::Theater { root_user, .. } => !root_user,
        }
    }

    /// The operating system, as AOS names it.
    pub(super) fn os(&self) -> &str {
        match self {
            Self::Here { .. } => crate::directorates::q::overlays::current_os(),
            Self::Theater { os, .. } => os,
        }
    }

    /// What messages call it.
    pub(super) fn name(&self) -> String {
        match self {
            Self::Here { .. } => "this Station's host".to_owned(),
            Self::Theater { id, .. } => format!("Theater {id:?}"),
        }
    }

    /// The key a setup record is kept under.
    pub(super) fn key(&self) -> String {
        match self {
            Self::Here { .. } => "local".to_owned(),
            Self::Theater { id, .. } => format!("theater-{id}"),
        }
    }

    /// `relative`, `/`-separated, under the directory that holds overlays here.
    pub(super) fn path(&self, relative: &str) -> String {
        match self {
            Self::Here { dir } => dir
                .join(relative.replace('/', std::path::MAIN_SEPARATOR_STR))
                .to_string_lossy()
                .into_owned(),
            Self::Theater { root, .. } => format!("{root}/{relative}"),
        }
    }

    fn script(
        client: &TransportClient,
        script: &str,
        args: &[&str],
        stdin: Option<Vec<u8>>,
    ) -> Result<Output, String> {
        client
            .run(&RunSpec {
                argv: shell_argv(script, args),
                stdin: stdin.map(Bytes),
                timeout_ms: 600_000,
                ..RunSpec::default()
            })
            .map_err(|error| describe(&error))
    }

    fn checked(
        client: &TransportClient,
        script: &str,
        args: &[&str],
        stdin: Option<Vec<u8>>,
    ) -> Result<(), String> {
        let output = Self::script(client, script, args, stdin)?;
        if output.code == Some(0) {
            Ok(())
        } else {
            Err(String::from_utf8_lossy(&output.stderr.0).trim().to_owned())
        }
    }

    /// The `PATH` a process started with no other has here.
    pub(super) fn login_path(&self) -> Result<String, String> {
        match self {
            Self::Here { .. } => Ok(programs::NO_PATH_SEARCH.to_owned()),
            Self::Theater { client, id, .. } => {
                let output = Self::script(client, LOGIN_PATH, &[], None)
                    .map_err(|why| format!("Theater {id:?}: {why}"))?;
                Ok(String::from_utf8_lossy(&output.stdout.0).trim().to_owned())
            }
        }
    }

    /// Whether `path` exists.
    pub(super) fn exists(&self, path: &str) -> Result<bool, String> {
        match self {
            Self::Here { .. } => Ok(Path::new(path).exists()),
            Self::Theater { client, .. } => {
                Ok(Self::script(client, EXISTS, &[path], None)?.code == Some(0))
            }
        }
    }

    /// Makes `path` a directory this user can write, keeping what is in it.
    pub(super) fn open_dir(&self, path: &str) -> Result<(), String> {
        match self {
            Self::Here { .. } => {
                if Path::new(path).exists() {
                    open_here(Path::new(path)).map_err(|why| why.to_string())?;
                }
                fs::create_dir_all(path).map_err(|why| why.to_string())
            }
            Self::Theater { client, .. } => Self::checked(client, OPEN, &[path], None),
        }
    }

    /// Makes `path` read-only.
    pub(super) fn close_dir(&self, path: &str) -> Result<(), String> {
        match self {
            Self::Here { .. } => {
                if Path::new(path).exists() {
                    protect(Path::new(path)).map_err(|why| why.to_string())
                } else {
                    Ok(())
                }
            }
            Self::Theater { client, .. } => Self::checked(client, CLOSE, &[path], None),
        }
    }

    /// Makes `path` an empty directory, whatever was there.
    pub(super) fn empty_dir(&self, path: &str) -> Result<(), String> {
        match self {
            Self::Here { .. } => {
                if Path::new(path).exists() {
                    release(Path::new(path)).map_err(|why| why.to_string())?;
                }
                fs::create_dir_all(path).map_err(|why| why.to_string())
            }
            Self::Theater { client, .. } => Self::checked(client, EMPTY, &[path], None),
        }
    }

    /// Removes `path` and what is under it, if it is there.
    pub(crate) fn remove(&self, path: &str) -> Result<(), String> {
        match self {
            Self::Here { .. } => {
                if Path::new(path).exists() {
                    release(Path::new(path)).map_err(|why| why.to_string())
                } else {
                    Ok(())
                }
            }
            Self::Theater { client, .. } => Self::checked(client, REMOVE, &[path], None),
        }
    }

    /// Copies a file already here to `target`, its directory made.
    pub(super) fn copy_file(&self, source: &str, target: &str) -> Result<(), String> {
        match self {
            Self::Here { .. } => {
                if let Some(parent) = Path::new(target).parent() {
                    fs::create_dir_all(parent).map_err(|why| why.to_string())?;
                }
                fs::copy(source, target)
                    .map(drop)
                    .map_err(|why| format!("{target}: {why}"))
            }
            Self::Theater { client, .. } => Self::checked(client, COPY, &[source, target], None),
        }
    }

    /// Writes `bytes` to the file `path`, its directory made.
    pub(super) fn write_file(&self, path: &str, bytes: &[u8]) -> Result<(), String> {
        match self {
            Self::Here { .. } => {
                let path = Path::new(path);
                if let Some(parent) = path.parent() {
                    fs::create_dir_all(parent).map_err(|why| why.to_string())?;
                }
                fs::write(path, bytes).map_err(|why| format!("{}: {why}", path.display()))
            }
            Self::Theater { client, .. } => {
                Self::checked(client, COPY_IN, &[path], Some(bytes.to_vec()))
            }
        }
    }

    /// Writes `bytes` to the program `path`.
    pub(super) fn write_program(&self, path: &str, bytes: &[u8]) -> Result<(), String> {
        match self {
            Self::Here { .. } => {
                let path = Path::new(path);
                if let Some(parent) = path.parent() {
                    fs::create_dir_all(parent).map_err(|why| why.to_string())?;
                }
                fs::write(path, bytes).map_err(|why| format!("{}: {why}", path.display()))?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    fs::set_permissions(path, fs::Permissions::from_mode(0o555))
                        .map_err(|why| why.to_string())?;
                }
                Ok(())
            }
            Self::Theater { client, .. } => {
                Self::checked(client, WRITE_PROGRAM, &[path], Some(bytes.to_vec()))
            }
        }
    }

    /// The read-only copy of a stored tree here, placed if it is not, its every file checked
    /// against `files`. `kind` is `trees` or `plugins`, as on the Station. A copy with other bytes
    /// refuses: nothing on a Theater is taken over by a start, and the Station cannot tell who
    /// changed it.
    pub(super) fn tree(
        &self,
        kind: &str,
        digest: &Digest,
        source: &Path,
        files: &BTreeMap<String, Digest>,
        limits: &TreeLimits,
    ) -> Result<String, String> {
        let target = self.path(&format!("{kind}/{}", hex_of(digest)));
        let Self::Theater { client, id, .. } = self else {
            return Ok(target);
        };
        let room = (limits.files.or_max() as u64)
            .saturating_mul(4096)
            .max(DEFAULT_LISTING_BYTES);
        let hashed = client
            .run(&RunSpec {
                argv: shell_argv(HASH_TREE, &[&target]),
                timeout_ms: 600_000,
                max_output: room,
                ..RunSpec::default()
            })
            .map_err(|error| format!("Theater {id:?}: {}", describe(&error)))?;
        match hashed.code {
            Some(44) => {
                self.place(kind, digest, source, files)?;
                Ok(target)
            }
            Some(0) => {
                check_listing(&String::from_utf8_lossy(&hashed.stdout.0), files).map_err(
                    |why| {
                        format!(
                            "Theater {id:?}: the copy at {target} {why}; it changed after it was \
                         placed, so nothing issues it. Remove that directory to place it again"
                        )
                    },
                )?;
                Ok(target)
            }
            _ => Err(format!(
                "Theater {id:?}: the copy at {target} cannot be checked: {}",
                String::from_utf8_lossy(&hashed.stderr.0).trim()
            )),
        }
    }

    /// Copies a tree to the Theater file by file, appearing there only when whole.
    fn place(
        &self,
        kind: &str,
        digest: &Digest,
        source: &Path,
        files: &BTreeMap<String, Digest>,
    ) -> Result<(), String> {
        let Self::Theater {
            client,
            id,
            files: writes,
            ..
        } = self
        else {
            return Ok(());
        };
        let partial = self.path(&format!("partial/{}", uuid::Uuid::new_v4().simple()));
        let target = self.path(&format!("{kind}/{}", hex_of(digest)));
        let refuse = |why: String| format!("Theater {id:?}: {why}");
        let mut executable = String::new();
        for path in files.keys() {
            let bytes = fs::read(source.join(path))
                .map_err(|why| format!("{path} in the Station's store: {why}"))?;
            if bytes.starts_with(b"#!") || path.starts_with("bin/") {
                executable.push_str(&format!("./{path}\n"));
            }
            let file = format!("{partial}/{path}");
            if *writes {
                client
                    .write_file(&WriteSpec {
                        path: file,
                        data: Bytes(bytes),
                        append: false,
                        create_dirs: true,
                    })
                    .map_err(|error| refuse(describe(&error)))?;
            } else {
                Self::checked(client, COPY_IN, &[&file], Some(bytes)).map_err(refuse)?;
            }
        }
        Self::checked(
            client,
            PLACE_TREE,
            &[&partial, &target],
            Some(executable.into_bytes()),
        )
        .map_err(|why| {
            let _ = Self::checked(client, REMOVE, &[&partial], None);
            refuse(why)
        })
    }

    /// Runs `step` to its end in a process group of its own, so what it starts ends with it.
    pub(super) fn run(&self, step: &Step<'_>) -> Result<Finished, String> {
        match self {
            Self::Here { .. } => run_here(step),
            Self::Theater { client, id, .. } => {
                let seconds = step.limit.map_or(0, |limit| limit.as_secs().max(1));
                let mut argv = shell_argv(IN_A_GROUP, &[&seconds.to_string()]);
                argv.extend(step.argv.iter().cloned());
                let started = Instant::now();
                let output = client
                    .run(&RunSpec {
                        argv,
                        cwd: step.cwd.map(str::to_owned),
                        env: Some(step.env.clone()),
                        stdin: None,
                        // The script keeps the time itself, and ends what the step started.
                        timeout_ms: step.limit.map_or(0, |limit| {
                            millis(limit.saturating_add(Duration::from_secs(30)))
                        }),
                        max_output: step.max_output,
                    })
                    .map_err(|error| format!("Theater {id:?}: {}", describe(&error)))?;
                Ok(finished(output, step.limit, started.elapsed()))
            }
        }
    }

    /// Each of `names` found on `path`, with its absolute path and version. A batch file is not
    /// found: `cmd.exe` would read its arguments again (`spec/v0/runtime.md#interpreters`).
    pub(super) fn find_tools(
        &self,
        names: &[&str],
        path: &str,
    ) -> Result<BTreeMap<String, Found>, String> {
        match self {
            Self::Here { .. } => Ok(find_here(names, path)),
            Self::Theater { client, id, .. } => {
                let mut env = BTreeMap::new();
                if !path.is_empty() {
                    env.insert("PATH".to_owned(), path.to_owned());
                }
                let output = client
                    .run(&RunSpec {
                        argv: shell_argv(FIND_TOOLS, names),
                        env: Some(env),
                        timeout_ms: 60_000,
                        ..RunSpec::default()
                    })
                    .map_err(|error| format!("Theater {id:?}: {}", describe(&error)))?;
                Ok(parse_tools(&String::from_utf8_lossy(&output.stdout.0)))
            }
        }
    }
}

/// A listing a tree of up to this many bytes of names is sure to fit.
const DEFAULT_LISTING_BYTES: u64 = 1024 * 1024;

/// Writes standard input to `$1`, for a transport with no file access.
const COPY_IN: &str = r#"set -e
mkdir -p "${1%/*}"
cat > "$1"
"#;

fn shell_argv(script: &str, args: &[&str]) -> Vec<String> {
    let mut argv = vec![
        "sh".to_owned(),
        "-c".to_owned(),
        script.to_owned(),
        "sh".to_owned(),
    ];
    argv.extend(args.iter().map(|arg| (*arg).to_owned()));
    argv
}

fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

fn finished(output: Output, limit: Option<Duration>, elapsed: Duration) -> Finished {
    // The group script ends a step at its limit with TERM or KILL.
    let stopped = matches!(output.code, Some(143 | 137 | -15 | -9))
        && limit.is_some_and(|limit| elapsed >= limit);
    Finished {
        timed_out: output.timed_out || stopped,
        code: output.code,
        stdout: output.stdout.0,
        stderr: output.stderr.0,
    }
}

/// Compares `HEX  ./PATH` lines with what the tree holds; the names a tree digest leaves out do not
/// count.
fn check_listing(listing: &str, files: &BTreeMap<String, Digest>) -> Result<(), String> {
    let mut seen: BTreeMap<String, String> = BTreeMap::new();
    for line in listing.lines() {
        if let Some(other) = line.strip_prefix("other\t") {
            return Err(format!("holds {other}, which is no file or directory"));
        }
        let Some((hex, path)) = line.split_once(' ') else {
            return Err(format!("was listed in an unreadable line: {line:?}"));
        };
        let path = path.trim_start_matches([' ', '*']);
        let path = path.strip_prefix("./").unwrap_or(path);
        if valid_relative_path(path) {
            seen.insert(path.to_owned(), format!("sha256:{hex}"));
        }
    }
    for (path, digest) in files {
        match seen.remove(path) {
            None => return Err(format!("has no {path}")),
            Some(found) if found != digest.to_string() => {
                return Err(format!("has other bytes in {path}"));
            }
            Some(_) => {}
        }
    }
    match seen.keys().next() {
        Some(extra) => Err(format!("has a file it was not given, {extra}")),
        None => Ok(()),
    }
}

fn parse_tools(listing: &str) -> BTreeMap<String, Found> {
    listing
        .lines()
        .filter_map(|line| {
            let mut fields = line.splitn(3, '\t');
            let (name, path) = (fields.next()?, fields.next()?);
            Some((
                name.to_owned(),
                Found {
                    path: path.to_owned(),
                    version: fields.next().unwrap_or_default().to_owned(),
                },
            ))
        })
        .collect()
}

fn find_here(names: &[&str], path: &str) -> BTreeMap<String, Found> {
    let environment = BTreeMap::from([("PATH".to_owned(), path.to_owned())]);
    let mut found = BTreeMap::new();
    for name in names {
        let Some(program) = programs::find_program(name, &environment) else {
            continue;
        };
        let text = program.to_string_lossy().into_owned();
        let lower = text.to_ascii_lowercase();
        if lower.ends_with(".cmd") || lower.ends_with(".bat") {
            continue;
        }
        let output = run_process(
            Command::new(&program).arg("--version"),
            &ProcessInput {
                stdin: None,
                timeout_ms: 30_000,
                max_output: 64 * 1024,
            },
            Duration::from_secs(1),
        );
        // A stub that cannot run (Windows' store alias for Python) is not the program.
        let Ok(output) = output else { continue };
        if output.code != Some(0) {
            continue;
        }
        let version = [&output.stdout.0, &output.stderr.0]
            .iter()
            .find_map(|stream| {
                String::from_utf8_lossy(stream)
                    .lines()
                    .next()
                    .map(str::to_owned)
            })
            .unwrap_or_default();
        found.insert(
            (*name).to_owned(),
            Found {
                path: text,
                version,
            },
        );
    }
    found
}

/// Takes a store directory's protection off, keeping its files.
#[cfg(unix)]
fn open_here(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let metadata = fs::symlink_metadata(path)?;
    if metadata.is_dir() {
        fs::set_permissions(path, fs::Permissions::from_mode(0o755))?;
        for entry in fs::read_dir(path)? {
            open_here(&entry?.path())?;
        }
    } else if metadata.is_file() {
        let mode = metadata.permissions().mode() | 0o200;
        fs::set_permissions(path, fs::Permissions::from_mode(mode))?;
    }
    Ok(())
}

#[cfg(windows)]
fn open_here(path: &Path) -> std::io::Result<()> {
    icacls()
        .arg(path)
        .args(["/reset", "/T", "/C", "/Q"])
        .output()
        .map(drop)
}

#[cfg(not(any(unix, windows)))]
fn open_here(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

/// Runs a step on this host through the same script a Theater's goes through.
#[cfg(unix)]
fn run_here(step: &Step<'_>) -> Result<Finished, String> {
    let seconds = step.limit.map_or(0, |limit| limit.as_secs().max(1));
    // By its path: the Session's PATH need not have it.
    let mut command = Command::new("/bin/sh");
    command.args(
        shell_argv(IN_A_GROUP, &[&seconds.to_string()])
            .iter()
            .skip(1),
    );
    command.args(&step.argv).env_clear().envs(step.env);
    if let Some(cwd) = step.cwd {
        command.current_dir(cwd);
    }
    let started = Instant::now();
    let output = run_process(
        &mut command,
        &ProcessInput {
            stdin: None,
            timeout_ms: step.limit.map_or(0, |limit| {
                millis(limit.saturating_add(Duration::from_secs(30)))
            }),
            max_output: step.max_output,
        },
        Duration::from_secs(1),
    )
    .map_err(|error| describe(&error))?;
    Ok(finished(output, step.limit, started.elapsed()))
}

/// Runs a step on this host, ending its whole process tree at its time limit. A process it left
/// running when it ended by itself is not found, since Windows keeps no group of it here.
#[cfg(not(unix))]
fn run_here(step: &Step<'_>) -> Result<Finished, String> {
    let mut command = Command::new(step.argv.first().ok_or("an empty command")?);
    command
        .args(&step.argv[1..])
        .env_clear()
        .envs(step.env)
        .stdin(std::process::Stdio::null());
    if let Some(cwd) = step.cwd {
        command.current_dir(cwd);
    }
    let files = tempfile::tempdir().map_err(|why| why.to_string())?;
    let (out, err) = (files.path().join("out"), files.path().join("err"));
    command.stdout(fs::File::create(&out).map_err(|why| why.to_string())?);
    command.stderr(fs::File::create(&err).map_err(|why| why.to_string())?);
    let mut child = command
        .spawn()
        .map_err(|why| format!("{}: {why}", step.argv[0]))?;
    let started = Instant::now();
    let mut timed_out = false;
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|why| why.to_string())? {
            break status;
        }
        if step.limit.is_some_and(|limit| started.elapsed() >= limit) {
            timed_out = true;
            let system = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
            let _ = Command::new(Path::new(&system).join("System32").join("taskkill.exe"))
                .args(["/T", "/F", "/PID", &child.id().to_string()])
                .output();
            break child.wait().map_err(|why| why.to_string())?;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let read = |path: &Path| {
        use std::io::Read as _;
        let mut kept = Vec::new();
        if let Ok(file) = fs::File::open(path) {
            let _ = file.take(step.max_output).read_to_end(&mut kept);
        }
        kept
    };
    Ok(Finished {
        code: status.code().map(i64::from),
        timed_out,
        stdout: read(&out),
        stderr: read(&err),
    })
}

/// The files of a tree in the store, each with its digest.
pub(super) fn store_tree_files(
    root: &Path,
    limits: &TreeLimits,
) -> Result<BTreeMap<String, Digest>, String> {
    tree_digest(root, limits)
        .map(|digest| digest.files)
        .map_err(|why| why.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_listing_is_held_to_every_file_of_the_tree() {
        let digest = Digest::of(b"a");
        let hex = hex_of(&digest);
        let files = BTreeMap::from([("kb/a.md".to_owned(), digest)]);
        let good = format!("{hex}  ./kb/a.md\n");
        assert_eq!(check_listing(&good, &files), Ok(()));
        // `shasum` in binary mode and `sha256 -r` write it differently.
        assert_eq!(
            check_listing(&format!("{hex} *./kb/a.md\n"), &files),
            Ok(())
        );
        assert_eq!(check_listing(&format!("{hex} ./kb/a.md\n"), &files), Ok(()));
        assert!(
            check_listing("", &files)
                .unwrap_err()
                .contains("has no kb/a.md")
        );
        let other = format!("{}  ./kb/a.md\n", hex_of(&Digest::of(b"b")));
        assert!(
            check_listing(&other, &files)
                .unwrap_err()
                .contains("other bytes")
        );
        let extra = format!("{good}{hex}  ./kb/b.md\n");
        assert!(
            check_listing(&extra, &files)
                .unwrap_err()
                .contains("kb/b.md")
        );
        let link = "other\t./kb/link\n";
        assert!(check_listing(link, &files).unwrap_err().contains("kb/link"));
        // What a Python run left is no part of a tree.
        let cache = format!("{good}{hex}  ./lib/python/__pycache__/m.pyc\n");
        assert_eq!(check_listing(&cache, &files), Ok(()));
    }

    #[test]
    fn programs_are_read_from_the_tab_separated_listing() {
        let found = parse_tools("python3\t/usr/bin/python3\tPython 3.12.1\nsh\t/bin/sh\t\n");
        assert_eq!(found["python3"].path, "/usr/bin/python3");
        assert_eq!(found["python3"].version, "Python 3.12.1");
        assert_eq!(found["sh"].version, "");
    }

    #[test]
    fn a_path_under_a_site_uses_its_own_separator() {
        let client = Arc::new(TransportClient::new(Box::new(
            crate::station::transports::testing::DirTransport::new(Path::new(".")),
        )));
        let theater = Site::Theater {
            id: "box".to_owned(),
            client,
            os: "linux".to_owned(),
            root: "/home/a/.ai6/overlays".to_owned(),
            files: true,
            root_user: false,
        };
        assert_eq!(theater.path("trees/x"), "/home/a/.ai6/overlays/trees/x");
        assert_eq!(theater.key(), "theater-box");
    }
}
