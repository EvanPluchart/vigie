//! Machine-wide figures for the Performance view: rates from counter deltas, short histories
//! for the graphs, and the memory pressure definition the diagnostics (M3) will reuse.

use vigie_probe::{CoreTimes, DiskCounters, MemoryInfo, NetCounters, SystemSample};

/// Points kept per graph: one minute at the default interval. M3 adds longer tiers.
pub const HISTORY: usize = 60;

/// A fixed-size history of values, oldest first. Allocated once.
#[derive(Clone, Debug)]
pub struct Ring {
    values: Vec<f32>,
    next: usize,
    full: bool,
}

impl Default for Ring {
    fn default() -> Self {
        Self::new(HISTORY)
    }
}

impl Ring {
    pub fn new(capacity: usize) -> Self {
        Self {
            values: vec![0.0; capacity.max(1)],
            next: 0,
            full: false,
        }
    }

    pub fn push(&mut self, value: f32) {
        self.values[self.next] = value;
        self.next = (self.next + 1) % self.values.len();
        self.full |= self.next == 0;
    }

    pub fn len(&self) -> usize {
        if self.full {
            self.values.len()
        } else {
            self.next
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn capacity(&self) -> usize {
        self.values.len()
    }

    /// Values from the oldest to the newest.
    pub fn iter(&self) -> impl Iterator<Item = f32> + '_ {
        let start = if self.full { self.next } else { 0 };
        (0..self.len()).map(move |i| self.values[(start + i) % self.values.len()])
    }

    pub fn last(&self) -> Option<f32> {
        (!self.is_empty())
            .then(|| self.values[(self.next + self.values.len() - 1) % self.values.len()])
    }

    pub fn max(&self) -> f32 {
        self.iter().fold(0.0, f32::max)
    }
}

/// How hard memory is pushed, for colour and, later, diagnostics.
///
/// * `High`: less than 10 % of physical memory available, or commit above 90 % of its limit:
///   the system is paging or about to refuse allocations.
/// * `Medium`: less than 20 % available, or commit above 80 %.
/// * `Low` otherwise.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Pressure {
    #[default]
    Low,
    Medium,
    High,
}

impl Pressure {
    pub fn of(memory: &MemoryInfo) -> Pressure {
        if memory.total == 0 {
            return Pressure::Low;
        }
        let available = memory.available as f64 / memory.total as f64;
        let commit = if memory.commit_limit == 0 {
            0.0
        } else {
            memory.committed as f64 / memory.commit_limit as f64
        };
        if available < 0.10 || commit > 0.90 {
            Pressure::High
        } else if available < 0.20 || commit > 0.80 {
            Pressure::Medium
        } else {
            Pressure::Low
        }
    }
}

/// One disk as the Performance view shows it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DiskView {
    pub index: u32,
    /// Busy share of the last interval, 0 to 100.
    pub active_percent: Option<f32>,
    pub read_per_s: Option<f64>,
    pub write_per_s: Option<f64>,
}

/// What the Performance view shows after one tick.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PerfFrame {
    pub cpu_percent: Option<f32>,
    pub core_percents: Vec<f32>,
    pub memory: MemoryInfo,
    pub memory_used_percent: f32,
    pub pressure: Pressure,
    pub received_per_s: Option<f64>,
    pub sent_per_s: Option<f64>,
    pub disks: Vec<DiskView>,
    pub uptime_s: u64,
}

/// Turns successive system samples into rates and histories.
#[derive(Debug, Default)]
pub struct Performance {
    prev_at_ns: u64,
    prev_cores: Vec<CoreTimes>,
    prev_net: Option<NetCounters>,
    prev_disks: Vec<DiskCounters>,
    pub cpu: Ring,
    pub memory: Ring,
    /// Received and sent, bytes per second.
    pub received: Ring,
    pub sent: Ring,
    /// Active share per disk, by position in the last sample.
    pub disks: Vec<Ring>,
}

