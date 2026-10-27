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
//!
//! Exposure is Universal Exports' to record (`Conversation.exposed`), but the desk sees one
//! way in it cannot: text she wrote in `personal/` while exposed. (The repositories in
//! `shared/` are trusted text and expose nothing.) Their paths are kept in the personal
//! repository, in [`OUTSIDE_TEXT`] (a hidden directory she cannot write). A file tool call
//! that can answer with one of them exposes the conversation: a `Read` or `Edit` of one, a
//! `Grep` or `Glob` whose search covers one, or a call whose input the desk cannot read. So
//! does a `MEMORY.md` that imports one. Reading other files does not. The record is cleared
//! by removing the files it names, or the record itself, in the repository.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use sha2::{Digest, Sha256};

use crate::directorates::m::assistant::RepoKind;
use crate::directorates::q::safehouse::{
    CheckpointFile, CheckpointSpec, SafeHouseCheckpoint, SafeHouseClone, SafeHouseProvider,
    SafeHouseSpec,
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
/// The record, in the personal repository, of the files she wrote while exposed: a directory
/// of entries, each holding one path relative to the repository and named by the SHA-256 of
/// it, so conversations adding to it at once never touch the same file.
pub(super) const OUTSIDE_TEXT: &str = ".moneypenny/outside-text";
/// The built-in tools that answer with text of a file or its name: `Read`, `Grep` and `Glob`,
/// and `Edit`, whose answer quotes the lines around its change.
const READING_TOOLS: [&str; 4] = ["Read", "Grep", "Glob", "Edit"];
/// How deep Claude Code follows `@path` imports from a memory file.
const MAX_IMPORT_DEPTH: usize = 5;

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
    /// The conversation has read outside text, as far as the desk knows; never cleared.
    exposed: bool,
    /// The files of `personal/` she wrote while exposed ([`OUTSIDE_TEXT`]).
    outside: BTreeSet<String>,
    /// What exposed the conversation as the workspace was prepared, until it is reported.
    found: Option<String>,
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
    /// Makes the workspace in `area`, the conversation's empty directory, with the
    /// repositories' git directories in `repos` and the standing text in `context`, both
    /// outside `area`; answers it with notices for the conversation.
    pub(super) fn prepare(
        houses: &Houses,
        area: &Path,
        repos: &Path,
        context: &Path,
        conversation_id: &str,
        job: &AssistantWorkspace,
        exposed: bool,
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
        )
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
            exposed,
            outside: BTreeSet::new(),
            found: None,
        };
        workspace.outside = workspace.read_record().into_values().collect();
        if !workspace.exposed
            && let Some(file) = workspace.imported_outside()
        {
            workspace.exposed = true;
            workspace.found = Some(format!("personal:{file} (imported by {MEMORY}/MEMORY.md)"));
        }
        let mut notices = Vec::new();
        let index = workspace.path.join(MEMORY).join("MEMORY.md");
        if fs::symlink_metadata(&index).is_err() {
            write_file(area, &index, MEMORY_INDEX)?;
            notices.extend(workspace.save("start her memory")?);
        }
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
            ) {
                Ok(_) => shared.push(name.clone()),
                Err(reason) => notices.push(format!(
                    "The shared repository {name:?} could not be opened, so shared/{name}/ is \
                     missing: {reason}"
                )),
            }
        }
        fs::create_dir_all(area.join(SCRATCH)).map_err(|error| error.to_string())?;
        let memory = read_text(area, &index).unwrap_or_default();
        let standing = instructions(workspace.exposed, &shared);
        write_file(
            area,
            &area.join("CLAUDE.md"),
            &format!("{standing}\n## Memory\n\n@{PERSONAL}/{MEMORY}/MEMORY.md\n"),
        )?;
        let inlined = format!(
            "{standing}\n## Memory\n\n{}\n",
            cut(&memory, MAX_INLINED_MEMORY)
        );
        write_file(area, &area.join("AGENTS.md"), &inlined)?;
        write_file(context.parent().unwrap_or(context), context, &inlined)?;
        workspace.begin_turn(false);
        Ok((workspace, notices))
    }

    /// Whether the conversation has read outside text, as far as the desk knows.
    pub(super) fn exposed(&self) -> bool {
        self.exposed
    }

    /// Notes the tools a harness event calls, with the event's raw line (`data`): a call that
    /// returns the text of a file the record names makes the conversation exposed. A reading
    /// call whose input cannot be read counts as reaching them all. Answers the source to
    /// report when this call is what exposed it.
    pub(super) fn note_tools(&mut self, tools: &[String], data: Option<&[u8]>) -> Option<String> {
        let reads = tools
            .iter()
            .any(|tool| READING_TOOLS.contains(&tool.as_str()));
        if self.exposed || !reads || self.outside.is_empty() {
            return None;
        }
        let area = self.area().to_path_buf();
        let listed = self.listed();
        let source = match reading_calls(data) {
            Some(calls) => calls.iter().find_map(|(tool, scope)| {
                let covered = scope.resolve(&area);
                listed
                    .iter()
                    .find(|(_, key)| covered.covers(key))
                    .map(|(file, _)| format!("personal:{file} (read by {tool})"))
            }),
            None => Some(format!(
                "personal:{} (a file tool call that could not be read)",
                self.outside
                    .iter()
                    .take(3)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(",")
            )),
        };
        if source.is_some() {
            self.exposed = true;
        }
        source
    }

    /// Her working directory, holding `personal/`.
    fn area(&self) -> &Path {
        self.path.parent().unwrap_or(&self.path)
    }

    /// The record's files, each with the key [`Scope::covers`] compares.
    fn listed(&self) -> Vec<(String, String)> {
        self.outside
            .iter()
            .map(|file| (file.clone(), path_key(&in_tree(&self.path, file))))
            .collect()
    }

    /// The first recorded file `MEMORY.md` imports (`@path`, followed through the files it
    /// imports), which a harness that loads `CLAUDE.md` reads with it.
    fn imported_outside(&self) -> Option<String> {
        if self.outside.is_empty() {
            return None;
        }
        let listed = self.listed();
        let mut seen = BTreeSet::new();
        let mut next = vec![self.path.join(MEMORY).join("MEMORY.md")];
        for _ in 0..=MAX_IMPORT_DEPTH {
            let mut found = Vec::new();
            for file in next.drain(..) {
                // Only her own files are followed: nothing else can import what she wrote.
                let Some(text) = read_text(self.area(), &file) else {
                    continue;
                };
                let base = file.parent().unwrap_or(&self.path).to_path_buf();
                for import in imports(&text) {
                    // An import of a home that is not known could be any of them.
                    let Some(target) = expand(&base, import) else {
                        return self.outside.first().cloned();
                    };
                    let target = plain_path(&target);
                    let key = path_key(&target);
                    if let Some((listed, _)) = listed.iter().find(|(_, at)| *at == key) {
                        return Some(listed.clone());
                    }
                    if seen.insert(key) {
                        found.push(target);
                    }
                }
            }
            next = found;
        }
        None
    }

    /// What exposed the conversation as the workspace was prepared, once, for Universal
    /// Exports.
    pub(super) fn take_found(&mut self) -> Option<String> {
        self.found.take()
    }

    /// Notes that the conversation is exposed, as Universal Exports says or as the desk found.
    pub(super) fn mark_exposed(&mut self) {
        self.exposed = true;
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

    /// Notes what `personal/` holds as a turn starts, and whether the job said the
    /// conversation is exposed.
    pub(super) fn begin_turn(&mut self, exposed: bool) {
        self.exposed |= exposed;
        // Another conversation's save this one replayed on may have added to the record.
        self.outside = self.read_record().into_values().collect();
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
                let _ = remove(self.area(), &self.path.join(path));
            }
        }
        for (path, bytes) in &self.memory {
            if now.get(path) != Some(bytes)
                && let Err(error) = write_bytes(self.area(), &self.path.join(path), bytes)
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
        if let Err(reason) = self.record_outside_text(&changed) {
            end.notices.push(format!(
                "Which of Moneypenny's files hold outside text could not be recorded: {reason}"
            ));
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
        self.begin_turn(false);
        end
    }

    /// Writes a memory commit the person let through into this checkout, as the turn's
    /// starting point too, and commits exactly its files, never what a turn in progress has
    /// written beside them; answers where it landed.
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
                Some(exactly(&commit.change)),
            ))
            .map_err(|error| error.to_string())?;
        self.outside = self.read_record().into_values().collect();
        Ok(landed(&saved, &self.repo))
    }

    /// The record's entries in `personal/`: entry name and the path it names.
    fn read_record(&self) -> BTreeMap<String, String> {
        let dir = in_tree(&self.path, OUTSIDE_TEXT);
        let mut found = BTreeMap::new();
        if no_links(self.area(), &dir).is_err() {
            return found;
        }
        for entry in fs::read_dir(&dir).into_iter().flatten().flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if let Some(text) = read_text(self.area(), &entry.path()) {
                let named = text.trim();
                if !named.is_empty() {
                    found.insert(name, named.to_owned());
                }
            }
        }
        found
    }

    /// Adds the files a turn changed to [`OUTSIDE_TEXT`] when the conversation is exposed and
    /// drops the entries of files that no longer exist, writing and committing only those
    /// entries: what other conversations recorded meanwhile stays, and is read back after.
    fn record_outside_text(&mut self, changed: &[String]) -> Result<(), String> {
        let record = self.read_record();
        let known: BTreeSet<&String> = record.values().collect();
        let exists = |file: &str| {
            fs::symlink_metadata(in_tree(&self.path, file)).is_ok_and(|metadata| metadata.is_file())
        };
        let mut files = Vec::new();
        if self.exposed {
            for file in changed {
                if !known.contains(file) && exists(file) {
                    files.push(CheckpointFile {
                        path: format!("{OUTSIDE_TEXT}/{}", record_entry(file)),
                        text: Some(format!("{file}\n")),
                    });
                }
            }
        }
        for (entry, file) in &record {
            if !exists(file) {
                files.push(CheckpointFile {
                    path: format!("{OUTSIDE_TEXT}/{entry}"),
                    text: None,
                });
            }
        }
        if files.is_empty() {
            self.outside = record.into_values().collect();
            return Ok(());
        }
        // Written here too, so this checkout reads what it committed.
        for file in &files {
            let at = in_tree(&self.path, &file.path);
            match &file.text {
                Some(text) => write_file(self.area(), &at, text)?,
                None => remove(self.area(), &at).map_err(|error| error.to_string())?,
            }
        }
        let message = format!(
            "moneypenny: record which files hold outside text\n\nVia: Moneypenny (conversation \
             {})",
            self.conversation_id
        );
        let saved = self
            .personal
            .checkpoint(&self.spec(message, &self.author_name, &self.author_email, Some(files)))
            .map(|_| ())
            .map_err(|error| error.to_string());
        self.outside = self.read_record().into_values().collect();
        saved
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
            .checkpoint(&self.spec(message, &self.author_name, &self.author_email, None))
            .map_err(|error| error.to_string())?;
        Ok((saved.branch != MAIN).then(|| {
            format!(
                "main of the personal repository moved on in a way that conflicts with \
                 Moneypenny's work, so it was saved to its branch {}.",
                saved.branch
            )
        }))
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
    let house = open_clone(
        provider.as_ref(),
        &commit.repo,
        &target,
        None,
        create,
        digest,
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
        })
        .collect()
}

