//! Machine-wide figures for the Performance view: rates from counter deltas, their tiered
//! histories, and the memory pressure definition the diagnostics reuse.

use crate::history::{GAUGE_TIERS, History, SYSTEM_TIERS};
use vigie_probe::{CoreTimes, DiskCounters, GpuCounters, MemoryInfo, NetCounters, SystemSample};

/// How hard memory is pushed, for colour and diagnostics.
///
/// * `High`: less than 10 % of physical memory available, or commit above 90 % of its limit:
///   the system is paging or about to refuse allocations.
/// * `Medium`: less than 20 % available, or commit above 80 %.
/// * `Low` otherwise.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum Pressure {
    #[default]
    Low,
    Medium,
    High,
}

/// What sets the pressure level: physical memory running out, or commit nearing its limit.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PressureCause {
    #[default]
    Available,
    Commit,
}

impl Pressure {
    pub fn of(memory: &MemoryInfo) -> Pressure {
        Self::with_cause(memory).0
    }

    /// The level and what sets it. When both sides reach the same level, physical memory wins:
    /// it is what makes the machine page.
    pub fn with_cause(memory: &MemoryInfo) -> (Pressure, PressureCause) {
        if memory.total == 0 {
            return (Pressure::Low, PressureCause::Available);
        }
        let available = memory.available as f64 / memory.total as f64;
        let commit = commit_share(memory);
        let by_available = if available < 0.10 {
            Pressure::High
        } else if available < 0.20 {
            Pressure::Medium
        } else {
            Pressure::Low
        };
        let by_commit = if commit > 0.90 {
            Pressure::High
        } else if commit > 0.80 {
            Pressure::Medium
        } else {
            Pressure::Low
        };
        if by_commit > by_available {
            (by_commit, PressureCause::Commit)
        } else {
            (by_available, PressureCause::Available)
        }
    }
}

/// Committed memory as a share of its limit, 0 to 1 (0 when the limit is unknown).
pub fn commit_share(memory: &MemoryInfo) -> f64 {
    if memory.commit_limit == 0 {
        0.0
    } else {
        memory.committed as f64 / memory.commit_limit as f64
    }
}

/// One disk as the Performance view shows it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DiskView {
    pub index: u32,
    /// Drive letters on the disk, bit 0 is A: (see `DiskCounters::letters`).
    pub letters: u32,
    /// Busy share of the last interval, 0 to 100.
    pub active_percent: Option<f32>,
    pub read_per_s: Option<f64>,
    pub write_per_s: Option<f64>,
}

/// One GPU as the Performance view shows it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GpuView {
    pub index: u32,
    /// Share of the busiest engine over the last interval, 0 to 100, as Task Manager shows it.
    pub percent: Option<f32>,
    pub dedicated_used: u64,
    pub dedicated_total: u64,
}

/// What the Performance view shows after one tick.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PerfFrame {
    /// Monotonic time of the sample, same clock as `SystemSample::at_ns`.
    pub at_ns: u64,
    pub cpu_percent: Option<f32>,
    pub core_percents: Vec<f32>,
    pub memory: MemoryInfo,
    pub memory_used_percent: f32,
    pub pressure: Pressure,
    pub pressure_cause: PressureCause,
    pub received_per_s: Option<f64>,
    pub sent_per_s: Option<f64>,
    pub disks: Vec<DiskView>,
    /// Empty until GPU figures have been read twice.
    pub gpus: Vec<GpuView>,
    pub uptime_s: u64,
}

/// Turns successive system samples into rates and histories.
#[derive(Debug)]
pub struct Performance {
    prev_at_ns: u64,
    prev_cores: Vec<CoreTimes>,
    prev_net: Option<NetCounters>,
    prev_disks: Vec<DiskCounters>,
    /// Total CPU, percent.
    pub cpu: History,
    /// Memory in use, percent.
    pub memory: History,
    /// Received and sent, bytes per second.
    pub received: History,
    pub sent: History,
    /// Active share per disk, by position in the last sample.
    pub disks: Vec<History>,
    /// Busiest-engine share per GPU, by index.
    pub gpus: Vec<History>,
    prev_gpus: Vec<GpuCounters>,
    prev_gpus_at_ns: u64,
    gpu_views: Vec<GpuView>,
}

impl Default for Performance {
    fn default() -> Self {
        Self {
            prev_at_ns: 0,
            prev_cores: Vec::new(),
            prev_net: None,
            prev_disks: Vec::new(),
            cpu: History::new(&SYSTEM_TIERS),
            memory: History::new(&GAUGE_TIERS),
            received: History::new(&SYSTEM_TIERS),
            sent: History::new(&SYSTEM_TIERS),
            disks: Vec::new(),
            gpus: Vec::new(),
            prev_gpus: Vec::new(),
            prev_gpus_at_ns: 0,
            gpu_views: Vec::new(),
        }
    }
}

impl Performance {
    /// Bytes held by every history.
    pub fn history_bytes(&self) -> usize {
        [&self.cpu, &self.memory, &self.received, &self.sent]
            .into_iter()
            .chain(&self.disks)
            .chain(&self.gpus)
            .map(History::bytes)
            .sum()
    }

