// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Unix: a terminal program's process tree, killed whole. Compiled into the `pipe` and `pty`
//! providers alike (each includes this file with `#[path]`), so both kill the same way.
//!
//! The program leads a session and a process group of its own. Killing the group misses a
//! process that left it (`setsid`, `setpgid`), so the tree is also found by parent: every
//! process whose parent is in the tree, as many times as it takes for a scan to find nothing
//! new. Each process found is stopped (`SIGSTOP`) before any is killed, so none can start a new
//! one out of reach in between. A process whose parent chain no longer reaches the tree (its
//! parent exited and it was reparented) is not found, unless it is still in the program's group
//! or, on Linux and Android, its session.

#![allow(unsafe_code)]

use std::collections::BTreeSet;
use std::io;

/// How many times the process table is read again for processes started meanwhile; each scan
/// stops what it finds, so a tree stops growing after a few.
const MAX_SCANS: usize = 32;

/// A program's process tree: its process ID, which is also its process group and session ID.
pub(crate) struct ProcessTree(libc::pid_t);

/// One process of the table: its ID, its parent's, its process group's and, where the platform
/// says, its session's.
struct Entry {
    pid: libc::pid_t,
    ppid: libc::pid_t,
    pgid: libc::pid_t,
    sid: Option<libc::pid_t>,
}

impl ProcessTree {
    /// The tree of the program with process ID `pid`, which leads its own process group.
    pub(crate) fn of(pid: Option<u32>) -> io::Result<Self> {
        pid.and_then(|pid| libc::pid_t::try_from(pid).ok())
            .filter(|&pid| pid > 1)
            .map(Self)
            .ok_or_else(|| io::Error::other("the child's process ID is unknown or out of range"))
    }

    /// Kills every process of the tree with `SIGKILL`: the program's process group, and every
    /// process found by parent from it. `leader_alive` says the program has not been reaped:
    /// only then may a process whose parent ID is the program's be taken for its child, since a
    /// reaped program's ID can be reused.
    pub(crate) fn kill(&self, leader_alive: bool) -> io::Result<()> {
        let leader = self.0;
        // SAFETY: killpg and kill take plain integers and touch no memory. An error means the
        // process or group is gone (or not ours to signal), which leaves nothing to do.
        unsafe { libc::killpg(leader, libc::SIGSTOP) };
        let found = self.stopped_tree(leader_alive);
        // SAFETY: as above.
        unsafe { libc::killpg(leader, libc::SIGKILL) };
        for pid in &found {
            // SAFETY: as above; every pid here is stopped, so it has not exited and its ID is
            // not reused.
            unsafe { libc::kill(*pid, libc::SIGKILL) };
        }
        Ok(())
    }

    /// Finds and stops every process of the tree but the program's group, which `kill` stops as
    /// a whole, until a scan of the process table finds nothing new.
    fn stopped_tree(&self, leader_alive: bool) -> BTreeSet<libc::pid_t> {
        let leader = self.0;
        let me = std::process::id() as libc::pid_t;
        let mut found = BTreeSet::new();
        for _ in 0..MAX_SCANS {
            let table = processes();
            let before = found.len();
            loop {
                let mut grew = false;
                for entry in &table {
                    if entry.pid <= 1 || entry.pid == me || found.contains(&entry.pid) {
                        continue;
                    }
                    let member = entry.pgid == leader
                        || entry.sid == Some(leader)
                        || (leader_alive && entry.ppid == leader)
                        || found.contains(&entry.ppid);
                    if member {
                        // SAFETY: kill takes plain integers and touches no memory.
                        unsafe { libc::kill(entry.pid, libc::SIGSTOP) };
                        found.insert(entry.pid);
                        grew = true;
                    }
                }
                if !grew {
                    break;
                }
            }
            if found.len() == before {
                break;
            }
        }
        found
    }

    /// Always `false`: a Unix directory or file can be removed while processes still use it,
    /// so nothing waits on the rest of the tree once the program itself has exited.
    pub(crate) fn running(&self) -> bool {
        false
    }
}

/// Every process `/proc` lists, with its parent, process group and session.
#[cfg(any(target_os = "linux", target_os = "android"))]
fn processes() -> Vec<Entry> {
    let Ok(dir) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    dir.filter_map(|entry| {
        let pid: libc::pid_t = entry.ok()?.file_name().to_str()?.parse().ok()?;
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        // `pid (comm) state ppid pgrp session …`; the command name may hold spaces and `)`.
        let mut fields = stat.get(stat.rfind(')')? + 1..)?.split_whitespace().skip(1);
        let mut next = || fields.next()?.parse::<libc::pid_t>().ok();
        Some(Entry {
            pid,
            ppid: next()?,
            pgid: next()?,
            sid: next(),
        })
    })
    .collect()
}

/// Every process `ps` lists, with its parent and process group; `ps` here names no session.
#[cfg(not(any(target_os = "linux", target_os = "android")))]
fn processes() -> Vec<Entry> {
    let Ok(output) = std::process::Command::new("/bin/ps")
        .args(["-A", "-o", "pid=", "-o", "ppid=", "-o", "pgid="])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
    else {
        return Vec::new();
    };
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let mut fields = line
                .split_whitespace()
                .map(|field| field.parse::<libc::pid_t>().ok());
            Some(Entry {
                pid: fields.next()??,
                ppid: fields.next()??,
                pgid: fields.next()??,
                sid: None,
            })
        })
        .collect()
}
