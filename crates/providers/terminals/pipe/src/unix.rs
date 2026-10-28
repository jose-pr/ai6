// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Unix: the child leads a session and a process group of its own, its tree is found from
//! them, and it inherits no descriptor but its standard streams.

#![allow(unsafe_code)]

use std::io;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::process::{Child, Command, ExitStatus, Stdio};

use ai6::directorates::q::terminal::ExitInfo;

use crate::process_tree::ProcessTree;
use crate::{ErrorOutput, Launch};

/// The highest descriptor the child marks close-on-exec where it cannot mark them all at once
/// (`close_range` is Linux 5.11 or later): a soft `RLIMIT_NOFILE` of a million would take as many
/// calls.
const MAX_SWEPT_FD: libc::rlim_t = 65536;

/// A started child.
pub(crate) struct Process(Child);

impl Process {
    pub(crate) fn id(&self) -> u32 {
        self.0.id()
    }

    /// How the child ended, once it has; reaps it.
    pub(crate) fn try_wait(&mut self) -> io::Result<Option<ExitInfo>> {
        Ok(self.0.try_wait()?.map(exit_info))
    }

    pub(crate) fn kill(&mut self) -> io::Result<()> {
        self.0.kill()
    }

    pub(crate) fn wait(&mut self) -> io::Result<()> {
        self.0.wait().map(drop)
    }
}

/// Starts the program as the leader of a new session, and so of a new process group, with no
/// controlling terminal and no descriptor but its standard streams; the standard library
/// searches the child's `PATH` when it has one. Its tree is rooted at that session and group,
/// whose id is the child's pid.
pub(crate) fn spawn(launch: Launch<'_>) -> io::Result<(Process, ProcessTree)> {
    let mut cmd = Command::new(launch.program);
    cmd.args(launch.args);
    if let Some(env) = launch.env {
        cmd.env_clear();
        cmd.envs(env);
    }
    if let Some(dir) = launch.cwd {
        cmd.current_dir(dir);
    }
    let stderr = match launch.stderr {
        ErrorOutput::Stdout => Stdio::from(launch.stdout.try_clone()?),
        ErrorOutput::Null => Stdio::null(),
        ErrorOutput::Pipe(writer) => Stdio::from(writer),
    };
    cmd.stdin(launch.stdin).stdout(launch.stdout).stderr(stderr);
    // SAFETY: the hook runs in the child between fork and exec, and calls only setsid, fcntl,
    // getrlimit and close_range, which are async-signal-safe and allocate nothing.
    unsafe { cmd.pre_exec(start_alone) };
    let mut child = cmd.spawn()?;
    // The command holds the child's ends of the pipes.
    drop(cmd);
    match ProcessTree::of(Some(child.id())) {
        Ok(tree) => Ok((Process(child), tree)),
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            Err(error)
        }
    }
}

fn start_alone() -> io::Result<()> {
    // SAFETY: setsid takes no argument and touches no memory.
    if unsafe { libc::setsid() } == -1 {
        return Err(io::Error::last_os_error());
    }
    // Marked close-on-exec rather than closed: the standard library reports a failed exec over
    // a close-on-exec pipe of its own, which must stay open until the exec.
    close_on_exec_from(3);
    Ok(())
}

/// Marks every descriptor from `first` up close-on-exec.
fn close_on_exec_from(first: libc::c_int) {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        const CLOSE_RANGE_CLOEXEC: libc::c_uint = 1 << 2;
        // SAFETY: close_range takes plain integers and touches no memory.
        let marked = unsafe {
            libc::syscall(
                libc::SYS_close_range,
                first as libc::c_uint,
                libc::c_uint::MAX,
                CLOSE_RANGE_CLOEXEC,
            )
        };
        if marked == 0 {
            return;
        }
    }
    let mut limit = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: getrlimit writes only the `rlimit` it is given.
    let top = match unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) } {
        0 if limit.rlim_cur != libc::RLIM_INFINITY => limit.rlim_cur.min(MAX_SWEPT_FD),
        _ => MAX_SWEPT_FD,
    };
    for fd in first..top as libc::c_int {
        // SAFETY: fcntl on a descriptor number touches no memory; one that is not open fails.
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
        if flags >= 0 && flags & libc::FD_CLOEXEC == 0 {
            // SAFETY: as above.
            unsafe { libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC) };
        }
    }
}

fn exit_info(status: ExitStatus) -> ExitInfo {
    ExitInfo {
        running: false,
        code: status.code().map(i64::from),
        signal: status.signal().map(i64::from),
    }
}
