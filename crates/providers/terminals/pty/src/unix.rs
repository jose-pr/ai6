// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Unix: the child is a session leader, so its process group is its whole tree.

use std::borrow::Cow;
use std::io;

pub(crate) struct ProcessTree(libc::pid_t);

impl ProcessTree {
    /// `portable-pty` starts the child with `setsid`, so its process group id is its pid.
    pub(crate) fn adopt(pid: Option<u32>) -> io::Result<Self> {
        pid.and_then(|pid| libc::pid_t::try_from(pid).ok())
            .filter(|&pid| pid > 0)
            .map(Self)
            .ok_or_else(|| io::Error::other("portable-pty did not return the child's process ID"))
    }

    pub(crate) fn kill(&self) -> io::Result<()> {
        // SAFETY: killpg takes plain integers and touches no memory. An error means the
        // group is already gone, which is what was asked.
        unsafe { libc::killpg(self.0, libc::SIGKILL) };
        Ok(())
    }
}

pub(crate) fn translate_input(data: &[u8]) -> Cow<'_, [u8]> {
    Cow::Borrowed(data)
}
