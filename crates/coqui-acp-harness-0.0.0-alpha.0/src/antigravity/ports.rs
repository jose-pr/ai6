//! The loopback ports a set of processes listens on, found through the operating system's own
//! tables: Windows' TCP table in this process (the one `unsafe` module), Linux's `/proc`, and macOS'
//! `lsof`, which is a program started and so is bounded.
//!
//! Each system's table is read by a function of its own that takes the bytes or text, so every one is
//! tested on every system from a recording.

use alloc::collections::BTreeSet;
use alloc::vec::Vec;
use core::net::SocketAddr;
use core::time::Duration;
use std::io;
use std::time::Instant;

/// How long [`loopback_listeners`] lets the system's tool run.
const LISTING_LIMIT: Duration = Duration::from_secs(3);
/// The most of a tool's output that is read.
#[cfg_attr(
    not(any(target_os = "macos", test)),
    allow(dead_code, reason = "only the tool's output is bounded")
)]
const OUTPUT_LIMIT: u64 = 16 * 1024 * 1024;

/// The loopback addresses that `owners` (process ids) listen on, TCP, without repeats, in order. A tool
/// the system is asked through is ended after 3 s.
///
/// Windows reads `GetExtendedTcpTable` for IPv4 and IPv6 and starts no program, Linux `/proc/<pid>/fd` joined with `/proc/net/tcp` and `tcp6`,
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

/// Runs `launch` with its standard output read, and ends it at `deadline`. It starts through this crate's own start, so a
/// host's starter, when one is named, starts it and not this process.
#[cfg_attr(
    not(any(target_os = "macos", test)),
    allow(dead_code, reason = "only the macOS tool is started this way")
)]
pub(super) fn run_launch_until(
    launch: &crate::process::Launch,
    deadline: Option<Instant>,
) -> io::Result<(crate::process::Exit, Vec<u8>)> {
    use std::io::Read as _;
    let spawned = crate::process::start(launch, crate::process::ErrorStream::Discarded)?;
    drop(spawned.stdin);
    let stdout = spawned.stdout;
    let reader = std::thread::spawn(move || {
        let mut output = Vec::new();
        stdout
            .take(OUTPUT_LIMIT)
            .read_to_end(&mut output)
            .map(|_| output)
    });
    loop {
        if let Some(exit) = spawned.process.try_wait()? {
            let output = reader
                .join()
                .map_err(|_| io::Error::other("the output was not read"))??;
            return Ok((exit, output));
        }
        if deadline.is_none_or(|deadline| Instant::now() >= deadline) {
            let _ = spawned.process.kill();
            let _ = reader.join();
            return Err(io::ErrorKind::TimedOut.into());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(windows)]
#[allow(
    unsafe_code,
    reason = "the system's TCP table is read through its own call"
)]
mod win32;

