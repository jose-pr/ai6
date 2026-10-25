//! Inert bubblewrap argument generation with canonical path confinement.

use super::SandboxProfile;
use std::fmt;
use std::path::{Path, PathBuf};

const WORKTREE: &str = "$WORKTREE";
const WORKSPACE: &str = "/workspace";

/// Inputs for translating a sandbox profile into a process argument vector.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxRequest {
    worktree: PathBuf,
    command: Vec<String>,
    allowed_roots: Vec<PathBuf>,
    executable: String,
}

impl SandboxRequest {
    /// Creates a request for an existing Mission worktree and an argument vector.
    pub fn new(worktree: impl Into<PathBuf>, command: impl IntoIterator<Item = String>) -> Self {
        Self {
            worktree: worktree.into(),
            command: command.into_iter().collect(),
            allowed_roots: Vec::new(),
            executable: "bwrap".to_owned(),
        }
    }

    /// Adds a root that may contain additional absolute mounts.
    pub fn with_allowed_root(mut self, root: impl Into<PathBuf>) -> Self {
        self.allowed_roots.push(root.into());
        self
    }

    /// Selects the bubblewrap executable name or path.
    pub fn with_executable(mut self, executable: impl Into<String>) -> Self {
        self.executable = executable.into();
        self
    }
}

/// A rejected or unsupported sandbox translation.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxError {
    /// The configured backend is not supported by this translator.
    UnsupportedBackend,
    /// Bubblewrap translation is available only on Linux.
    UnsupportedPlatform,
    /// The profile requests network access that bubblewrap cannot constrain here.
    UnsupportedNetworkPolicy,
    /// The command vector or executable is invalid.
    InvalidCommand,
    /// A configured path is missing, outside its authorized root or ambiguous.
    InvalidPath,
    /// The worktree path does not resolve to a directory.
    InvalidWorktree,
    /// The policy does not mount the worktree at `/workspace`.
    MissingWorktreeMount,
    /// Two mounts resolve to the same destination.
    DuplicateMount,
}

impl fmt::Display for SandboxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::UnsupportedBackend => "sandbox backend is unsupported",
            Self::UnsupportedPlatform => "bubblewrap sandbox translation requires Linux",
            Self::UnsupportedNetworkPolicy => {
                "bubblewrap cannot enforce the requested network policy"
            }
            Self::InvalidCommand => "sandbox command vector is invalid",
            Self::InvalidPath => "sandbox path is missing or outside its authorized roots",
            Self::InvalidWorktree => "Mission worktree is unavailable or not a directory",
            Self::MissingWorktreeMount => "sandbox policy must mount the worktree at /workspace",
            Self::DuplicateMount => "sandbox policy contains duplicate mount destinations",
        })
    }
}

impl std::error::Error for SandboxError {}

impl SandboxProfile {
    /// Returns the bubblewrap argument vector without invoking a process or shell.
    pub fn bubblewrap_argv(&self, request: &SandboxRequest) -> Result<Vec<String>, SandboxError> {
        build_argv(self, request, cfg!(target_os = "linux"))
    }
}

