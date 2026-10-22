//! Process model: CPU % from deltas, tree, grouping, history, diagnostics.
//! No UI and no OS calls here. See docs/ARCHITECTURE.md.

mod tree;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

pub use tree::Group;
use tree::{Entry, Figures, Node};
pub use vigie_probe::{Pid, ProcSample, Sample, Target};

/// Identifies a process across ticks. A PID can be reused, a `(pid, start_time)` pair cannot.
pub type ProcKey = (Pid, u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortKey {
    Cpu,
    Memory,
    Name,
    Pid,
}

impl SortKey {
    /// Numbers read best largest first, names and PIDs in natural order.
    pub fn default_ascending(self) -> bool {
        matches!(self, SortKey::Name | SortKey::Pid)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sort {
    pub key: SortKey,
    pub ascending: bool,
}

impl Sort {
    /// The sort after clicking `key`'s column header: the same column flips direction, another
    /// column starts in its natural direction.
    pub fn clicked(self, key: SortKey) -> Sort {
        if key == self.key {
            Sort {
                key,
                ascending: !self.ascending,
            }
        } else {
            Sort {
                key,
                ascending: key.default_ascending(),
            }
        }
    }
}

impl Default for Sort {
    fn default() -> Self {
        Self {
            key: SortKey::Cpu,
            ascending: false,
        }
    }
}

/// What a row of the process table is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowKind {
    /// A section header; `count` is the number of processes in it.
    Group(Group),
    /// A process. With descendants, its figures are the total of its subtree.
    Process,
    /// The first line under an expanded process: that process alone.
    Itself,
}

/// One row of the process table, ready for the UI.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub kind: RowKind,
    pub pid: Pid,
    pub start_time: u64,
    pub name: String,
    /// Percentage of the whole machine (all logical cores). `None` on a process's first sample.
    pub cpu_percent: Option<f32>,
    pub memory_bytes: u64,
    /// 0 (cool) to 3 (hot), for background tints.
    pub cpu_heat: u8,
    pub memory_heat: u8,
    /// Indentation level in the tree.
    pub depth: u8,
    /// Group: processes in it. Process: its descendants.
    pub count: u32,
    pub expanded: bool,
    /// Icon of the process's executable, as an index chosen by the caller of `set_details`.
    pub icon: Option<u32>,
    pub publisher: Option<Arc<str>>,
    /// Processes "end process tree" would end besides this one: all real descendants, which
    /// may include processes the display tree shows elsewhere (another app, say).
    pub tree_size: u32,
    /// Vigie itself.
    pub own: bool,
}

/// Rarely changing facts about a process, read by the sampler from `Probe::details` and
/// `Probe::exe_info`, kept until the process exits.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Details {
    pub icon: Option<u32>,
    pub publisher: Option<Arc<str>>,
    pub description: Option<Arc<str>>,
}

/// Heat thresholds: CPU in percent of the whole machine, memory in percent of physical memory.
/// One busy core on a 16-thread machine is about 6 %, so it reads as warm, not hot.
const CPU_HEAT: [f32; 3] = [2.0, 10.0, 30.0];
const MEMORY_HEAT: [f32; 3] = [2.0, 5.0, 15.0];

/// 0 below the first threshold, then 1, 2 or 3.
pub fn heat(value: f32, thresholds: [f32; 3]) -> u8 {
    thresholds.iter().filter(|&&t| value >= t).count() as u8
}

/// What the UI needs after one tick: the visible rows, sorted and filtered, and Vigie's own cost.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Frame {
    pub rows: Vec<Row>,
    pub process_count: usize,
    /// Processes matching the search; equals `process_count` when not searching.
    pub matching_count: usize,
    pub self_cpu_percent: Option<f32>,
    pub self_memory_bytes: u64,
}

#[derive(Clone, Copy, Debug)]
struct Measure {
    cpu_ns: u64,
    at_ns: u64,
    percent: Option<f32>,
}

