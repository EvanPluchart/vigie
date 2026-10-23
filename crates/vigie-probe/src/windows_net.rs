//! Sockets and their owning processes on Windows (decision 0007): the extended TCP and UDP
//! tables of the IP helper, IPv4 and IPv6. Read on demand, while the Ports view is shown.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use windows_sys::Win32::Foundation::{ERROR_INSUFFICIENT_BUFFER, NO_ERROR};
use windows_sys::Win32::NetworkManagement::IpHelper::{
    GetExtendedTcpTable, GetExtendedUdpTable, MIB_TCP_STATE_CLOSE_WAIT, MIB_TCP_STATE_ESTAB,
    MIB_TCP_STATE_LISTEN, MIB_TCP_STATE_TIME_WAIT, MIB_TCP6ROW_OWNER_PID, MIB_TCPROW_OWNER_PID,
    MIB_UDP6ROW_OWNER_PID, MIB_UDPROW_OWNER_PID, TCP_TABLE_OWNER_PID_ALL, UDP_TABLE_OWNER_PID,
};

use crate::{ProbeError, Protocol, Result, Socket, TcpState};

const AF_INET: u32 = 2;
const AF_INET6: u32 = 23;

/// Fills `out` with every TCP and UDP socket, IPv4 and IPv6.
pub(crate) fn sockets(buffer: &mut Vec<u64>, out: &mut Vec<Socket>) -> Result<()> {
    out.clear();
    for family in [AF_INET, AF_INET6] {
        let rows = table(buffer, |data, size| unsafe {
            // SAFETY: `data` points to `size` writable bytes (or is null with size 0).
            GetExtendedTcpTable(data, size, 0, family, TCP_TABLE_OWNER_PID_ALL, 0)
        })?;
        if family == AF_INET {
            for row in rows_of::<MIB_TCPROW_OWNER_PID>(buffer, rows) {
                let state = tcp_state(row.dwState);
                out.push(Socket {
                    protocol: Protocol::Tcp,
                    local: v4(row.dwLocalAddr, row.dwLocalPort),
                    remote: (state != TcpState::Listen)
                        .then(|| v4(row.dwRemoteAddr, row.dwRemotePort)),
                    state: Some(state),
                    pid: row.dwOwningPid,
                });
            }
        } else {
            for row in rows_of::<MIB_TCP6ROW_OWNER_PID>(buffer, rows) {
                let state = tcp_state(row.dwState);
                out.push(Socket {
                    protocol: Protocol::Tcp,
                    local: v6(row.ucLocalAddr, row.dwLocalPort),
                    remote: (state != TcpState::Listen)
                        .then(|| v6(row.ucRemoteAddr, row.dwRemotePort)),
                    state: Some(state),
                    pid: row.dwOwningPid,
                });
            }
        }
        let rows = table(buffer, |data, size| unsafe {
            // SAFETY: as above.
            GetExtendedUdpTable(data, size, 0, family, UDP_TABLE_OWNER_PID, 0)
        })?;
        if family == AF_INET {
            for row in rows_of::<MIB_UDPROW_OWNER_PID>(buffer, rows) {
                out.push(Socket {
                    protocol: Protocol::Udp,
                    local: v4(row.dwLocalAddr, row.dwLocalPort),
                    remote: None,
                    state: None,
                    pid: row.dwOwningPid,
                });
            }
        } else {
            for row in rows_of::<MIB_UDP6ROW_OWNER_PID>(buffer, rows) {
                out.push(Socket {
                    protocol: Protocol::Udp,
                    local: v6(row.ucLocalAddr, row.dwLocalPort),
                    remote: None,
                    state: None,
                    pid: row.dwOwningPid,
                });
            }
        }
    }
    Ok(())
}