impl Performance {
    pub fn update(&mut self, sample: &SystemSample) -> PerfFrame {
        let wall_ns = sample.at_ns.saturating_sub(self.prev_at_ns);
        let first = self.prev_at_ns == 0;
        let mut frame = PerfFrame {
            memory: sample.memory,
            pressure: Pressure::of(&sample.memory),
            uptime_s: sample.uptime_s,
            ..PerfFrame::default()
        };

        // CPU: busy share of each core over the interval, and of the whole machine.
        if !first && self.prev_cores.len() == sample.cores.len() {
            let (mut busy, mut total) = (0u64, 0u64);
            for (now, before) in sample.cores.iter().zip(&self.prev_cores) {
                let b = now.busy_ns.saturating_sub(before.busy_ns);
                let t = now.total_ns.saturating_sub(before.total_ns);
                busy += b;
                total += t;
                frame.core_percents.push(share(b, t));
            }
            let cpu = share(busy, total);
            frame.cpu_percent = Some(cpu);
            self.cpu.push(cpu);
        }

        // Memory in use: what is not available.
        if sample.memory.total > 0 {
            let used = sample.memory.total.saturating_sub(sample.memory.available);
            frame.memory_used_percent = (used as f64 / sample.memory.total as f64 * 100.0) as f32;
            self.memory.push(frame.memory_used_percent);
        }

        // Network rates.
        if let (Some(now), Some(before)) = (sample.network, self.prev_net)
            && !first
            && wall_ns > 0
        {
            let per_s = |n: u64, b: u64| n.saturating_sub(b) as f64 * 1e9 / wall_ns as f64;
            let (rx, tx) = (
                per_s(now.received, before.received),
                per_s(now.sent, before.sent),
            );
            frame.received_per_s = Some(rx);
            frame.sent_per_s = Some(tx);
            self.received.push(rx as f32);
            self.sent.push(tx as f32);
        }

        // Disks: matched by number, so a disk plugged in mid-run starts without a value.
        self.disks.resize_with(sample.disks.len(), Ring::default);
        for (i, now) in sample.disks.iter().enumerate() {
            let before = self.prev_disks.iter().find(|d| d.index == now.index);
            let mut view = DiskView {
                index: now.index,
                ..DiskView::default()
            };
            if let Some(before) = before
                && !first
                && wall_ns > 0
            {
                let active = share(
                    now.active_ns.saturating_sub(before.active_ns),
                    now.clock_ns.saturating_sub(before.clock_ns),
                );
                let per_s = |n: u64, b: u64| n.saturating_sub(b) as f64 * 1e9 / wall_ns as f64;
                view.active_percent = Some(active);
                view.read_per_s = Some(per_s(now.read_bytes, before.read_bytes));
                view.write_per_s = Some(per_s(now.written_bytes, before.written_bytes));
                self.disks[i].push(active);
            }
            frame.disks.push(view);
        }

        self.prev_at_ns = sample.at_ns;
        self.prev_cores.clone_from(&sample.cores);
        self.prev_net = sample.network;
        self.prev_disks.clone_from(&sample.disks);
        frame
    }
}

/// `part / whole` as a percentage clamped to 0..=100; 0 when nothing elapsed.
fn share(part: u64, whole: u64) -> f32 {
    if whole == 0 {
        0.0
    } else {
        (part as f64 / whole as f64 * 100.0).clamp(0.0, 100.0) as f32
    }
}

