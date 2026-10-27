//! A conversation's workspace on a Station and its commit cycle.
//!
//! When the job names a personal repository, the conversation's directory holds `personal/`
//! (a clone of that repository on `main`, through the `git` safe-house provider), one
//! `shared/<name>/` per shared repository (cloned afresh at each harness start, read-only for
//! her), `scratch/`, and the desk's `CLAUDE.md` and `AGENTS.md`. The same standing text goes
//! into a context file beside the directory, which the harness gets with its system prompt.
//!
//! At the start of each turn the desk keeps a copy of `personal/memory/`. When the turn ends,
//! whatever she changed there is set aside (the copy is put back) and reported for the person
//! to let through; everything else in `personal/` is committed to `main`, authored by the
//! person. A `main` that refuses the work twice gets it on `moneypenny/<conversation digest>`.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use crate::directorates::m::assistant::RepoKind;
use crate::directorates::q::safehouse::{
    CheckpointSpec, SafeHouseCheckpoint, SafeHouseClone, SafeHouseProvider, SafeHouseSpec,
};
use crate::directorates::section_00::assistant::{
    AssistantChange, AssistantCommit, AssistantWorkspace, COMMIT_MEMORY, MAX_ASSISTANT_EVENT_BYTES,
};
use crate::station::safehouse::SafeHouse;

/// The personal repository's checkout, and the one directory besides `scratch` she may write.
pub(super) const PERSONAL: &str = "personal";
/// Where the shared repositories are checked out.
pub(super) const SHARED: &str = "shared";
/// Her scratch directory, gone with the conversation's directory.
pub(super) const SCRATCH: &str = "scratch";
/// Her memory, inside the personal repository.
const MEMORY: &str = "memory";
/// The branch every repository is read from and saved to.
const MAIN: &str = "main";
/// The safe-house provider the workspace's repositories are opened with.
pub(super) const PROVIDER: &str = "git";
/// The most of `MEMORY.md` inlined into `AGENTS.md` and the context file.
const MAX_INLINED_MEMORY: usize = 32 * 1024;
/// The most files of `personal/` looked at to name what a turn changed.
const MAX_WALK: usize = 10_000;

/// `MEMORY.md` as a first use writes it.
const MEMORY_INDEX: &str = "# Memory\n\nWhat Moneypenny remembers about the person she works \
                            for. Keep this index short: one line per file in `memory/`, saying \
                            what it holds.\n";

/// Opens safe houses: the Station's provider, or a test's.
pub(super) type Houses = Arc<dyn Fn() -> Result<Box<dyn SafeHouseProvider>, String> + Send + Sync>;

/// One conversation's checkout of the personal repository and its turn-start copies.
pub(super) struct Workspace {
    conversation_id: String,
    digest: String,
    repo: String,
    personal: SafeHouse,
    path: PathBuf,
    author_name: String,
    author_email: String,
    /// `memory/` at the start of the turn, by path relative to `personal/`.
    memory: BTreeMap<String, Vec<u8>>,
    /// The other files of `personal/` at the start of the turn: size and modification time.
    files: BTreeMap<String, (u64, Option<SystemTime>)>,
    /// The harness closed and its directory is going: a commit goes to a clone of its own.
    closed: bool,
}

/// What the end of a turn found besides the work it saved.
#[derive(Debug, Default)]
pub(super) struct TurnEnd {
    /// The memory change set aside and its unified diff.
    pub(super) memory: Option<(AssistantChange, String)>,
    /// What the person and Moneypenny should know.
    pub(super) notices: Vec<String>,
}