/// Calls `read` until the table fits in `buffer` (8-byte aligned). Returns the row count, the
/// first `u32` of every such table.
fn table(
    buffer: &mut Vec<u64>,
    read: impl Fn(*mut core::ffi::c_void, *mut u32) -> u32,
) -> Result<usize> {
    // Tables grow between calls when connections open: retry a few times.
    for _ in 0..4 {
        let mut size = (buffer.len() * 8) as u32;
        let data = if buffer.is_empty() {
            std::ptr::null_mut()
        } else {
            buffer.as_mut_ptr().cast()
        };
        match read(data, &mut size) {
            NO_ERROR => {
                return Ok(if buffer.is_empty() {
                    0
                } else {
                    (buffer[0] & 0xffff_ffff) as usize
                });
            }
            ERROR_INSUFFICIENT_BUFFER => {
                // Some room for connections opened meanwhile.
                buffer.resize((size as usize).div_ceil(8) + 64, 0);
            }
            code => return Err(ProbeError::Failed(format!("socket table: error {code}"))),
        }
    }
    Err(ProbeError::Failed("socket table kept growing".into()))
}

/// The rows that follow the count in a table read by `table`.
fn rows_of<T: Copy>(buffer: &[u64], count: usize) -> impl Iterator<Item = T> + '_ {
    let bytes = buffer.len() * 8;
    // Rows start after the u32 count, at the alignment of T (4 for every row type here).
    let offset = std::mem::size_of::<u32>().next_multiple_of(std::mem::align_of::<T>());
    let fits = bytes.saturating_sub(offset) / std::mem::size_of::<T>();
    let base = buffer.as_ptr().cast::<u8>();
    (0..count.min(fits)).map(move |i| {
        // SAFETY: in bounds (checked against `fits`); unaligned read, rows are plain data.
        unsafe {
            base.add(offset + i * std::mem::size_of::<T>())
                .cast::<T>()
                .read_unaligned()
        }
    })
}

/// Ports are stored in network byte order in the low 16 bits.
fn port(raw: u32) -> u16 {
    u16::from_be((raw & 0xffff) as u16)
}

/// IPv4 addresses are stored in network byte order: their bytes in memory are the address.
fn v4(addr: u32, raw_port: u32) -> SocketAddr {
    SocketAddr::new(
        IpAddr::V4(Ipv4Addr::from(addr.to_ne_bytes())),
        port(raw_port),
    )
}

fn v6(addr: [u8; 16], raw_port: u32) -> SocketAddr {
    SocketAddr::new(IpAddr::V6(Ipv6Addr::from(addr)), port(raw_port))
}

fn tcp_state(raw: u32) -> TcpState {
    match raw as i32 {
        MIB_TCP_STATE_LISTEN => TcpState::Listen,
        MIB_TCP_STATE_ESTAB => TcpState::Established,
        MIB_TCP_STATE_TIME_WAIT => TcpState::TimeWait,
        MIB_TCP_STATE_CLOSE_WAIT => TcpState::CloseWait,
        _ => TcpState::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_our_own_sockets_with_the_right_port_and_pid() {
        let tcp = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let udp = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let tcp6 = std::net::TcpListener::bind("[::1]:0").ok();
        let me = std::process::id();
        let mut out = Vec::new();
        sockets(&mut Vec::new(), &mut out).unwrap();
        fn find(out: &[Socket], protocol: Protocol, addr: SocketAddr) -> Socket {
            *out.iter()
                .find(|s| s.protocol == protocol && s.local == addr)
                .unwrap_or_else(|| panic!("{addr} not listed"))
        }
        let listening = find(&out, Protocol::Tcp, tcp.local_addr().unwrap());
        assert_eq!(
            (listening.pid, listening.state, listening.remote),
            (me, Some(TcpState::Listen), None)
        );
        assert_eq!(find(&out, Protocol::Udp, udp.local_addr().unwrap()).pid, me);
        if let Some(tcp6) = tcp6 {
            assert_eq!(
                find(&out, Protocol::Tcp, tcp6.local_addr().unwrap()).pid,
                me
            );
        }
        // A connection shows both ends.
        let client = std::net::TcpStream::connect(tcp.local_addr().unwrap()).unwrap();
        sockets(&mut Vec::new(), &mut out).unwrap();
        let conn = find(&out, Protocol::Tcp, client.local_addr().unwrap());
        assert_eq!(conn.remote, Some(tcp.local_addr().unwrap()));
        assert_eq!(conn.state, Some(TcpState::Established));
    }
}