/// Turns successive samples into frames.
#[derive(Debug, Default)]
pub struct Core {
    sort: Sort,
    /// Lowercased once, so matching does not allocate per process.
    filter: String,
    /// Last measurement of each process.
    prev: HashMap<ProcKey, Measure>,
    next: HashMap<ProcKey, Measure>,
    /// Processes whose subtree is shown.
    expanded: HashSet<ProcKey>,
    /// Start time of each live PID, to resolve UI requests made by PID.
    starts: HashMap<Pid, u64>,
    details: HashMap<ProcKey, Details>,
    /// Sections folded to their header.
    collapsed_groups: HashSet<Group>,
    /// Real parent links of the last sample (start times checked), for ending a whole tree.
    children: HashMap<Pid, Vec<Pid>>,
}

impl Core {
    pub fn new(sort: Sort, filter: impl Into<String>) -> Self {
        Self {
            sort,
            filter: filter.into().to_lowercase(),
            ..Self::default()
        }
    }

    pub fn sort(&self) -> Sort {
        self.sort
    }

    pub fn set_sort(&mut self, sort: Sort) {
        self.sort = sort;
    }

    pub fn set_filter(&mut self, filter: &str) {
        self.filter = filter.to_lowercase();
    }

    /// Expands or collapses a process's subtree. Unknown PIDs are ignored.
    pub fn toggle(&mut self, pid: Pid) {
        if let Some(&start) = self.starts.get(&pid) {
            let key = (pid, start);
            if !self.expanded.remove(&key) {
                self.expanded.insert(key);
            }
        }
    }

    /// Folds or unfolds a section of the table. Searching shows every section.
    pub fn toggle_group(&mut self, group: Group) {
        if !self.collapsed_groups.remove(&group) {
            self.collapsed_groups.insert(group);
        }
    }
    /// The live process with this PID, if its start time ends with these 31 bits (the UI keeps
    /// only that much). `None` if the PID is gone or now names another process.
    pub fn identify(&self, pid: Pid, start_low_bits: u32) -> Option<Target> {
        let &start_time = self.starts.get(&pid)?;
        (start_time & 0x7fff_ffff == u64::from(start_low_bits))
            .then_some(Target { pid, start_time })
    }

    /// The process `target` names and all its descendants, children before their parents (the
    /// order to end them in). Empty if `target` is no longer the live process with that PID.
    pub fn subtree(&self, target: Target) -> Vec<Target> {
        if self.starts.get(&target.pid) != Some(&target.start_time) {
            return Vec::new();
        }
        let mut order = Vec::new();
        // Iterative post-order; `seen` guards against parent loops.
        let mut seen = HashSet::new();
        let mut stack = vec![(target.pid, false)];
        while let Some((pid, visited)) = stack.pop() {
            if visited {
                if let Some(&start_time) = self.starts.get(&pid) {
                    order.push(Target { pid, start_time });
                }
            } else if seen.insert(pid) {
                stack.push((pid, true));
                if let Some(kids) = self.children.get(&pid) {
                    stack.extend(kids.iter().map(|&k| (k, false)));
                }
            }
        }
        order
    }

    /// PIDs in `sample` whose details were never set: ask the probe for these.
    pub fn missing_details(&self, sample: &Sample) -> Vec<Pid> {
        sample
            .processes
            .iter()
            .filter(|p| !self.details.contains_key(&(p.pid, p.start_time)))
            .map(|p| p.pid)
            .collect()
    }

    /// Records details for a process, including empty ones for processes that cannot be read,
    /// so they are not asked for again.
    pub fn set_details(&mut self, key: ProcKey, details: Details) {
        self.details.insert(key, details);
    }

