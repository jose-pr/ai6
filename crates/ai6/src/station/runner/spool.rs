//! A Session's events on disk, one JSON line each with its sequence number.

use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::directorates::section_00::agent::AgentEvent;

const EVENTS_FILE: &str = "events.jsonl";
const CURSOR_FILE: &str = "cursor";

/// One spooled event and its place in the Session's sequence, which starts at 1.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Spooled {
    pub(crate) seq: u64,
    pub(crate) event: AgentEvent,
}

/// The events of one Session: kept in memory for readers and appended to `events.jsonl`
/// before anyone can read them.
pub(super) struct Spool {
    dir: PathBuf,
    file: File,
    events: Vec<Spooled>,
    cursor: u64,
}

impl Spool {
    /// Opens the spool in `dir`, reading back the events already in it. A last line cut
    /// short by a crash is dropped.
    pub(super) fn open(dir: &Path) -> io::Result<Self> {
        fs::create_dir_all(dir)?;
        let path = dir.join(EVENTS_FILE);
        let mut events = Vec::new();
        let mut kept = 0u64;
        if path.exists() {
            let mut reader = BufReader::new(File::open(&path)?);
            let mut line = Vec::new();
            loop {
                line.clear();
                let read = reader.read_until(b'\n', &mut line)?;
                // Only a line that reached its newline was written whole.
                if read == 0 || line.last() != Some(&b'\n') {
                    break;
                }
                match serde_json::from_slice::<Spooled>(&line[..line.len() - 1]) {
                    Ok(spooled) if spooled.seq == events.len() as u64 + 1 => {
                        kept += read as u64;
                        events.push(spooled);
                    }
                    _ => break,
                }
            }
        }
        // Anything after the last whole line is a torn write; later lines must follow on.
        if path.exists() && fs::metadata(&path)?.len() != kept {
            OpenOptions::new().write(true).open(&path)?.set_len(kept)?;
        }
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        let cursor = fs::read_to_string(dir.join(CURSOR_FILE))
            .ok()
            .and_then(|text| text.trim().parse::<u64>().ok())
            .unwrap_or(0)
            .min(events.len() as u64);
        Ok(Self {
            dir: dir.to_path_buf(),
            file,
            events,
            cursor,
        })
    }

    /// Writes `event` to disk and answers its sequence number.
    pub(super) fn append(&mut self, event: AgentEvent) -> io::Result<u64> {
        let spooled = Spooled {
            seq: self.events.len() as u64 + 1,
            event,
        };
        let mut line = serde_json::to_vec(&spooled).map_err(io::Error::other)?;
        line.push(b'\n');
        self.file.write_all(&line)?;
        self.file.flush()?;
        let seq = spooled.seq;
        self.events.push(spooled);
        Ok(seq)
    }

    /// Up to `limit` events after `seq`.
    pub(super) fn after(&self, seq: u64, limit: usize) -> Vec<Spooled> {
        let start = usize::try_from(seq)
            .unwrap_or(usize::MAX)
            .min(self.events.len());
        self.events[start..].iter().take(limit).cloned().collect()
    }

    /// The sequence number of the newest event; `0` when there is none.
    pub(super) fn last(&self) -> u64 {
        self.events.len() as u64
    }

    /// The last sequence number a reader acknowledged.
    pub(super) fn cursor(&self) -> u64 {
        self.cursor
    }

    /// Records that a reader has seen every event up to `seq`.
    pub(super) fn acknowledge(&mut self, seq: u64) -> io::Result<()> {
        let seq = seq.min(self.last());
        if seq == self.cursor {
            return Ok(());
        }
        let path = self.dir.join(CURSOR_FILE);
        let partial = self.dir.join(format!("{CURSOR_FILE}.tmp"));
        fs::write(&partial, format!("{seq}\n"))?;
        fs::rename(&partial, &path)?;
        self.cursor = seq;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(kind: &str, text: &str) -> AgentEvent {
        AgentEvent {
            kind: kind.to_owned(),
            text: Some(text.to_owned()),
            data: None,
            limit: None,
            usage: None,
            tools: Vec::new(),
        }
    }

    #[test]
    fn events_survive_a_reopen_with_their_numbers_and_the_cursor() {
        let dir = tempfile::tempdir().unwrap();
        let mut spool = Spool::open(dir.path()).unwrap();
        assert_eq!(spool.append(text("text", "one")).unwrap(), 1);
        assert_eq!(spool.append(text("text", "two")).unwrap(), 2);
        spool.acknowledge(1).unwrap();
        drop(spool);

        let mut spool = Spool::open(dir.path()).unwrap();
        assert_eq!(spool.last(), 2);
        assert_eq!(spool.cursor(), 1);
        let missed = spool.after(spool.cursor(), 10);
        assert_eq!(missed.len(), 1);
        assert_eq!(missed[0].seq, 2);
        assert_eq!(missed[0].event.text.as_deref(), Some("two"));
        assert_eq!(spool.append(text("completed", "done")).unwrap(), 3);
        assert!(spool.after(3, 10).is_empty());
        assert_eq!(spool.after(0, 2).len(), 2);
    }

    #[test]
    fn a_torn_last_line_is_dropped_and_numbering_continues() {
        let dir = tempfile::tempdir().unwrap();
        let mut spool = Spool::open(dir.path()).unwrap();
        spool.append(text("text", "whole")).unwrap();
        drop(spool);
        let mut file = OpenOptions::new()
            .append(true)
            .open(dir.path().join(EVENTS_FILE))
            .unwrap();
        file.write_all(br#"{"seq":2,"event":{"kind":"te"#).unwrap();
        drop(file);

        let mut spool = Spool::open(dir.path()).unwrap();
        assert_eq!(spool.last(), 1);
        assert_eq!(spool.append(text("text", "next")).unwrap(), 2);
        drop(spool);
        let spool = Spool::open(dir.path()).unwrap();
        assert_eq!(spool.last(), 2);
        assert_eq!(spool.after(1, 1)[0].event.text.as_deref(), Some("next"));
    }
}
