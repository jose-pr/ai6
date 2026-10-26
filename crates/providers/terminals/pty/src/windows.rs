// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Windows: ConPTY, a Job Object owning the process tree, and Enter as CR.

use std::borrow::Cow;
use std::io;
use std::ptr;

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_BASIC_ACCOUNTING_INFORMATION, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JobObjectBasicAccountingInformation, JobObjectExtendedLimitInformation,
    QueryInformationJobObject, SetInformationJobObject, TerminateJobObject,
};
use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_SET_QUOTA, PROCESS_TERMINATE};

pub(crate) struct ProcessTree(HANDLE);

// Job Object handles are process-wide kernel capabilities. The handle is
// immutable, the Win32 operations are thread-safe, and Drop owns the only close.
unsafe impl Send for ProcessTree {}
unsafe impl Sync for ProcessTree {}

impl ProcessTree {
    /// Puts the child in a kill-on-close Job Object, so killing the job kills every
    /// process the child started.
    pub(crate) fn adopt(pid: Option<u32>) -> io::Result<Self> {
        let pid = pid.ok_or_else(|| {
            io::Error::other("portable-pty did not return the Windows process ID")
        })?;
        // SAFETY: a null name and security descriptor create a private Job Object.
        let job = unsafe { CreateJobObjectW(ptr::null(), ptr::null()) };
        if job.is_null() {
            return Err(io::Error::last_os_error());
        }
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        // SAFETY: `limits` is the correctly sized structure for this info class.
        if unsafe {
            SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                std::mem::size_of_val(&limits) as u32,
            )
        } == 0
        {
            let error = io::Error::last_os_error();
            // SAFETY: `job` is the valid handle created above.
            unsafe { CloseHandle(job) };
            return Err(error);
        }
        // SAFETY: opens the just-spawned child for job assignment.
        let process = unsafe { OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, 0, pid) };
        if process.is_null() {
            let error = io::Error::last_os_error();
            // SAFETY: `job` is the valid handle created above.
            unsafe { CloseHandle(job) };
            return Err(error);
        }
        // SAFETY: both handles remain valid for the duration of the call.
        let assigned = unsafe { AssignProcessToJobObject(job, process) } != 0;
        // SAFETY: `process` is the valid handle returned by OpenProcess.
        unsafe { CloseHandle(process) };
        if !assigned {
            let error = io::Error::last_os_error();
            // SAFETY: `job` is the valid handle created above.
            unsafe { CloseHandle(job) };
            return Err(error);
        }
        Ok(Self(job))
    }

    pub(crate) fn kill(&self) -> io::Result<()> {
        // SAFETY: `self.0` is owned and remains open until Drop.
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
        // SAFETY: this type owns the handle and drops it once.
        unsafe { CloseHandle(self.0) };
    }
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