fn build_argv(
    profile: &SandboxProfile,
    request: &SandboxRequest,
    is_linux: bool,
) -> Result<Vec<String>, SandboxError> {
    if profile.backend != "bubblewrap" {
        return Err(SandboxError::UnsupportedBackend);
    }
    if !is_linux {
        return Err(SandboxError::UnsupportedPlatform);
    }
    if !profile.network.is_empty() {
        return Err(SandboxError::UnsupportedNetworkPolicy);
    }
    if request.executable.is_empty()
        || request.executable.chars().any(char::is_control)
        || request.command.is_empty()
        || request
            .command
            .iter()
            .any(|value| value.chars().any(char::is_control))
        || request.command[0].is_empty()
    {
        return Err(SandboxError::InvalidCommand);
    }

    let root = request
        .worktree
        .canonicalize()
        .map_err(|_| SandboxError::InvalidWorktree)?;
    if !root.is_dir() {
        return Err(SandboxError::InvalidWorktree);
    }
    let allowed_roots = request
        .allowed_roots
        .iter()
        .map(|path| {
            let path = path.canonicalize().map_err(|_| SandboxError::InvalidPath)?;
            if !path.is_dir() {
                return Err(SandboxError::InvalidPath);
            }
            Ok(path)
        })
        .collect::<Result<Vec<_>, _>>()?;

    let mut mounts = Vec::new();
    for (mode, paths) in [
        ("bind", &profile.read_write),
        ("ro-bind", &profile.read_only),
    ] {
        for declaration in paths {
            let (source, destination) = resolve_mount(declaration, &root, &allowed_roots)?;
            mounts.push((mode, source, destination));
        }
    }
    if !mounts
        .iter()
        .any(|(_, source, destination)| source == &root && destination == WORKSPACE)
    {
        return Err(SandboxError::MissingWorktreeMount);
    }
    mounts.sort_by_key(|(_, _, destination)| destination.matches('/').count());
    let mut destinations = mounts
        .iter()
        .map(|(_, _, destination)| destination.as_str())
        .collect::<Vec<_>>();
    destinations.sort_unstable();
    if destinations.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(SandboxError::DuplicateMount);
    }

    let mut args = vec![
        request.executable.clone(),
        "--die-with-parent".to_owned(),
        "--new-session".to_owned(),
        "--unshare-user".to_owned(),
        "--unshare-pid".to_owned(),
        "--unshare-ipc".to_owned(),
        "--unshare-uts".to_owned(),
        "--unshare-net".to_owned(),
        "--clearenv".to_owned(),
        "--setenv".to_owned(),
        "PATH".to_owned(),
        "/usr/bin:/bin".to_owned(),
        "--setenv".to_owned(),
        "HOME".to_owned(),
        "/tmp".to_owned(),
        "--proc".to_owned(),
        "/proc".to_owned(),
        "--dev".to_owned(),
        "/dev".to_owned(),
        "--tmpfs".to_owned(),
        "/tmp".to_owned(),
        "--dir".to_owned(),
        WORKSPACE.to_owned(),
    ];
    for (mode, source, destination) in mounts {
        args.push(format!("--{mode}"));
        args.push(path_text(&source)?);
        args.push(destination);
    }
    args.push("--chdir".to_owned());
    args.push(WORKSPACE.to_owned());
    args.push("--".to_owned());
    args.extend(request.command.iter().cloned());
    Ok(args)
}

fn resolve_mount(
    declaration: &str,
    worktree: &Path,
    allowed_roots: &[PathBuf],
) -> Result<(PathBuf, String), SandboxError> {
    if declaration == WORKTREE || declaration.starts_with(&format!("{WORKTREE}/")) {
        let suffix = declaration.strip_prefix(WORKTREE).unwrap_or_default();
        let candidate = worktree
            .join(suffix.trim_start_matches('/'))
            .canonicalize()
            .map_err(|_| SandboxError::InvalidPath)?;
        let relative = candidate
            .strip_prefix(worktree)
            .map_err(|_| SandboxError::InvalidPath)?;
        let suffix = path_text(relative)?;
        let destination = if suffix.is_empty() {
            WORKSPACE.to_owned()
        } else {
            format!("{WORKSPACE}/{suffix}")
        };
        return Ok((candidate, destination));
    }

    let path = Path::new(declaration);
    if !path.is_absolute() {
        return Err(SandboxError::InvalidPath);
    }
    let candidate = path.canonicalize().map_err(|_| SandboxError::InvalidPath)?;
    if !allowed_roots.iter().any(|root| candidate.starts_with(root)) {
        return Err(SandboxError::InvalidPath);
    }
    let destination = path_text(&candidate)?;
    Ok((candidate, destination))
}

fn path_text(path: &Path) -> Result<String, SandboxError> {
    path.to_str()
        .map(|value| value.replace('\\', "/"))
        .ok_or(SandboxError::InvalidPath)
}

#[cfg(test)]
mod tests {
    use super::{SandboxError, SandboxRequest, build_argv};
    use crate::directorates::q::LoadoutCatalog;
    use std::str::FromStr;
    use tempfile::tempdir;

    fn profile(
        read_write: &str,
        read_only: &str,
        network: &str,
        backend: &str,
    ) -> super::super::SandboxProfile {
        let document = format!(
            "[loadout.007]\nsandbox='profile'\n[sandbox.profile]\nbackend='{backend}'\nread_write={read_write}\nread_only={read_only}\nnetwork={network}"
        );
        LoadoutCatalog::from_str(&document)
            .unwrap()
            .sandboxes()
            .next()
            .unwrap()
            .clone()
    }

