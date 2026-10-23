//! Alerts (decision 0008): when to send a system notification. Pure functions over the
//! histories and the findings, run at the diagnostics cadence. One notification per fact: a
//! threshold crossed and the critical finding about the same resource share one cooldown.

use crate::diagnose::{Culprit, Finding};
use crate::history::SECOND;
use crate::perf::{Performance, Pressure};

/// At most one notification per resource in this time.
pub const COOLDOWN_NS: u64 = 15 * 60 * SECOND;
/// A threshold alert clears this many points under its threshold.
const HYSTERESIS: f32 = 10.0;
/// The share of the window that must be measured: all of it, so nothing fires at launch.
const FULL: f32 = 0.99;
/// Memory thresholds look at the last minute.
const MEMORY_WINDOW_S: u64 = 60;
/// A critical finding notifies once it has lasted this long…
const CRITICAL_HOLD_NS: u64 = 60 * SECOND;
/// …and not before Vigie has run this long: a machine already under pressure at launch should
/// not greet the user with a notification every time.
const LAUNCH_GRACE_NS: u64 = 120 * SECOND;

/// User thresholds; 0 turns one off.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rules {
    pub cpu_percent: f32,
    pub cpu_window_s: u64,
    pub memory_percent: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Resource {
    Cpu,
    Memory,
}

/// A notification to send.
#[derive(Clone, Debug, PartialEq)]
pub enum Alert {
    /// Total CPU at or above the user's threshold over the window.
    CpuThreshold {
        mean: f32,
        window_s: u64,
        culprit: Option<Culprit>,
    },
    /// Memory in use at or above the user's threshold over a minute.
    MemoryThreshold {
        percent: f32,
        culprit: Option<Culprit>,
    },
    /// A critical finding was just raised (CPU saturated, high memory pressure).
    Critical(Finding),
}

impl Alert {
    pub fn resource(&self) -> Resource {
        match self {
            Alert::CpuThreshold { .. } => Resource::Cpu,
            Alert::MemoryThreshold { .. } => Resource::Memory,
            Alert::Critical(Finding::MemoryPressure { .. }) => Resource::Memory,
            Alert::Critical(_) => Resource::Cpu,
        }
    }
}

#[derive(Debug, Default)]
pub struct Alerts {
    started_ns: Option<u64>,
    cpu_raised: bool,
    memory_raised: bool,
    critical_cpu: Critical,
    critical_memory: Critical,
    last_cpu_ns: Option<u64>,
    last_memory_ns: Option<u64>,
}

/// How long a critical finding has lasted, and whether it was already notified.
#[derive(Debug, Default)]
struct Critical {
    since_ns: Option<u64>,
    notified: bool,
}

impl Critical {
    /// True once, when a finding present now has lasted long enough after the launch grace.
    fn due(&mut self, present: bool, now_ns: u64, started_ns: u64) -> bool {
        if !present {
            *self = Critical::default();
            return false;
        }
        let since = *self.since_ns.get_or_insert(now_ns);
        let ready = now_ns.saturating_sub(since) >= CRITICAL_HOLD_NS
            && now_ns.saturating_sub(started_ns) >= LAUNCH_GRACE_NS;
        if ready && !self.notified {
            self.notified = true;
            return true;
        }
        false
    }
}

