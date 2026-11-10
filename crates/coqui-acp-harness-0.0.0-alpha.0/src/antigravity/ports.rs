//! The loopback ports a set of processes listens on, found through the operating system's own
//! tools and no `unsafe`.
//!
//! Each system's text is read by a function of its own that takes the text, so every one is
//! tested on every system from a recording.

use alloc::collections::BTreeSet;
use alloc::string::String;
use alloc::vec::Vec;
use core::net::SocketAddr;
use core::time::Duration;
use std::io;
use std::time::Instant;

/// How long [`loopback_listeners`] lets the system's tool run.
const LISTING_LIMIT: Duration = Duration::from_secs(3);
/// The most of a tool's output that is read.
#[cfg_attr(
    not(any(windows, target_os = "macos", test)),
    allow(dead_code, reason = "only the tools' output is bounded")
)]
const OUTPUT_LIMIT: u64 = 16 * 1024 * 1024;

/// The loopback addresses that `owners` (process ids) listen on, TCP, without repeats, in order. A tool
/// the system is asked through is ended after 3 s.
///
/// Windows reads `netstat.exe -ano`, Linux `/proc/<pid>/fd` joined with `/proc/net/tcp` and `tcp6`,
/// macOS `lsof`. A process that has ended, or whose descriptors may not be read, has none.
///
/// # Errors
///
/// The system's tool could not be run, answered failure or took too long
/// ([`io::ErrorKind::TimedOut`]), or this system has none to ask ([`io::ErrorKind::Unsupported`]).
pub fn loopback_listeners(owners: &[u32]) -> io::Result<Vec<SocketAddr>> {
    listeners_before(owners, Instant::now().checked_add(LISTING_LIMIT))
}

/// [`loopback_listeners`] with a deadline of the caller's: `None` is one already passed.
pub(super) fn listeners_before(
    owners: &[u32],
    deadline: Option<Instant>,
) -> io::Result<Vec<SocketAddr>> {
    if owners.is_empty() {
        return Ok(Vec::new());
    }
    let mut found = system(owners, deadline)?;
    found.sort_unstable();
    found.dedup();
    Ok(found)
}

fn is_loopback(address: &SocketAddr) -> bool {
    address.port() != 0 && address.ip().to_canonical().is_loopback()
}

