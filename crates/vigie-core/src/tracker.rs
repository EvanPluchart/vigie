//! Per-process history for a bounded set of processes (decision 0006): the top consumers of
//! CPU and memory, and the one the user selected. Memory stays bounded whatever the number of
//! processes; the diagnostics read their evidence from here.

use std::collections::HashMap;

use crate::history::{History, PROCESS_TIERS, SECOND};
use crate::{Group, ProcKey};

/// Processes with a history at most.
pub const SLOTS: usize = 32;
/// Candidates per round from each ranking (CPU, memory).
const TOP: usize = 12;
/// How often the top consumers are looked for.
const ROUND_NS: u64 = 10 * SECOND;

/// One tracked process.
#[derive(Clone, Debug)]
pub struct Tracked {
    pub key: ProcKey,
    /// Display name, as the table shows it.
    pub name: String,
    pub group: Group,
    /// Share of the whole machine, percent.
    pub cpu: History,
    /// Bytes.
    pub memory: History,
    /// Monotonic time the process was first tracked.
    pub since_ns: u64,
    /// Kept while selected, even when it consumes nothing.
    pub pinned: bool,
}

/// What a round needs to know about one process.
#[derive(Clone, Copy, Debug)]
pub struct Candidate<'a> {
    pub key: ProcKey,
    pub name: &'a str,
    pub group: Group,
    pub cpu: Option<f32>,
    pub memory: u64,
}

#[derive(Debug, Default)]
pub struct Tracker {
    slots: Vec<Tracked>,
    by_key: HashMap<ProcKey, usize>,
    last_round_ns: Option<u64>,
    /// Indices into the candidates, reused between rounds.
    order: Vec<usize>,
    pinned: Option<ProcKey>,
    /// Memory total, to compare memory with CPU when choosing whom to evict.
    memory_total: u64,
}

impl Tracker {
    /// Records this tick's figures of a process, if it is tracked.
    pub fn record(&mut self, key: ProcKey, at_ns: u64, cpu: Option<f32>, memory: u64) {
        if let Some(&i) = self.by_key.get(&key) {
            let slot = &mut self.slots[i];
            slot.cpu.record(at_ns, cpu.unwrap_or(f32::NAN));
            slot.memory.record(at_ns, memory as f32);
        }
    }

    /// Forgets processes that exited.
    pub fn retain(&mut self, mut alive: impl FnMut(ProcKey) -> bool) {
        let before = self.slots.len();
        self.slots.retain(|s| alive(s.key));
        if self.slots.len() != before {
            self.reindex();
        }
    }

    /// Whether `round` has work to do at this time: the caller can skip building candidates.
    pub fn wants_round(&self, at_ns: u64) -> bool {
        self.last_round_ns
            .is_none_or(|last| at_ns.saturating_sub(last) >= ROUND_NS)
            || self.pinned.is_some_and(|key| !self.by_key.contains_key(&key))
    }

    /// Every ten seconds, starts tracking the top consumers, replacing the weakest tracked
    /// processes when all slots are taken. Also picks up the pinned process.
    pub fn round(&mut self, at_ns: u64, memory_total: u64, candidates: &[Candidate]) {
        self.memory_total = memory_total;
        let due = self
            .last_round_ns
            .is_none_or(|last| at_ns.saturating_sub(last) >= ROUND_NS);
        if let Some(pinned) = self.pinned
            && !self.by_key.contains_key(&pinned)
            && let Some(c) = candidates.iter().find(|c| c.key == pinned)
        {
            self.admit(c, at_ns, true);
        }
        if !due {
            return;
        }
        self.last_round_ns = Some(at_ns);
        let mut order = std::mem::take(&mut self.order);
        for by_cpu in [true, false] {
            order.clear();
            order.extend(0..candidates.len());
            let value = |i: &usize| {
                let c = &candidates[*i];
                if by_cpu {
                    f64::from(c.cpu.unwrap_or(0.0))
                } else {
                    c.memory as f64
                }
            };
            let top = TOP.min(order.len());
            if top == 0 {
                continue;
            }
            if top < order.len() {
                order.select_nth_unstable_by(top - 1, |a, b| value(b).total_cmp(&value(a)));
            }
            for &i in &order[..top] {
                let c = &candidates[i];
                // Idle processes are not worth a slot.
                if value(&i) <= 0.0 {
                    continue;
                }
                if let Some(&slot) = self.by_key.get(&c.key) {
                    // Names and sections can change (details arrive, a window opens).
                    let slot = &mut self.slots[slot];
                    if slot.name != c.name {
                        slot.name.clear();
                        slot.name.push_str(c.name);
                    }
                    slot.group = c.group;
                } else {
                    self.admit(c, at_ns, false);
                }
            }
        }
        self.order = order;
    }

