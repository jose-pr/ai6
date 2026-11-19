//! A session's history, from the transcript Claude Code keeps of it.

use alloc::string::ToString;
use alloc::sync::Arc;
use alloc::vec::Vec;
use std::ffi::OsString;
use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use coqui_acp_harness::event::Event;
use coqui_acp_harness::lines::{self, Line};

use crate::Error;
use crate::launch::valid_session_id;
use crate::parse::{Parser, ParserLimits, Writer};

/// Claude Code's configuration directory for an environment: `CLAUDE_CONFIG_DIR`, else `.claude` in the home
/// directory (`USERPROFILE` on Windows, `HOME` elsewhere). `env` is what the launch sets, over this process's
/// environment unless `clear_env`.
#[must_use]
pub fn config_dir(
    env: &alloc::collections::BTreeMap<OsString, OsString>,
    clear_env: bool,
) -> Option<PathBuf> {
    let get = |name: &str| {
        env.iter()
            .find(|(key, _)| key.to_string_lossy().eq_ignore_ascii_case(name))
            .map(|(_, value)| value.clone())
            .or_else(|| {
                if clear_env {
                    None
                } else {
                    std::env::var_os(name)
                }
            })
            .filter(|value| !value.is_empty())
    };
    if let Some(dir) = get("CLAUDE_CONFIG_DIR") {
        return Some(PathBuf::from(dir));
    }
    let home = if cfg!(windows) {
        get("USERPROFILE").or_else(|| get("HOME"))
    } else {
        get("HOME")
    };
    home.map(|home| PathBuf::from(home).join(".claude"))
}

/// The directory under `projects` where Claude Code keeps the sessions of `cwd`: every character but an ASCII letter or
/// digit as `-`.
#[must_use]
pub fn project_directory(cwd: &Path) -> String {
    cwd.to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// The transcript of session `id` in `cwd`: `projects/<project_directory(cwd)>/<id>.jsonl` under `config`. Only
/// that directory is looked in, as `--resume` finds a session only from the directory that recorded it, and a
/// link is not followed.
///
/// # Errors
///
/// [`Error::Refused`] for an id that cannot name a session, or one with no transcript there.
pub fn transcript(config: &Path, cwd: &Path, id: &str) -> Result<PathBuf, Error> {
    if !valid_session_id(id) {
        return Err(Error::Refused(
            "that is not a Claude Code session id".into(),
        ));
    }
    let path = config
        .join("projects")
        .join(project_directory(cwd))
        .join(alloc::format!("{id}.jsonl"));
    match std::fs::symlink_metadata(&path) {
        Ok(meta) if meta.is_file() => Ok(path),
        _ => Err(Error::Refused(
            "no transcript of that session was found for this directory".into(),
        )),
    }
}

/// Sends the history in `transcript` to `sink` as events: the person's messages, the agent's text and thinking, and
/// its tool calls with their results. At most `max_bytes` of it are read; past that the rest is left out, and an
/// [`Event::Notice`] says so.
///
/// # Errors
///
/// [`Error::Refused`] when the transcript cannot be read.
pub fn replay(
    transcript: &Path,
    max_bytes: u64,
    max_tools: usize,
    limits: ParserLimits,
    sink: &dyn Fn(Event) -> bool,
) -> Result<(), Error> {
    let file = File::open(transcript)
        .map_err(|_| Error::Refused("the session's transcript could not be read".into()))?;
    let size = file.metadata().map(|meta| meta.len()).unwrap_or(0);
    let writer: Writer = Arc::new(|_| Err(io::Error::other("a replayed session asks nothing")));
    let mut parser = Parser::with_limits(writer, max_tools, limits);
    let mut events = Vec::new();
    let read = lines::read_lines(file.take(max_bytes), max_bytes, |line| {
        if let Line::Json(value) = line {
            events.extend(parser.history(&value));
        }
        true
    });
    read.map_err(|_| Error::Refused("the session's transcript could not be read".into()))?;
    for event in events {
        if !sink(event) {
            return Ok(());
        }
    }
    if size > max_bytes {
        sink(Event::Notice {
            severity: coqui_acp_harness::event::Severity::Warning,
            title: alloc::format!(
                "Only the first {max_bytes} bytes of this session's history were replayed"
            )
            .to_string(),
        });
    }
    Ok(())
}
