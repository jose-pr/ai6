//! The listening TCP tables through `GetExtendedTcpTable`, in this process. This is the only code in the crate that is
//! `unsafe`: one query per address family that fills a buffer this module owns and keeps no pointer past the call.

use alloc::vec::Vec;
use std::io;

use windows_sys::Win32::Foundation::{ERROR_INSUFFICIENT_BUFFER, NO_ERROR};
use windows_sys::Win32::NetworkManagement::IpHelper::{
    GetExtendedTcpTable, TCP_TABLE_OWNER_PID_LISTENER,
};
use windows_sys::Win32::Networking::WinSock::{AF_INET, AF_INET6};

/// How many times the table is asked for again when it grew between the size query and the read.
const ATTEMPTS: usize = 8;

/// The raw `MIB_TCPTABLE_OWNER_PID` (`v6` false) or `MIB_TCP6TABLE_OWNER_PID` of the listening sockets, unsorted.
pub(super) fn listening_table(v6: bool) -> io::Result<Vec<u8>> {
    let family = u32::from(if v6 { AF_INET6 } else { AF_INET });
    let mut size: u32 = 0;
    for _ in 0..ATTEMPTS {
        let mut buffer = alloc::vec![0_u8; size as usize];
        // SAFETY: `buffer` is `size` writable bytes (or null with size 0, which only asks for the size) and `size` is
        // writable; the call keeps neither past its return.
        let status = unsafe {
            GetExtendedTcpTable(
                if size == 0 {
                    core::ptr::null_mut()
                } else {
                    buffer.as_mut_ptr().cast()
                },
                &raw mut size,
                0,
                family,
                TCP_TABLE_OWNER_PID_LISTENER,
                0,
            )
        };
        match status {
            NO_ERROR => {
                buffer.truncate(size as usize);
                return Ok(buffer);
            }
            ERROR_INSUFFICIENT_BUFFER => {}
            other => {
                return Err(io::Error::from_raw_os_error(
                    i32::try_from(other).unwrap_or(i32::MAX),
                ));
            }
        }
    }
    Err(io::Error::other("the TCP table kept growing"))
}
