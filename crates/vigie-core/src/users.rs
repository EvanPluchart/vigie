//! Task Manager's Users tab: processes grouped by login session, with their totals.

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::{Core, Sample, Target};

/// One login session and what its processes use.
#[derive(Clone, Debug, PartialEq)]
pub struct UserGroup {
    pub session: u32,
    /// Percentage of the whole machine; `None` until a process has a CPU figure.
    pub cpu_percent: Option<f32>,
    pub memory_bytes: u64,
    /// The session's processes, busiest first.
    pub members: Vec<Member>,
}

/// A process of a session, named as the process table names it.
#[derive(Clone, Debug, PartialEq)]
pub struct Member {
    pub target: Target,
    pub name: String,
    pub file: String,
    pub path: Option<Arc<str>>,
    pub icon: Option<u32>,
    pub cpu_percent: Option<f32>,
    pub memory_bytes: u64,
    /// Processes "End process tree" would end besides this one.
    pub tree_size: u32,
    /// Vigie itself.
    pub own: bool,
}

impl Core {
    /// The user sessions of `sample` (the last one given to `update`), in session order.
    /// Session 0 holds services and the system: it belongs to no user and is left out, as in
    /// Task Manager.
    pub fn users(&self, sample: &Sample) -> Vec<UserGroup> {
        let mut by_session: BTreeMap<u32, UserGroup> = BTreeMap::new();
        for (label, p) in self.labels(sample).zip(&sample.processes) {
            let Some(session) = p.session.filter(|&s| s != 0) else {
                continue;
            };
            let cpu = self.cpu_of((p.pid, p.start_time));
            let group = by_session.entry(session).or_insert_with(|| UserGroup {
                session,
                cpu_percent: None,
                memory_bytes: 0,
                members: Vec::new(),
            });
            group.cpu_percent = match (group.cpu_percent, cpu) {
                (Some(a), Some(b)) => Some(a + b),
                (a, b) => a.or(b),
            };
            group.memory_bytes += p.memory_bytes;
            group.members.push(Member {
                target: label.target,
                name: label.name.to_owned(),
                file: p.name.clone(),
                path: self.path_of((p.pid, p.start_time)),
                icon: label.icon,
                cpu_percent: cpu,
                memory_bytes: p.memory_bytes,
                tree_size: self.tree_size(p.pid),
                own: p.pid == sample.self_pid,
            });
        }
        let mut groups: Vec<UserGroup> = by_session.into_values().collect();
        for group in &mut groups {
            group.members.sort_by(|a, b| {
                b.cpu_percent
                    .unwrap_or(-1.0)
                    .total_cmp(&a.cpu_percent.unwrap_or(-1.0))
                    .then(b.memory_bytes.cmp(&a.memory_bytes))
                    .then(a.target.pid.cmp(&b.target.pid))
            });
        }
        groups
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ProcSample, Sort};

    fn proc(pid: u32, session: Option<u32>, cpu_s: u64, memory: u64) -> ProcSample {
        ProcSample {
            pid,
            start_time: 100,
            name: format!("p{pid}.exe"),
            cpu_time_ns: cpu_s * 1_000_000_000,
            memory_bytes: memory,
            session,
            ..ProcSample::default()
        }
    }

    fn sample(at_s: u64, processes: Vec<ProcSample>) -> Sample {
        Sample {
            at_ns: at_s * 1_000_000_000,
            logical_cores: 1,
            memory_total_bytes: 1000,
            processes: processes
                .into_iter()
                .map(|p| ProcSample {
                    measured_at_ns: at_s * 1_000_000_000,
                    ..p
                })
                .collect(),
            ..Sample::default()
        }
    }

    #[test]
    fn groups_by_session_without_services() {
        let mut core = Core::new(Sort::default(), "");
        let first = vec![
            proc(1, Some(0), 0, 50),
            proc(2, Some(1), 0, 10),
            proc(3, Some(1), 0, 20),
            proc(4, Some(2), 0, 5),
            proc(5, None, 0, 1),
        ];
        core.update(&sample(1, first));
        // One second later, process 3 used half a second, process 2 a tenth.
        let mut second = vec![
            proc(1, Some(0), 0, 50),
            proc(2, Some(1), 0, 10),
            proc(3, Some(1), 0, 20),
            proc(4, Some(2), 0, 5),
            proc(5, None, 0, 1),
        ];
        second[1].cpu_time_ns = 100_000_000;
        second[2].cpu_time_ns = 500_000_000;
        let s = sample(2, second);
        core.update(&s);
        let users = core.users(&s);
        assert_eq!(users.iter().map(|u| u.session).collect::<Vec<_>>(), [1, 2]);
        let first = &users[0];
        assert_eq!(first.memory_bytes, 30);
        assert!((first.cpu_percent.unwrap() - 60.0).abs() < 0.01);
        assert_eq!(
            first
                .members
                .iter()
                .map(|m| m.target.pid)
                .collect::<Vec<_>>(),
            [3, 2],
            "busiest first"
        );
        assert_eq!(first.members[0].file, "p3.exe");
    }
}