impl Workspace {
    /// Makes the workspace in `area`, the conversation's empty directory, and writes the
    /// standing text to `context`; answers it with notices for the conversation.
    pub(super) fn prepare(
        houses: &Houses,
        area: &Path,
        context: &Path,
        conversation_id: &str,
        digest: &str,
        job: &AssistantWorkspace,
    ) -> Result<(Self, Vec<String>), String> {
        let provider = houses()?;
        let create = RepoKind::of(&job.personal)? == RepoKind::Local;
        let path = area.join(PERSONAL);
        let personal = open_clone(provider.as_ref(), &job.personal, &path, create, digest)
            .map_err(|reason| format!("the personal repository could not be opened: {reason}"))?;
        let mut workspace = Self {
            conversation_id: conversation_id.to_owned(),
            digest: digest.to_owned(),
            repo: job.personal.clone(),
            personal,
            path,
            author_name: job.author_name.clone(),
            author_email: job.author_email.clone(),
            memory: BTreeMap::new(),
            files: BTreeMap::new(),
            closed: false,
        };
        let mut notices = Vec::new();
        let index = workspace.path.join(MEMORY).join("MEMORY.md");
        if !index.exists() {
            write_file(&index, MEMORY_INDEX)?;
            notices.extend(workspace.save("start her memory")?);
        }
        let mut shared = Vec::new();
        for (name, repo) in &job.shared {
            let target = area.join(SHARED).join(name);
            match open_clone(provider.as_ref(), repo, &target, false, digest) {
                Ok(_) => shared.push(name.clone()),
                Err(reason) => notices.push(format!(
                    "The shared repository {name:?} could not be opened, so shared/{name}/ is \
                     missing: {reason}"
                )),
            }
        }
        fs::create_dir_all(area.join(SCRATCH)).map_err(|error| error.to_string())?;
        let memory = fs::read_to_string(&index).unwrap_or_default();
        let standing = instructions(job.exposed, &shared);
        write_file(
            &area.join("CLAUDE.md"),
            &format!("{standing}\n## Memory\n\n@{PERSONAL}/{MEMORY}/MEMORY.md\n"),
        )?;
        let inlined = format!(
            "{standing}\n## Memory\n\n{}\n",
            cut(&memory, MAX_INLINED_MEMORY)
        );
        write_file(&area.join("AGENTS.md"), &inlined)?;
        write_file(context, &inlined)?;
        workspace.begin_turn();
        Ok((workspace, notices))
    }

    /// Whether a commit of `repo` can be made here: the checkout is of it and still open.
    pub(super) fn takes(&self, repo: &str) -> bool {
        !self.closed && self.repo == repo
    }

    /// Ends the last turn as the harness closes; afterwards commits go elsewhere.
    pub(super) fn close(&mut self) -> TurnEnd {
        let end = self.end_turn();
        self.closed = true;
        end
    }

    /// Notes what `personal/` holds as a turn starts.
    pub(super) fn begin_turn(&mut self) {
        self.memory = read_tree(&self.path.join(MEMORY), MEMORY);
        self.files = stat_tree(&self.path);
    }

    /// Ends a turn: sets her memory change aside and saves the rest of `personal/`.
    pub(super) fn end_turn(&mut self) -> TurnEnd {
        let mut end = TurnEnd::default();
        let now = read_tree(&self.path.join(MEMORY), MEMORY);
        let mut files = BTreeMap::new();
        for (path, bytes) in &now {
            if self.memory.get(path) != Some(bytes) {
                match String::from_utf8(bytes.clone()) {
                    Ok(text) => {
                        files.insert(path.clone(), Some(text));
                    }
                    Err(_) => end.notices.push(format!(
                        "{path} is not text, so it was not kept in Moneypenny's memory."
                    )),
                }
            }
        }
        for path in self.memory.keys() {
            if !now.contains_key(path) {
                files.insert(path.clone(), None);
            }
        }
        // The memory goes back to what it was; the change waits for the person.
        for path in now.keys() {
            if !self.memory.contains_key(path) {
                let _ = fs::remove_file(self.path.join(path));
            }
        }
        for (path, bytes) in &self.memory {
            if now.get(path) != Some(bytes)
                && let Err(error) = write_bytes(&self.path.join(path), bytes)
            {
                end.notices
                    .push(format!("{path} could not be put back: {error}"));
            }
        }
        let changed = changed_files(&self.files, &stat_tree(&self.path));
        match self.save(&summary(&changed)) {
            Ok(notice) => end.notices.extend(notice),
            Err(reason) => end.notices.push(format!(
                "Moneypenny's work in personal/ could not be saved: {reason}"
            )),
        }
        if !files.is_empty() {
            let change = AssistantChange::new(files);
            let diff = unified_diff(&self.memory, &change);
            match change.validate(true) {
                Ok(()) if diff.len() <= MAX_ASSISTANT_EVENT_BYTES => {
                    end.memory = Some((change, diff));
                }
                Ok(()) => end.notices.push(
                    "Moneypenny's memory change was too large to show, so it was not kept."
                        .to_owned(),
                ),
                Err(reason) => end.notices.push(format!(
                    "Moneypenny's memory change was not kept: {reason}."
                )),
            }
        }
        self.begin_turn();
        end
    }

    /// Writes a memory commit the person let through into this checkout, as the turn's
    /// starting point too, and saves it; answers where it landed.
    pub(super) fn apply(&mut self, commit: &AssistantCommit) -> Result<String, String> {
        write_change(&self.path, &commit.change)?;
        for (path, text) in &commit.change.files {
            match text {
                Some(text) => {
                    self.memory.insert(path.clone(), text.as_bytes().to_vec());
                }
                None => {
                    self.memory.remove(path);
                }
            }
        }
        let saved = self
            .personal
            .checkpoint(&self.spec(
                commit.message.clone(),
                &commit.author_name,
                &commit.author_email,
            ))
            .map_err(|error| error.to_string())?;
        Ok(landed(&saved, &self.repo))
    }

