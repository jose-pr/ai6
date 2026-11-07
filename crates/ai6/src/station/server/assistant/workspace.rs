//! A conversation's workspace on a Station and its commit cycle.
//!
//! When the job names a personal repository, the conversation's directory holds `personal/`
//! (a clone of that repository on `main`, through the `git` safe-house provider), one
//! `shared/<name>/` per shared repository (cloned afresh at each harness start, read-only for
//! her), `scratch/`, and the desk's `CLAUDE.md` and `AGENTS.md`. The same standing text goes
//! into a context file beside the directory, which the harness gets with its system prompt.
//!
//! When a turn ends, everything she changed in `personal/` is committed to `main`, authored by the
//! person. A `main` that refuses the work twice gets it on `moneypenny/<conversation digest>`.
//!
//! Each `propose_shared_change` call is reported with what the file it changes held in her
//! `shared/` checkout ([`AssistantSharedBase`]), so its commit is refused when that
//! repository's `main` has changed the file since.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use coqui_peer::PeerId;

use crate::bounds::Limit;
use crate::directorates::m::assistant::{
    AssistantDeskLimits, AssistantLimits, RepoKind, SKILLS_PLUGIN,
};
use crate::directorates::q::safehouse::{
    CheckpointFile, CheckpointSpec, SafeHouseCheckpoint, SafeHouseClone, SafeHouseProvider,
    SafeHouseSpec,
};
use crate::directorates::section_00::assistant::{
    ASSISTANT_MCP_SERVER, ASSISTANT_PUBLISH_PAGE_TOOL, ASSISTANT_SHARED_CHANGE_TOOL,
    AssistantChange, AssistantCommit, AssistantPageFile, AssistantSharedBase, AssistantWorkspace,
    repository_path,
};
use crate::station::safehouse::SafeHouse;

/// The personal repository's checkout, and the one directory besides `scratch` she may write.
pub(super) const PERSONAL: &str = "personal";
/// Where the shared repositories are checked out.
pub(super) const SHARED: &str = "shared";
/// Her scratch directory, gone with the conversation's directory.
pub(super) const SCRATCH: &str = "scratch";
/// The branch every repository is read from and saved to.
pub(super) const MAIN: &str = "main";
/// The safe-house provider the workspace's repositories are opened with.
pub(super) const PROVIDER: &str = "git";

/// Opens safe houses: the Station's provider, or a test's.
pub(super) type Houses = Arc<dyn Fn() -> Result<Box<dyn SafeHouseProvider>, String> + Send + Sync>;

/// One conversation's checkout of the personal repository and its note of the turn's start.
pub(super) struct Workspace {
    conversation_id: String,
    digest: String,
    personal: SafeHouse,
    path: PathBuf,
    author_name: String,
    author_email: String,
    /// The files of `personal/` at the start of the turn: size and modification time.
    files: BTreeMap<String, (u64, Option<SystemTime>)>,
    /// The shared repositories checked out under `shared/`, by name.
    shared: BTreeSet<String>,
    /// The most a save of her work commits, in bytes.
    save_bytes: Limit,
    /// What her shared bases and the walk of her folder are held to.
    limits: AssistantLimits,
}

