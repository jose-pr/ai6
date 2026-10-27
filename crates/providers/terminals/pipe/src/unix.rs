// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Unix: the child leads a process group of its own, which is its whole tree.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::io;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::process::{Child, Command, ExitStatus};

use ai6::directorates::q::terminal::ExitInfo;
use nix::sys::signal::{Signal, killpg};
use nix::unistd::Pid;

pub(crate) struct ProcessTree(Pid);

impl ProcessTree {
    /// The child's process group, which [`own_process_tree`] made with the child's pid as its id.
    pub(crate) fn adopt(child: &Child) -> io::Result<Self> {
        i32::try_from(child.id())
            .ok()
            .filter(|&pid| pid > 0)
            .map(|pid| Self(Pid::from_raw(pid)))
            .ok_or_else(|| io::Error::other("the child's process ID is out of range"))
    }

    pub(crate) fn kill(&self) -> io::Result<()> {
        // An error means the group is already gone, which is what was asked.
        let _ = killpg(self.0, Signal::SIGKILL);
        Ok(())
    }

    /// Always `false`: a Unix directory or file can be removed while processes still use it,
    /// so nothing waits on the rest of the group once the child itself has exited.
    pub(crate) fn running(&self) -> bool {
        false
    }
}

/// Starts the child as the leader of a new process group, so killing the group kills every
/// process it started that did not leave it.
pub(crate) fn own_process_tree(cmd: &mut Command) {
    cmd.process_group(0);
}

/// The program as given: the standard library searches the child's `PATH` when it has one.
pub(crate) fn resolve(program: &str, _env: Option<&BTreeMap<String, String>>) -> String {
    program.to_owned()
}

pub(crate) fn exit_info(status: ExitStatus) -> ExitInfo {
    ExitInfo {
        running: false,
        code: status.code().map(i64::from),
        signal: status.signal().map(i64::from),
    }
}
