//! Groups (apps, background, system), the process tree and the flattened rows the UI shows.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

use crate::{Pid, ProcKey, ProcSample, Sort, SortKey};

/// The sections of the process table, in display order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Group {
    /// Processes with a visible window, and the processes they started.
    Apps,
    /// Other processes in a user session.
    Background,
    /// Services and the system (session 0).
    System,
    /// Systems without session or window information: one flat section.
    All,
}

/// One process with its CPU % for this tick.
pub(crate) struct Node<'a> {
    pub p: &'a ProcSample,
    pub cpu: Option<f32>,
}

/// What a row shows: the process alone, or the total of its subtree.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Figures {
    pub cpu: Option<f32>,
    pub memory: u64,
}

impl Figures {
    fn add(&mut self, other: Figures) {
        self.cpu = match (self.cpu, other.cpu) {
            (Some(a), Some(b)) => Some(a + b),
            (a, b) => a.or(b),
        };
        self.memory += other.memory;
    }
}

/// A flattened row before formatting: a group header, a process, or a process's own line under
/// its expanded subtree.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Entry {
    Group {
        group: Group,
        count: usize,
        collapsed: bool,
    },
    Process {
        node: usize,
        depth: u8,
        total: bool,
    },
    Itself {
        node: usize,
        depth: u8,
    },
}

pub(crate) struct Tree {
    pub entries: Vec<Entry>,
    /// Subtree totals, by node.
    pub totals: Vec<Figures>,
    /// Number of descendants, by node.
    pub descendants: Vec<u32>,
    /// Section of each node.
    pub group: Vec<Group>,
    /// Processes matching the search (all of them when not searching).
    pub matching: usize,
}

/// Deepest ancestor walk; real trees are shallow, this only guards against corrupt parent loops.
const MAX_DEPTH: usize = 64;

pub(crate) fn build(
    nodes: &[Node],
    shell: Option<Pid>,
    filter: &dyn Fn(&ProcSample) -> bool,
    sort: Sort,
    expanded: &HashSet<ProcKey>,
    collapsed: &HashSet<Group>,
) -> Tree {
    let index: HashMap<Pid, usize> = nodes
        .iter()
        .enumerate()
        .map(|(i, n)| (n.p.pid, i))
        .collect();
    // A parent is only trusted if it started before its child: Windows keeps stale parent PIDs
    // that may have been reused by a younger process.
    let parent: Vec<Option<usize>> = nodes
        .iter()
        .map(|n| {
            let &i = index.get(&n.p.parent?)?;
            (nodes[i].p.start_time <= n.p.start_time && nodes[i].p.pid != n.p.pid).then_some(i)
        })
        .collect();
    let has_sessions = nodes
        .iter()
        .any(|n| n.p.session.is_some() || n.p.has_window);
    let group: Vec<Group> = (0..nodes.len())
        .map(|i| group_of(i, nodes, &parent, shell, has_sessions))
        .collect();

    // Nest a process under its parent when both are in the same section, except under the shell
    // (it starts every app), for windowed processes (each is an app of its own) and in the system
    // section (services all descend from services.exe; nesting would hide them).
    let mut nest: Vec<Option<usize>> = (0..nodes.len())
        .map(|i| {
            let p = parent[i]?;
            let ok = Some(nodes[p].p.pid) != shell
                && !nodes[i].p.has_window
                && group[p] == group[i]
                && group[i] != Group::System;
            ok.then_some(p)
        })
        .collect();
    let mut children: Vec<Vec<usize>> = vec![Vec::new(); nodes.len()];
    for (i, p) in nest.iter().enumerate() {
        if let Some(p) = *p {
            children[p].push(i);
        }
    }

    let own: Vec<Figures> = nodes
        .iter()
        .map(|n| Figures {
            cpu: n.cpu,
            memory: n.p.memory_bytes,
        })
        .collect();
    let mut totals = own.clone();
    let mut descendants = vec![0u32; nodes.len()];
    let mut reached = vec![false; nodes.len()];
    let roots: Vec<usize> = (0..nodes.len()).filter(|&i| nest[i].is_none()).collect();
    for i in roots {
        accumulate(
            i,
            &children,
            &own,
            &mut totals,
            &mut descendants,
            &mut reached,
        );
    }
    // Parent loops (possible only with corrupt or reused PIDs) are unreachable from any root:
    // cut them so their processes still show, at the top level.
    for i in 0..nodes.len() {
        if !reached[i] {
            if let Some(p) = nest[i].take() {
                children[p].retain(|&c| c != i);
            }
            accumulate(
                i,
                &children,
                &own,
                &mut totals,
                &mut descendants,
                &mut reached,
            );
        }
    }

    let compare = |a: usize, b: usize, figures: &[Figures]| -> Ordering {
        let (fa, fb) = (figures[a], figures[b]);
        let primary = match sort.key {
            SortKey::Cpu => fa.cpu.unwrap_or(-1.0).total_cmp(&fb.cpu.unwrap_or(-1.0)),
            SortKey::Memory => fa.memory.cmp(&fb.memory),
            SortKey::Name => {
                crate::lowercase(&nodes[a].p.name).cmp(crate::lowercase(&nodes[b].p.name))
            }
            SortKey::Pid => nodes[a].p.pid.cmp(&nodes[b].p.pid),
        };
        let primary = if sort.ascending {
            primary
        } else {
            primary.reverse()
        };
        // Stable tie-break so rows do not jump between ticks.
        primary.then_with(|| nodes[a].p.pid.cmp(&nodes[b].p.pid))
    };

    let matching = nodes.iter().filter(|n| filter(n.p)).count();
    let searching = matching < nodes.len();
    let mut entries = Vec::with_capacity(nodes.len() + 4);
    for section in [Group::Apps, Group::Background, Group::System, Group::All] {
        if searching {
            // Searching shows matches flat, each with its own figures.
            let mut matches: Vec<usize> = (0..nodes.len())
                .filter(|&i| group[i] == section && filter(nodes[i].p))
                .collect();
            if matches.is_empty() {
                continue;
            }
            matches.sort_by(|&a, &b| compare(a, b, &own));
            entries.push(Entry::Group {
                group: section,
                count: matches.len(),
                collapsed: false,
            });
            entries.extend(matches.into_iter().map(|node| Entry::Process {
                node,
                depth: 0,
                total: false,
            }));
            continue;
        }
        let mut roots: Vec<usize> = (0..nodes.len())
            .filter(|&i| group[i] == section && nest[i].is_none())
            .collect();
        if roots.is_empty() {
            continue;
        }
        let count = (0..nodes.len()).filter(|&i| group[i] == section).count();
        let folded = collapsed.contains(&section);
        entries.push(Entry::Group {
            group: section,
            count,
            collapsed: folded,
        });
        if folded {
            continue;
        }
        roots.sort_by(|&a, &b| compare(a, b, &totals));
        for root in roots {
            emit(
                root,
                0,
                nodes,
                &children,
                &totals,
                expanded,
                &compare,
                &mut entries,
            );
        }
    }
    Tree {
        entries,
        totals,
        descendants,
        matching,
        group,
    }
}