/// Runs `command` with its standard output read, and ends it at `deadline`.
#[cfg_attr(
    not(any(windows, target_os = "macos", test)),
    allow(dead_code, reason = "only the tools' output is bounded")
)]
pub(super) fn run_until(
    command: &mut std::process::Command,
    deadline: Option<Instant>,
) -> io::Result<(std::process::ExitStatus, Vec<u8>)> {
    use std::io::Read as _;
    let mut child = command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()?;
    let stdout = child.stdout.take().ok_or(io::ErrorKind::BrokenPipe)?;
    let reader = std::thread::spawn(move || {
        let mut output = Vec::new();
        stdout
            .take(OUTPUT_LIMIT)
            .read_to_end(&mut output)
            .map(|_| output)
    });
    loop {
        if let Some(status) = child.try_wait()? {
            let output = reader
                .join()
                .map_err(|_| io::Error::other("the output was not read"))??;
            return Ok((status, output));
        }
        if deadline.is_none_or(|deadline| Instant::now() >= deadline) {
            let _ = child.kill();
            let _ = child.wait();
            let _ = reader.join();
            return Err(io::ErrorKind::TimedOut.into());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(windows)]
fn system(owners: &[u32], deadline: Option<Instant>) -> io::Result<Vec<SocketAddr>> {
    use std::os::windows::process::CommandExt as _;
    /// `CREATE_NO_WINDOW`: no console window flashes up for a program started from a GUI.
    const NO_WINDOW: u32 = 0x0800_0000;
    let root = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
    let netstat = std::path::Path::new(&root)
        .join("System32")
        .join("netstat.exe");
    let mut found = Vec::new();
    for (family, needed) in [("TCP", true), ("TCPv6", false)] {
        let ran = run_until(
            std::process::Command::new(&netstat)
                .args(["-ano", "-p", family])
                .creation_flags(NO_WINDOW),
            deadline,
        );
        let output = match ran {
            Ok((status, output)) if status.success() => output,
            Ok(_) if !needed => continue,
            Err(error) if !needed && error.kind() != io::ErrorKind::TimedOut => continue,
            Ok((status, _)) => return Err(io::Error::other(alloc::format!("netstat {status}"))),
            Err(error) => return Err(error),
        };
        found.extend(parse_netstat(&String::from_utf8_lossy(&output), owners));
    }
    Ok(found)
}

#[cfg(target_os = "linux")]
fn system(owners: &[u32], _deadline: Option<Instant>) -> io::Result<Vec<SocketAddr>> {
    let mut inodes = BTreeSet::new();
    for pid in owners {
        let Ok(entries) = std::fs::read_dir(alloc::format!("/proc/{pid}/fd")) else {
            continue;
        };
        for entry in entries.flatten() {
            if let Ok(target) = std::fs::read_link(entry.path()) {
                inodes.extend(socket_inode(&target.to_string_lossy()));
            }
        }
    }
    let mut found = Vec::new();
    if inodes.is_empty() {
        return Ok(found);
    }
    for (path, v6) in [("/proc/net/tcp", false), ("/proc/net/tcp6", true)] {
        match std::fs::read_to_string(path) {
            Ok(text) => found.extend(parse_proc_net(&text, &inodes, v6)),
            Err(error) if v6 && error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(found)
}

#[cfg(target_os = "macos")]
fn system(owners: &[u32], deadline: Option<Instant>) -> io::Result<Vec<SocketAddr>> {
    let pids = owners
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(",");
    let (status, output) = run_until(
        std::process::Command::new("/usr/sbin/lsof").args([
            "-nP",
            "-a",
            "-iTCP",
            "-sTCP:LISTEN",
            "-p",
            &pids,
            "-Fpn",
        ]),
        deadline,
    )?;
    // lsof exits 1 when it finds nothing to list.
    if !status.success() && !output.is_empty() {
        return Err(io::Error::other(alloc::format!("lsof {status}")));
    }
    Ok(parse_lsof(&String::from_utf8_lossy(&output), owners))
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
fn system(_owners: &[u32], _deadline: Option<Instant>) -> io::Result<Vec<SocketAddr>> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "listening ports are not read on this system",
    ))
}

/// The listeners of `owners` in the output of `netstat -ano -p TCP` (or `TCPv6`): a row whose
/// foreign address is the unspecified one with port 0 is listening, whatever language its State
/// column is in, and the last column is the owner.
#[cfg_attr(
    not(windows),
    allow(dead_code, reason = "read from a recording on every system")
)]
pub(super) fn parse_netstat(text: &str, owners: &[u32]) -> Vec<SocketAddr> {
    text.lines()
        .filter_map(|line| {
            let columns: Vec<&str> = line.split_whitespace().collect();
            let [protocol, local, foreign, .., owner] = columns.as_slice() else {
                return None;
            };
            if !protocol.eq_ignore_ascii_case("TCP") || !matches!(*foreign, "0.0.0.0:0" | "[::]:0")
            {
                return None;
            }
            let address: SocketAddr = local.parse().ok()?;
            let owner: u32 = owner.parse().ok()?;
            (owners.contains(&owner) && is_loopback(&address)).then_some(address)
        })
        .collect()
}

/// The inode in a descriptor's link text, `socket:[12345]`.
#[cfg_attr(
    not(target_os = "linux"),
    allow(dead_code, reason = "read from a recording on every system")
)]
pub(super) fn socket_inode(link: &str) -> Option<u64> {
    link.strip_prefix("socket:[")?
        .strip_suffix(']')?
        .parse()
        .ok()
}

/// The listeners among `inodes` in `/proc/net/tcp` (`v6` false) or `tcp6`: state `0A`.
#[cfg_attr(
    not(target_os = "linux"),
    allow(dead_code, reason = "read from a recording on every system")
)]
pub(super) fn parse_proc_net(text: &str, inodes: &BTreeSet<u64>, v6: bool) -> Vec<SocketAddr> {
    text.lines()
        .skip(1)
        .filter_map(|line| {
            let columns: Vec<&str> = line.split_whitespace().collect();
            let (local, state, inode) = (columns.get(1)?, columns.get(3)?, columns.get(9)?);
            if *state != "0A" || !inodes.contains(&inode.parse().ok()?) {
                return None;
            }
            let address = proc_address(local, v6)?;
            is_loopback(&address).then_some(address)
        })
        .collect()
}

/// `0100007F:1F90` as an address. Each group of four bytes is a native-endian integer printed in
/// hex, so reading it back with the same endianness restores the network order.
#[cfg_attr(
    not(target_os = "linux"),
    allow(dead_code, reason = "read from a recording on every system")
)]
fn proc_address(text: &str, v6: bool) -> Option<SocketAddr> {
    let (host, port) = text.split_once(':')?;
    let port = u16::from_str_radix(port, 16).ok()?;
    let mut bytes = Vec::new();
    for group in host.as_bytes().chunks(8) {
        let group = core::str::from_utf8(group).ok()?;
        bytes.extend(u32::from_str_radix(group, 16).ok()?.to_ne_bytes());
    }
    let ip = if v6 {
        core::net::IpAddr::from(<[u8; 16]>::try_from(bytes.as_slice()).ok()?)
    } else {
        core::net::IpAddr::from(<[u8; 4]>::try_from(bytes.as_slice()).ok()?)
    };
    Some(SocketAddr::new(ip, port))
}