    /// Commits everything in `personal/` and pushes it; a notice when it landed elsewhere
    /// than `main`.
    fn save(&self, summary: &str) -> Result<Option<String>, String> {
        let message = format!(
            "moneypenny: {summary}\n\nVia: Moneypenny (conversation {})",
            self.conversation_id
        );
        let saved = self
            .personal
            .checkpoint(&self.spec(message, &self.author_name, &self.author_email))
            .map_err(|error| error.to_string())?;
        Ok((saved.branch != MAIN).then(|| {
            format!(
                "main of the personal repository moved on in a way that conflicts with \
                 Moneypenny's work, so it was saved to its branch {}.",
                saved.branch
            )
        }))
    }

    fn spec(&self, message: String, name: &str, email: &str) -> CheckpointSpec {
        CheckpointSpec {
            remote: "origin".to_owned(),
            message,
            author_name: Some(name.to_owned()),
            author_email: Some(email.to_owned()),
            branch: Some(MAIN.to_owned()),
            fallback: Some(fallback(&self.digest)),
        }
    }
}

/// Makes `commit` in a clone of its repository of its own, which goes afterwards; answers
/// where it landed.
pub(super) fn commit_once(
    houses: &Houses,
    commit: &AssistantCommit,
    digest: &str,
) -> Result<String, String> {
    let provider = houses()?;
    let scratch = tempfile::Builder::new()
        .prefix("ai6-assistant-commit-")
        .tempdir()
        .map_err(|error| error.to_string())?;
    let create = commit.kind == COMMIT_MEMORY && RepoKind::of(&commit.repo)? == RepoKind::Local;
    let target = scratch.path().join("repository");
    let house = open_clone(provider.as_ref(), &commit.repo, &target, create, digest)?;
    let saved = write_change(&target, &commit.change).and_then(|()| {
        house
            .checkpoint(&CheckpointSpec {
                remote: "origin".to_owned(),
                message: commit.message.clone(),
                author_name: Some(commit.author_name.clone()),
                author_email: Some(commit.author_email.clone()),
                branch: Some(MAIN.to_owned()),
                fallback: Some(fallback(digest)),
            })
            .map_err(|error| error.to_string())
    });
    let _ = house.discard();
    Ok(landed(&saved?, &commit.repo))
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

fn open_clone(
    provider: &dyn SafeHouseProvider,
    repo: &str,
    path: &Path,
    create: bool,
    digest: &str,
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
        }),
    };
    SafeHouse::open(provider, &spec).map_err(|error| error.to_string())
}

/// What the desk tells her about her workspace, before her memory.
fn instructions(exposed: bool, shared: &[String]) -> String {
    let memory = if exposed {
        "In this conversation you have read text other people wrote, so a change to your \
         memory waits until the person confirms it."
    } else {
        "A change to your memory is saved after your answer and shown to the person; once you \
         read text other people wrote, it waits for their confirmation instead."
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
         - `{PERSONAL}/{MEMORY}/` is your memory of the person: who they are, their \
         preferences, standing instructions, ongoing work, summaries of past conversations. \
         `{MEMORY}/MEMORY.md` is its index; keep it short and name the other files from it. \
         {memory}\n\
         - {shared}\n\
         - `{SCRATCH}/` is for anything temporary; it goes when this conversation's session \
         closes.\n\
         - Nothing else here is yours to change.\n"
    )
}

/// Every file under `root`, by `prefix/<path>`, with its bytes; links are not followed.
fn read_tree(root: &Path, prefix: &str) -> BTreeMap<String, Vec<u8>> {
    let mut found = BTreeMap::new();
    walk(root, prefix, &mut |path, file| {
        if let Ok(bytes) = fs::read(file) {
            found.insert(path, bytes);
        }
    });
    found
}

/// The files of `personal/` outside `.git` and `memory/`, with size and modification time.
fn stat_tree(root: &Path) -> BTreeMap<String, (u64, Option<SystemTime>)> {
    let mut found = BTreeMap::new();
    let Ok(entries) = fs::read_dir(root) else {
        return found;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name == ".git" || name == MEMORY {
            continue;
        }
        walk(&entry.path(), &name, &mut |path, file| {
            if found.len() < MAX_WALK
                && let Ok(metadata) = fs::metadata(file)
            {
                found.insert(path, (metadata.len(), metadata.modified().ok()));
            }
        });
    }
    found
}