/// SVG path commands for a graph of `values` (oldest first) in a `width` × `height` box, with
/// `max` at the top. The newest value sits at the right edge; a short history starts partway.
/// Returns the line and the closed area under it.
pub fn graph_paths(
    values: impl Iterator<Item = f32>,
    capacity: usize,
    max: f32,
    width: f32,
    height: f32,
) -> (String, String) {
    let values: Vec<f32> = values.collect();
    if values.is_empty() || max <= 0.0 {
        return (String::new(), String::new());
    }
    let step = width / (capacity.max(2) - 1) as f32;
    let x0 = width - step * (values.len() - 1) as f32;
    let point = |i: usize, v: f32| {
        let x = x0 + step * i as f32;
        let y = height - (v / max).clamp(0.0, 1.0) * height;
        (x, y)
    };
    let mut line = String::with_capacity(values.len() * 14);
    for (i, &v) in values.iter().enumerate() {
        let (x, y) = point(i, v);
        line.push_str(if i == 0 { "M " } else { " L " });
        line.push_str(&format!("{x:.1} {y:.1}"));
    }
    let area = format!("{line} L {width:.1} {height:.1} L {x0:.1} {height:.1} Z");
    (line, area)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SEC: u64 = 1_000_000_000;

    #[test]
    fn ring_keeps_the_last_values_in_order() {
        let mut ring = Ring::new(3);
        assert!(ring.is_empty() && ring.last().is_none());
        for v in [1.0, 2.0, 3.0, 4.0] {
            ring.push(v);
        }
        assert_eq!(ring.iter().collect::<Vec<_>>(), [2.0, 3.0, 4.0]);
        assert_eq!((ring.len(), ring.last(), ring.max()), (3, Some(4.0), 4.0));
    }

    fn system(at: u64, busy: [u64; 2], net: (u64, u64), disk: (u64, u64, u64)) -> SystemSample {
        SystemSample {
            at_ns: at,
            cores: busy
                .iter()
                .map(|&b| CoreTimes {
                    busy_ns: b,
                    total_ns: at,
                })
                .collect(),
            memory: MemoryInfo {
                total: 100,
                available: 25,
                committed: 50,
                commit_limit: 100,
                ..MemoryInfo::default()
            },
            network: Some(NetCounters {
                received: net.0,
                sent: net.1,
            }),
            disks: vec![DiskCounters {
                index: 0,
                read_bytes: disk.0,
                written_bytes: disk.1,
                active_ns: disk.2,
                clock_ns: at,
            }],
            uptime_s: 1,
        }
    }

    #[test]
    fn rates_come_from_deltas() {
        let mut perf = Performance::default();
        let first = perf.update(&system(SEC, [0, 0], (0, 0), (0, 0, 0)));
        assert_eq!(first.cpu_percent, None, "no rate on the first sample");
        assert_eq!(first.memory_used_percent, 75.0);
        // One second later: core 0 fully busy, core 1 idle; 1 MB in, 0.5 MB out; disk half busy.
        let frame = perf.update(&system(
            2 * SEC,
            [SEC, 0],
            (1_000_000, 500_000),
            (4096, 0, SEC / 2),
        ));
        assert_eq!(frame.core_percents, [100.0, 0.0]);
        assert_eq!(frame.cpu_percent, Some(50.0));
        assert_eq!(frame.received_per_s, Some(1_000_000.0));
        assert_eq!(frame.sent_per_s, Some(500_000.0));
        let disk = &frame.disks[0];
        assert_eq!(
            (disk.active_percent, disk.read_per_s),
            (Some(50.0), Some(4096.0))
        );
        assert_eq!(perf.cpu.last(), Some(50.0));
    }

    #[test]
    fn pressure_levels() {
        let memory = |available, committed| MemoryInfo {
            total: 100,
            available,
            committed,
            commit_limit: 100,
            ..MemoryInfo::default()
        };
        assert_eq!(Pressure::of(&memory(50, 50)), Pressure::Low);
        assert_eq!(Pressure::of(&memory(15, 50)), Pressure::Medium);
        assert_eq!(Pressure::of(&memory(50, 85)), Pressure::Medium);
        assert_eq!(Pressure::of(&memory(5, 50)), Pressure::High);
        assert_eq!(Pressure::of(&memory(50, 95)), Pressure::High);
        assert_eq!(Pressure::of(&MemoryInfo::default()), Pressure::Low);
    }

    #[test]
    fn graph_paths_fit_the_box() {
        let (line, area) = graph_paths([0.0, 50.0, 100.0].into_iter(), 3, 100.0, 200.0, 100.0);
        assert_eq!(line, "M 0.0 100.0 L 100.0 50.0 L 200.0 0.0");
        assert!(area.starts_with(&line) && area.ends_with('Z'));
        // A short history hugs the right edge.
        let (line, _) = graph_paths([10.0].into_iter(), 60, 100.0, 200.0, 100.0);
        assert_eq!(line, "M 200.0 90.0");
        assert_eq!(graph_paths(std::iter::empty(), 60, 100.0, 1.0, 1.0).0, "");
    }
}
