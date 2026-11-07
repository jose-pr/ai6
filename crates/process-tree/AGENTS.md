# `ai6-process-tree` — API header

Header-file-style reference for the `ai6-process-tree` crate, so it can be used without
reading its source. MSRV: Rust 1.89. No Cargo features. Depends on `libc` on Unix and
`windows-sys` on Windows. Unsafe code is limited to the platform calls in `unix.rs` and
`windows.rs`, each with a safety comment.

The crate root exports `ProcessTree`, a different type on each platform:

```rust
// Unix: the program leads a session and process group of its own.
impl ProcessTree {
    pub fn of(pid: Option<u32>, scans: usize) -> io::Result<Self>; // Err for no ID, 0, 1 or out of range
    pub fn kill(&self, leader_alive: bool) -> io::Result<()>;
    pub fn running(&self) -> bool;                                  // always false
}

// Windows: a kill-on-close Job Object.
impl ProcessTree {
    pub fn new() -> io::Result<Self>;
    pub fn handle(&self) -> HANDLE;                                 // valid while self is
    pub fn kill(&self, leader_alive: bool) -> io::Result<()>;       // leader_alive is ignored
    pub fn running(&self) -> bool;                                  // a process of the job is active
}
```

- `scans` (Unix) is how many times the process table is read again for processes started
  meanwhile; each scan stops what it finds, so a tree stops growing after a few.
- On Unix `kill` stops every process of the tree (`SIGSTOP`) before killing any (`SIGKILL`);
  `leader_alive` says the program has not been reaped, since a reaped ID can be reused. On
  Windows it terminates the job.
- Dropping a Windows tree closes the job, which kills its processes.
