//! Process model: CPU % from deltas, tree, grouping, history, diagnostics.
//! No UI and no OS calls here. See docs/ARCHITECTURE.md.

use std::collections::HashMap;

pub use vigie_probe::{Pid, ProcSample, Sample};

/// Identifies a process across ticks. A PID can be reused, a `(pid, start_time)` pair cannot.
pub type ProcKey = (Pid, u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortKey {
    Cpu,
    Memory,
    Name,
    Pid,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sort {
    pub key: SortKey,
    pub ascending: bool,
}

impl Default for Sort {
    fn default() -> Self {
        Self {
            key: SortKey::Cpu,
            ascending: false,
        }
    }
}

/// One row of the process table, ready for the UI.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub pid: Pid,
    pub start_time: u64,
    pub name: String,
    /// Percentage of the whole machine (all logical cores). `None` on a process's first sample.
    pub cpu_percent: Option<f32>,
    pub memory_bytes: u64,
}

/// What the UI needs after one tick: the visible rows, sorted and filtered, and Vigie's own cost.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Frame {
    pub rows: Vec<Row>,
    pub process_count: usize,
    pub self_cpu_percent: Option<f32>,
    pub self_memory_bytes: u64,
}

/// Turns successive samples into frames.
#[derive(Debug, Default)]
pub struct Core {
    sort: Sort,
    /// Lowercased once, so matching does not allocate per process.
    filter: String,
    prev_at_ns: u64,
    /// CPU time of each process at the previous tick.
    prev_cpu: HashMap<ProcKey, u64>,
    next_cpu: HashMap<ProcKey, u64>,
}

impl Core {
    pub fn new(sort: Sort, filter: impl Into<String>) -> Self {
        Self {
            sort,
            filter: filter.into().to_lowercase(),
            ..Self::default()
        }
    }

    pub fn update(&mut self, sample: &Sample) -> Frame {
        let wall_ns = sample.at_ns.saturating_sub(self.prev_at_ns);
        let mut frame = Frame {
            process_count: sample.processes.len(),
            ..Frame::default()
        };

        self.next_cpu.clear();
        for p in &sample.processes {
            let key = (p.pid, p.start_time);
            let cpu_percent = match self.prev_cpu.get(&key) {
                Some(&prev) => cpu_percent(prev, p.cpu_time_ns, wall_ns, sample.logical_cores),
                _ => None,
            };
            self.next_cpu.insert(key, p.cpu_time_ns);

            if p.pid == sample.self_pid {
                frame.self_cpu_percent = cpu_percent;
                frame.self_memory_bytes = p.memory_bytes;
            }
            if matches_filter(p, &self.filter) {
                frame.rows.push(Row {
                    pid: p.pid,
                    start_time: p.start_time,
                    name: p.name.clone(),
                    cpu_percent,
                    memory_bytes: p.memory_bytes,
                });
            }
        }
        std::mem::swap(&mut self.prev_cpu, &mut self.next_cpu);
        self.prev_at_ns = sample.at_ns;

        sort_rows(&mut frame.rows, self.sort);
        frame
    }
}

/// CPU time / wall time / logical cores, as a percentage clamped to 0..=100.
pub fn cpu_percent(prev_ns: u64, now_ns: u64, wall_ns: u64, logical_cores: u32) -> Option<f32> {
    if wall_ns == 0 || logical_cores == 0 {
        return None;
    }
    let used = now_ns.saturating_sub(prev_ns) as f64;
    let available = wall_ns as f64 * f64::from(logical_cores);
    Some((used / available * 100.0).clamp(0.0, 100.0) as f32)
}

fn matches_filter(p: &ProcSample, filter: &str) -> bool {
    if filter.is_empty() {
        return true;
    }
    if let Ok(pid) = filter.parse::<Pid>()
        && pid == p.pid
    {
        return true;
    }
    contains_ignore_case(&p.name, filter)
}

/// Case-insensitive substring search without allocating. `needle` is already lowercase.
fn contains_ignore_case(haystack: &str, needle: &str) -> bool {
    haystack.char_indices().any(|(i, _)| {
        let mut hay = lowercase(&haystack[i..]);
        needle.chars().all(|n| hay.next() == Some(n))
    })
}

fn lowercase(s: &str) -> impl Iterator<Item = char> + '_ {
    s.chars().flat_map(char::to_lowercase)
}

