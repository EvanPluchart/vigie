//! "Why is my computer slow?" (decision 0006): rules over the current figures and the
//! histories. Each finding carries its evidence and one suggested action. Rules raise at one
//! threshold and clear at a lower one, so findings do not flicker. No sentences here: the UI
//! words findings, through translations.

use std::collections::HashSet;

use crate::history::SECOND;
use crate::perf::{PerfFrame, Performance, Pressure, PressureCause, commit_share};
use crate::tracker::{Tracked, Tracker};
use crate::{Group, ProcKey, Target};

/// Machine CPU: mean over two minutes.
const SATURATED_WINDOW_S: u64 = 120;
const SATURATED_RAISE: f32 = 90.0;
const SATURATED_CLEAR: f32 = 75.0;
/// One process on a full core: mean over five minutes, in cores.
const HOG_WINDOW_S: u64 = 300;
const HOG_RAISE: f32 = 0.9;
const HOG_CLEAR: f32 = 0.6;
/// Memory pressure clears once available is back above 25 % and commit below 75 %.
const PRESSURE_CLEAR_AVAILABLE: f64 = 0.25;
const PRESSURE_CLEAR_COMMIT: f64 = 0.75;
/// Growth: at least 30 minutes of one-minute buckets, up to six hours.
const GROWTH_MIN_POINTS: usize = 30;
const GROWTH_WINDOW_S: u64 = 6 * 3600;
const GROWTH_RAISE_BYTES: f64 = 256.0 * 1024.0 * 1024.0;
const GROWTH_CLEAR_BYTES: f64 = 128.0 * 1024.0 * 1024.0;
/// Growth must also be a quarter of where it started, steady (R²), and still at its top.
const GROWTH_MIN_SHARE: f64 = 0.25;
const GROWTH_RAISE_FIT: f64 = 0.8;
const GROWTH_CLEAR_FIT: f64 = 0.6;
/// Disk: mean active share over one minute.
const DISK_WINDOW_S: u64 = 60;
const DISK_RAISE: f32 = 90.0;
const DISK_CLEAR: f32 = 70.0;
/// Uptime after which a restart is worth suggesting.
const LONG_UPTIME_S: u64 = 14 * 24 * 3600;
/// Least share of a window that must have values for a mean to count.
const MIN_COVERAGE: f32 = 0.5;
/// Processes named as evidence, at most.
const CULPRITS: usize = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Info,
    Warning,
    Critical,
}

/// A process named by a finding.
#[derive(Clone, Debug, PartialEq)]
pub struct Culprit {
    pub target: Target,
    pub name: String,
    pub group: Group,
    /// Mean share of the machine over the finding's window, percent.
    pub cpu: Option<f32>,
    pub memory: u64,
}

/// The one thing a finding suggests doing.
#[derive(Clone, Debug, PartialEq)]
pub enum Suggestion {
    /// End (or restart) this process. The UI asks first.
    End(Culprit),
    /// The culprit belongs to the system: ending it could crash the machine.
    LeaveSystem(Culprit),
    /// Close programs you do not need (no single culprit to end).
    ClosePrograms,
    /// Look at the Performance view.
    ShowPerformance,
    /// Restart the computer (a real restart: fast startup keeps the kernel running).
    Restart,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Finding {
    /// The whole CPU is busy.
    CpuSaturated {
        mean: f32,
        window_s: u64,
        culprits: Vec<Culprit>,
    },
    /// One process keeps a full core busy.
    CpuHog {
        culprit: Culprit,
        /// Mean, in cores.
        cores: f32,
        window_s: u64,
    },
    MemoryPressure {
        level: Pressure,
        cause: PressureCause,
        /// Percent of physical memory available, and committed share of the limit.
        available_percent: f32,
        commit_percent: f32,
        culprits: Vec<Culprit>,
    },
    /// A process whose memory keeps growing.
    MemoryGrowth {
        culprit: Culprit,
        from: u64,
        to: u64,
        window_s: u64,
    },
    DiskBusy {
        index: u32,
        letters: u32,
        mean: f32,
        window_s: u64,
    },
    LongUptime {
        days: u64,
    },
}

impl Finding {
    pub fn severity(&self) -> Severity {
        match self {
            Finding::CpuSaturated { .. } => Severity::Critical,
            Finding::MemoryPressure { level, .. } if *level == Pressure::High => Severity::Critical,
            Finding::CpuHog { .. }
            | Finding::MemoryPressure { .. }
            | Finding::MemoryGrowth { .. }
            | Finding::DiskBusy { .. } => Severity::Warning,
            Finding::LongUptime { .. } => Severity::Info,
        }
    }