#[cfg(windows)]
fn system(owners: &[u32], _deadline: Option<Instant>) -> io::Result<Vec<SocketAddr>> {
    let mut found = Vec::new();
    for (v6, needed) in [(false, true), (true, false)] {
        match win32::listening_table(v6) {
            Ok(table) => found.extend(parse_tcp_table(&table, v6, owners)),
            // A machine with IPv6 off has no such table.
            Err(_) if !needed => {}
            Err(error) => return Err(error),
        }
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
    let (status, output) = run_launch_until(
        &crate::process::Launch::new("/usr/sbin/lsof").args([
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
    if status.code != Some(0) && !output.is_empty() {
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

/// The listeners of `owners` in a `MIB_TCPTABLE_OWNER_PID` (`v6` false) or `MIB_TCP6TABLE_OWNER_PID` as
/// `GetExtendedTcpTable` fills it: a row count, then rows of 24 or 56 bytes, whose address is in network order, port in the
/// low two bytes of a word in network byte order, and last word the owning process.
#[cfg_attr(
    not(windows),
    allow(dead_code, reason = "read from a recording on every system")
)]
pub(super) fn parse_tcp_table(table: &[u8], v6: bool, owners: &[u32]) -> Vec<SocketAddr> {
    let word = |at: usize| -> Option<u32> {
        Some(u32::from_le_bytes(
            table.get(at..at.checked_add(4)?)?.try_into().ok()?,
        ))
    };
    let (size, address, port, owner) = if v6 { (56, 0, 20, 52) } else { (24, 4, 8, 20) };
    let rows = word(0).map_or(0, |rows| rows as usize);
    (0..rows)
        .filter_map(|row| {
            let start = row.checked_mul(size)?.checked_add(4)?;
            if !owners.contains(&word(start + owner)?) {
                return None;
            }
            let port = u16::try_from(word(start + port)? & 0xFFFF)
                .ok()?
                .swap_bytes();
            let ip = if v6 {
                let bytes = table.get(start + address..start + address + 16)?;
                core::net::IpAddr::from(<[u8; 16]>::try_from(bytes).ok()?)
            } else {
                let bytes = table.get(start + address..start + address + 4)?;
                core::net::IpAddr::from(<[u8; 4]>::try_from(bytes).ok()?)
            };
            let address = SocketAddr::new(ip, port);
            is_loopback(&address).then_some(address)
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
    fn a_launch_that_outlasts_the_deadline_is_ended_by_the_starter_it_is_given() {
        let launch = if cfg!(windows) {
            crate::process::Launch::new("ping").args(["-n", "30", "127.0.0.1"])
        } else {
            crate::process::Launch::new("sleep").arg("30")
        };
        let started = Instant::now();
        let error = run_launch_until(
            &launch,
            Instant::now().checked_add(Duration::from_millis(300)),
        )
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert!(started.elapsed() < Duration::from_secs(5));
        let (exit, output) = run_launch_until(
            &if cfg!(windows) {
                crate::process::Launch::new("cmd").args(["/d", "/c", "echo hi"])
            } else {
                crate::process::Launch::new("/bin/sh").args(["-c", "echo hi"])
            },
            Instant::now().checked_add(Duration::from_secs(20)),
        )
        .expect("it runs");
        assert_eq!(exit.code, Some(0));
        assert!(String::from_utf8_lossy(&output).contains("hi"));
    }

    #[test]
    fn a_deadline_that_has_passed_starts_no_tool() {
        let listed = listeners_before(&[1], None);
        if cfg!(target_os = "macos") {
            assert_eq!(listed.unwrap_err().kind(), io::ErrorKind::TimedOut);
        } else {
            assert!(listed.is_ok());
        }
    }

    /// A `MIB_TCPTABLE_OWNER_PID` of three rows: 127.0.0.1:60946 of process 7, 0.0.0.0:135 of process 7, and
    /// 127.0.0.1:7000 of process 4.
    fn recorded_v4() -> Vec<u8> {
        let mut table = 3_u32.to_le_bytes().to_vec();
        for (address, port, owner) in [
            ([127, 0, 0, 1], 60946_u16, 7_u32),
            ([0, 0, 0, 0], 135, 7),
            ([127, 0, 0, 1], 7000, 4),
        ] {
            table.extend(2_u32.to_le_bytes()); // state: listening
            table.extend(address);
            table.extend(u32::from(port.swap_bytes()).to_le_bytes());
            table.extend([0; 8]); // remote address and port
            table.extend(owner.to_le_bytes());
        }
        table
    }

    #[test]
    fn a_tcp_table_is_read_per_owner_and_only_for_loopback() {
        let found = parse_tcp_table(&recorded_v4(), false, &[7]);
        assert_eq!(
            found.iter().map(SocketAddr::to_string).collect::<Vec<_>>(),
            ["127.0.0.1:60946"]
        );
        let mut six = 1_u32.to_le_bytes().to_vec();
        let mut loopback = [0_u8; 16];
        loopback[15] = 1;
        six.extend(loopback);
        six.extend(0_u32.to_le_bytes()); // scope
        six.extend(u32::from(5000_u16.swap_bytes()).to_le_bytes());
        six.extend([0; 16]);
        six.extend([0; 4]); // remote scope
        six.extend([0; 4]); // remote port
        six.extend(2_u32.to_le_bytes()); // state
        six.extend(7_u32.to_le_bytes());
        assert_eq!(six.len(), 4 + 56);
        let found = parse_tcp_table(&six, true, &[7]);
        assert_eq!(
            found.iter().map(SocketAddr::to_string).collect::<Vec<_>>(),
            ["[::1]:5000"]
        );
    }

    #[test]
    fn a_short_or_lying_tcp_table_gives_nothing_and_does_not_panic() {
        assert!(parse_tcp_table(&[], false, &[7]).is_empty());
        let mut table = recorded_v4();
        table.truncate(30);
        assert!(parse_tcp_table(&table, false, &[7]).len() <= 1);
        let mut huge = u32::MAX.to_le_bytes().to_vec();
        huge.extend([0; 24]);
        assert!(parse_tcp_table(&huge, false, &[7]).is_empty());
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