fn sort_rows(rows: &mut [Row], sort: Sort) {
    rows.sort_by(|a, b| {
        let primary = match sort.key {
            SortKey::Cpu => a
                .cpu_percent
                .unwrap_or(-1.0)
                .total_cmp(&b.cpu_percent.unwrap_or(-1.0)),
            SortKey::Memory => a.memory_bytes.cmp(&b.memory_bytes),
            SortKey::Name => lowercase(&a.name).cmp(lowercase(&b.name)),
            SortKey::Pid => a.pid.cmp(&b.pid),
        };
        let primary = if sort.ascending {
            primary
        } else {
            primary.reverse()
        };
        // Stable tie-break so rows do not jump between ticks.
        primary.then_with(|| a.pid.cmp(&b.pid))
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    const SEC: u64 = 1_000_000_000;

    fn proc(
        pid: Pid,
        start_time: u64,
        name: &str,
        cpu_time_ns: u64,
        memory_bytes: u64,
    ) -> ProcSample {
        ProcSample {
            pid,
            parent: None,
            start_time,
            name: name.into(),
            cpu_time_ns,
            memory_bytes,
        }
    }

    fn sample(at_ns: u64, processes: Vec<ProcSample>) -> Sample {
        Sample {
            at_ns,
            logical_cores: 4,
            self_pid: 1,
            processes,
        }
    }

    fn row(frame: &Frame, pid: Pid) -> &Row {
        frame.rows.iter().find(|r| r.pid == pid).expect("row")
    }

    #[test]
    fn cpu_percent_from_two_samples() {
        let mut core = Core::default();
        core.update(&sample(
            SEC,
            vec![proc(1, 10, "vigie", 0, 100), proc(2, 10, "busy", SEC, 200)],
        ));
        // Over 1 s on 4 cores: pid 2 used 2 s of CPU (50 %), pid 1 used 0.04 s (1 %).
        let frame = core.update(&sample(
            2 * SEC,
            vec![
                proc(1, 10, "vigie", SEC / 25, 150),
                proc(2, 10, "busy", 3 * SEC, 200),
            ],
        ));
        assert_eq!(row(&frame, 2).cpu_percent, Some(50.0));
        assert_eq!(row(&frame, 1).cpu_percent, Some(1.0));
        assert_eq!(frame.self_cpu_percent, Some(1.0));
        assert_eq!(frame.self_memory_bytes, 150);
    }

    #[test]
    fn first_sample_has_no_cpu_percent() {
        let mut core = Core::default();
        let frame = core.update(&sample(SEC, vec![proc(1, 10, "vigie", 5 * SEC, 100)]));
        assert_eq!(row(&frame, 1).cpu_percent, None);
        assert_eq!(frame.self_cpu_percent, None);

        // A process appearing later also starts without a value.
        let frame = core.update(&sample(
            2 * SEC,
            vec![
                proc(1, 10, "vigie", 5 * SEC, 100),
                proc(7, 11, "new", 9 * SEC, 100),
            ],
        ));
        assert_eq!(row(&frame, 1).cpu_percent, Some(0.0));
        assert_eq!(row(&frame, 7).cpu_percent, None);
    }

    #[test]
    fn reused_pid_is_a_new_process() {
        let mut core = Core::default();
        core.update(&sample(SEC, vec![proc(5, 100, "old", 50 * SEC, 1)]));
        // Same PID, different start time: no delta against the old process.
        let frame = core.update(&sample(2 * SEC, vec![proc(5, 200, "new", SEC, 1)]));
        let r = row(&frame, 5);
        assert_eq!(r.name, "new");
        assert_eq!(r.cpu_percent, None);
    }

    #[test]
    fn sorts_and_filters() {
        let processes = vec![
            proc(1, 0, "Vigie", 0, 30),
            proc(2, 0, "code", 0, 10),
            proc(3, 0, "VS Code Helper", 0, 20),
        ];
        let mut core = Core::new(
            Sort {
                key: SortKey::Memory,
                ascending: false,
            },
            "",
        );
        let pids: Vec<Pid> = core
            .update(&sample(SEC, processes.clone()))
            .rows
            .iter()
            .map(|r| r.pid)
            .collect();
        assert_eq!(pids, [1, 3, 2]);

        let mut core = Core::new(
            Sort {
                key: SortKey::Name,
                ascending: true,
            },
            "CODE",
        );
        let frame = core.update(&sample(SEC, processes.clone()));
        let pids: Vec<Pid> = frame.rows.iter().map(|r| r.pid).collect();
        assert_eq!(pids, [2, 3]);
        assert_eq!(frame.process_count, 3);

        let mut core = Core::new(Sort::default(), "3");
        let pids: Vec<Pid> = core
            .update(&sample(SEC, processes))
            .rows
            .iter()
            .map(|r| r.pid)
            .collect();
        assert_eq!(pids, [3]);
    }
}
