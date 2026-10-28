// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Unix: the child leads a process group of its own, and its tree is found from it.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::io;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::process::{Child, Command, ExitStatus};

use ai6::directorates::q::terminal::ExitInfo;

pub(crate) use crate::process_tree::ProcessTree;

/// The child's process tree, rooted at its process group, which [`own_process_tree`] made with
/// the child's pid as its id.
pub(crate) fn adopt(child: &Child) -> io::Result<ProcessTree> {
    ProcessTree::of(Some(child.id()))
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