impl Workspace {
    /// Makes the workspace in `area`, the conversation's empty directory, with the
    /// repositories' git directories in `repos` and the standing text in `context`, both
    /// outside `area`, each repository cloned and each save made within `limits`, her folder
    /// walked within `bounds`; answers it with notices for the conversation.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn prepare(
        houses: &Houses,
        area: &Path,
        repos: &Path,
        context: &Path,
        conversation_id: &str,
        job: &AssistantWorkspace,
        skills: &[String],
        limits: &AssistantDeskLimits,
        bounds: &AssistantLimits,
    ) -> Result<(Self, Vec<String>), String> {
        let provider = houses()?;
        let digest = &super::conversation_digest(conversation_id);
        let create = RepoKind::of(&job.personal)? == RepoKind::Local;
        let path = area.join(PERSONAL);
        let personal = open_clone(
            provider.as_ref(),
            &job.personal,
            &path,
            Some(&repos.join(PERSONAL)),
            create,
            digest,
            CloneLimits::of(limits),
        )
        .map_err(|reason| format!("the personal repository could not be opened: {reason}"))?;
        let mut workspace = Self {
            conversation_id: conversation_id.to_owned(),
            digest: digest.to_owned(),
            personal,
            path,
            author_name: job.author_name.clone(),
            author_email: job.author_email.clone(),
            files: BTreeMap::new(),
            shared: BTreeSet::new(),
            save_bytes: limits.save_bytes,
            limits: bounds.clone(),
        };
        let mut notices = Vec::new();
        let mut shared = Vec::new();
        for (name, repo) in &job.shared {
            let target = area.join(SHARED).join(name);
            let git_dir = repos.join(SHARED).join(name);
            match open_clone(
                provider.as_ref(),
                repo,
                &target,
                Some(&git_dir),
                false,
                digest,
                // Read only, so its history is not needed.
                CloneLimits {
                    depth: Some(1),
                    ..CloneLimits::of(limits)
                },
            ) {
                Ok(_) => shared.push(name.clone()),
                Err(reason) => notices.push(format!(
                    "The shared repository {name:?} could not be opened, so shared/{name}/ is \
                     missing: {reason}"
                )),
            }
        }
        fs::create_dir_all(area.join(SCRATCH)).map_err(|error| error.to_string())?;
        let standing = instructions(&shared, skills);
        workspace.shared = shared.into_iter().collect();
        write_file(area, &area.join("CLAUDE.md"), &standing)?;
        write_file(area, &area.join("AGENTS.md"), &standing)?;
        write_file(context.parent().unwrap_or(context), context, &standing)?;
        workspace.begin_turn();
        Ok((workspace, notices))
    }

    /// What each [`ASSISTANT_SHARED_CHANGE_TOOL`] call in a harness event's raw line (`data`)
    /// was made from: the file it changes as her `shared/<group>/` checkout holds it. A call
    /// naming a repository not checked out, a path that is not a plain file or no file, or
    /// one the desk cannot read gets none, and Universal Exports then refuses it.
    pub(super) fn shared_bases(&self, data: Option<&[u8]>) -> Vec<AssistantSharedBase> {
        shared_change_calls(data)
            .into_iter()
            .filter(|(group, path, _)| self.shared.contains(group) && repository_path(path))
            .filter_map(|(group, path, content)| {
                let file = in_tree(&self.area().join(SHARED).join(&group), &path);
                no_links(self.area(), &file).ok()?;
                let base = match fs::symlink_metadata(&file) {
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                    _ => Some(file_digest(&file)?),
                };
                Some(AssistantSharedBase {
                    group,
                    path,
                    content: sha256_hex(content.as_bytes()),
                    base,
                })
            })
            .take(self.limits.shared_bases.or_max())
            .collect()
    }

    /// The files `publish_page` publishes from `path`, a file or a directory under `personal/`
    /// or `scratch/`, each read to its cap: nothing outside those two, no `..`, no hidden name
    /// and no link is followed, and a file or a page past the page limits is refused, never
    /// cut.
    pub(super) fn page_files(
        &self,
        path: &str,
    ) -> Result<Vec<(AssistantPageFile, Vec<u8>)>, String> {
        read_page(self.area(), path, &self.limits)
    }

    /// The `path` of each [`ASSISTANT_PUBLISH_PAGE_TOOL`] call in a harness event's raw line
    /// that names one, in either harness's spelling.
    pub(super) fn page_calls(data: Option<&[u8]>) -> Vec<String> {
        publish_page_paths(data)
    }

    /// Her working directory, holding `personal/`.
    fn area(&self) -> &Path {
        self.path.parent().unwrap_or(&self.path)
    }

    /// Ends the last turn as the harness closes.
    pub(super) fn close(&mut self) -> Vec<String> {
        self.end_turn()
    }

    /// Notes what `personal/` holds as a turn starts.
    pub(super) fn begin_turn(&mut self) {
        self.files = stat_tree(&self.path, self.limits.personal_walk);
    }

    /// Ends a turn: saves what she changed in `personal/`, and answers what should be known.
    pub(super) fn end_turn(&mut self) -> Vec<String> {
        let changed = changed_files(
            &self.files,
            &stat_tree(&self.path, self.limits.personal_walk),
        );
        let notices = match self.save(&summary(&changed)) {
            Ok(notice) => notice.into_iter().collect(),
            Err(reason) => vec![format!(
                "Moneypenny's work in personal/ could not be saved: {reason}"
            )],
        };
        self.begin_turn();
        notices
    }

    /// Commits everything in `personal/` and pushes it; a notice when it landed elsewhere
    /// than `main`.
    fn save(&mut self, summary: &str) -> Result<Option<String>, String> {
        let message = format!(
            "moneypenny: {summary}\n\nVia: Moneypenny (conversation {})",
            self.conversation_id
        );
        let mut spec = self.spec(message, &self.author_name, &self.author_email, None);
        spec.max_bytes = self.save_bytes.get();
        let saved = self
            .personal
            .checkpoint(&spec)
            .map_err(|error| error.to_string())?;
        Ok((saved.branch != MAIN).then(|| moved_on(&saved.branch)))
    }

    fn spec(
        &self,
        message: String,
        name: &str,
        email: &str,
        files: Option<Vec<CheckpointFile>>,
    ) -> CheckpointSpec {
        CheckpointSpec {
            remote: "origin".to_owned(),
            message,
            author_name: Some(name.to_owned()),
            author_email: Some(email.to_owned()),
            branch: Some(MAIN.to_owned()),
            fallback: Some(fallback(&self.digest)),
            files,
            exclude: None,
            max_bytes: None,
        }
    }
}