    pub fn suggestion(&self) -> Suggestion {
        match self {
            Finding::CpuSaturated { culprits, .. } | Finding::MemoryPressure { culprits, .. } => {
                match culprits.first() {
                    Some(c) => end_or_leave(c),
                    None => Suggestion::ClosePrograms,
                }
            }
            Finding::CpuHog { culprit, .. } | Finding::MemoryGrowth { culprit, .. } => {
                end_or_leave(culprit)
            }
            Finding::DiskBusy { .. } => Suggestion::ShowPerformance,
            Finding::LongUptime { .. } => Suggestion::Restart,
        }
    }

    /// The resource it is about, for the "what was checked" list.
    pub fn area(&self) -> Area {
        match self {
            Finding::CpuSaturated { .. } | Finding::CpuHog { .. } => Area::Cpu,
            Finding::MemoryPressure { .. } | Finding::MemoryGrowth { .. } => Area::Memory,
            Finding::DiskBusy { .. } => Area::Disk,
            Finding::LongUptime { .. } => Area::System,
        }
    }

    /// Identifies the finding across runs, for hysteresis.
    fn id(&self) -> Id {
        match self {
            Finding::CpuSaturated { .. } => Id::Saturated,
            Finding::CpuHog { culprit, .. } => Id::Hog(key(culprit)),
            Finding::MemoryPressure { .. } => Id::Pressure,
            Finding::MemoryGrowth { culprit, .. } => Id::Growth(key(culprit)),
            Finding::DiskBusy { index, .. } => Id::Disk(*index),
            Finding::LongUptime { .. } => Id::Uptime,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Area {
    Cpu,
    Memory,
    Disk,
    System,
}

fn end_or_leave(c: &Culprit) -> Suggestion {
    if c.group == Group::System {
        Suggestion::LeaveSystem(c.clone())
    } else {
        Suggestion::End(c.clone())
    }
}

fn key(c: &Culprit) -> ProcKey {
    (c.target.pid, c.target.start_time)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Id {
    Saturated,
    Hog(ProcKey),
    Pressure,
    Growth(ProcKey),
    Disk(u32),
    Uptime,
}

/// What the rules read.
pub struct Input<'a> {
    pub now_ns: u64,
    pub perf: &'a Performance,
    pub last: &'a PerfFrame,
    pub tracker: &'a Tracker,
    pub logical_cores: u32,
}

/// Runs the rules and remembers which findings are raised, for hysteresis.
#[derive(Debug, Default)]
pub struct Diagnostics {
    active: HashSet<Id>,
}

impl Diagnostics {
    /// Findings, most severe first.
    pub fn run(&mut self, input: &Input) -> Vec<Finding> {
        let mut findings = Vec::new();
        let active = &self.active;
        let raised = |id: Id| active.contains(&id);
        let now = input.now_ns;

        // The whole CPU, then single processes on a full core.
        let saturated = input
            .perf
            .cpu
            .mean_over(now, SATURATED_WINDOW_S)
            .filter(|&(_, coverage)| coverage >= MIN_COVERAGE)
            .map(|(mean, _)| mean)
            .filter(|&mean| {
                mean >= if raised(Id::Saturated) {
                    SATURATED_CLEAR
                } else {
                    SATURATED_RAISE
                }
            });
        let mut named = None;
        if let Some(mean) = saturated {
            let culprits = top(input.tracker, now, SATURATED_WINDOW_S, |t| {
                t.cpu
                    .mean_over(now, SATURATED_WINDOW_S)
                    .map(|(m, _)| m)
                    .filter(|&m| m >= 1.0)
            });
            named = culprits.first().map(key);
            findings.push(Finding::CpuSaturated {
                mean,
                window_s: SATURATED_WINDOW_S,
                culprits,
            });
        }
        let cores = input.logical_cores.max(1) as f32;
        for t in input.tracker.iter() {
            if named == Some(t.key) {
                continue;
            }
            let Some((mean, coverage)) = t.cpu.mean_over(now, HOG_WINDOW_S) else {
                continue;
            };
            let in_cores = mean / 100.0 * cores;
            let threshold = if raised(Id::Hog(t.key)) {
                HOG_CLEAR
            } else {
                HOG_RAISE
            };
            if coverage >= 0.8 && in_cores >= threshold {
                findings.push(Finding::CpuHog {
                    culprit: culprit(t, now, HOG_WINDOW_S),
                    cores: in_cores,
                    window_s: HOG_WINDOW_S,
                });
            }
        }

        // Memory pressure, with its cause.
        let memory = &input.last.memory;
        let (level, cause) = Pressure::with_cause(memory);
        let available = if memory.total == 0 {
            1.0
        } else {
            memory.available as f64 / memory.total as f64
        };
        let commit = commit_share(memory);
        let still = raised(Id::Pressure)
            && (available < PRESSURE_CLEAR_AVAILABLE || commit > PRESSURE_CLEAR_COMMIT);
        if level > Pressure::Low || still {
            let culprits = top(input.tracker, now, 60, |t| t.memory.last());
            findings.push(Finding::MemoryPressure {
                level: level.max(Pressure::Medium),
                cause: if level > Pressure::Low {
                    cause
                } else if available < PRESSURE_CLEAR_AVAILABLE {
                    PressureCause::Available
                } else {
                    PressureCause::Commit
                },
                available_percent: (available * 100.0) as f32,
                commit_percent: (commit * 100.0) as f32,
                culprits,
            });
        }

        // Processes whose memory keeps growing.
        for t in input.tracker.iter() {
            if let Some((from, to)) = growth(t, now, raised(Id::Growth(t.key))) {
                findings.push(Finding::MemoryGrowth {
                    culprit: culprit(t, now, 60),
                    from: from as u64,
                    to: to as u64,
                    // How long the process has been watched, up to the window.
                    window_s: (now.saturating_sub(t.since_ns) / SECOND).min(GROWTH_WINDOW_S),
                });
            }
        }
        // Disks.
        for (history, disk) in input.perf.disks.iter().zip(&input.last.disks) {
            let threshold = if raised(Id::Disk(disk.index)) {
                DISK_CLEAR
            } else {
                DISK_RAISE
            };
            if let Some((mean, coverage)) = history.mean_over(now, DISK_WINDOW_S)
                && coverage >= MIN_COVERAGE
                && mean >= threshold
            {
                findings.push(Finding::DiskBusy {
                    index: disk.index,
                    letters: disk.letters,
                    mean,
                    window_s: DISK_WINDOW_S,
                });
            }
        }

        if input.last.uptime_s >= LONG_UPTIME_S {
            findings.push(Finding::LongUptime {
                days: input.last.uptime_s / 86_400,
            });
        }

        self.active = findings.iter().map(Finding::id).collect();
        // Stable: rules already run in a sensible order within a severity.
        findings.sort_by_key(|f| std::cmp::Reverse(f.severity()));
        findings
    }
}

/// The tracked processes with the largest `value`, largest first, with their mean CPU over the
/// last `window_s`.
fn top(
    tracker: &Tracker,
    now_ns: u64,
    window_s: u64,
    value: impl Fn(&Tracked) -> Option<f32>,
) -> Vec<Culprit> {
    let mut ranked: Vec<(f32, &Tracked)> = tracker
        .iter()
        .filter_map(|t| value(t).filter(|v| v.is_finite()).map(|v| (v, t)))
        .collect();
    ranked.sort_by(|a, b| b.0.total_cmp(&a.0));
    ranked
        .into_iter()
        .take(CULPRITS)
        .map(|(_, t)| culprit(t, now_ns, window_s))
        .collect()
}

/// A tracked process as evidence, with its mean CPU over the last `window_s`.
fn culprit(t: &Tracked, now_ns: u64, window_s: u64) -> Culprit {
    Culprit {
        target: Target {
            pid: t.key.0,
            start_time: t.key.1,
        },
        name: t.name.clone(),
        group: t.group,
        cpu: t.cpu.mean_over(now_ns, window_s).map(|(m, _)| m),
        memory: t.memory.last().unwrap_or(0.0) as u64,
    }
}

/// Fitted memory at the start and end of the window when it grows steadily, else `None`.
fn growth(t: &Tracked, now_ns: u64, raised: bool) -> Option<(f64, f64)> {
    // Ages in minutes, newest first, from the one-minute tier.
    let mut points: Vec<(f64, f64)> = Vec::new();
    t.memory.recent(1, now_ns, GROWTH_WINDOW_S, |age, mean, _| {
        if mean.is_finite() {
            points.push((-(age as f64) / 60.0, f64::from(mean)));
        }
    });
    if points.len() < GROWTH_MIN_POINTS {
        return None;
    }
    let n = points.len() as f64;
    let (sx, sy) = points
        .iter()
        .fold((0.0, 0.0), |(a, b), &(x, y)| (a + x, b + y));
    let (mx, my) = (sx / n, sy / n);
    let (mut sxy, mut sxx, mut syy) = (0.0, 0.0, 0.0);
    for &(x, y) in &points {
        sxy += (x - mx) * (y - my);
        sxx += (x - mx) * (x - mx);
        syy += (y - my) * (y - my);
    }
    if sxx == 0.0 || syy == 0.0 {
        return None;
    }
    let slope = sxy / sxx;
    let fit = sxy * sxy / (sxx * syy);
    let (x_new, x_old) = (points[0].0, points[points.len() - 1].0);
    let to = my + slope * (x_new - mx);
    let from = (my + slope * (x_old - mx)).max(0.0);
    let grown = to - from;
    let peak = points.iter().map(|p| p.1).fold(0.0, f64::max);
    let still_at_top = points[0].1 >= 0.95 * peak;
    let (min_bytes, min_fit) = if raised {
        (GROWTH_CLEAR_BYTES, GROWTH_CLEAR_FIT)
    } else {
        (GROWTH_RAISE_BYTES, GROWTH_RAISE_FIT)
    };
    (slope > 0.0
        && grown >= min_bytes
        && grown >= GROWTH_MIN_SHARE * from
        && fit >= min_fit
        && still_at_top)
        .then_some((from, to))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tracker::Candidate;
    use vigie_probe::{MemoryInfo, SystemSample};

    const GB: u64 = 1 << 30;

    struct World {
        perf: Performance,
        last: PerfFrame,
        tracker: Tracker,
        diagnostics: Diagnostics,
        now: u64,
    }

    impl World {
        fn new() -> Self {
            let mut perf = Performance::default();
            let last = perf.update(&system(SECOND, GB * 8, 0));
            Self {
                perf,
                last,
                tracker: Tracker::default(),
                diagnostics: Diagnostics::default(),
                now: SECOND,
            }
        }

        /// One second of the machine: total CPU, available memory, disk activity; the given
        /// processes as (pid, cpu %, memory, group).
        fn tick(&mut self, cpu: f32, available: u64, disk: f32, procs: &[(u32, f32, u64, Group)]) {
            self.now += SECOND;
            let sample = system(self.now, available, (disk * 10_000_000.0) as u64);
            self.last = self.perf.update(&sample);
            // The CPU history is set directly: core times are not the point here.
            self.perf.cpu.record(self.now, cpu);
            let candidates: Vec<Candidate> = procs
                .iter()
                .map(|&(pid, cpu, memory, group)| Candidate {
                    key: (pid, 7),
                    name: "app",
                    group,
                    cpu: Some(cpu),
                    memory,
                })
                .collect();
            if self.tracker.wants_round(self.now) {
                self.tracker.round(self.now, 16 * GB, &candidates);
            }
            for c in &candidates {
                self.tracker.record(c.key, self.now, c.cpu, c.memory);
            }
        }

        fn run(&mut self) -> Vec<Finding> {
            self.diagnostics.run(&Input {
                now_ns: self.now,
                perf: &self.perf,
                last: &self.last,
                tracker: &self.tracker,
                logical_cores: 8,
            })
        }
    }

    /// 16 GB machine; disk busy share as active nanoseconds per second elapsed.
    fn system(at: u64, available: u64, disk_active_per_s: u64) -> SystemSample {
        SystemSample {
            at_ns: at,
            memory: MemoryInfo {
                total: 16 * GB,
                available,
                committed: 8 * GB,
                commit_limit: 32 * GB,
                ..MemoryInfo::default()
            },
            disks: vec![vigie_probe::DiskCounters {
                index: 0,
                letters: 1 << 2,
                active_ns: disk_active_per_s * (at / SECOND),
                clock_ns: at,
                ..vigie_probe::DiskCounters::default()
            }],
            uptime_s: at / SECOND,
            ..SystemSample::default()
        }
    }

    #[test]
    fn a_quiet_machine_has_nothing_to_say() {
        let mut w = World::new();
        for _ in 0..300 {
            w.tick(5.0, 8 * GB, 1.0, &[(1, 1.0, GB, Group::Apps)]);
        }
        assert_eq!(w.run(), []);
    }

    #[test]
    fn saturation_names_the_busiest_and_clears_with_hysteresis() {
        let mut w = World::new();
        let procs = [(1, 70.0, GB, Group::Apps), (2, 20.0, GB, Group::System)];
        for _ in 0..120 {
            w.tick(97.0, 8 * GB, 0.0, &procs);
        }
        let findings = w.run();
        let Finding::CpuSaturated { culprits, mean, .. } = &findings[0] else {
            panic!("{findings:?}");
        };
        assert!(*mean > 90.0);
        assert_eq!(culprits[0].target.pid, 1);
        assert!(matches!(findings[0].suggestion(), Suggestion::End(c) if c.target.pid == 1));
        // At 80 % it stays raised (clears below 75 %), and a fresh rule would not raise it.
        for _ in 0..120 {
            w.tick(80.0, 8 * GB, 0.0, &procs);
        }
        assert!(matches!(w.run()[0], Finding::CpuSaturated { .. }));
        assert!(
            Diagnostics::default()
                .run(&Input {
                    now_ns: w.now,
                    perf: &w.perf,
                    last: &w.last,
                    tracker: &w.tracker,
                    logical_cores: 8,
                })
                .iter()
                .all(|f| !matches!(f, Finding::CpuSaturated { .. }))
        );
        for _ in 0..120 {
            w.tick(50.0, 8 * GB, 0.0, &procs);
        }
        assert!(
            w.run()
                .iter()
                .all(|f| !matches!(f, Finding::CpuSaturated { .. }))
        );
    }

    #[test]
    fn one_full_core_for_five_minutes_is_a_hog() {
        let mut w = World::new();
        // 8 cores: 12.5 % of the machine is one core.
        for _ in 0..299 {
            w.tick(20.0, 8 * GB, 0.0, &[(5, 12.5, GB, Group::Background)]);
        }
        let findings = w.run();
        let Finding::CpuHog { culprit, cores, .. } = &findings[0] else {
            panic!("{findings:?}");
        };
        assert_eq!(culprit.target.pid, 5);
        assert!((cores - 1.0).abs() < 0.01);
        // System processes are left alone.
        let mut w = World::new();
        for _ in 0..299 {
            w.tick(20.0, 8 * GB, 0.0, &[(6, 12.5, GB, Group::System)]);
        }
        assert!(matches!(
            w.run()[0].suggestion(),
            Suggestion::LeaveSystem(_)
        ));
    }

    #[test]
    fn memory_pressure_says_why() {
        let mut w = World::new();
        // 1 GB of 16 available: high, from physical memory.
        w.tick(5.0, GB, 0.0, &[(1, 1.0, 6 * GB, Group::Apps)]);
        let findings = w.run();
        let Finding::MemoryPressure {
            level,
            cause,
            culprits,
            ..
        } = &findings[0]
        else {
            panic!("{findings:?}");
        };
        assert_eq!((*level, *cause), (Pressure::High, PressureCause::Available));
        assert_eq!(culprits[0].memory, 6 * GB);
        assert_eq!(findings[0].severity(), Severity::Critical);
        // 22 % available: below the medium threshold no more, but not clear yet (25 %).
        w.tick(5.0, GB * 22 / 10 * 16 / 10, 0.0, &[]);
        assert!(matches!(w.run()[0], Finding::MemoryPressure { .. }));
        w.tick(5.0, 8 * GB, 0.0, &[]);
        assert_eq!(w.run(), []);
    }

    #[test]
    fn steady_growth_is_found_noise_is_not() {
        let mut w = World::new();
        // 40 minutes: process 3 grows by 20 MB a minute from 500 MB; process 4 wobbles.
        for s in 0..40 * 60u64 {
            let grow = 500 * (1 << 20) + s * 20 * (1 << 20) / 60;
            let wobble = GB + if (s / 60) % 2 == 0 { 300 << 20 } else { 0 };
            w.tick(
                5.0,
                8 * GB,
                0.0,
                &[(3, 1.0, grow, Group::Apps), (4, 1.0, wobble, Group::Apps)],
            );
        }
        let findings = w.run();
        let growths: Vec<_> = findings
            .iter()
            .filter_map(|f| match f {
                Finding::MemoryGrowth {
                    culprit, from, to, ..
                } => Some((culprit.target.pid, *from, *to)),
                _ => None,
            })
            .collect();
        assert_eq!(growths.len(), 1, "{findings:?}");
        let (pid, from, to) = growths[0];
        assert_eq!(pid, 3);
        assert!(to - from > 700 << 20, "{from} → {to}");
    }

    #[test]
    fn a_busy_disk_points_to_the_performance_view() {
        let mut w = World::new();
        for _ in 0..60 {
            w.tick(5.0, 8 * GB, 95.0, &[]);
        }
        let findings = w.run();
        assert!(
            matches!(findings[0], Finding::DiskBusy { index: 0, mean, .. } if mean > 90.0),
            "{findings:?}"
        );
        assert_eq!(findings[0].suggestion(), Suggestion::ShowPerformance);
    }
}