    /// Keeps the history of this process while it is selected; `None` releases it.
    pub fn pin(&mut self, key: Option<ProcKey>) {
        if let Some(old) = self.pinned
            && let Some(&i) = self.by_key.get(&old)
        {
            self.slots[i].pinned = false;
        }
        self.pinned = key;
        if let Some(key) = key
            && let Some(&i) = self.by_key.get(&key)
        {
            self.slots[i].pinned = true;
        }
    }

    pub fn get(&self, key: ProcKey) -> Option<&Tracked> {
        self.by_key.get(&key).map(|&i| &self.slots[i])
    }

    pub fn iter(&self) -> impl Iterator<Item = &Tracked> {
        self.slots.iter()
    }

    /// Bytes held by the histories.
    pub fn bytes(&self) -> usize {
        self.slots
            .iter()
            .map(|s| s.cpu.bytes() + s.memory.bytes())
            .sum()
    }

    fn admit(&mut self, c: &Candidate, at_ns: u64, pinned: bool) {
        let slot = Tracked {
            key: c.key,
            name: c.name.to_owned(),
            group: c.group,
            cpu: History::new(&PROCESS_TIERS),
            memory: History::new(&PROCESS_TIERS),
            since_ns: at_ns,
            pinned,
        };
        if self.slots.len() < SLOTS {
            self.slots.push(slot);
        } else {
            // Replace the tracked process that matters least now, unless the newcomer matters
            // even less. The pinned one stays.
            let newcomer = self.weight(c.cpu, c.memory as f32);
            let weakest = self
                .slots
                .iter()
                .enumerate()
                .filter(|(_, s)| !s.pinned)
                .map(|(i, s)| (i, self.weight(s.cpu.last(), s.memory.last().unwrap_or(0.0))))
                .min_by(|a, b| a.1.total_cmp(&b.1));
            match weakest {
                Some((i, weight)) if pinned || weight < newcomer => self.slots[i] = slot,
                _ => return,
            }
        }
        self.reindex();
    }

    /// CPU and memory on one scale: percent of the machine, the larger of the two.
    fn weight(&self, cpu: Option<f32>, memory: f32) -> f32 {
        let memory = if self.memory_total == 0 {
            0.0
        } else {
            memory / self.memory_total as f32 * 100.0
        };
        cpu.unwrap_or(0.0).max(memory)
    }

    fn reindex(&mut self) {
        self.by_key.clear();
        self.by_key
            .extend(self.slots.iter().enumerate().map(|(i, s)| (s.key, i)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(pid: u32, cpu: f32, memory: u64) -> Candidate<'static> {
        Candidate {
            key: (pid, 1),
            name: "p",
            group: Group::Apps,
            cpu: Some(cpu),
            memory,
        }
    }

    #[test]
    fn tracks_top_consumers_within_bounds() {
        let mut tracker = Tracker::default();
        // 100 processes: pid n uses n % CPU and (100 - n) MB.
        let all: Vec<Candidate> = (1..=100)
            .map(|n| candidate(n, n as f32 / 10.0, (100 - u64::from(n)) << 20))
            .collect();
        tracker.round(SECOND, 1 << 30, &all);
        assert_eq!(tracker.iter().count(), 2 * TOP);
        assert!(tracker.get((100, 1)).is_some(), "busiest CPU");
        assert!(tracker.get((1, 1)).is_some(), "largest memory");
        assert!(tracker.get((50, 1)).is_none());
        // Recording only touches tracked processes.
        tracker.record((100, 1), 2 * SECOND, Some(10.0), 1 << 20);
        tracker.record((50, 1), 2 * SECOND, Some(5.0), 1 << 20);
        assert_eq!(tracker.get((100, 1)).unwrap().cpu.last(), Some(10.0));
        // Rounds run every ten seconds, never past the slot count.
        for round in 2..10u64 {
            let busy: Vec<Candidate> = (1..=100)
                .map(|n| candidate(n + 1000 * round as u32, n as f32, 0))
                .collect();
            tracker.round(round * 10 * SECOND, 1 << 30, &busy);
            assert!(tracker.iter().count() <= SLOTS);
        }
    }

    #[test]
    fn exits_free_slots_and_pins_stay() {
        let mut tracker = Tracker::default();
        let all: Vec<Candidate> = (1..=40).map(|n| candidate(n, n as f32, 0)).collect();
        tracker.round(SECOND, 0, &all);
        tracker.retain(|key| key.0 != 40);
        assert!(tracker.get((40, 1)).is_none());
        // An idle process the user selects gets a slot, and keeps it.
        let mut with_idle = all.clone();
        with_idle.push(candidate(99, 0.0, 0));
        tracker.pin(Some((99, 1)));
        tracker.round(2 * SECOND, 0, &with_idle);
        assert!(tracker.get((99, 1)).is_some_and(|t| t.pinned));
        let busy: Vec<Candidate> = (200..260).map(|n| candidate(n, 50.0, 0)).collect();
        tracker.round(20 * SECOND, 0, &busy);
        assert!(tracker.get((99, 1)).is_some(), "pinned survives a busy round");
    }
}