/// Makes `commit` in a clone of its repository of its own, made within `limits`, which goes
/// afterwards; answers where it landed.
pub(super) fn commit_once(
    houses: &Houses,
    commit: &AssistantCommit,
    digest: &str,
    limits: CloneLimits,
) -> Result<String, String> {
    let provider = houses()?;
    let scratch = tempfile::Builder::new()
        .prefix("ai6-assistant-commit-")
        .tempdir()
        .map_err(|error| error.to_string())?;
    let target = scratch.path().join("repository");
    let house = open_clone(
        provider.as_ref(),
        &commit.repo,
        &target,
        None,
        false,
        digest,
        limits,
    )?;
    // The save takes the files' text from the commit, never from this working tree, which
    // nothing here writes.
    let saved = house
        .checkpoint(&CheckpointSpec {
            remote: "origin".to_owned(),
            message: commit.message.clone(),
            author_name: Some(commit.author_name.clone()),
            author_email: Some(commit.author_email.clone()),
            branch: Some(MAIN.to_owned()),
            fallback: Some(fallback(digest)),
            files: Some(exactly(&commit.change)),
            exclude: None,
            max_bytes: None,
        })
        .map_err(|error| error.to_string());
    let _ = house.discard();
    Ok(landed(&saved?, &commit.repo))
}

/// The files of `change` for a save that commits exactly them.
fn exactly(change: &AssistantChange) -> Vec<CheckpointFile> {
    change
        .files
        .iter()
        .map(|(path, text)| CheckpointFile {
            path: path.clone(),
            text: text.clone(),
            base: change
                .base
                .get(path)
                .map(|base| base.clone().unwrap_or_default()),
        })
        .collect()
}

/// The hex SHA-256 of `bytes`: a file's [`AssistantChange::base`].
pub(super) fn sha256_hex(bytes: &[u8]) -> String {
    PeerId::of(bytes).hex()
}

/// The notice for work saved to `branch` because `main` refused it.
fn moved_on(branch: &str) -> String {
    format!(
        "main of the personal repository moved on in a way that conflicts with Moneypenny's \
         work, so it was saved to its branch {branch}."
    )
}

/// The branch a conversation's work goes to when `main` refuses it.
fn fallback(digest: &str) -> String {
    format!("moneypenny/{digest}")
}

/// Where a save landed, in words.
fn landed(saved: &SafeHouseCheckpoint, repo: &str) -> String {
    let short: String = saved.commit.chars().take(12).collect();
    if saved.commit.is_empty() {
        format!("nothing to commit in {repo}")
    } else if saved.branch == MAIN {
        format!("committed {short} to main of {repo}")
    } else {
        format!(
            "main of {repo} refused it, so {short} was saved to branch {}",
            saved.branch
        )
    }
}

/// How a repository is cloned: shallow to `depth` commits when set, and given up after
/// `timeout` or past `max_bytes` on disk when set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct CloneLimits {
    pub(super) depth: Option<u32>,
    pub(super) timeout: Duration,
    pub(super) max_bytes: Option<u64>,
}

impl CloneLimits {
    /// A whole clone within `[assistant.desk]`.
    pub(super) fn of(limits: &AssistantDeskLimits) -> Self {
        Self {
            depth: None,
            timeout: limits.clone_timeout.0,
            max_bytes: limits.clone_bytes.get(),
        }
    }
}

/// Clones `repo` into `path` on `main`, its git directory in `git_dir` when given, within
/// `limits`.
pub(super) fn open_clone(
    provider: &dyn SafeHouseProvider,
    repo: &str,
    path: &Path,
    git_dir: Option<&Path>,
    create: bool,
    digest: &str,
    limits: CloneLimits,
) -> Result<SafeHouse, String> {
    let parent = path.parent().unwrap_or(path);
    let spec = SafeHouseSpec {
        operation_id: "assistant".to_owned(),
        mission_id: format!("assistant--{digest}"),
        title: "Moneypenny".to_owned(),
        source: repo.to_owned(),
        base: MAIN.to_owned(),
        branch: MAIN.to_owned(),
        isolate: true,
        state_root: parent.to_string_lossy().into_owned(),
        reopen: None,
        checkpoint: None,
        clone: Some(SafeHouseClone {
            path: path.to_string_lossy().into_owned(),
            create_source: create,
            git_dir: git_dir.map(|dir| dir.to_string_lossy().into_owned()),
            depth: limits.depth,
            // Rounded up, so a deadline under a second still allows one.
            timeout_secs: Some(limits.timeout.as_secs_f64().ceil() as u64),
            max_bytes: limits.max_bytes,
        }),
    };
    SafeHouse::open(provider, &spec).map_err(|error| error.to_string())
}