fn walk(path: &Path, name: &str, visit: &mut dyn FnMut(String, &Path)) {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return;
    };
    if metadata.is_file() {
        visit(name.to_owned(), path);
    } else if metadata.is_dir()
        && let Ok(entries) = fs::read_dir(path)
    {
        for entry in entries.flatten() {
            let child = entry.file_name().to_string_lossy().into_owned();
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

/// The unified diff of `change` against `before`, one file after another.
fn unified_diff(before: &BTreeMap<String, Vec<u8>>, change: &AssistantChange) -> String {
    let mut diff = String::new();
    for (path, text) in &change.files {
        let old = before
            .get(path)
            .map(|bytes| String::from_utf8_lossy(bytes).into_owned())
            .unwrap_or_default();
        let new = text.clone().unwrap_or_default();
        let (from, to) = match (before.contains_key(path), text.is_some()) {
            (false, _) => ("/dev/null".to_owned(), format!("b/{path}")),
            (true, false) => (format!("a/{path}"), "/dev/null".to_owned()),
            (true, true) => (format!("a/{path}"), format!("b/{path}")),
        };
        diff.push_str(
            &similar::TextDiff::from_lines(&old, &new)
                .unified_diff()
                .header(&from, &to)
                .to_string(),
        );
    }
    diff
}

/// Writes each file of `change` under `root`, or removes it.
fn write_change(root: &Path, change: &AssistantChange) -> Result<(), String> {
    for (path, text) in &change.files {
        let file = path
            .split('/')
            .fold(root.to_path_buf(), |file, name| file.join(name));
        match text {
            Some(text) => write_file(&file, text)?,
            None => match fs::remove_file(&file) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(format!("{path}: {error}")),
            },
        }
    }
    Ok(())
}

fn write_file(path: &Path, text: &str) -> Result<(), String> {
    write_bytes(path, text.as_bytes()).map_err(|error| format!("{}: {error}", path.display()))
}

fn write_bytes(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, bytes)
}

fn cut(text: &str, limit: usize) -> &str {
    let mut end = text.len().min(limit);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_summary_names_up_to_three_files() {
        assert_eq!(summary(&[]), "save her work");
        assert_eq!(summary(&["notes/a.md".to_owned()]), "update notes/a.md");
        let many: Vec<String> = (1..=5).map(|n| format!("{n}.md")).collect();
        assert_eq!(summary(&many), "update 1.md, 2.md, 3.md and 2 more");
    }

    #[test]
    fn a_diff_shows_added_changed_and_removed_files() {
        let before = BTreeMap::from([
            ("memory/MEMORY.md".to_owned(), b"# Memory\n".to_vec()),
            ("memory/old.md".to_owned(), b"old\n".to_vec()),
        ]);
        let change = AssistantChange::new(BTreeMap::from([
            (
                "memory/MEMORY.md".to_owned(),
                Some("# Memory\n\n- prefers tables\n".to_owned()),
            ),
            ("memory/old.md".to_owned(), None),
            ("memory/prefs.md".to_owned(), Some("tables\n".to_owned())),
        ]));
        let diff = unified_diff(&before, &change);
        assert!(
            diff.contains("--- a/memory/MEMORY.md\n+++ b/memory/MEMORY.md"),
            "{diff}"
        );
        assert!(diff.contains("+- prefers tables"), "{diff}");
        assert!(
            diff.contains("--- a/memory/old.md\n+++ /dev/null"),
            "{diff}"
        );
        assert!(
            diff.contains("--- /dev/null\n+++ b/memory/prefs.md"),
            "{diff}"
        );
        assert_eq!(change.validate(true), Ok(()));
    }

    #[test]
    fn a_change_is_written_only_under_its_root() {
        let root = tempfile::tempdir().unwrap();
        let change = AssistantChange::new(BTreeMap::from([(
            "memory/notes/a.md".to_owned(),
            Some("a".to_owned()),
        )]));
        write_change(root.path(), &change).unwrap();
        assert_eq!(
            fs::read_to_string(root.path().join("memory").join("notes").join("a.md")).unwrap(),
            "a"
        );
        let escape = AssistantChange::new(BTreeMap::from([(
            "../x.md".to_owned(),
            Some("x".to_owned()),
        )]));
        assert!(escape.validate(true).is_err());
        let outside = AssistantChange::new(BTreeMap::from([(
            "notes/x.md".to_owned(),
            Some("x".to_owned()),
        )]));
        assert!(outside.validate(true).is_err(), "memory only");
        assert_eq!(outside.validate(false), Ok(()));
    }
}
