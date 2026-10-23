//! Ports, file holders, startup items and services as their views show them (decision 0007):
//! joined with the processes the table knows, filtered and sorted. No OS calls here.

use std::collections::HashMap;
use std::net::SocketAddr;

use vigie_probe::{Locker, LockerKind, Pid, Protocol, Service, Socket, StartupItem, TcpState};

use crate::{Group, Label, Target};

/// The kernel owns sockets on behalf of the system (http.sys and the like).
const KERNEL_PID: Pid = 4;

/// One socket as the Ports view shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct PortRow {
    pub protocol: Protocol,
    pub local: SocketAddr,
    pub remote: Option<SocketAddr>,
    pub state: Option<TcpState>,
    /// Listening TCP, and UDP: what serves a port, as opposed to a connection.
    pub listening: bool,
    pub pid: Pid,
    /// `None` when the process is not in the last sample (it just exited, or PID 0).
    pub target: Option<Target>,
    pub name: String,
    /// Whether "End task" may be offered: never for the system or Vigie itself.
    pub can_end: bool,
    /// The search was a port number and this socket uses it.
    pub exact: bool,
}

/// Sockets joined with their processes, filtered by `filter` (a port, a PID, a name or part of
/// an address) and sorted: exact port matches first, then what listens, then by port.
/// Connections in TIME_WAIT belong to no process any more and are left out.
pub fn port_rows<'a>(
    sockets: &[Socket],
    labels: impl Iterator<Item = Label<'a>>,
    self_pid: Pid,
    filter: &str,
) -> Vec<PortRow> {
    let labels: HashMap<Pid, Label> = labels.map(|l| (l.target.pid, l)).collect();
    let filter = filter.trim().to_lowercase();
    let number: Option<u32> = filter.parse().ok();
    let mut rows: Vec<PortRow> = sockets
        .iter()
        .filter(|s| s.state != Some(TcpState::TimeWait))
        .filter_map(|s| {
            let label = labels.get(&s.pid);
            let name = label.map_or("", |l| l.name);
            let exact = number.is_some_and(|n| {
                u32::from(s.local.port()) == n || s.remote.is_some_and(|r| u32::from(r.port()) == n)
            });
            let matches = filter.is_empty()
                || exact
                || number.is_some_and(|n| n == s.pid)
                || name.to_lowercase().contains(&filter)
                || s.local.to_string().contains(&filter)
                || s.remote.is_some_and(|r| r.to_string().contains(&filter));
            matches.then(|| PortRow {
                protocol: s.protocol,
                local: s.local,
                remote: s.remote,
                state: s.state,
                listening: s.protocol == Protocol::Udp || s.state == Some(TcpState::Listen),
                pid: s.pid,
                target: label.map(|l| l.target),
                name: name.to_owned(),
                can_end: s.pid != 0
                    && s.pid != KERNEL_PID
                    && s.pid != self_pid
                    && label.is_some_and(|l| l.group != Group::System),
                exact,
            })
        })
        .collect();
    rows.sort_by(|a, b| {
        b.exact
            .cmp(&a.exact)
            .then(b.listening.cmp(&a.listening))
            .then(a.local.port().cmp(&b.local.port()))
            .then(a.protocol.cmp(&b.protocol))
            .then(a.pid.cmp(&b.pid))
            .then(a.local.cmp(&b.local))
            .then(a.remote.cmp(&b.remote))
    });
    rows
}

/// A process holding a file, as the Files view shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LockRow {
    pub target: Target,
    pub name: String,
    pub service: Option<String>,
    pub kind: LockerKind,
    pub can_end: bool,
}

/// Holders named as the table names them, and whether ending them may be offered: never for
/// processes the system calls critical, the system section, or a process whose identity no
/// longer matches the last sample.
pub fn lock_rows<'a>(
    lockers: &[Locker],
    labels: impl Iterator<Item = Label<'a>>,
    self_pid: Pid,
) -> Vec<LockRow> {
    let labels: HashMap<Pid, Label> = labels.map(|l| (l.target.pid, l)).collect();
    let mut rows: Vec<LockRow> = lockers
        .iter()
        .map(|l| {
            let label = labels
                .get(&l.pid)
                .filter(|label| label.target.start_time == l.start_time);
            LockRow {
                target: Target {
                    pid: l.pid,
                    start_time: l.start_time,
                },
                name: label.map_or_else(|| l.name.clone(), |label| label.name.to_owned()),
                service: l.service.clone(),
                kind: l.kind,
                can_end: l.kind != LockerKind::Critical
                    && l.pid != self_pid
                    && label.is_some_and(|label| label.group != Group::System),
            }
        })
        .collect();
    rows.sort_by_cached_key(|r| r.name.to_lowercase());
    rows
}

/// Startup items by name, case-insensitive.
pub fn sort_startup(items: &mut [StartupItem]) {
    items.sort_by_cached_key(|i| (i.name.to_lowercase(), i.location));
}