/// What the desk tells her about her workspace.
fn instructions(shared: &[String], skills: &[String]) -> String {
    let skills = if skills.is_empty() {
        String::new()
    } else {
        format!(
            "- `.claude/skills/` holds the skills you may use, read-only: {}. Call one with the \
             Skill tool as `{SKILLS_PLUGIN}:<name>`.\n",
            skills
                .iter()
                .map(|name| format!("`{name}`"))
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    let shared = if shared.is_empty() {
        "No shared repository is checked out.".to_owned()
    } else {
        format!(
            "`{SHARED}/` holds the repositories the person's groups share ({}), read-only for \
             you: to change one, call `propose_shared_change`.",
            shared.join(", ")
        )
    };
    format!(
        "# Your workspace\n\n\
         This directory is your workspace for this conversation.\n\n\
         - `{PERSONAL}/` is the person's personal repository: keep what you make for them \
         there. After each answer everything you changed in it is committed to its `main`, \
         authored by the person.\n\
         - {shared}\n\
         - `{SCRATCH}/` is for anything temporary; it goes when this conversation's session \
         closes.\n\
         {skills}\
         - Nothing else here is yours to change.\n"
    )
}

/// The plain file `path`, opened for reading as [`read_capped`] opens it; `None` when it
/// cannot be opened, is not a plain file, or is a link.
fn open_plain(path: &Path) -> Option<fs::File> {
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // O_NOFOLLOW: a symlink at the final component makes the open fail (ELOOP).
        options.custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // FILE_FLAG_OPEN_REPARSE_POINT opens the reparse point itself; its metadata is then a
        // link and is refused below, rather than the target being read.
        options.custom_flags(0x0020_0000);
    }
    let file = options.open(path).ok()?;
    // The handle, not the path, decides: a swap between the open and here cannot change it.
    let metadata = file.metadata().ok()?;
    if !metadata.is_file() || is_link(&metadata) {
        return None;
    }
    Some(file)
}

/// The files of `personal/` outside `.git`, with size and modification time; at
/// most `files` of them.
fn stat_tree(root: &Path, files: Limit) -> BTreeMap<String, (u64, Option<SystemTime>)> {
    let mut found = BTreeMap::new();
    let Ok(entries) = fs::read_dir(root) else {
        return found;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        walk(&entry.path(), &name, &mut |path, file| {
            if files.allows(found.len() + 1)
                && let Ok(metadata) = fs::symlink_metadata(file)
            {
                found.insert(path, (metadata.len(), metadata.modified().ok()));
            }
        });
    }
    found
}

/// Visits each plain file under `path` by `name/<path>`, never through a link and never into
/// a hidden name.
pub(super) fn walk(path: &Path, name: &str, visit: &mut dyn FnMut(String, &Path)) {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return;
    };
    if is_link(&metadata) {
        return;
    }
    if metadata.is_file() {
        visit(name.to_owned(), path);
    } else if metadata.is_dir()
        && let Ok(entries) = fs::read_dir(path)
    {
        for entry in entries.flatten() {
            let child = entry.file_name().to_string_lossy().into_owned();
            // Hidden paths are never saved, so they are never part of her work or memory.
            if child.starts_with('.') {
                continue;
            }
            walk(&entry.path(), &format!("{name}/{child}"), visit);
        }
    }
}

/// The paths added, changed or removed between two looks at `personal/`.
fn changed_files(
    before: &BTreeMap<String, (u64, Option<SystemTime>)>,
    after: &BTreeMap<String, (u64, Option<SystemTime>)>,
) -> Vec<String> {
    let mut changed: Vec<String> = after
        .iter()
        .filter(|(path, stat)| before.get(*path) != Some(stat))
        .map(|(path, _)| path.clone())
        .chain(
            before
                .keys()
                .filter(|path| !after.contains_key(*path))
                .cloned(),
        )
        .collect();
    changed.sort();
    changed
}

/// The commit subject's summary of what changed.
fn summary(changed: &[String]) -> String {
    match changed.len() {
        0 => "save her work".to_owned(),
        1..=3 => format!("update {}", changed.join(", ")),
        more => format!("update {} and {} more", changed[..3].join(", "), more - 3),
    }
}

fn write_file(root: &Path, path: &Path, text: &str) -> Result<(), String> {
    write_bytes(root, path, text.as_bytes()).map_err(|error| format!("{}: {error}", path.display()))
}