impl Alerts {
    /// Notifications due now. `findings` are the diagnostics of this run.
    pub fn evaluate(
        &mut self,
        now_ns: u64,
        perf: &Performance,
        findings: &[Finding],
        rules: Rules,
    ) -> Vec<Alert> {
        let mut due = Vec::new();
        let started = *self.started_ns.get_or_insert(now_ns);

        // Thresholds, with hysteresis: an edge from below to above is what notifies.
        let mut cpu_edge = None;
        if rules.cpu_percent > 0.0 {
            let threshold = if self.cpu_raised {
                rules.cpu_percent - HYSTERESIS
            } else {
                rules.cpu_percent
            };
            let measured = perf
                .cpu
                .mean_over(now_ns, rules.cpu_window_s)
                .filter(|&(_, coverage)| coverage >= FULL);
            let above = measured.is_some_and(|(mean, _)| mean >= threshold);
            if above && !self.cpu_raised {
                cpu_edge = measured.map(|(mean, _)| mean);
            }
            self.cpu_raised = above;
        } else {
            self.cpu_raised = false;
        }
        let mut memory_edge = None;
        if rules.memory_percent > 0.0 {
            let threshold = if self.memory_raised {
                rules.memory_percent - HYSTERESIS
            } else {
                rules.memory_percent
            };
            let measured = perf
                .memory
                .mean_over(now_ns, MEMORY_WINDOW_S)
                .filter(|&(_, coverage)| coverage >= FULL);
            let above = measured.is_some_and(|(mean, _)| mean >= threshold);
            if above && !self.memory_raised {
                memory_edge = measured.map(|(mean, _)| mean);
            }
            self.memory_raised = above;
        } else {
            self.memory_raised = false;
        }

        // Critical findings: an edge as well.
        let saturated = findings
            .iter()
            .find(|f| matches!(f, Finding::CpuSaturated { .. }));
        let pressure = findings.iter().find(|f| {
            matches!(
                f,
                Finding::MemoryPressure {
                    level: Pressure::High,
                    ..
                }
            )
        });
        let culprit = |f: Option<&Finding>| match f {
            Some(Finding::CpuSaturated { culprits, .. })
            | Some(Finding::MemoryPressure { culprits, .. }) => culprits.first().cloned(),
            _ => None,
        };

        let cpu_alert = if let Some(mean) = cpu_edge {
            Some(Alert::CpuThreshold {
                mean,
                window_s: rules.cpu_window_s,
                culprit: culprit(saturated),
            })
        } else {
            saturated
                .filter(|_| self.critical_cpu.due(true, now_ns, started))
                .map(|f| Alert::Critical(f.clone()))
        };
        if saturated.is_none() {
            self.critical_cpu.due(false, now_ns, started);
        }
        let memory_alert = if let Some(percent) = memory_edge {
            Some(Alert::MemoryThreshold {
                percent,
                culprit: culprit(pressure),
            })
        } else {
            pressure
                .filter(|_| self.critical_memory.due(true, now_ns, started))
                .map(|f| Alert::Critical(f.clone()))
        };
        if pressure.is_none() {
            self.critical_memory.due(false, now_ns, started);
        }

        // One notification per resource per cooldown.
        for alert in [cpu_alert, memory_alert].into_iter().flatten() {
            let last = match alert.resource() {
                Resource::Cpu => &mut self.last_cpu_ns,
                Resource::Memory => &mut self.last_memory_ns,
            };
            if last.is_none_or(|at| now_ns.saturating_sub(at) >= COOLDOWN_NS) {
                *last = Some(now_ns);
                due.push(alert);
            }
        }
        due
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnose::Severity;

    const RULES: Rules = Rules {
        cpu_percent: 90.0,
        cpu_window_s: 120,
        memory_percent: 0.0,
    };

    fn perf_with_cpu(values: impl IntoIterator<Item = (u64, f32)>) -> Performance {
        let mut perf = Performance::default();
        for (s, v) in values {
            perf.cpu.record(s * SECOND, v);
        }
        perf
    }

    #[test]
    fn nothing_fires_before_the_window_is_measured() {
        let perf = perf_with_cpu((1..=60).map(|s| (s, 100.0)));
        assert!(
            Alerts::default()
                .evaluate(60 * SECOND, &perf, &[], RULES)
                .is_empty()
        );
    }

    #[test]
    fn a_threshold_notifies_once_then_waits_for_the_cooldown() {
        let mut alerts = Alerts::default();
        let mut perf = perf_with_cpu((1..=120).map(|s| (s, 95.0)));
        let first = alerts.evaluate(120 * SECOND, &perf, &[], RULES);
        assert!(matches!(first[..], [Alert::CpuThreshold { mean, .. }] if mean > 90.0));
        // Still high: no repeat.
        for s in 121..=240 {
            perf.cpu.record(s * SECOND, 95.0);
        }
        assert!(alerts.evaluate(240 * SECOND, &perf, &[], RULES).is_empty());
        // Down (clears), up again within 15 minutes: suppressed by the cooldown.
        for s in 241..=360 {
            perf.cpu.record(s * SECOND, 10.0);
        }
        assert!(alerts.evaluate(360 * SECOND, &perf, &[], RULES).is_empty());
        for s in 361..=480 {
            perf.cpu.record(s * SECOND, 99.0);
        }
        assert!(alerts.evaluate(480 * SECOND, &perf, &[], RULES).is_empty());
    }

    #[test]
    fn a_critical_finding_and_a_threshold_make_one_notification() {
        let perf = perf_with_cpu((1..=120).map(|s| (s, 97.0)));
        let saturated = Finding::CpuSaturated {
            mean: 97.0,
            window_s: 120,
            culprits: Vec::new(),
        };
        assert_eq!(saturated.severity(), Severity::Critical);
        let mut alerts = Alerts::default();
        let due = alerts.evaluate(120 * SECOND, &perf, std::slice::from_ref(&saturated), RULES);
        assert_eq!(due.len(), 1, "{due:?}");
        // Without a CPU threshold, the finding alone notifies, once it has lasted a minute and
        // Vigie has run for two.
        let mut alerts = Alerts::default();
        let none = Rules {
            cpu_percent: 0.0,
            ..RULES
        };
        let at = |alerts: &mut Alerts, s: u64| {
            alerts.evaluate(s * SECOND, &perf, std::slice::from_ref(&saturated), none)
        };
        assert!(at(&mut alerts, 120).is_empty(), "just launched");
        assert!(
            at(&mut alerts, 180).is_empty(),
            "one minute in: still the launch grace"
        );
        assert!(matches!(
            at(&mut alerts, 240)[..],
            [Alert::Critical(Finding::CpuSaturated { .. })]
        ));
        assert!(at(&mut alerts, 245).is_empty(), "once");
    }

    #[test]
    fn memory_already_low_at_launch_waits_and_must_last() {
        let low = Finding::MemoryPressure {
            level: Pressure::High,
            cause: crate::perf::PressureCause::Available,
            available_percent: 4.0,
            commit_percent: 60.0,
            culprits: Vec::new(),
        };
        let perf = Performance::default();
        let rules = Rules::default();
        let mut alerts = Alerts::default();
        let run = |alerts: &mut Alerts, s: u64, present: bool| {
            let findings = if present {
                vec![low.clone()]
            } else {
                Vec::new()
            };
            alerts.evaluate(s * SECOND, &perf, &findings, rules)
        };
        assert!(
            run(&mut alerts, 5, true).is_empty(),
            "already low at launch: no greeting"
        );
        assert!(run(&mut alerts, 100, true).is_empty());
        // Gone, then back after the grace: it must last a minute again.
        assert!(run(&mut alerts, 130, false).is_empty());
        assert!(run(&mut alerts, 140, true).is_empty());
        assert!(run(&mut alerts, 170, true).is_empty());
        assert_eq!(run(&mut alerts, 200, true).len(), 1);
    }
}
