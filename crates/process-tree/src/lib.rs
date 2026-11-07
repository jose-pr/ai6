// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! A program's process tree, killed whole: the one implementation the `pipe` and `pty`
//! terminal providers and the Codex sandbox probe share.
//!
//! [`ProcessTree`] is a process group and the processes found by parent from it on Unix, and a
//! Job Object on Windows. The two differ in how one is made (`of` a process ID on Unix, `new`
//! and then adoption on Windows); `kill` and `running` are the same on both.

#![deny(unsafe_code)]

#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

#[cfg(unix)]
pub use unix::ProcessTree;
#[cfg(windows)]
pub use windows::ProcessTree;