/// The listeners of `owners` in `lsof -Fpn` output: `p<pid>` lines, each followed by the `n<name>` of
/// its sockets (`127.0.0.1:50000`, `[::1]:50000`, `*:80`).
#[cfg_attr(
    not(target_os = "macos"),
    allow(dead_code, reason = "read from a recording on every system")
)]
pub(super) fn parse_lsof(text: &str, owners: &[u32]) -> Vec<SocketAddr> {
    let mut found = Vec::new();
    let mut owned = false;
    for line in text.lines() {
        if let Some(pid) = line.strip_prefix('p') {
            owned = pid.parse().is_ok_and(|pid| owners.contains(&pid));
        } else if let Some(name) = line.strip_prefix('n')
            && owned
            && let Ok(address) = name.parse::<SocketAddr>()
            && is_loopback(&address)
        {
            found.push(address);
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn netstat_rows_are_read_in_any_language() {
        let text = "\
Active Connections

  Proto  Local Address          Foreign Address        State           PID
  TCP    0.0.0.0:135            0.0.0.0:0              LISTENING       1200
  TCP    127.0.0.1:60946        0.0.0.0:0              ESCUCHANDO      79860
  TCP    127.0.0.1:60947        0.0.0.0:0              LISTENING       79860
  TCP    127.0.0.1:60947        127.0.0.1:50000        ESTABLISHED     79860
  TCP    127.0.0.1:7000         0.0.0.0:0              LISTENING       4
  TCP    192.168.1.5:9000       0.0.0.0:0              LISTENING       79860
  TCP    [::1]:5000             [::]:0                 LISTENING       79860
";
        let found = parse_netstat(text, &[79860]);
        let texts: Vec<String> = found.iter().map(SocketAddr::to_string).collect();
        assert_eq!(texts, ["127.0.0.1:60946", "127.0.0.1:60947", "[::1]:5000"]);
    }

    #[test]
    fn a_proc_net_table_is_joined_with_the_descriptors_inodes() {
        let text = "\
  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode
   0: 0100007F:EC02 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 7001 1 0 100 0 0
   1: 0100007F:EC03 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 7002 1 0 100 0 0
   2: 0100007F:EC04 0100007F:EC02 01 00000000:00000000 00:00000000 00000000  1000        0 7001 1 0 100 0 0
   3: 00000000:0050 00000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 7001 1 0 100 0 0
";
        let inodes = BTreeSet::from([7001]);
        let found = parse_proc_net(text, &inodes, false);
        assert_eq!(
            found.iter().map(SocketAddr::to_string).collect::<Vec<_>>(),
            ["127.0.0.1:60418"]
        );
        let six = "\
  sl  local_address                         remote_address                        st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode
   0: 00000000000000000000000001000000:1388 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 7001 1 0 100 0 0
";
        let found = parse_proc_net(six, &inodes, true);
        assert_eq!(
            found.iter().map(SocketAddr::to_string).collect::<Vec<_>>(),
            ["[::1]:5000"]
        );
        assert_eq!(socket_inode("socket:[7001]"), Some(7001));
        assert_eq!(socket_inode("pipe:[7001]"), None);
    }

    #[test]
    fn a_tool_that_outlasts_the_deadline_is_ended() {
        #[cfg(windows)]
        let mut command = {
            let mut command = std::process::Command::new("ping");
            command.args(["-n", "30", "127.0.0.1"]);
            command
        };
        #[cfg(not(windows))]
        let mut command = {
            let mut command = std::process::Command::new("sleep");
            command.arg("30");
            command
        };
        let started = Instant::now();
        let error = run_until(
            &mut command,
            Instant::now().checked_add(Duration::from_millis(300)),
        )
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn a_deadline_that_has_passed_starts_no_tool() {
        let listed = listeners_before(&[1], None);
        if cfg!(any(windows, target_os = "macos")) {
            assert_eq!(listed.unwrap_err().kind(), io::ErrorKind::TimedOut);
        } else {
            assert!(listed.is_ok());
        }
    }

    #[test]
    fn lsof_fields_are_read_per_process() {
        let text = "p100\nn127.0.0.1:50000\nn*:80\nn[::1]:50001\np200\nn127.0.0.1:50002\n";
        let found = parse_lsof(text, &[100]);
        assert_eq!(
            found.iter().map(SocketAddr::to_string).collect::<Vec<_>>(),
            ["127.0.0.1:50000", "[::1]:50001"]
        );
    }
}