/// Writes `path`, under `root`, making the directories it needs; never through a link.
fn write_bytes(root: &Path, path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    no_links(root, path)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, bytes)
}

/// Refuses `path` when `root`, or anything from `root` down to it, is a link: a symbolic link,
/// a junction or any other reparse point, which would take a read or write of the Station's
/// somewhere else. What does not exist yet is fine.
pub(super) fn no_links(root: &Path, path: &Path) -> std::io::Result<()> {
    let refused = |at: &Path| {
        std::io::Error::other(format!(
            "{} is a link, which is never followed",
            at.display()
        ))
    };
    let rest = path
        .strip_prefix(root)
        .map_err(|_| std::io::Error::other(format!("{} is outside its area", path.display())))?;
    let mut at = root.to_path_buf();
    if fs::symlink_metadata(&at).is_ok_and(|metadata| is_link(&metadata)) {
        return Err(refused(&at));
    }
    for part in rest.components() {
        let Component::Normal(name) = part else {
            return Err(std::io::Error::other(format!(
                "{} is not a plain path",
                path.display()
            )));
        };
        at.push(name);
        match fs::symlink_metadata(&at) {
            Ok(metadata) if is_link(&metadata) => return Err(refused(&at)),
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

/// Whether `metadata` is of a link: a symbolic link, or on Windows any reparse point.
pub(super) fn is_link(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        // FILE_ATTRIBUTE_REPARSE_POINT: junctions, symbolic links and the rest.
        if metadata.file_attributes() & 0x400 != 0 {
            return true;
        }
    }
    metadata.file_type().is_symlink()
}

/// The `group`, `path` and `content` of each [`ASSISTANT_SHARED_CHANGE_TOOL`] call in a
/// harness event's raw line, as Claude Code's `stream-json` writes it (`tool_use` blocks of
/// an assistant message) or as Codex's app server announces it (an `item/started`
/// notification of an `mcpToolCall` item with `server`, `tool` and `arguments`, Codex
/// 0.164.0).
fn shared_change_calls(data: Option<&[u8]>) -> Vec<(String, String, String)> {
    let tool = format!("mcp__{ASSISTANT_MCP_SERVER}__{ASSISTANT_SHARED_CHANGE_TOOL}");
    let Some(value) = data.and_then(|data| serde_json::from_slice::<serde_json::Value>(data).ok())
    else {
        return Vec::new();
    };
    if value.get("method").and_then(serde_json::Value::as_str) == Some("item/started") {
        let item = value.pointer("/params/item");
        let text = |pointer: &str| item?.pointer(pointer)?.as_str();
        if text("/type") != Some("mcpToolCall")
            || text("/server") != Some(ASSISTANT_MCP_SERVER)
            || text("/tool") != Some(ASSISTANT_SHARED_CHANGE_TOOL)
        {
            return Vec::new();
        }
        return match (
            text("/arguments/group"),
            text("/arguments/path"),
            text("/arguments/content"),
        ) {
            (Some(group), Some(path), Some(content)) => {
                vec![(group.to_owned(), path.to_owned(), content.to_owned())]
            }
            _ => Vec::new(),
        };
    }
    let blocks = value
        .pointer("/message/content")
        .and_then(serde_json::Value::as_array);
    blocks
        .into_iter()
        .flatten()
        .filter(|block| {
            block.get("type").and_then(serde_json::Value::as_str) == Some("tool_use")
                && block.get("name").and_then(serde_json::Value::as_str) == Some(tool.as_str())
        })
        .filter_map(|block| {
            let field = |name: &str| block.pointer(&format!("/input/{name}"))?.as_str();
            Some((
                field("group")?.to_owned(),
                field("path")?.to_owned(),
                field("content")?.to_owned(),
            ))
        })
        .collect()
}

/// What [`Workspace::page_files`] reads for `path` under `area`.
fn read_page(
    area: &Path,
    path: &str,
    limits: &AssistantLimits,
) -> Result<Vec<(AssistantPageFile, Vec<u8>)>, String> {
    use std::io::Read;
    let mut parts = path.split('/');
    let root = parts.next().unwrap_or_default();
    let rest: Vec<&str> = parts.collect();
    if (root != PERSONAL && root != SCRATCH)
        || rest.is_empty()
        || path.contains('\\')
        || rest
            .iter()
            .any(|part| part.is_empty() || *part == "." || *part == ".." || part.starts_with('.'))
    {
        return Err(format!(
            "{path:?} is not a path under personal/ or scratch/ without hidden names"
        ));
    }
    let target = in_tree(area, path);
    no_links(area, &target).map_err(|error| error.to_string())?;
    let mut found: Vec<(String, PathBuf)> = Vec::new();
    let name = rest.last().copied().unwrap_or(root).to_owned();
    let single = fs::symlink_metadata(&target).is_ok_and(|metadata| metadata.is_file());
    walk(&target, &name, &mut |key, file| {
        found.push((
            if single {
                name.clone()
            } else {
                key[name.len() + 1..].to_owned()
            },
            file.to_path_buf(),
        ));
    });
    if found.is_empty() {
        return Err(format!("{path} holds no file she may publish"));
    }
    if !limits.page_files.allows(found.len()) {
        return Err(format!("{path} has more than {} files", limits.page_files));
    }
    found.sort();
    let mut total = 0usize;
    let mut files = Vec::new();
    for (relative, file) in found {
        let cap = limits.page_file_bytes.or_max();
        let mut bytes = Vec::new();
        open_plain(&file)
            .ok_or_else(|| format!("{relative} is not a plain file"))?
            .take(cap.saturating_add(1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|error| format!("{relative}: {error}"))?;
        if !limits.page_file_bytes.allows(bytes.len()) {
            return Err(format!(
                "{relative} is longer than {} bytes",
                limits.page_file_bytes
            ));
        }
        total += bytes.len();
        if !limits.page_bytes.allows(total) {
            return Err(format!(
                "{path} is longer than {} bytes in all",
                limits.page_bytes
            ));
        }
        files.push((
            AssistantPageFile {
                path: relative,
                bytes: bytes.len() as u64,
                sha256: sha256_hex(&bytes),
            },
            bytes,
        ));
    }
    Ok(files)
}

/// The `path` of each [`ASSISTANT_PUBLISH_PAGE_TOOL`] call in a harness event's raw line, as
/// Claude Code's `stream-json` writes it or as Codex's app server announces it; a call with
/// no `path` (inline content) is not listed.
fn publish_page_paths(data: Option<&[u8]>) -> Vec<String> {
    let tool = format!("mcp__{ASSISTANT_MCP_SERVER}__{ASSISTANT_PUBLISH_PAGE_TOOL}");
    let Some(value) = data.and_then(|data| serde_json::from_slice::<serde_json::Value>(data).ok())
    else {
        return Vec::new();
    };
    if value.get("method").and_then(serde_json::Value::as_str) == Some("item/started") {
        let item = value.pointer("/params/item");
        let text = |pointer: &str| item?.pointer(pointer)?.as_str();
        if text("/type") == Some("mcpToolCall")
            && text("/server") == Some(ASSISTANT_MCP_SERVER)
            && text("/tool") == Some(ASSISTANT_PUBLISH_PAGE_TOOL)
            && let Some(path) = text("/arguments/path")
        {
            return vec![path.to_owned()];
        }
        return Vec::new();
    }
    value
        .pointer("/message/content")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter(|block| {
            block.get("type").and_then(serde_json::Value::as_str) == Some("tool_use")
                && block.get("name").and_then(serde_json::Value::as_str) == Some(tool.as_str())
        })
        .filter_map(|block| Some(block.pointer("/input/path")?.as_str()?.to_owned()))
        .collect()
}

/// The hex SHA-256 of the plain file `path`'s bytes, opened as [`open_plain`] opens it.
fn file_digest(path: &Path) -> Option<String> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let mut file = open_plain(path)?;
    let mut hasher = Sha256::new();
    let mut chunk = vec![0; 64 * 1024];
    loop {
        match file.read(&mut chunk).ok()? {
            0 => break,
            read => hasher.update(&chunk[..read]),
        }
    }
    Some(PeerId::from_bytes(hasher.finalize().into()).hex())
}

/// The path of `file`, a `/`-separated path relative to `root`.
pub(super) fn in_tree(root: &Path, file: &str) -> PathBuf {
    file.split('/')
        .fold(root.to_path_buf(), |at, name| at.join(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_digest_is_lowercase_sha256_hex() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(sha256_hex(b"").len(), 64);
    }

    fn limits() -> AssistantLimits {
        AssistantLimits::default()
    }

    /// Makes `at` a link to the directory `target`: a symbolic link, or a junction on Windows,
    /// which needs no privilege.
    fn link_directory(target: &Path, at: &Path) {
        #[cfg(unix)]
        std::os::unix::fs::symlink(target, at).unwrap();
        #[cfg(windows)]
        {
            let made = std::process::Command::new("cmd")
                .args(["/C", "mklink", "/J"])
                .arg(at)
                .arg(target)
                .output()
                .unwrap();
            assert!(made.status.success(), "{made:?}");
        }
    }

    #[test]
    fn nothing_is_written_through_a_link() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("secret.md"), "secret").unwrap();
        let area = root.path().join("area");
        let personal = area.join(PERSONAL);
        fs::create_dir_all(&personal).unwrap();
        let linked = personal.join("linked");
        link_directory(outside.path(), &linked);
        // The link works: anything following it reaches outside the area.
        assert_eq!(
            fs::read_to_string(linked.join("secret.md")).unwrap(),
            "secret"
        );

        assert!(write_file(&area, &linked.join("x.md"), "x").is_err());
        assert!(!outside.path().join("x.md").exists());
        assert!(write_file(&area, &linked.join("new").join("x.md"), "x").is_err());
        assert!(!outside.path().join("new").exists());
        assert!(stat_tree(&personal, limits().personal_walk).is_empty());
        assert_eq!(
            fs::read_to_string(outside.path().join("secret.md")).unwrap(),
            "secret"
        );

        // A plain path beside it is written as before.
        write_file(&area, &personal.join("notes").join("a.md"), "a").unwrap();
        assert_eq!(
            fs::read_to_string(personal.join("notes").join("a.md")).unwrap(),
            "a"
        );
    }

    fn page_area() -> (tempfile::TempDir, PathBuf) {
        let root = tempfile::tempdir().unwrap();
        let area = root.path().join("area");
        fs::create_dir_all(area.join(PERSONAL).join("site").join("img")).unwrap();
        fs::create_dir_all(area.join(PERSONAL).join(".git")).unwrap();
        fs::create_dir_all(area.join(SCRATCH)).unwrap();
        let site = area.join(PERSONAL).join("site");
        fs::write(site.join("index.html"), "<h1>hi</h1>").unwrap();
        fs::write(site.join("app.js"), "1").unwrap();
        fs::write(site.join("img").join("a.png"), [1u8, 2, 3]).unwrap();
        fs::write(site.join(".secret"), "no").unwrap();
        fs::write(area.join(PERSONAL).join(".git").join("config"), "no").unwrap();
        fs::write(area.join(SCRATCH).join("run.log"), "line\n").unwrap();
        (root, area)
    }

    #[test]
    fn a_page_is_read_from_personal_or_scratch_with_its_manifest() {
        let (_root, area) = page_area();
        let files = read_page(&area, "personal/site", &limits()).unwrap();
        let listed: Vec<_> = files.iter().map(|(file, _)| file.path.as_str()).collect();
        assert_eq!(
            listed,
            ["app.js", "img/a.png", "index.html"],
            "hidden names are left"
        );
        let index = &files[2];
        assert_eq!(index.0.bytes, 11);
        assert_eq!(index.0.sha256, sha256_hex(b"<h1>hi</h1>"));
        assert_eq!(index.1, b"<h1>hi</h1>");
        // One file, from either root.
        let single = read_page(&area, "scratch/run.log", &limits()).unwrap();
        assert_eq!(single.len(), 1);
        assert_eq!(single[0].0.path, "run.log");
        let one = read_page(&area, "personal/site/index.html", &limits()).unwrap();
        assert_eq!(one[0].0.path, "index.html");
    }

    #[test]
    fn a_page_path_outside_her_roots_or_with_a_hidden_name_is_refused() {
        let (_root, area) = page_area();
        for refused in [
            "../x",
            "personal/../x",
            "personal/.git/config",
            "personal/site/.secret",
            ".git/config",
            "shared/org/x",
            "memory/MEMORY.md",
            "personal//site",
            r"personal\site",
            "personal/missing",
            "personal",
        ] {
            assert!(
                read_page(&area, refused, &limits()).is_err(),
                "{refused} was read"
            );
        }
    }

    #[test]
    fn a_page_is_never_read_through_a_link() {
        let (root, area) = page_area();
        let outside = root.path().join("outside");
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("secret.html"), "secret").unwrap();
        link_directory(&outside, &area.join(PERSONAL).join("linked"));
        assert!(read_page(&area, "personal/linked", &limits()).is_err());
        assert!(read_page(&area, "personal/linked/secret.html", &limits()).is_err());
        // A link inside a bundle is skipped, not followed.
        link_directory(&outside, &area.join(PERSONAL).join("site").join("link"));
        let files = read_page(&area, "personal/site", &limits()).unwrap();
        assert!(files.iter().all(|(file, _)| !file.path.starts_with("link")));
    }

    #[test]
    fn a_page_past_the_limits_is_refused_not_cut() {
        let (_root, area) = page_area();
        let small: AssistantLimits =
            crate::directorates::m::records::from_toml("page_file_bytes = 10").unwrap();
        let error = read_page(&area, "personal/site", &small).unwrap_err();
        assert!(
            error.contains("index.html") || error.contains("longer"),
            "{error}"
        );
        let few: AssistantLimits =
            crate::directorates::m::records::from_toml("page_files = 2").unwrap();
        assert!(read_page(&area, "personal/site", &few).is_err());
        let total: AssistantLimits =
            crate::directorates::m::records::from_toml("page_bytes = 12").unwrap();
        assert!(read_page(&area, "personal/site", &total).is_err());
    }

    #[test]
    fn a_publish_call_is_read_from_either_harness() {
        let claude = serde_json::json!({"message": {"content": [
            {"type": "tool_use", "name": "mcp__moneypenny__publish_page",
             "input": {"kind": "dashboard", "title": "t", "path": "personal/site"}},
            {"type": "tool_use", "name": "mcp__moneypenny__publish_page",
             "input": {"kind": "file", "title": "t", "content": "x", "format": "text"}},
            {"type": "tool_use", "name": "mcp__other__publish_page",
             "input": {"path": "personal/x"}}
        ]}});
        assert_eq!(
            publish_page_paths(Some(claude.to_string().as_bytes())),
            ["personal/site"]
        );
        let codex = serde_json::json!({"method": "item/started", "params": {"item": {
            "type": "mcpToolCall", "server": "moneypenny", "tool": "publish_page",
            "arguments": {"path": "scratch/run.log"}}}});
        assert_eq!(
            publish_page_paths(Some(codex.to_string().as_bytes())),
            ["scratch/run.log"]
        );
        assert!(publish_page_paths(Some(b"not json")).is_empty());
        assert!(publish_page_paths(None).is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn a_file_link_is_never_written() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let secret = outside.path().join("secret.md");
        fs::write(&secret, "secret").unwrap();
        let area = root.path().join("area");
        fs::create_dir_all(&area).unwrap();
        let index = area.join("notes.md");
        std::os::unix::fs::symlink(&secret, &index).unwrap();
        assert_eq!(fs::read_to_string(&index).unwrap(), "secret");
        assert!(write_file(&area, &index, "x").is_err());
        assert_eq!(fs::read_to_string(&secret).unwrap(), "secret");
    }

    /// `item/started` of a `propose_shared_change` call as Codex 0.164.0's app server sends it
    /// (recorded against a stand-in for the model; ids shortened).
    const CODEX_SHARED_CALL: &str = r#"{"method":"item/started","params":{"item":{
        "type":"mcpToolCall","id":"exec-1","server":"moneypenny","tool":"propose_shared_change",
        "status":"inProgress","arguments":{"group":"ops","path":"notes/plan.md",
        "content":"SHARED-CONTENT\n","message":"plan"},"appContext":null,"result":null,
        "error":null,"durationMs":null},"threadId":"t","turnId":"u","startedAtMs":1}}"#;

    #[test]
    fn a_shared_change_is_read_from_codex_as_from_claude_code() {
        assert_eq!(
            shared_change_calls(Some(CODEX_SHARED_CALL.as_bytes())),
            [(
                "ops".to_owned(),
                "notes/plan.md".to_owned(),
                "SHARED-CONTENT\n".to_owned()
            )]
        );
        for (from, to) in [
            ("\"server\":\"moneypenny\"", "\"server\":\"other\""),
            ("\"tool\":\"propose_shared_change\"", "\"tool\":\"board\""),
            ("item/started", "item/completed"),
            ("\"path\":\"notes/plan.md\",", ""),
        ] {
            let changed = CODEX_SHARED_CALL.replace(from, to);
            assert!(
                shared_change_calls(Some(changed.as_bytes())).is_empty(),
                "{from} -> {to}"
            );
        }
    }

    #[test]
    fn her_standing_text_names_the_skills_installed_for_her() {
        let without = instructions(&[], &[]);
        assert!(!without.contains(".claude/skills"), "{without}");
        let with = instructions(&[], &["charts".to_owned(), "slides".to_owned()]);
        assert!(
            with.contains("`.claude/skills/`")
                && with.contains("`charts`, `slides`")
                && with.contains("`ai6:<name>`"),
            "{with}"
        );
    }

    #[test]
    fn a_summary_names_up_to_three_files() {
        assert_eq!(summary(&[]), "save her work");
        assert_eq!(summary(&["notes/a.md".to_owned()]), "update notes/a.md");
        let many: Vec<String> = (1..=5).map(|n| format!("{n}.md")).collect();
        assert_eq!(summary(&many), "update 1.md, 2.md, 3.md and 2 more");
    }

    #[test]
    fn a_change_names_only_files_of_its_repository() {
        let escape = AssistantChange::new(BTreeMap::from([(
            "../x.md".to_owned(),
            Some("x".to_owned()),
        )]));
        assert!(escape.validate(&limits()).is_err());
        let inside = AssistantChange::new(BTreeMap::from([(
            "notes/x.md".to_owned(),
            Some("x".to_owned()),
        )]));
        assert_eq!(inside.validate(&limits()), Ok(()));
    }
}
