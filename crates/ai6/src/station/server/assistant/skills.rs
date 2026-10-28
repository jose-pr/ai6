//! Moneypenny's skills on a Station: the catalog skills a job gives her, installed in her
//! working directory before her harness starts.
//!
//! Each skill is copied from the directory the Station's own roster names
//! (`[assistant.skills.<name>]`) into `.claude/skills/<name>/`, never through a link, and the
//! copy is checked: its [`ProjectManifest::read_tree`] digest against the skill's pin, and its
//! `SKILL.md` as the roster load checks it ([`check_skill`]). A skill that fails either is
//! left out: the Station's log says why and its logbook gets a `provider` finding, and the
//! conversation is told. `.claude/` is a plugin of the harness named
//! [`SKILLS_PLUGIN`], which Claude Code loads with `--plugin-dir` even under `--restricted`;
//! she cannot write there (no path with a name starting with `.`).

use std::fs;
use std::path::{Path, PathBuf};

use crate::directorates::m::assistant::{AssistantSettings, SKILLS_PLUGIN, check_skill};
use crate::directorates::m::harness::{
    MAX_MANIFEST_BYTES, MAX_MANIFEST_FILE_BYTES, MAX_MANIFEST_FILES, ProjectManifest,
};
use crate::directorates::registry::logbook::{
    Severity, StationFinding, StationFindingKind, StationReport,
};

use super::workspace::is_link;
use super::{DaemonState, Logger};

/// The directory of her working directory the skills are installed in: the plugin's root.
pub(super) const SKILLS_ROOT: &str = ".claude";

/// What [`install`] put in place.
#[derive(Debug, Default)]
pub(super) struct Installed {
    /// The skills installed, in the order asked.
    pub(super) names: Vec<String>,
    /// The plugin directory, when one skill or more is installed.
    pub(super) dir: Option<PathBuf>,
    /// What the conversation should be told: each skill left out.
    pub(super) notices: Vec<String>,
}

/// Installs each of `wanted`, all names of `settings`' catalog, in `area`; a skill that cannot
/// be installed as pinned is left out, logged through `log` and filed in the Station's
/// logbook. Blocks on the filesystem and the Registry.
///
/// # Errors
///
/// A message when `area`'s plugin directory cannot be made at all.
pub(super) fn install(
    state: &DaemonState,
    log: &Logger,
    area: &Path,
    settings: &AssistantSettings,
    wanted: &[String],
) -> Result<Installed, String> {
    let mut installed = Installed::default();
    if wanted.is_empty() {
        return Ok(installed);
    }
    let root = area.join(SKILLS_ROOT);
    match fs::symlink_metadata(&root) {
        Ok(_) => fs::remove_dir_all(&root)
            .map_err(|error| format!("{} not cleared: {error}", root.display()))?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("{}: {error}", root.display())),
    }
    for name in wanted {
        let Some(entry) = settings.skills().get(name) else {
            continue;
        };
        let target = root.join("skills").join(name);
        let outcome = copy_tree(Path::new(entry.path()), &target).and_then(|()| {
            if let Some(pin) = entry.pin() {
                let manifest = ProjectManifest::read_tree(&target)?;
                if manifest.digest_with(pin.digest()) != pin {
                    return Err(Skipped::Pin);
                }
            }
            check_skill(&target, name, settings.skills_allow_tools()).map_err(Skipped::Other)
        });
        match outcome {
            Ok(()) => installed.names.push(name.clone()),
            Err(skipped) => {
                let _ = fs::remove_dir_all(&target);
                let line = match &skipped {
                    Skipped::Pin => format!("assistant skill {name} does not match its pin"),
                    Skipped::Other(reason) => {
                        format!("assistant skill {name} was not installed: {reason}")
                    }
                };
                log(&line);
                file_finding(state, name, &line);
                installed.notices.push(format!(
                    "The skill {name:?} is not available in this conversation: the Station \
                     could not install it as the roster pins it."
                ));
            }
        }
    }
    if !installed.names.is_empty() {
        let manifest = serde_json::json!({
            "name": SKILLS_PLUGIN,
            "description": "Moneypenny's skills from the AI6 roster's catalog",
        });
        let dir = root.join(".claude-plugin");
        fs::create_dir_all(&dir)
            .and_then(|()| fs::write(dir.join("plugin.json"), manifest.to_string()))
            .map_err(|error| format!("the skills' plugin manifest was not written: {error}"))?;
        installed.dir = Some(root);
    }
    Ok(installed)
}

/// Why a skill was left out.
enum Skipped {
    /// Its copy does not have its pin's digest.
    Pin,
    /// It could not be copied or read, or its `SKILL.md` was refused.
    Other(String),
}

impl From<String> for Skipped {
    fn from(reason: String) -> Self {
        Self::Other(reason)
    }
}

/// Files a `provider` finding on this Station's logbook; a Registry that refuses it is only
/// logged.
fn file_finding(state: &DaemonState, name: &str, line: &str) {
    let report = StationReport::new(
        state.station_id.clone(),
        format!("Moneypenny's skill {name} was not installed."),
        state.clock.now(),
    )
    .with_findings([StationFinding::new(
        StationFindingKind::Provider,
        Severity::Warning,
        line,
    )]);
    if let Err(error) = state.intake.file_station_report(&report) {
        eprintln!("ai6: the finding on skill {name} was not filed: {error}");
    }
}

/// Copies the directory `from` to `to` (which must not exist), refusing a link, a device or a
/// pipe anywhere in it, and the limits a [`ProjectManifest`] keeps.
fn copy_tree(from: &Path, to: &Path) -> Result<(), Skipped> {
    let metadata = fs::symlink_metadata(from).map_err(|error| error.to_string())?;
    if is_link(&metadata) || !metadata.is_dir() {
        return Err(Skipped::Other(format!(
            "{} is not a directory (a link is never followed)",
            from.display()
        )));
    }
    let mut copied = (0usize, 0u64);
    copy_dir(from, to, &mut copied).map_err(Skipped::Other)
}

fn copy_dir(from: &Path, to: &Path, copied: &mut (usize, u64)) -> Result<(), String> {
    fs::create_dir_all(to).map_err(|error| format!("{}: {error}", to.display()))?;
    let mut entries: Vec<_> = fs::read_dir(from)
        .map_err(|error| format!("{}: {error}", from.display()))?
        .collect::<Result<_, _>>()
        .map_err(|error| format!("{}: {error}", from.display()))?;
    entries.sort_by_key(fs::DirEntry::file_name);
    for entry in entries {
        let path = entry.path();
        let target = to.join(entry.file_name());
        let metadata = fs::symlink_metadata(&path).map_err(|error| error.to_string())?;
        if is_link(&metadata) {
            return Err(format!(
                "{} is a link, which is never followed",
                path.display()
            ));
        }
        if metadata.is_dir() {
            copy_dir(&path, &target, copied)?;
            continue;
        }
        if !metadata.file_type().is_file() {
            return Err(format!("{} is not a regular file", path.display()));
        }
        copied.0 += 1;
        copied.1 += metadata.len();
        if metadata.len() > MAX_MANIFEST_FILE_BYTES
            || copied.0 > MAX_MANIFEST_FILES
            || copied.1 > MAX_MANIFEST_BYTES
        {
            return Err(format!(
                "the skill passes {MAX_MANIFEST_FILE_BYTES} bytes a file, {MAX_MANIFEST_FILES} \
                 files or {MAX_MANIFEST_BYTES} bytes"
            ));
        }
        fs::copy(&path, &target).map_err(|error| format!("{}: {error}", path.display()))?;
    }
    Ok(())
}