/// The name of `file`'s entry in [`OUTSIDE_TEXT`]: the SHA-256 of its path, in hex.
pub(super) fn record_entry(file: &str) -> String {
    Sha256::digest(file.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
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

/// Clones `repo` into `path` on `main`, its git directory in `git_dir` when given.
fn open_clone(
    provider: &dyn SafeHouseProvider,
    repo: &str,
    path: &Path,
    git_dir: Option<&Path>,
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
            git_dir: git_dir.map(|dir| dir.to_string_lossy().into_owned()),
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
        if name.starts_with('.') || name == MEMORY {
            continue;
        }
        walk(&entry.path(), &name, &mut |path, file| {
            if found.len() < MAX_WALK
                && let Ok(metadata) = fs::symlink_metadata(file)
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
        let file = in_tree(root, path);
        match text {
            Some(text) => write_file(root, &file, text)?,
            None => remove(root, &file).map_err(|error| format!("{path}: {error}"))?,
        }
    }
    Ok(())
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

/// Removes the file `path`, under `root`, if it is there; never through a link.
fn remove(root: &Path, path: &Path) -> std::io::Result<()> {
    no_links(root, path)?;
    match fs::remove_file(path) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

/// The text of the plain file `path`, under `root`, reached through no link.
fn read_text(root: &Path, path: &Path) -> Option<String> {
    no_links(root, path).ok()?;
    fs::symlink_metadata(path)
        .is_ok_and(|metadata| metadata.is_file())
        .then(|| fs::read_to_string(path).ok())
        .flatten()
}

/// Refuses `path` when `root`, or anything from `root` down to it, is a link: a symbolic link,
/// a junction or any other reparse point, which would take a read or write of the Station's
/// somewhere else. What does not exist yet is fine.
fn no_links(root: &Path, path: &Path) -> std::io::Result<()> {
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
fn is_link(metadata: &fs::Metadata) -> bool {
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

/// `path` with `.` and `..` resolved by name.
fn plain_path(path: &Path) -> PathBuf {
    let mut plain = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                plain.pop();
            }
            other => plain.push(other.as_os_str()),
        }
    }
    plain
}

/// What a file tool call can answer with: one file, or anything under a directory, as given.
enum Scope {
    File(String),
    Under(String),
}

/// A [`Scope`] resolved to [`path_key`] form; `None` covers everything.
struct Covered {
    key: Option<String>,
    under: bool,
}

impl Scope {
    /// Resolves a relative path against `area`, her working directory.
    fn resolve(&self, area: &Path) -> Covered {
        let (raw, under) = match self {
            Self::File(raw) => (raw, false),
            Self::Under(raw) => (raw, true),
        };
        Covered {
            key: expand(area, raw).map(|path| path_key(&path)),
            under,
        }
    }
}

impl Covered {
    fn covers(&self, key: &str) -> bool {
        let Some(scope) = &self.key else {
            return true;
        };
        key == scope
            || (self.under && key.starts_with(&format!("{}/", scope.trim_end_matches('/'))))
    }
}

/// The reading calls (`READING_TOOLS`) of a harness event's raw line, each with what it can
/// answer with; `None` when the line is not one this desk can read.
fn reading_calls(data: Option<&[u8]>) -> Option<Vec<(String, Scope)>> {
    let value: serde_json::Value = serde_json::from_slice(data?).ok()?;
    let mut calls = Vec::new();
    for block in value.pointer("/message/content")?.as_array()? {
        if block.get("type").and_then(serde_json::Value::as_str) != Some("tool_use") {
            continue;
        }
        let tool = block.get("name")?.as_str()?;
        if !READING_TOOLS.contains(&tool) {
            continue;
        }
        let input = block.get("input")?;
        // An absent path is her working directory; one that is not text is unreadable.
        let path = match input.get("path") {
            None | Some(serde_json::Value::Null) => ".",
            Some(path) => path.as_str()?,
        };
        match tool {
            "Read" | "Edit" => calls.push((
                tool.to_owned(),
                Scope::File(input.get("file_path")?.as_str()?.to_owned()),
            )),
            "Grep" => calls.push((tool.to_owned(), Scope::Under(path.to_owned()))),
            _ => {
                let pattern = input.get("pattern")?.as_str()?;
                let prefix = literal_prefix(pattern);
                // An absolute pattern may be taken as it is or under `path`: both are covered.
                if Path::new(&prefix).has_root() {
                    calls.push((tool.to_owned(), Scope::Under(prefix.clone())));
                }
                let under = Path::new(path).join(prefix.trim_start_matches(['/', '\\']));
                calls.push((
                    tool.to_owned(),
                    Scope::Under(under.to_string_lossy().into_owned()),
                ));
            }
        }
    }
    (!calls.is_empty()).then_some(calls)
}

/// The directories a glob pattern names before its first wildcard or escape.
fn literal_prefix(pattern: &str) -> String {
    let separators: &[char] = if cfg!(windows) { &['/', '\\'] } else { &['/'] };
    let mut parts: Vec<&str> = pattern.split(separators).collect();
    // The last name is matched, not a directory to look in.
    parts.pop();
    parts
        .into_iter()
        .take_while(|part| !part.contains(['*', '?', '[', ']', '{', '}', '!', '\\']))
        .collect::<Vec<_>>()
        .join("/")
}

/// The `@path` imports of a memory file's text, as written.
fn imports(text: &str) -> Vec<&str> {
    let mut found = Vec::new();
    for word in text.split_whitespace() {
        if let Some(path) = word.strip_prefix('@').filter(|path| !path.is_empty()) {
            found.push(path);
            let trimmed = path.trim_end_matches(['.', ',', ';', ':', ')', ']', '"', '\'', '`']);
            if trimmed != path && !trimmed.is_empty() {
                found.push(trimmed);
            }
        }
    }
    found
}

/// `raw` as a path: `~` is the Station user's home, and a relative path is under `base`.
/// `None` when it names a home that is not known.
fn expand(base: &Path, raw: &str) -> Option<PathBuf> {
    if let Some(rest) = raw.strip_prefix('~') {
        if !(rest.is_empty() || rest.starts_with(['/', '\\'])) {
            return None;
        }
        let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })?;
        return Some(PathBuf::from(home).join(rest.trim_start_matches(['/', '\\'])));
    }
    Some(base.join(raw))
}

/// The path of `file`, a `/`-separated path relative to `root`.
fn in_tree(root: &Path, file: &str) -> PathBuf {
    file.split('/')
        .fold(root.to_path_buf(), |at, name| at.join(name))
}

/// The form two paths are compared in: `.` and `..` resolved, the longest part that exists
/// resolved by the filesystem (long names for short ones, links followed), `/`-separated and
/// lowercase, so two spellings of one file meet (and some different files too, which only
/// counts more as read).
fn path_key(path: &Path) -> String {
    let plain = plain_path(path);
    let mut base = plain.clone();
    let mut rest = Vec::new();
    let resolved = loop {
        if let Ok(found) = fs::canonicalize(&base) {
            break rest
                .iter()
                .rev()
                .fold(found, |at: PathBuf, name| at.join(name));
        }
        match (base.file_name().map(ToOwned::to_owned), base.parent()) {
            (Some(name), Some(parent)) => {
                rest.push(name);
                base = parent.to_path_buf();
            }
            _ => break plain,
        }
    };
    let text = resolved.to_string_lossy().replace('\\', "/");
    let text = if let Some(unc) = text.strip_prefix("//?/UNC/") {
        format!("//{unc}")
    } else {
        text.strip_prefix("//?/").unwrap_or(&text).to_owned()
    };
    let trimmed = text.trim_end_matches(['/', '.', ' ']);
    if trimmed.is_empty() { "/" } else { trimmed }.to_lowercase()
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
    fn nothing_is_read_written_or_removed_through_a_link() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("secret.md"), "secret").unwrap();
        let area = root.path().join("area");
        let personal = area.join(PERSONAL);
        fs::create_dir_all(&personal).unwrap();
        let memory = personal.join(MEMORY);
        link_directory(outside.path(), &memory);
        // The link works: anything following it reaches outside the area.
        assert_eq!(
            fs::read_to_string(memory.join("secret.md")).unwrap(),
            "secret"
        );

        assert!(write_file(&area, &memory.join("x.md"), "x").is_err());
        assert!(!outside.path().join("x.md").exists());
        assert!(write_file(&area, &memory.join("new").join("x.md"), "x").is_err());
        assert!(!outside.path().join("new").exists());
        assert_eq!(read_text(&area, &memory.join("secret.md")), None);
        assert!(remove(&area, &memory.join("secret.md")).is_err());
        assert!(outside.path().join("secret.md").exists());
        assert!(read_tree(&memory, MEMORY).is_empty());
        assert!(stat_tree(&personal).is_empty());
        let change = AssistantChange::new(BTreeMap::from([(
            "memory/secret.md".to_owned(),
            Some("overwritten".to_owned()),
        )]));
        assert!(write_change(&personal, &change).is_err());
        assert_eq!(
            fs::read_to_string(outside.path().join("secret.md")).unwrap(),
            "secret"
        );

        // A plain path beside it is read and written as before.
        write_file(&area, &personal.join("notes").join("a.md"), "a").unwrap();
        assert_eq!(
            read_text(&area, &personal.join("notes").join("a.md")).as_deref(),
            Some("a")
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_file_link_is_never_read_or_written() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let secret = outside.path().join("secret.md");
        fs::write(&secret, "secret").unwrap();
        let area = root.path().join("area");
        fs::create_dir_all(&area).unwrap();
        let index = area.join("MEMORY.md");
        std::os::unix::fs::symlink(&secret, &index).unwrap();
        assert_eq!(fs::read_to_string(&index).unwrap(), "secret");
        assert_eq!(read_text(&area, &index), None);
        assert!(write_file(&area, &index, "x").is_err());
        assert_eq!(fs::read_to_string(&secret).unwrap(), "secret");
    }

    #[test]
    fn a_file_tool_call_is_read_for_what_it_can_answer_with() {
        let line = |name: &str, input: serde_json::Value| {
            serde_json::json!({ "type": "assistant", "message": { "content": [
                { "type": "text", "text": "Looking." },
                { "type": "tool_use", "id": "t", "name": name, "input": input }
            ] } })
            .to_string()
            .into_bytes()
        };
        let scopes = |data: Vec<u8>| {
            reading_calls(Some(&data)).map(|calls| {
                calls
                    .into_iter()
                    .map(|(_, scope)| match scope {
                        Scope::File(path) => format!("file {path}"),
                        Scope::Under(path) => format!("under {}", path.replace('\\', "/")),
                    })
                    .collect::<Vec<_>>()
            })
        };
        assert_eq!(
            scopes(line("Read", serde_json::json!({ "file_path": "/a/b.md" }))),
            Some(vec!["file /a/b.md".to_owned()])
        );
        assert_eq!(
            scopes(line("Grep", serde_json::json!({ "pattern": "x" }))),
            Some(vec!["under .".to_owned()])
        );
        assert_eq!(
            scopes(line(
                "Glob",
                serde_json::json!({ "pattern": "notes/*/x.md", "path": "personal" })
            )),
            Some(vec!["under personal/notes".to_owned()])
        );
        assert_eq!(
            scopes(line("Glob", serde_json::json!({ "pattern": "/srv/x/**" }))),
            Some(vec!["under /srv/x".to_owned(), "under ./srv/x".to_owned()])
        );
        // What cannot be read is no answer, which counts as every file.
        assert_eq!(scopes(line("Read", serde_json::json!({}))), None);
        assert_eq!(
            scopes(line(
                "Grep",
                serde_json::json!({ "pattern": "x", "path": 1 })
            )),
            None
        );
        assert_eq!(scopes(b"not json".to_vec()), None);
        assert_eq!(reading_calls(None).map(|calls| calls.len()), None);
        assert_eq!(scopes(line("Write", serde_json::json!({}))), None);
    }

    #[test]
    fn a_memory_file_imports_each_at_path() {
        assert_eq!(
            imports("See @notes/a.md.\n- @~/x and me@host\n@"),
            ["notes/a.md.", "notes/a.md", "~/x"]
        );
    }

    #[test]
    fn two_spellings_of_one_file_meet() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join("notes")).unwrap();
        fs::write(root.path().join("notes").join("a.md"), "a").unwrap();
        let key = path_key(&root.path().join("notes").join("a.md"));
        assert_eq!(
            path_key(
                &root
                    .path()
                    .join("NOTES")
                    .join(".")
                    .join("x")
                    .join("..")
                    .join("A.md")
            ),
            key
        );
        let scope = Covered {
            key: Some(path_key(root.path())),
            under: true,
        };
        assert!(scope.covers(&key));
        let other = Covered {
            key: Some(path_key(&root.path().join("note"))),
            under: true,
        };
        assert!(!other.covers(&key), "a name's prefix is not its directory");
    }

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
