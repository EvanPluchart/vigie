//! Process model: CPU % from deltas, tree, grouping, history, diagnostics.
//! No UI and no OS calls here. See docs/ARCHITECTURE.md.

pub mod alerts;
pub mod diagnose;
pub mod history;
pub mod perf;
pub mod settings;
pub mod tools;
pub mod tracker;
pub mod update;
mod tree;
pub mod users;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use tracker::{Candidate, Tracker};
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
    /// All I/O, in bytes per second.
    Io,
    Threads,
    Handles,
    /// Suspended processes first when descending.
    Status,
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

/// Names from A to Z, as Task Manager opens: the table stays still while figures change.
impl Default for Sort {
    fn default() -> Self {
        Self {
            key: SortKey::Name,
            ascending: true,
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
    /// All I/O (files, devices, network), in bytes per second. `None` until measured twice.
    pub io_bytes_per_s: Option<f32>,
    pub threads: u32,
    pub handles: u32,
    pub suspended: bool,
    /// The account the process runs as.
    pub user: Option<Arc<str>>,
    /// What the executable says it is ("Brave Browser").
    pub description: Option<Arc<str>>,
    /// 0 (cool) to 3 (hot), for background tints.
    pub cpu_heat: u8,
    pub memory_heat: u8,
    pub io_heat: u8,
    /// Indentation level in the tree.
    pub depth: u8,
    /// Group: processes in it. Process: its descendants.
    pub count: u32,
    pub expanded: bool,
    /// Icon of the process's executable, as an index chosen by the caller of `set_details`.
    pub icon: Option<u32>,
    pub publisher: Option<Arc<str>>,
    /// The executable's file name ("brave.exe"), even when `name` is a product name.
    pub file: String,
    /// Full path of the executable, when it could be read.
    pub path: Option<Arc<str>>,
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
    pub path: Option<Arc<str>>,
    pub user: Option<Arc<str>>,
}

/// Heat thresholds: CPU in percent of the whole machine, memory in percent of physical memory.
/// One busy core on a 16-thread machine is about 6 %, so it reads as warm, not hot.
pub const CPU_HEAT: [f32; 3] = [2.0, 10.0, 30.0];
pub const MEMORY_HEAT: [f32; 3] = [2.0, 5.0, 15.0];
/// I/O in megabytes per second: a copy or a download in progress reads as warm.
pub const IO_HEAT: [f32; 3] = [1.0, 10.0, 50.0];

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
    io_bytes: u64,
    io_at_ns: u64,
    io_rate: Option<f32>,
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
    /// One flat list instead of sections and trees.
    flat: bool,
    /// Rows named by their executable file, never by product name (Details).
    file_names: bool,
    /// Real parent links of the last sample (start times checked), for ending a whole tree.
    children: HashMap<Pid, Vec<Pid>>,
    /// Descendants of each parent in `children` (absent: none), counted once per sample.
    descendants: HashMap<Pid, u32>,
    /// History of the top consumers and of the selected process.
    tracker: Tracker,
    /// Section of each process of the last sample, in sample order.
    groups: Vec<Group>,
}

/// A process as the table names it, for the other views (ports, files).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Label<'a> {
    pub target: Target,
    pub name: &'a str,
    pub group: Group,
    /// Icon id, as in `Row::icon`.
    pub icon: Option<u32>,
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

    /// Shows one flat list (true) or sections with process trees.
    pub fn set_flat(&mut self, flat: bool) {
        self.flat = flat;
    }

    pub fn flat(&self) -> bool {
        self.flat
    }

    /// Names every row by its executable file ("AppleMusic.exe"), as Task Manager's Details tab.
    pub fn set_file_names(&mut self, file_names: bool) {
        self.file_names = file_names;
    }

    /// Expands every process that has children, and unfolds every section.
    pub fn expand_all(&mut self) {
        let starts = &self.starts;
        self.expanded.extend(
            self.children
                .keys()
                .filter_map(|pid| starts.get(pid).map(|&start| (*pid, start))),
        );
        self.collapsed_groups.clear();
    }