    pub fn update(&mut self, sample: &Sample) -> Frame {
        let mut frame = Frame {
            process_count: sample.processes.len(),
            ..Frame::default()
        };

        self.next.clear();
        self.starts.clear();
        let mut nodes = Vec::with_capacity(sample.processes.len());
        for p in &sample.processes {
            let key = (p.pid, p.start_time);
            let cpu_percent = match self.prev.get(&key) {
                // Not measured again since last tick: keep the last value.
                Some(prev) if prev.at_ns == p.measured_at_ns => prev.percent,
                Some(prev) => cpu_percent(
                    prev.cpu_ns,
                    p.cpu_time_ns,
                    p.measured_at_ns.saturating_sub(prev.at_ns),
                    sample.logical_cores,
                ),
                None => None,
            };
            self.next.insert(
                key,
                Measure {
                    cpu_ns: p.cpu_time_ns,
                    at_ns: p.measured_at_ns,
                    percent: cpu_percent,
                },
            );
            self.starts.insert(p.pid, p.start_time);
            if p.pid == sample.self_pid {
                frame.self_cpu_percent = cpu_percent;
                frame.self_memory_bytes = p.memory_bytes;
            }
            nodes.push(Node {
                p,
                cpu: cpu_percent,
            });
        }
        std::mem::swap(&mut self.prev, &mut self.next);
        // A parent is trusted only if it started no later than its child (PIDs get reused).
        self.children.clear();
        for p in &sample.processes {
            if let Some(parent) = p.parent
                && parent != p.pid
                && self.starts.get(&parent).is_some_and(|&s| s <= p.start_time)
            {
                self.children.entry(parent).or_default().push(p.pid);
            }
        }
        // Forget what belongs to processes that exited.
        let starts = &self.starts;
        self.expanded
            .retain(|(pid, start)| starts.get(pid) == Some(start));
        self.details
            .retain(|(pid, start), _| starts.get(pid) == Some(start));

        let filter = self.filter.as_str();
        let details = &self.details;
        let tree = tree::build(
            &nodes,
            sample.shell_pid,
            &|p| matches_filter(p, details.get(&(p.pid, p.start_time)), filter),
            self.sort,
            &self.expanded,
            &self.collapsed_groups,
        );
        frame.matching_count = tree.matching;
        let total_memory = sample.memory_total_bytes;
        let row = |kind, node: usize, depth, figures: Figures, count, expanded| {
            let p = nodes[node].p;
            Row {
                icon: details.get(&(p.pid, p.start_time)).and_then(|d| d.icon),
                tree_size: (self
                    .subtree(Target {
                        pid: p.pid,
                        start_time: p.start_time,
                    })
                    .len() as u32)
                    .saturating_sub(1),
                own: p.pid == sample.self_pid,
                publisher: details
                    .get(&(p.pid, p.start_time))
                    .and_then(|d| d.publisher.clone()),
                kind,
                pid: p.pid,
                start_time: p.start_time,
                // Apps read best by their product name ("Brave Browser"); the rest by file name.
                name: match details
                    .get(&(p.pid, p.start_time))
                    .and_then(|d| d.description.as_deref())
                {
                    Some(description) if tree.group[node] == Group::Apps => description.to_owned(),
                    _ => p.name.clone(),
                },
                cpu_percent: figures.cpu,
                memory_bytes: figures.memory,
                cpu_heat: figures.cpu.map_or(0, |c| heat(c, CPU_HEAT)),
                memory_heat: heat(memory_share(figures.memory, total_memory), MEMORY_HEAT),
                depth,
                count,
                expanded,
            }
        };
        frame.rows = tree
            .entries
            .iter()
            .map(|entry| match *entry {
                Entry::Group {
                    group,
                    count,
                    collapsed,
                } => Row {
                    kind: RowKind::Group(group),
                    pid: 0,
                    start_time: 0,
                    name: String::new(),
                    cpu_percent: None,
                    memory_bytes: 0,
                    cpu_heat: 0,
                    memory_heat: 0,
                    depth: 0,
                    count: count as u32,
                    expanded: !collapsed,
                    icon: None,
                    publisher: None,
                    tree_size: 0,
                    own: false,
                },
                Entry::Process { node, depth, total } => {
                    let p = nodes[node].p;
                    let figures = if total {
                        tree.totals[node]
                    } else {
                        Figures {
                            cpu: nodes[node].cpu,
                            memory: p.memory_bytes,
                        }
                    };
                    let expanded = self.expanded.contains(&(p.pid, p.start_time));
                    let count = if total { tree.descendants[node] } else { 0 };
                    row(RowKind::Process, node, depth, figures, count, expanded)
                }
                Entry::Itself { node, depth } => {
                    let figures = Figures {
                        cpu: nodes[node].cpu,
                        memory: nodes[node].p.memory_bytes,
                    };
                    row(RowKind::Itself, node, depth, figures, 0, false)
                }
            })
            .collect();
        frame
    }
}

