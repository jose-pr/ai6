// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Windows: a terminal program's process tree, a Job Object killed whole. Compiled into the
//! `pipe` and `pty` providers alike (each includes this file with `#[path]`). Every process the
//! program starts joins its job, so terminating the job ends them all.

#![allow(unsafe_code)]

use std::io;
use std::ptr;

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::System::JobObjects::{
    CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOBOBJECT_BASIC_ACCOUNTING_INFORMATION,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectBasicAccountingInformation,
    JobObjectExtendedLimitInformation, QueryInformationJobObject, SetInformationJobObject,
    TerminateJobObject,
};

/// A kill-on-close Job Object holding a program and every process it starts.
pub(crate) struct ProcessTree(HANDLE);

// SAFETY: a Job Object handle is a process-wide kernel capability. The handle is immutable, the
// Win32 operations on it are thread-safe, and Drop owns the only close.
unsafe impl Send for ProcessTree {}
// SAFETY: as for Send.
unsafe impl Sync for ProcessTree {}

impl ProcessTree {
    /// An empty Job Object whose processes are all killed when its last handle closes.
    pub(crate) fn new() -> io::Result<Self> {
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
        Ok(tree)
    }

    /// The Job Object's handle, valid while `self` is.
    pub(crate) fn handle(&self) -> HANDLE {
        self.0
    }

    /// Terminates every process of the job. `_leader_alive` is for the Unix tree's sake.
    pub(crate) fn kill(&self, _leader_alive: bool) -> io::Result<()> {
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