    /// Collapses every process tree (sections stay as they are).
    pub fn collapse_all(&mut self) {
        self.expanded.clear();
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

    /// Fills `descendants` bottom-up in one pass over `children`, instead of walking each
    /// process's subtree.
    fn count_descendants(&mut self) {
        self.descendants.clear();
        // Iterative post-order; `open` guards against parent loops.
        let mut open = HashSet::new();
        let mut stack = Vec::new();
        for &root in self.children.keys() {
            if self.descendants.contains_key(&root) {
                continue;
            }
            stack.push((root, false));
            while let Some((pid, visited)) = stack.pop() {
                let kids = self.children.get(&pid).map(Vec::as_slice).unwrap_or_default();
                if visited {
                    let count = kids
                        .iter()
                        .map(|k| 1 + self.descendants.get(k).copied().unwrap_or(0))
                        .sum();
                    self.descendants.insert(pid, count);
                } else if !self.descendants.contains_key(&pid) && open.insert(pid) {
                    stack.push((pid, true));
                    stack.extend(kids.iter().map(|&k| (k, false)));
                }
            }
        }
    }

    /// Processes "End process tree" on `pid` would end besides it.
    pub fn tree_size(&self, pid: Pid) -> u32 {
        self.descendants.get(&pid).copied().unwrap_or(0)
    }

    /// The processes of `sample` (the last one given to `update`), named as the table names
    /// them (product names for apps), with their sections.
    pub fn labels<'a>(&'a self, sample: &'a Sample) -> impl Iterator<Item = Label<'a>> + 'a {
        sample.processes.iter().enumerate().map(move |(i, p)| {
            let group = self.groups.get(i).copied().unwrap_or(Group::All);
            let details = self.details.get(&(p.pid, p.start_time));
            let description = details.and_then(|d| d.description.as_deref());
            Label {
                target: Target {
                    pid: p.pid,
                    start_time: p.start_time,
                },
                name: match description {
                    Some(d) if group == Group::Apps => d,
                    _ => &p.name,
                },
                group,
                icon: details.and_then(|d| d.icon),
            }
        })
    }

    /// CPU % of a process at the last update.
    pub(crate) fn cpu_of(&self, key: ProcKey) -> Option<f32> {
        self.prev.get(&key).and_then(|m| m.percent)
    }

    /// Executable path of a process, when its details were read.
    pub(crate) fn path_of(&self, key: ProcKey) -> Option<Arc<str>> {
        self.details.get(&key).and_then(|d| d.path.clone())
    }

    /// Per-process histories.
    pub fn tracker(&self) -> &Tracker {
        &self.tracker
    }