fn group_of(
    i: usize,
    nodes: &[Node],
    parent: &[Option<usize>],
    shell: Option<Pid>,
    has_sessions: bool,
) -> Group {
    if !has_sessions {
        return Group::All;
    }
    if nodes[i].p.has_window {
        return Group::Apps;
    }
    // A process started (directly or not) by an app belongs to that app, unless the chain goes
    // through the shell first.
    let mut at = i;
    for _ in 0..MAX_DEPTH {
        let Some(p) = parent[at] else { break };
        if Some(nodes[p].p.pid) == shell {
            break;
        }
        if nodes[p].p.has_window {
            return Group::Apps;
        }
        at = p;
    }
    match nodes[i].p.session {
        Some(0) => Group::System,
        _ => Group::Background,
    }
}

/// Post-order sums of figures and descendant counts.
fn accumulate(
    i: usize,
    children: &[Vec<usize>],
    own: &[Figures],
    totals: &mut [Figures],
    descendants: &mut [u32],
    reached: &mut [bool],
) {
    // Iterative, so a deep or corrupt tree cannot overflow the stack.
    let mut stack = vec![(i, false)];
    while let Some((node, visited)) = stack.pop() {
        if visited {
            let mut total = own[node];
            let mut count = 0;
            for &c in &children[node] {
                total.add(totals[c]);
                count += 1 + descendants[c];
            }
            totals[node] = total;
            descendants[node] = count;
        } else {
            if reached[node] {
                continue;
            }
            reached[node] = true;
            stack.push((node, true));
            stack.extend(children[node].iter().map(|&c| (c, false)));
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn emit(
    node: usize,
    depth: u8,
    nodes: &[Node],
    children: &[Vec<usize>],
    totals: &[Figures],
    expanded: &HashSet<ProcKey>,
    compare: &dyn Fn(usize, usize, &[Figures]) -> Ordering,
    entries: &mut Vec<Entry>,
) {
    let has_children = !children[node].is_empty();
    entries.push(Entry::Process {
        node,
        depth,
        total: has_children,
    });
    let p = nodes[node].p;
    if !has_children || !expanded.contains(&(p.pid, p.start_time)) || depth as usize >= MAX_DEPTH {
        return;
    }
    entries.push(Entry::Itself {
        node,
        depth: depth + 1,
    });
    let mut kids = children[node].clone();
    kids.sort_by(|&a, &b| compare(a, b, totals));
    for kid in kids {
        emit(
            kid,
            depth + 1,
            nodes,
            children,
            totals,
            expanded,
            compare,
            entries,
        );
    }
}
