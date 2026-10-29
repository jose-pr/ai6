// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Unix: the child is a session leader, its tree is found from its process group, and the
//! terminal has no line editing or echo.

use std::borrow::Cow;
use std::fs::OpenOptions;
use std::io;
use std::os::unix::fs::OpenOptionsExt;

use nix::sys::termios::{self, InputFlags, LocalFlags, SetArg, SpecialCharacterIndices};
use portable_pty::MasterPty;

pub(crate) use crate::process_tree::ProcessTree;

/// The child's process tree: `portable-pty` starts the child with `setsid`, so its process
/// group and session ids are its pid.
pub(crate) fn adopt(pid: Option<u32>) -> io::Result<ProcessTree> {
    ProcessTree::of(pid)
}

/// Turns off line editing and echo on the terminal before the program starts, so input of any
/// length reaches it whole and is never copied into the output.
pub(crate) fn set_terminal_mode(master: &dyn MasterPty) -> io::Result<()> {
    let path = master
        .tty_name()
        .ok_or_else(|| io::Error::other("portable-pty did not name the terminal"))?;
    // POSIX defines the attributes of the terminal device, not of the master side.
    let tty = OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_NOCTTY)
        .open(path)?;
    let mut mode = termios::tcgetattr(&tty)?;
    // Canonical mode holds a line to MAX_CANON bytes: Linux cuts it at 4095 and macOS never
    // delivers a longer one. IEXTEN's characters (^V, ^O) are line editing too.
    mode.local_flags
        .remove(LocalFlags::ICANON | LocalFlags::IEXTEN | LocalFlags::ECHO | LocalFlags::ECHONL);
    // Nothing uses flow control, and with it a ^S written would stop the program's output.
    // With IMAXBEL, FreeBSD 14.5 answers input past a full queue with BEL in the output, ahead
    // of the program's own (measured); the writer waits instead.
    mode.input_flags
        .remove(InputFlags::IXON | InputFlags::IMAXBEL);
    // ISIG, ICRNL and OPOST stay on: the Claude Code handler interrupts by writing ^C, Enter from
    // an attached viewer arrives as CR, and the viewer's terminal emulator needs CRLF line ends.

    // A read returns as soon as one byte is there.
    mode.control_chars[SpecialCharacterIndices::VMIN as usize] = 1;
    mode.control_chars[SpecialCharacterIndices::VTIME as usize] = 0;
    termios::tcsetattr(&tty, SetArg::TCSANOW, &mode)?;
    Ok(())
}

pub(crate) fn translate_input(data: &[u8]) -> Cow<'_, [u8]> {
    Cow::Borrowed(data)
}