/// Services matching `filter` (short or display name), by display name.
pub fn service_rows<'a>(services: &'a [Service], filter: &str) -> Vec<&'a Service> {
    let filter = filter.trim().to_lowercase();
    let mut rows: Vec<&Service> = services
        .iter()
        .filter(|s| {
            filter.is_empty()
                || s.name.to_lowercase().contains(&filter)
                || s.display_name.to_lowercase().contains(&filter)
        })
        .collect();
    rows.sort_by_cached_key(|s| s.display_name.to_lowercase());
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr};
    use vigie_probe::{ServiceState, StartupLocation};

    fn addr(port: u16) -> SocketAddr {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port)
    }

    fn label(pid: Pid, name: &'static str, group: Group) -> Label<'static> {
        Label {
            target: Target {
                pid,
                start_time: 100,
            },
            name,
            group,
        }
    }

    fn sockets() -> Vec<Socket> {
        let tcp = |local, remote: Option<u16>, state, pid| Socket {
            protocol: Protocol::Tcp,
            local: addr(local),
            remote: remote.map(addr),
            state: Some(state),
            pid,
        };
        vec![
            tcp(52000, Some(3000), TcpState::Established, 20),
            tcp(3000, None, TcpState::Listen, 10),
            tcp(445, None, TcpState::Listen, KERNEL_PID),
            tcp(52001, Some(443), TcpState::TimeWait, 0),
            tcp(8080, None, TcpState::Listen, 30),
            Socket {
                protocol: Protocol::Udp,
                local: addr(5353),
                remote: None,
                state: None,
                pid: 40,
            },
        ]
    }

    fn labels() -> Vec<Label<'static>> {
        vec![
            label(10, "node.exe", Group::Background),
            label(20, "Brave Browser", Group::Apps),
            label(30, "svchost.exe", Group::System),
            label(40, "Vigie", Group::Apps),
            label(KERNEL_PID, "System", Group::System),
        ]
    }

    #[test]
    fn ports_list_listeners_first_and_skip_closing_connections() {
        let rows = port_rows(&sockets(), labels().into_iter(), 40, "");
        let ports: Vec<u16> = rows.iter().map(|r| r.local.port()).collect();
        assert_eq!(ports, [445, 3000, 5353, 8080, 52000]);
        let node = &rows[1];
        assert_eq!((node.name.as_str(), node.can_end), ("node.exe", true));
        assert!(!rows[0].can_end, "the kernel");
        assert!(!rows[2].can_end, "Vigie itself");
        assert!(!rows[3].can_end, "a system service");
    }

    #[test]
    fn a_port_search_puts_exact_matches_first() {
        let rows = port_rows(&sockets(), labels().into_iter(), 0, "3000");
        // The server on 3000, then the browser connected to it; nothing else.
        let pids: Vec<Pid> = rows.iter().map(|r| r.pid).collect();
        assert_eq!(pids, [10, 20]);
        assert!(rows.iter().all(|r| r.exact));
        let by_name = port_rows(&sockets(), labels().into_iter(), 0, "BRAVE");
        assert_eq!(by_name.len(), 1);
        assert!(!by_name[0].exact);
    }

    #[test]
    fn holders_keep_their_identity_and_critical_ones_are_protected() {
        let locker = |pid, start_time, kind| Locker {
            pid,
            start_time,
            name: format!("rm{pid}"),
            service: None,
            kind,
        };
        let rows = lock_rows(
            &[
                locker(20, 100, LockerKind::App),
                locker(10, 999, LockerKind::Console),
                locker(30, 100, LockerKind::Critical),
            ],
            labels().into_iter(),
            40,
        );
        let find = |pid| rows.iter().find(|r| r.target.pid == pid).unwrap();
        assert_eq!(
            (find(20).name.as_str(), find(20).can_end),
            ("Brave Browser", true)
        );
        // PID 10 now belongs to another process than the one holding the file.
        assert_eq!((find(10).name.as_str(), find(10).can_end), ("rm10", false));
        assert!(!find(30).can_end);
    }

    #[test]
    fn services_filter_and_sort_by_display_name() {
        let service = |name: &str, display: &str| Service {
            name: name.into(),
            display_name: display.into(),
            state: ServiceState::Running,
            start_type: None,
            pid: None,
        };
        let all = [
            service("wuauserv", "Windows Update"),
            service("Spooler", "Print Spooler"),
            service("EventLog", "Windows Event Log"),
        ];
        let names: Vec<&str> = service_rows(&all, "")
            .iter()
            .map(|s| s.name.as_str())
            .collect();
        assert_eq!(names, ["Spooler", "EventLog", "wuauserv"]);
        assert_eq!(service_rows(&all, "SPOOL").len(), 1);
        let mut items = vec![
            StartupItem {
                name: "zed".into(),
                command: String::new(),
                location: StartupLocation::UserRun,
                enabled: true,
                exe: None,
            },
            StartupItem {
                name: "Alpha".into(),
                command: String::new(),
                location: StartupLocation::MachineRun,
                enabled: false,
                exe: None,
            },
        ];
        sort_startup(&mut items);
        assert_eq!(items[0].name, "Alpha");
    }
}