    pub fn update(&mut self, sample: &SystemSample) -> PerfFrame {
        let wall_ns = sample.at_ns.saturating_sub(self.prev_at_ns);
        let first = self.prev_at_ns == 0;
        let (pressure, pressure_cause) = Pressure::with_cause(&sample.memory);
        let at = sample.at_ns;
        let mut frame = PerfFrame {
            at_ns: at,
            memory: sample.memory,
            pressure,
            pressure_cause,
            uptime_s: sample.uptime_s,
            ..PerfFrame::default()
        };

        // CPU: busy share of each core over the interval, and of the whole machine.
        if !first && !sample.cores.is_empty() && self.prev_cores.len() == sample.cores.len() {
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
            self.cpu.record(at, cpu);
        }

        // Memory in use: what is not available.
        if sample.memory.total > 0 {
            let used = sample.memory.total.saturating_sub(sample.memory.available);
            frame.memory_used_percent = (used as f64 / sample.memory.total as f64 * 100.0) as f32;
            self.memory.record(at, frame.memory_used_percent);
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
            self.received.record(at, rx as f32);
            self.sent.record(at, tx as f32);
        }

        // Disks: matched by number, so a disk plugged in mid-run starts without a value.
        self.disks
            .resize_with(sample.disks.len(), || History::new(&SYSTEM_TIERS));
        for (i, now) in sample.disks.iter().enumerate() {
            let before = self.prev_disks.iter().find(|d| d.index == now.index);
            let mut view = DiskView {
                index: now.index,
                letters: now.letters,
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
                self.disks[i].record(at, active);
            }
            frame.disks.push(view);
        }

        self.prev_at_ns = sample.at_ns;
        self.prev_cores.clone_from(&sample.cores);
        self.prev_net = sample.network;
        self.prev_disks.clone_from(&sample.disks);
        self.update_gpus(sample);
        frame.gpus.clone_from(&self.gpu_views);
        frame
    }
}

impl Performance {
    /// GPU figures change only when the sampler read them again (`gpus_at_ns`), which it does
    /// less often than every tick. Between reads, the last views are kept.
    fn update_gpus(&mut self, sample: &SystemSample) {
        if sample.gpus_at_ns == self.prev_gpus_at_ns {
            return;
        }
        let wall_ns = sample.gpus_at_ns.saturating_sub(self.prev_gpus_at_ns);
        let first = self.prev_gpus_at_ns == 0;
        self.gpus
            .resize_with(sample.gpus.len(), || History::new(&SYSTEM_TIERS));
        self.gpu_views.clear();
        for (i, now) in sample.gpus.iter().enumerate() {
            let before = self.prev_gpus.iter().find(|g| g.index == now.index);
            let percent = before.filter(|_| !first && wall_ns > 0).map(|before| {
                now.engines_ns
                    .iter()
                    .zip(&before.engines_ns)
                    .map(|(&n, &b)| {
                        let delta = n.wrapping_sub(b);
                        // Some drivers report nonsense; a delta longer than the interval (with
                        // a little slack for timing) cannot be real.
                        if delta > wall_ns + wall_ns / 2 {
                            0.0
                        } else {
                            share(delta, wall_ns)
                        }
                    })
                    .fold(0.0, f32::max)
            });
            if let Some(p) = percent {
                self.gpus[i].record(sample.gpus_at_ns, p);
            }
            self.gpu_views.push(GpuView {
                index: now.index,
                percent,
                dedicated_used: now.dedicated_used,
                dedicated_total: now.dedicated_total,
            });
        }
        self.prev_gpus.clone_from(&sample.gpus);
        self.prev_gpus_at_ns = sample.gpus_at_ns;
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

#[cfg(test)]
mod tests {
    use super::*;

    const SEC: u64 = 1_000_000_000;

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
                letters: 0,
                read_bytes: disk.0,
                written_bytes: disk.1,
                active_ns: disk.2,
                clock_ns: at,
            }],
            uptime_s: 1,
            ..SystemSample::default()
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
    fn gpu_is_the_busiest_engine_and_ignores_driver_noise() {
        let gpu = |at, engines: Vec<u64>| SystemSample {
            at_ns: at,
            gpus: vec![GpuCounters {
                index: 0,
                engines_ns: engines,
                dedicated_used: 1,
                dedicated_total: 4,
            }],
            gpus_at_ns: at,
            ..SystemSample::default()
        };
        let mut perf = Performance::default();
        assert_eq!(
            perf.update(&gpu(SEC, vec![0, 0, u64::MAX])).gpus[0].percent,
            None
        );
        // Over 1 s: engine 0 busy 25 %, engine 1 busy 60 %, engine 2 jumps by nonsense.
        let frame = perf.update(&gpu(2 * SEC, vec![SEC / 4, SEC * 6 / 10, 123 * SEC]));
        assert_eq!(frame.gpus[0].percent, Some(60.0));
        assert_eq!(frame.gpus[0].dedicated_total, 4);
        // A tick without a new GPU read keeps the last figures.
        let quiet = SystemSample {
            at_ns: 3 * SEC,
            gpus_at_ns: 2 * SEC,
            ..gpu(3 * SEC, vec![0, 0, 0])
        };
        assert_eq!(perf.update(&quiet).gpus[0].percent, Some(60.0));
        assert_eq!(perf.gpus[0].len(), 1);
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
        assert_eq!(
            Pressure::with_cause(&memory(50, 85)).1,
            PressureCause::Commit
        );
        assert_eq!(
            Pressure::with_cause(&memory(5, 95)).1,
            PressureCause::Available,
            "physical memory wins a tie"
        );
    }
}