    /// Keeps a history of this process while the user has it selected (`None`: nothing
    /// selected). Unknown or stale processes are ignored.
    pub fn select(&mut self, process: Option<(Pid, u32)>) {
        let target = process.and_then(|(pid, bits)| self.identify(pid, bits));
        self.tracker.pin(target.map(|t| (t.pid, t.start_time)));
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
            let io_rate = match self.prev.get(&key) {
                _ if p.io_measured_at_ns == 0 => None,
                Some(prev) if prev.io_at_ns == p.io_measured_at_ns => prev.io_rate,
                Some(prev) if prev.io_at_ns > 0 && p.io_measured_at_ns > prev.io_at_ns => {
                    let seconds = (p.io_measured_at_ns - prev.io_at_ns) as f64 / 1e9;
                    Some((p.io_bytes.saturating_sub(prev.io_bytes) as f64 / seconds) as f32)
                }
                _ => None,
            };
            self.next.insert(
                key,
                Measure {
                    cpu_ns: p.cpu_time_ns,
                    at_ns: p.measured_at_ns,
                    percent: cpu_percent,
                    io_bytes: p.io_bytes,
                    io_at_ns: p.io_measured_at_ns,
                    io_rate,
                },
            );
            self.starts.insert(p.pid, p.start_time);
            self.tracker
                .record(key, sample.at_ns, cpu_percent, p.memory_bytes);
            if p.pid == sample.self_pid {
                frame.self_cpu_percent = cpu_percent;
                frame.self_memory_bytes = p.memory_bytes;
            }
            nodes.push(Node {
                p,
                cpu: cpu_percent,
                io: io_rate,
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
        self.count_descendants();
        // Forget what belongs to processes that exited.
        let starts = &self.starts;
        self.expanded
            .retain(|(pid, start)| starts.get(pid) == Some(start));
        self.details
            .retain(|(pid, start), _| starts.get(pid) == Some(start));
        self.tracker
            .retain(|(pid, start)| starts.get(&pid) == Some(&start));

        let filter = self.filter.as_str();
        let details = &self.details;
        let file_names = self.file_names;
        let tree = tree::build(
            &nodes,
            sample.shell_pid,
            &|p| matches_filter(p, details.get(&(p.pid, p.start_time)), filter),
            &|p| {
                (!file_names)
                    .then(|| details.get(&(p.pid, p.start_time))?.description.as_deref())
                    .flatten()
            },
            self.sort,
            &self.expanded,
            &self.collapsed_groups,
            self.flat,
        );
        frame.matching_count = tree.matching;
        self.groups.clear();
        self.groups.extend_from_slice(&tree.group);
        let total_memory = sample.memory_total_bytes;
        let display_name = |node: usize| -> &str {
            let p = nodes[node].p;
            // Apps read best by their product name ("Brave Browser"); the rest by file name.
            match details
                .get(&(p.pid, p.start_time))
                .and_then(|d| d.description.as_deref())
            {
                Some(description) if tree.group[node] == Group::Apps && !self.file_names => {
                    description
                }
                _ => &p.name,
            }
        };
        if self.tracker.wants_round(sample.at_ns) {
            let candidates: Vec<Candidate> = nodes
                .iter()
                .enumerate()
                .filter(|(_, n)| n.p.pid != sample.self_pid)
                .map(|(i, n)| Candidate {
                    key: (n.p.pid, n.p.start_time),
                    name: display_name(i),
                    group: tree.group[i],
                    cpu: n.cpu,
                    memory: n.p.memory_bytes,
                })
                .collect();
            self.tracker.round(sample.at_ns, total_memory, &candidates);
        }
        let row = |kind, node: usize, depth, figures: Figures, count, expanded| {
            let p = nodes[node].p;
            Row {
                icon: details.get(&(p.pid, p.start_time)).and_then(|d| d.icon),
                tree_size: self.tree_size(p.pid),
                own: p.pid == sample.self_pid,
                publisher: details
                    .get(&(p.pid, p.start_time))
                    .and_then(|d| d.publisher.clone()),
                file: p.name.clone(),
                path: details
                    .get(&(p.pid, p.start_time))
                    .and_then(|d| d.path.clone()),
                kind,
                pid: p.pid,
                start_time: p.start_time,
                name: display_name(node).to_owned(),
                cpu_percent: figures.cpu,
                memory_bytes: figures.memory,
                io_bytes_per_s: figures.io,
                threads: figures.threads,
                handles: figures.handles,
                suspended: p.suspended,
                user: details
                    .get(&(p.pid, p.start_time))
                    .and_then(|d| d.user.clone()),
                description: details
                    .get(&(p.pid, p.start_time))
                    .and_then(|d| d.description.clone()),
                cpu_heat: figures.cpu.map_or(0, |c| heat(c, CPU_HEAT)),
                memory_heat: heat(memory_share(figures.memory, total_memory), MEMORY_HEAT),
                io_heat: figures.io.map_or(0, |b| heat(b / 1_048_576.0, IO_HEAT)),
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
                    io_bytes_per_s: None,
                    threads: 0,
                    handles: 0,
                    suspended: false,
                    user: None,
                    description: None,
                    cpu_heat: 0,
                    memory_heat: 0,
                    io_heat: 0,
                    depth: 0,
                    count: count as u32,
                    expanded: !collapsed,
                    icon: None,
                    publisher: None,
                    file: String::new(),
                    path: None,
                    tree_size: 0,
                    own: false,
                },
                Entry::Process { node, depth, total } => {
                    let p = nodes[node].p;
                    let figures = if total {
                        tree.totals[node]
                    } else {
                        Figures::of(&nodes[node])
                    };
                    let expanded = self.expanded.contains(&(p.pid, p.start_time));
                    let count = if total { tree.descendants[node] } else { 0 };
                    row(RowKind::Process, node, depth, figures, count, expanded)
                }
                Entry::Itself { node, depth } => {
                    let figures = Figures::of(&nodes[node]);
                    row(RowKind::Itself, node, depth, figures, 0, false)
                }
            })
            .collect();
        frame
    }
}

/// Memory use as a percentage of physical memory; 0 when the total is unknown.
pub fn memory_share(bytes: u64, total: u64) -> f32 {
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
            ..ProcSample::default()
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

    #[test]
    fn the_busiest_processes_get_a_history_but_not_vigie() {
        let mut core = Core::new(Sort::default(), "");
        let procs = |cpu: u64| {
            vec![
                proc(1, 1, "vigie", cpu, 900 << 20),
                proc(2, 1, "big", 0, 500 << 20),
                proc(3, 1, "busy", cpu, 1 << 20),
            ]
        };
        core.update(&sample(SEC, procs(0)));
        core.update(&sample(2 * SEC, procs(SEC)));
        assert!(core.tracker().get((2, 1)).is_some(), "largest memory");
        assert!(core.tracker().get((1, 1)).is_none(), "never Vigie itself");
        // Process 3 had no CPU figure on the first tick; the next round, 10 s on, takes it.
        core.update(&sample(12 * SEC, procs(4 * SEC)));
        let busy = core.tracker().get((3, 1)).expect("busiest CPU");
        assert_eq!(busy.name, "busy");
        core.update(&sample(13 * SEC, procs(5 * SEC)));
        assert_eq!(busy_cpu(&core), Some(25.0));
    }

    fn busy_cpu(core: &Core) -> Option<f32> {
        core.tracker().get((3, 1)).and_then(|t| t.cpu.last())
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
        assert_eq!(
            Sort::default(),
            Sort {
                key: SortKey::Name,
                ascending: true
            }
        );
        let cpu = Sort {
            key: SortKey::Cpu,
            ascending: false,
        };
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
    fn io_rates_threads_and_status() {
        let io = |pid, bytes, at_s: u64, threads| ProcSample {
            io_bytes: bytes,
            io_measured_at_ns: at_s * SEC,
            threads,
            handles: threads * 10,
            suspended: pid == 2,
            ..proc(pid, 0, "p", 0, 1)
        };
        let mut core = Core::new(
            Sort {
                key: SortKey::Io,
                ascending: false,
            },
            "",
        );
        let first = core.update(&sample(SEC, vec![io(1, 0, 1, 4), io(2, 0, 1, 2)]));
        assert_eq!(
            row(&first, 1).io_bytes_per_s,
            None,
            "one reading gives no rate"
        );
        let frame = core.update(&sample(
            3 * SEC,
            vec![io(1, 4_000_000, 3, 4), io(2, 1_000_000, 3, 2)],
        ));
        assert_eq!(pids(&frame), [1, 2], "busiest I/O first");
        assert_eq!(row(&frame, 1).io_bytes_per_s, Some(2_000_000.0));
        assert_eq!((row(&frame, 1).threads, row(&frame, 1).handles), (4, 40));
        assert!(row(&frame, 2).suspended && !row(&frame, 1).suspended);
        core.set_sort(Sort {
            key: SortKey::Status,
            ascending: false,
        });
        let frame = core.update(&sample(
            4 * SEC,
            vec![io(1, 4_000_000, 3, 4), io(2, 1_000_000, 3, 2)],
        ));
        assert_eq!(pids(&frame), [2, 1], "suspended first");
        assert_eq!(
            row(&frame, 1).io_bytes_per_s,
            Some(2_000_000.0),
            "an unchanged reading keeps its rate"
        );
    }

    #[test]
    fn flat_lists_every_process_alone() {
        let mut core = by_memory();
        core.set_flat(true);
        let frame = core.update(&desktop_sample());
        // No section headers, no nesting, own figures: the app is 100, not its subtree's 135.
        assert_eq!(pids(&frame), [20, 10, 22, 21, 30, 23, 41, 40]);
        assert!(
            frame
                .rows
                .iter()
                .all(|r| r.kind == RowKind::Process && r.depth == 0)
        );
        assert_eq!(row(&frame, 20).memory_bytes, 100);
        core.set_filter("svc");
        assert_eq!(pids(&core.update(&desktop_sample())), [41, 40]);
    }

    #[test]
    fn expand_all_then_collapse_all() {
        let mut core = by_memory();
        core.toggle_group(Group::System);
        core.update(&desktop_sample());
        core.expand_all();
        let frame = core.update(&desktop_sample());
        assert!(row(&frame, 20).expanded && row(&frame, 22).expanded);
        assert!(pids(&frame).contains(&23), "the grandchild shows");
        assert!(pids(&frame).contains(&41), "folded sections open too");
        core.collapse_all();
        let frame = core.update(&desktop_sample());
        assert!(!pids(&frame).contains(&23));
    }

    #[test]
    fn rows_name_the_executable_file() {
        let mut core = by_memory();
        let s = desktop_sample();
        core.set_details(
            (20, 100),
            Details {
                description: Some("App Deluxe".into()),
                path: Some(r"C:\Apps\app.exe".into()),
                ..Details::default()
            },
        );
        let frame = core.update(&s);
        let app = row(&frame, 20);
        assert_eq!(
            (app.name.as_str(), app.file.as_str()),
            ("App Deluxe", "app")
        );
        assert_eq!(app.path.as_deref(), Some(r"C:\Apps\app.exe"));
    }

    #[test]
    fn name_sort_follows_the_names_shown() {
        let mut processes = desktop();
        processes.push(ProcSample {
            parent: Some(10),
            session: Some(1),
            has_window: true,
            ..proc(50, 100, "zeta", 0, 1)
        });
        let mut core = Core::new(
            Sort {
                key: SortKey::Name,
                ascending: true,
            },
            "",
        );
        let named = |description: &str| Details {
            description: Some(description.into()),
            ..Details::default()
        };
        // app.exe shows as "Zebra", zeta.exe as "Alpha": Alpha comes first.
        core.set_details((20, 100), named("Zebra"));
        core.set_details((50, 100), named("Alpha"));
        let order = pids(&core.update(&Sample {
            shell_pid: Some(10),
            ..sample(SEC, processes)
        }));
        let at = |pid| order.iter().position(|&p| p == pid).unwrap();
        assert!(at(50) < at(20), "{order:?}");
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
                path: None,
                user: None,
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
