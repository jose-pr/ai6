// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Windows: ConPTY, a Job Object owning the process tree, and Enter as CR.
//!
//! `portable-pty` starts the child itself, with no way to name a Job Object, so the child joins
//! its job just after it has started. Any process it started before then is found by parent
//! and creation time and put in the job too.

use std::borrow::Cow;
use std::collections::BTreeSet;
use std::io;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};

use windows_sys::Win32::Foundation::{FILETIME, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::JobObjects::{AssignProcessToJobObject, IsProcessInJob};
use windows_sys::Win32::System::Threading::{
    GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SET_QUOTA,
    PROCESS_TERMINATE,
};

pub(crate) use crate::process_tree::ProcessTree;

/// Puts the child in a new kill-on-close Job Object, with every process it started before it
/// joined, so killing the job kills every process the child started.
pub(crate) fn adopt(pid: Option<u32>, scans: usize) -> io::Result<ProcessTree> {
    let pid =
        pid.ok_or_else(|| io::Error::other("portable-pty did not return the Windows process ID"))?;
    let tree = ProcessTree::new()?;
    let child = open(pid)?;
    // SAFETY: both handles are open for the call.
    if unsafe { AssignProcessToJobObject(tree.handle(), child.as_raw_handle()) } == 0 {
        return Err(io::Error::last_os_error());
    }
    adopt_descendants(&tree, pid, created(&child)?, scans);
    Ok(tree)
}

/// Puts in `tree` every process descended from `root` that is not in it yet, until a scan finds
/// none. A process counts as a child only when it was created after its parent `root` (the
/// tree's first process, created at `since`): a parent ID is never updated, so a process whose
/// parent exited long ago may name a reused one.
fn adopt_descendants(tree: &ProcessTree, root: u32, since: u64, scans: usize) {
    let mut found = BTreeSet::from([root]);
    for _ in 0..scans {
        let mut grew = false;
        for (pid, parent) in processes() {
            if found.contains(&pid) || !found.contains(&parent) {
                continue;
            }
            let Ok(process) = open(pid) else {
                continue;
            };
            if created(&process).is_ok_and(|at| at >= since) {
                found.insert(pid);
                grew = true;
                let mut inside = 0;
                // SAFETY: both handles are open for the calls; `inside` outlives the first.
                unsafe {
                    if IsProcessInJob(process.as_raw_handle(), tree.handle(), &mut inside) != 0
                        && inside == 0
                    {
                        AssignProcessToJobObject(tree.handle(), process.as_raw_handle());
                    }
                }
            }
        }
        if !grew {
            break;
        }
    }
}

/// Every process the system lists, with its parent's ID.
fn processes() -> Vec<(u32, u32)> {
    // SAFETY: a process snapshot needs no process ID.
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snapshot == INVALID_HANDLE_VALUE || snapshot.is_null() {
        return Vec::new();
    }
    // SAFETY: CreateToolhelp32Snapshot returned this handle for the caller to own.
    let snapshot = unsafe { OwnedHandle::from_raw_handle(snapshot) };
    let mut entry = PROCESSENTRY32W {
        dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    let mut table = Vec::new();
    // SAFETY: `entry` is a correctly sized structure the calls fill in.
    let mut more = unsafe { Process32FirstW(snapshot.as_raw_handle(), &mut entry) } != 0;
    while more {
        table.push((entry.th32ProcessID, entry.th32ParentProcessID));
        // SAFETY: as above.
        more = unsafe { Process32NextW(snapshot.as_raw_handle(), &mut entry) } != 0;
    }
    table
}

/// Process `pid`, open to be put in a job, terminated and asked its times.
fn open(pid: u32) -> io::Result<OwnedHandle> {
    // SAFETY: OpenProcess takes plain values and returns a handle for the caller to own.
    let process = unsafe {
        OpenProcess(
            PROCESS_SET_QUOTA | PROCESS_TERMINATE | PROCESS_QUERY_LIMITED_INFORMATION,
            0,
            pid,
        )
    };
    if process.is_null() {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: as above.
    Ok(unsafe { OwnedHandle::from_raw_handle(process as HANDLE) })
}

/// When `process` was created, in 100-nanosecond intervals since 1601.
fn created(process: &OwnedHandle) -> io::Result<u64> {
    let mut times = [FILETIME::default(); 4];
    let [creation, exit, kernel, user] = &mut times;
    // SAFETY: the handle is open and every FILETIME outlives the call.
    if unsafe { GetProcessTimes(process.as_raw_handle(), creation, exit, kernel, user) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok((u64::from(creation.dwHighDateTime) << 32) | u64::from(creation.dwLowDateTime))
}

/// Nothing to set: a program under ConPTY chooses its own console modes.
pub(crate) fn set_terminal_mode(_master: &dyn portable_pty::MasterPty) -> io::Result<()> {
    Ok(())
}

/// ConPTY reads Enter as CR; a bare LF or a CRLF pair becomes one CR.
pub(crate) fn translate_input(data: &[u8]) -> Cow<'_, [u8]> {
    let mut translated = Vec::with_capacity(data.len());
    let mut index = 0;
    while index < data.len() {
        match data[index] {
            b'\r' => {
                translated.push(b'\r');
                index += if data.get(index + 1) == Some(&b'\n') {
                    2
                } else {
                    1
                };
            }
            b'\n' => {
                translated.push(b'\r');
                index += 1;
            }
            byte => {
                translated.push(byte);
                index += 1;
            }
        }
    }
    Cow::Owned(translated)
}

#[cfg(test)]
mod tests {
    use std::os::windows::process::CommandExt;
    use std::time::{Duration, Instant};

    use super::*;

    /// A process the child started before it joined its job is put in the job too, and dies
    /// with it.
    #[test]
    fn a_process_started_before_the_child_joined_its_job_is_adopted() {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let mut child = std::process::Command::new("cmd.exe")
            .args([
                "/d",
                "/c",
                "start /b ping -n 30 127.0.0.1 >NUL & ping -n 30 127.0.0.1 >NUL",
            ])
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .unwrap();
        // Its console host and both pings.
        let deadline = Instant::now() + Duration::from_secs(10);
        let started = loop {
            let started: Vec<u32> = processes()
                .into_iter()
                .filter(|(_, parent)| *parent == child.id())
                .map(|(pid, _)| pid)
                .collect();
            if started.len() >= 3 {
                break started;
            }
            assert!(Instant::now() < deadline, "cmd.exe started {started:?}");
            std::thread::sleep(Duration::from_millis(20));
        };
        let tree = adopt(Some(child.id()), 8).unwrap();
        for pid in started {
            let process = open(pid).unwrap();
            let mut inside = 0;
            // SAFETY: both handles are open; `inside` outlives the call.
            assert_ne!(
                unsafe { IsProcessInJob(process.as_raw_handle(), tree.handle(), &mut inside) },
                0
            );
            assert_ne!(inside, 0, "process {pid}, started earlier, is in the job");
        }
        tree.kill(true).unwrap();
        let _ = child.wait();
        let deadline = Instant::now() + Duration::from_secs(5);
        while tree.running() {
            assert!(Instant::now() < deadline, "the job still runs");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}
