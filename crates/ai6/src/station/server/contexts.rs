//! Filing a Session's context in the Registry: the harness's own record of the Session, read
//! where its handler says it keeps one, else the events the Station saw, said so in the record.
//! Either is held to `[registry.limits.contexts] context_bytes`, its newest part kept.

use std::collections::VecDeque;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

use coqui_acp_provider::AgentEvent;
use serde_json::json;

use super::{DaemonState, unix_now};
use crate::bounds::Limit;
use crate::directorates::registry::contexts::{
    ContextOwner, ContextSource, EVENTS_FORMAT, SessionContext,
};
use crate::directorates::section_00::agent::SessionRecordFile;

/// The events a Session reported, one JSON object per line, the newest that fit a bound.
#[derive(Debug)]
pub(super) struct EventTape {
    lines: VecDeque<String>,
    bytes: usize,
    limit: Limit,
}

impl EventTape {
    /// A tape that keeps at most `limit` bytes of lines.
    pub(super) fn new(limit: Limit) -> Self {
        Self {
            lines: VecDeque::new(),
            bytes: 0,
            limit,
        }
    }

    /// Keeps `event` as a line: when it came, its kind, text, tools, usage and the harness's
    /// own line for it.
    pub(super) fn record(&mut self, event: &AgentEvent) {
        let mut line = json!({
            "at": unix_now(),
            "kind": event.kind,
            "text": event.text,
            "tools": event.tools,
        });
        if let Some(usage) = &event.usage {
            line["usage"] = json!(usage);
        }
        if let Some(data) = &event.data {
            line["data"] = json!(String::from_utf8_lossy(data));
        }
        let mut line = line.to_string();
        line.push('\n');
        self.bytes += line.len();
        self.lines.push_back(line);
        while !self.limit.allows(self.bytes) && self.lines.len() > 1 {
            if let Some(dropped) = self.lines.pop_front() {
                self.bytes -= dropped.len();
            }
        }
    }

    fn content(&self) -> String {
        self.lines.iter().map(String::as_str).collect()
    }
}

/// The newest `limit` bytes of the file at `path`, from the start of a line when it was cut,
/// and the file's size when it was.
fn read_tail(path: &Path, limit: Limit) -> io::Result<(String, Option<u64>)> {
    let mut file = File::open(path)?;
    let size = file.metadata()?.len();
    let keep = limit.get().map_or(size, |limit| limit.min(size));
    let start = size - keep;
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::with_capacity(usize::try_from(keep).unwrap_or(0));
    file.take(keep).read_to_end(&mut bytes)?;
    if start == 0 {
        return Ok((String::from_utf8_lossy(&bytes).into_owned(), None));
    }
    let from = bytes
        .iter()
        .position(|byte| *byte == b'\n')
        .map_or(0, |line| line + 1);
    Ok((
        String::from_utf8_lossy(&bytes[from..]).into_owned(),
        Some(size),
    ))
}

/// Files the context of provider Session `session` for `owner`: the harness's `record` when
/// its file can be read, else `tape`. A failure is logged, never fatal to the Session.
pub(super) fn file(
    state: &DaemonState,
    owner: ContextOwner,
    session: &str,
    handler: &str,
    record: Option<SessionRecordFile>,
    tape: &EventTape,
) {
    let limit = state.intake.limits().contexts.context_bytes;
    let from_harness = record.and_then(|record| match read_tail(Path::new(&record.path), limit) {
        Ok(read) => Some((record.format, read)),
        Err(error) => {
            eprintln!(
                "ai6: Session {session}: its harness record could not be read, so its events \
                     are kept instead: {error}"
            );
            None
        }
    });
    let (format, source, (content, cut_from)) = match from_harness {
        Some((format, read)) => (format, ContextSource::Harness, read),
        None => (
            EVENTS_FORMAT.to_owned(),
            ContextSource::Events,
            (tape.content(), None),
        ),
    };
    let mut context = SessionContext::new(
        session,
        owner,
        state.station_id.as_str(),
        handler,
        format,
        source,
        unix_now(),
        content,
    );
    context.cut_from = cut_from;
    if let Err(error) = state.intake.file_session_context(&context) {
        eprintln!("ai6: Session {session}: its context was not filed: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_long_record_keeps_its_newest_lines_and_its_size() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("s.jsonl");
        std::fs::write(&path, "{\"n\":1}\n{\"n\":2}\n{\"n\":3}\n").unwrap();
        let (whole, cut) = read_tail(&path, Limit::UNBOUNDED).unwrap();
        assert_eq!((whole.lines().count(), cut), (3, None));
        let (tail, cut) = read_tail(&path, Limit::at(10)).unwrap();
        assert_eq!(tail, "{\"n\":3}\n");
        assert_eq!(cut, Some(24));
    }

    #[test]
    fn the_tape_keeps_the_newest_events_within_its_bound() {
        let mut tape = EventTape::new(Limit::at(200));
        for index in 0..10 {
            tape.record(&AgentEvent::new("text", Some(format!("line {index}"))));
        }
        let content = tape.content();
        assert!(content.len() <= 200, "{content}");
        assert!(content.contains("line 9") && !content.contains("line 0"));
        let first: serde_json::Value =
            serde_json::from_str(content.lines().next().unwrap()).unwrap();
        assert_eq!(first["kind"], "text");
    }
}