    #[test]
    fn generated_args_isolate_network_clear_environment_and_mount_worktree() {
        let temp = tempdir().unwrap();
        let worktree = temp.path().join("worktree");
        std::fs::create_dir_all(worktree.join("src")).unwrap();
        let profile = profile("['$WORKTREE']", "['$WORKTREE/src']", "[]", "bubblewrap");
        let request = SandboxRequest::new(&worktree, ["agent".to_owned(), "arg".to_owned()]);
        let args = build_argv(&profile, &request, true).unwrap();
        assert!(
            args.windows(2)
                .any(|pair| pair == ["--unshare-net", "--clearenv"])
        );
        assert!(
            args.windows(3)
                .any(|pair| pair[0] == "--bind" && pair[2] == "/workspace")
        );
        assert!(
            args.windows(3)
                .any(|pair| pair[0] == "--ro-bind" && pair[2] == "/workspace/src")
        );
        assert_eq!(&args[args.len() - 3..], ["--", "agent", "arg"]);
    }

    #[test]
    fn parent_mounts_precede_narrower_mounts_and_external_paths_need_roots() {
        let temp = tempdir().unwrap();
        let worktree = temp.path().join("worktree");
        let external = temp.path().join("external");
        std::fs::create_dir_all(worktree.join("src")).unwrap();
        std::fs::create_dir_all(&external).unwrap();
        let profile = profile(
            "['$WORKTREE']",
            &format!("['{}', '$WORKTREE/src']", external.display()),
            "[]",
            "bubblewrap",
        );
        let request = SandboxRequest::new(&worktree, ["agent".to_owned()]);
        assert_eq!(
            build_argv(&profile, &request, true),
            Err(SandboxError::InvalidPath)
        );
        let args = build_argv(&profile, &request.with_allowed_root(&external), true).unwrap();
        let rw = args.iter().position(|arg| arg == "--bind").unwrap();
        let ro_worktree = args.iter().position(|arg| arg == "/workspace/src").unwrap();
        assert!(rw < ro_worktree);
    }

    #[test]
    fn unsupported_or_ambiguous_policy_fails_closed() {
        let temp = tempdir().unwrap();
        let worktree = temp.path().join("worktree");
        std::fs::create_dir_all(&worktree).unwrap();
        let request = SandboxRequest::new(&worktree, ["agent".to_owned()]);
        assert_eq!(
            build_argv(
                &profile("['$WORKTREE']", "[]", "['host:443']", "bubblewrap"),
                &request,
                true
            ),
            Err(SandboxError::UnsupportedNetworkPolicy)
        );
        assert_eq!(
            build_argv(
                &profile("['$WORKTREE']", "[]", "[]", "other"),
                &request,
                true
            ),
            Err(SandboxError::UnsupportedBackend)
        );
        assert_eq!(
            build_argv(
                &profile("['$WORKTREE']", "[]", "[]", "bubblewrap"),
                &request,
                false
            ),
            Err(SandboxError::UnsupportedPlatform)
        );
        assert_eq!(
            build_argv(&profile("[]", "[]", "[]", "bubblewrap"), &request, true),
            Err(SandboxError::MissingWorktreeMount)
        );
        assert_eq!(
            build_argv(
                &profile("['$WORKTREE']", "['$WORKTREE']", "[]", "bubblewrap"),
                &request,
                true,
            ),
            Err(SandboxError::DuplicateMount)
        );
    }

    #[test]
    fn invalid_commands_and_paths_are_rejected() {
        let temp = tempdir().unwrap();
        let worktree = temp.path().join("worktree");
        std::fs::create_dir_all(&worktree).unwrap();
        let sandbox = profile("['$WORKTREE']", "[]", "[]", "bubblewrap");
        assert_eq!(
            build_argv(
                &sandbox,
                &SandboxRequest::new(&worktree, Vec::<String>::new()),
                true
            ),
            Err(SandboxError::InvalidCommand)
        );
        let escape = profile("['$WORKTREE/../outside']", "[]", "[]", "bubblewrap");
        assert_eq!(
            build_argv(
                &escape,
                &SandboxRequest::new(&worktree, ["agent".to_owned()]),
                true
            ),
            Err(SandboxError::InvalidPath)
        );
    }
}
