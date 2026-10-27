// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Windows: a Job Object owning the process tree, no console window, and `PATHEXT` lookup.

#![allow(unsafe_code)]

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io;
use std::os::windows::io::AsRawHandle;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus};
use std::ptr;

use ai6::directorates::q::terminal::ExitInfo;
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_BASIC_ACCOUNTING_INFORMATION, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JobObjectBasicAccountingInformation, JobObjectExtendedLimitInformation,
    QueryInformationJobObject, SetInformationJobObject, TerminateJobObject,
};
use windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;

/// What `PATHEXT` holds when the environment does not say.
const DEFAULT_PATHEXT: &str = ".COM;.EXE;.BAT;.CMD";

pub(crate) struct ProcessTree(HANDLE);

// SAFETY: a Job Object handle is a process-wide kernel capability. The handle is immutable, the
// Win32 operations on it are thread-safe, and Drop owns the only close.
unsafe impl Send for ProcessTree {}
// SAFETY: as for Send.
unsafe impl Sync for ProcessTree {}

impl ProcessTree {
    /// Puts the child in a kill-on-close Job Object, so killing the job kills every process the
    /// child started.
    pub(crate) fn adopt(child: &Child) -> io::Result<Self> {
        // SAFETY: a null name and security descriptor create a private Job Object.
        let job = unsafe { CreateJobObjectW(ptr::null(), ptr::null()) };
        if job.is_null() {
            return Err(io::Error::last_os_error());
        }
        let tree = Self(job);
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        // SAFETY: `limits` is the correctly sized structure for this info class, and `job` is
        // the handle created above.
        if unsafe {
            SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                std::mem::size_of_val(&limits) as u32,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `child` owns its process handle, open with full access, for this call.
        if unsafe { AssignProcessToJobObject(job, child.as_raw_handle() as HANDLE) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(tree)
    }

    pub(crate) fn kill(&self) -> io::Result<()> {
        // SAFETY: `self.0` is owned and stays open until Drop.
        if unsafe { TerminateJobObject(self.0, 1) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    /// Whether a process of the tree is still running. Until none is, the tree may hold its
    /// working directory and files open, and Windows refuses to delete them.
    pub(crate) fn running(&self) -> bool {
        let mut info = JOBOBJECT_BASIC_ACCOUNTING_INFORMATION::default();
        // SAFETY: `info` is the correctly sized structure for this info class, and `self.0`
        // stays open until Drop.
        let queried = unsafe {
            QueryInformationJobObject(
                self.0,
                JobObjectBasicAccountingInformation,
                (&mut info as *mut JOBOBJECT_BASIC_ACCOUNTING_INFORMATION).cast(),
                std::mem::size_of_val(&info) as u32,
                ptr::null_mut(),
            )
        } != 0;
        queried && info.ActiveProcesses > 0
    }
}

impl Drop for ProcessTree {
    fn drop(&mut self) {
        // SAFETY: this type owns the handle and closes it once.
        unsafe { CloseHandle(self.0) };
    }
}

/// Starts the child with a console of its own that has no window: a Station running without a
/// console would otherwise open a visible one for every console program it starts.
pub(crate) fn own_process_tree(cmd: &mut Command) {
    cmd.creation_flags(CREATE_NO_WINDOW);
}

/// The program found on the child's `PATH` as a shell finds it: a name with an extension as
/// given, else with each `PATHEXT` extension (`codex` as `codex.cmd`); the standard library
/// tries only `.exe`. A name with a directory in it, or one found nowhere, is passed on as given.
pub(crate) fn resolve(program: &str, env: Option<&BTreeMap<String, String>>) -> PathBuf {
    if program.contains(['/', '\\']) {
        return PathBuf::from(program);
    }
    let lookup = |name: &str| -> Option<OsString> {
        match env {
            Some(env) => env
                .iter()
                .find(|(key, _)| key.eq_ignore_ascii_case(name))
                .map(|(_, value)| OsString::from(value)),
            None => std::env::var_os(name),
        }
    };
    let Some(path) = lookup("PATH") else {
        return PathBuf::from(program);
    };
    let extensions = lookup("PATHEXT")
        .and_then(|value| value.into_string().ok())
        .unwrap_or_else(|| DEFAULT_PATHEXT.to_owned());
    // npm puts a POSIX script named `codex` beside `codex.cmd`: a bare name is not run as is.
    let named = Path::new(program).extension().is_some();
    for dir in std::env::split_paths(&path) {
        let exact = dir.join(program);
        if named && exact.is_file() {
            return exact;
        }
        for extension in extensions.split(';').filter(|e| e.starts_with('.')) {
            let candidate = dir.join(format!("{program}{extension}"));
            if candidate.is_file() {
                return candidate;
            }
        }
    }
    Path::new(program).to_path_buf()
}

/// The exit code as the signed 32-bit value the standard library and every transport report:
/// Windows' `0xFFFFFFFF` is `-1` and its NTSTATUS `0xC000013A` is `-1073741510`.
pub(crate) fn exit_info(status: ExitStatus) -> ExitInfo {
    ExitInfo {
        running: false,
        code: status.code().map(i64::from),
        signal: None,
    }
}

#[cfg(test)]
mod tests {
    use std::os::windows::process::ExitStatusExt;

    use super::*;

    #[test]
    fn an_exit_code_with_the_high_bit_set_is_negative() {
        for (raw, code) in [
            (0, 0),
            (1, 1),
            (0xFFFF_FFFF, -1),
            (0xC000_013A, -1_073_741_510),
        ] {
            let exit = exit_info(ExitStatus::from_raw(raw));
            assert_eq!(exit.code, Some(code), "{raw:#010X}");
            assert!(!exit.running && exit.signal.is_none());
        }
    }
}