/// Memory use as a percentage of physical memory; 0 when the total is unknown.
fn memory_share(bytes: u64, total: u64) -> f32 {
    if total == 0 {
        return 0.0;
    }
    (bytes as f64 / total as f64 * 100.0) as f32
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

/// Search by name, PID, publisher or description.
fn matches_filter(p: &ProcSample, details: Option<&Details>, filter: &str) -> bool {
    if filter.is_empty() {
        return true;
    }
    if let Ok(pid) = filter.parse::<Pid>()
        && pid == p.pid
    {
        return true;
    }
    let in_details = |field: fn(&Details) -> &Option<Arc<str>>| {
        details
            .and_then(|d| field(d).as_deref())
            .is_some_and(|text| contains_ignore_case(text, filter))
    };
    contains_ignore_case(&p.name, filter)
        || in_details(|d| &d.publisher)
        || in_details(|d| &d.description)
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
            measured_at_ns: 0,
            session: None,
            has_window: false,
        }
    }

    /// Processes without a measurement time are measured at the tick.
    fn sample(at_ns: u64, mut processes: Vec<ProcSample>) -> Sample {
        for p in &mut processes {
            if p.measured_at_ns == 0 {
                p.measured_at_ns = at_ns;
            }
        }
        Sample {
            at_ns,
            logical_cores: 4,
            self_pid: 1,
            processes,
            ..Sample::default()
        }
    }

    /// The process row of `pid` (not a group header, not an "itself" line).
    fn row(frame: &Frame, pid: Pid) -> &Row {
        frame
            .rows
            .iter()
            .find(|r| r.kind == RowKind::Process && r.pid == pid)
            .expect("row")
    }

    /// PIDs of the process rows, in display order.
    fn pids(frame: &Frame) -> Vec<Pid> {
        frame
            .rows
            .iter()
            .filter(|r| !matches!(r.kind, RowKind::Group(_)))
            .map(|r| r.pid)
            .collect()
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
    fn slow_measurements_keep_their_value_between_refreshes() {
        let mut core = Core::default();
        let stale = |cpu, at| ProcSample {
            measured_at_ns: at,
            ..proc(9, 10, "service", cpu, 1)
        };
        core.update(&sample(SEC, vec![stale(0, SEC)]));
        // Refreshed at 3 s: 2 s of CPU over 2 s on 4 cores is 25 %.
        let frame = core.update(&sample(3 * SEC, vec![stale(2 * SEC, 3 * SEC)]));
        assert_eq!(row(&frame, 9).cpu_percent, Some(25.0));
        // Not measured again at 4 s: the value is kept, not dropped to 0.
        let frame = core.update(&sample(4 * SEC, vec![stale(2 * SEC, 3 * SEC)]));
        assert_eq!(row(&frame, 9).cpu_percent, Some(25.0));
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
    fn heat_levels() {
        assert_eq!(heat(0.0, CPU_HEAT), 0);
        assert_eq!(heat(2.0, CPU_HEAT), 1);
        assert_eq!(heat(29.9, CPU_HEAT), 2);
        assert_eq!(heat(80.0, CPU_HEAT), 3);
        let mut core = Core::default();
        let mut s = sample(
            SEC,
            vec![proc(1, 0, "big", 0, 16), proc(2, 0, "small", 0, 1)],
        );
        s.memory_total_bytes = 100;
        let frame = core.update(&s);
        assert_eq!(row(&frame, 1).memory_heat, 3);
        assert_eq!(row(&frame, 2).memory_heat, 0);
        assert_eq!(row(&frame, 1).cpu_heat, 0, "no CPU % yet, no heat");
    }

    #[test]
    fn header_clicks_flip_or_reset_the_direction() {
        let cpu = Sort::default();
        assert!(!cpu.ascending);
        let flipped = cpu.clicked(SortKey::Cpu);
        assert_eq!((flipped.key, flipped.ascending), (SortKey::Cpu, true));
        let name = flipped.clicked(SortKey::Name);
        assert_eq!((name.key, name.ascending), (SortKey::Name, true));
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
        assert_eq!(
            pids(&core.update(&sample(SEC, processes.clone()))),
            [1, 3, 2]
        );

        let mut core = Core::new(
            Sort {
                key: SortKey::Name,
                ascending: true,
            },
            "CODE",
        );
        let frame = core.update(&sample(SEC, processes.clone()));
        assert_eq!(pids(&frame), [2, 3]);
        assert_eq!(frame.process_count, 3);

        let mut core = Core::new(Sort::default(), "3");
        assert_eq!(pids(&core.update(&sample(SEC, processes))), [3]);
    }

    /// shell(10) starts app(20, window) which starts helper(21) and worker(22); worker starts
    /// grandchild(23). shell also starts tool(30, no window). svc(40) and svchost(41) are in
    /// session 0, svchost started by svc.
    fn desktop() -> Vec<ProcSample> {
        let p = |pid, parent, name: &str, session, window, mem| ProcSample {
            parent,
            session: Some(session),
            has_window: window,
            ..proc(pid, 100, name, 0, mem)
        };
        vec![
            p(10, None, "explorer", 1, true, 50),
            p(20, Some(10), "app", 1, true, 100),
            p(21, Some(20), "helper", 1, false, 10),
            p(22, Some(20), "worker", 1, false, 20),
            p(23, Some(22), "grandchild", 1, false, 5),
            p(30, Some(10), "tool", 1, false, 7),
            p(40, None, "svc", 0, false, 3),
            p(41, Some(40), "svchost", 0, false, 4),
        ]
    }

    fn desktop_sample() -> Sample {
        Sample {
            shell_pid: Some(10),
            ..sample(SEC, desktop())
        }
    }

    fn by_memory() -> Core {
        Core::new(
            Sort {
                key: SortKey::Memory,
                ascending: false,
            },
            "",
        )
    }

    #[test]
    fn groups_apps_background_and_system() {
        let frame = by_memory().update(&desktop_sample());
        let groups: Vec<(Group, u32)> = frame
            .rows
            .iter()
            .filter_map(|r| match r.kind {
                RowKind::Group(g) => Some((g, r.count)),
                _ => None,
            })
            .collect();
        // The app's helpers belong to the app; the tool started by the shell does not.
        assert_eq!(
            groups,
            [(Group::Apps, 5), (Group::Background, 1), (Group::System, 2)]
        );
        // Collapsed: apps and their subtree totals, then background, then flat system.
        assert_eq!(pids(&frame), [20, 10, 30, 41, 40]);
        let app = row(&frame, 20);
        assert_eq!((app.memory_bytes, app.count), (135, 3));
        assert_eq!(
            row(&frame, 10).count,
            0,
            "the shell does not adopt the apps it starts"
        );
    }

    #[test]
    fn folded_sections_keep_their_header_only() {
        let mut core = by_memory();
        core.toggle_group(Group::System);
        let frame = core.update(&desktop_sample());
        assert_eq!(pids(&frame), [20, 10, 30], "no system processes");
        let header = frame.rows.last().expect("system header");
        assert_eq!(
            (header.kind, header.count, header.expanded),
            (RowKind::Group(Group::System), 2, false)
        );
        // Searching shows matches even in a folded section.
        core.set_filter("svc");
        assert_eq!(pids(&core.update(&desktop_sample())), [41, 40]);
    }

    #[test]
    fn expanding_shows_the_process_itself_then_its_children() {
        let mut core = by_memory();
        core.update(&desktop_sample());
        core.toggle(20);
        let frame = core.update(&desktop_sample());
        let apps: Vec<(Pid, RowKind, u8)> = frame
            .rows
            .iter()
            .take_while(|r| r.kind != RowKind::Group(Group::Background))
            .skip(1)
            .map(|r| (r.pid, r.kind, r.depth))
            .collect();
        assert_eq!(
            apps,
            [
                (20, RowKind::Process, 0),
                (20, RowKind::Itself, 1),
                (22, RowKind::Process, 1),
                (21, RowKind::Process, 1),
                (10, RowKind::Process, 0),
            ]
        );
        assert!(row(&frame, 20).expanded);
        // The worker's subtree total includes its child; collapsing hides it again.
        assert_eq!(row(&frame, 22).memory_bytes, 25);
        core.toggle(20);
        assert_eq!(pids(&core.update(&desktop_sample()))[..2], [20, 10]);
    }

    #[test]
    fn stale_parent_is_ignored() {
        // PID 22's parent 20 started after it: 20 is a reused PID, not its parent.
        let mut processes = desktop();
        processes[1].start_time = 200;
        let frame = by_memory().update(&Sample {
            shell_pid: Some(10),
            ..sample(SEC, processes)
        });
        assert_eq!(row(&frame, 20).count, 0);
    }

    #[test]
    fn parent_loops_still_show_every_process() {
        let mut a = proc(1, 100, "a", 0, 1);
        let mut b = proc(2, 100, "b", 0, 1);
        a.parent = Some(2);
        b.parent = Some(1);
        // The loop is cut: one of them becomes the root, the other its child.
        let mut core = Core::default();
        let frame = core.update(&sample(SEC, vec![a.clone(), b.clone()]));
        assert_eq!(pids(&frame), [1]);
        assert_eq!(row(&frame, 1).count, 1);
        core.toggle(1);
        let frame = core.update(&sample(SEC, vec![a, b]));
        assert_eq!(pids(&frame), [1, 1, 2]);
    }

    #[test]
    fn details_feed_icons_and_search() {
        let mut core = by_memory();
        let s = desktop_sample();
        assert_eq!(core.missing_details(&s).len(), 8);
        core.set_details(
            (20, 100),
            Details {
                icon: Some(3),
                publisher: Some("Lagoon Labs".into()),
                description: Some("App Deluxe".into()),
            },
        );
        for pid in core.missing_details(&s) {
            core.set_details((pid, 100), Details::default());
        }
        assert!(core.missing_details(&s).is_empty());
        let frame = core.update(&s);
        assert_eq!(row(&frame, 20).icon, Some(3));
        assert_eq!(
            row(&frame, 20).name,
            "App Deluxe",
            "apps show their product name"
        );
        assert_eq!(row(&frame, 30).name, "tool");
        assert_eq!(frame.matching_count, 8);
        assert_eq!(row(&frame, 30).icon, None);

        core.set_filter("lagoon");
        assert_eq!(pids(&core.update(&s)), [20]);
        core.set_filter("deluxe");
        assert_eq!(core.update(&s).matching_count, 1);
        assert_eq!(pids(&core.update(&s)), [20]);

        // A process that exits loses its details; a new one with the same PID needs them again.
        let mut next = desktop();
        next[1].start_time = 300;
        let next = Sample {
            shell_pid: Some(10),
            ..sample(2 * SEC, next)
        };
        core.update(&next);
        assert_eq!(core.missing_details(&next), [20]);
    }

    #[test]
    fn tree_size_counts_what_end_tree_ends() {
        // The app starts another windowed app (24): the display tree shows it apart, but ending
        // the app's tree ends it too, so the confirmation must count it.
        let mut processes = desktop();
        processes.push(ProcSample {
            parent: Some(20),
            session: Some(1),
            has_window: true,
            ..proc(24, 100, "child-app", 0, 1)
        });
        let s = Sample {
            shell_pid: Some(10),
            self_pid: 21,
            ..sample(SEC, processes)
        };
        let mut core = by_memory();
        let frame = core.update(&s);
        let app = row(&frame, 20);
        let real = core
            .subtree(Target {
                pid: 20,
                start_time: 100,
            })
            .len() as u32
            - 1;
        assert_eq!(app.count, 3, "display tree: helper, worker, grandchild");
        assert_eq!(app.tree_size, real);
        assert_eq!(real, 4);
        core.toggle(20);
        let frame = core.update(&s);
        assert!(row(&frame, 21).own);
        assert!(!row(&frame, 20).own);
    }

    #[test]
    fn subtree_lists_children_before_parents() {
        let mut core = by_memory();
        core.update(&desktop_sample());
        let order: Vec<Pid> = core
            .subtree(Target {
                pid: 20,
                start_time: 100,
            })
            .iter()
            .map(|t| t.pid)
            .collect();
        // grandchild before worker, both children before the app.
        let at = |pid| order.iter().position(|&p| p == pid).expect("listed");
        assert_eq!(order.len(), 4);
        assert!(at(23) < at(22) && at(22) < at(20) && at(21) < at(20));
        assert_eq!(order.last(), Some(&20));
        // A stale identity gets nothing.
        assert!(
            core.subtree(Target {
                pid: 20,
                start_time: 99
            })
            .is_empty()
        );
    }

    #[test]
    fn search_shows_matches_flat() {
        let mut core = by_memory();
        core.set_filter("er");
        let frame = core.update(&desktop_sample());
        // helper, worker (apps), explorer (app); flat with their own figures.
        assert_eq!(pids(&frame), [10, 22, 21]);
        assert_eq!(row(&frame, 22).memory_bytes, 20);
        assert!(frame.rows.iter().all(|r| r.depth == 0));
    }
}
