//! Frames to Slint rows, and patching the table model in place.

use std::collections::HashSet;

use slint::{Image, Model, ModelRc, Rgba8Pixel, SharedPixelBuffer, SharedString, VecModel};
use vigie_core::alerts::Alert;
use vigie_core::diagnose::{Culprit, Finding, Severity, Suggestion};
use vigie_core::history::{History, SECOND, Series, graph_paths};
use vigie_core::perf::{PerfFrame, Performance, Pressure, PressureCause, commit_share};
use vigie_core::tools::{LockRow, PortRow};
use vigie_core::tracker::{Tracked, Tracker};
use vigie_core::{Core, Details, Frame, Group, Row, RowKind};
use vigie_probe::{
    Icon, LockerKind, Protocol, Service, ServiceState, StartType, StartupItem, StartupLocation,
    SystemInfo, TcpState,
};

use crate::{
    AppWindow, CheckSummary, DiskCard, FindingRow, GpuCard, GraphData, LockRowUi, MiniRow,
    PerfView, PortRowUi, ProcRow, ServiceRowUi, StartupRowUi,
};

/// Row kinds as `ui/app.slint` reads them.
const KIND_GROUP: i32 = 0;
const KIND_PROCESS: i32 = 1;
const KIND_ITSELF: i32 = 2;

/// A frame converted on the sampler thread, so the UI thread only patches models.
pub struct UiFrame {
    /// `None` while the process table is hidden: its model is left as it is.
    pub rows: Option<Vec<ProcRow>>,
    pub process_count: SharedString,
    pub matching_count: SharedString,
    pub self_cpu: SharedString,
    /// Machine-wide CPU and memory in use, for the column headers ("34 %").
    pub total_cpu: SharedString,
    pub total_memory: SharedString,
    pub self_memory: SharedString,
    /// Icons the UI has not seen yet, with their index in its icon model. Pixel buffers cross
    /// threads; `Image`s are made on the UI thread.
    pub icons: Vec<(u32, SharedPixelBuffer<Rgba8Pixel>)>,
    pub notice: Option<Notice>,
    /// `Some` only while the Performance view is shown.
    pub perf: Option<PerfUi>,
    /// `Some` only while the Diagnostics view is shown.
    pub diagnostics: Option<DiagUi>,
    /// Rail badge, every frame.
    pub badge: i32,
    /// `Some` when the process history panel has something new to show.
    pub process: Option<ProcessUi>,
    /// Memory held by the histories, for the About window.
    pub history_memory: SharedString,
    /// Offsets of panned ranges, which grow as time passes (see `Anchored`).
    pub perf_offset: Option<i32>,
    pub process_offset: Option<i32>,
    pub tools: ToolsUi,
    /// Notifications to send now.
    pub alerts: Vec<AlertUi>,
    /// Machine CPU and memory for the tray tooltip, at most every 2 s.
    pub tooltip: Option<(SharedString, SharedString)>,
    /// `Some` while the tray mini view is open.
    pub mini: Option<MiniUi>,
}

/// The Performance view's data, ready for its models.
pub struct PerfUi {
    pub view: PerfView,
    pub cores: Vec<f32>,
    pub disks: Vec<DiskCard>,
    pub gpus: Vec<GpuCard>,
    /// `None` when the graphs have not changed since the last frame.
    pub graphs: Option<PerfGraphs>,
}

impl PerfUi {
    /// For systems whose probe has no machine-wide figures.
    pub fn unavailable() -> Self {
        Self {
            view: PerfView {
                ready: true,
                available: false,
                ..PerfView::default()
            },
            cores: Vec::new(),
            disks: Vec::new(),
            gpus: Vec::new(),
            graphs: None,
        }
    }
}

impl UiFrame {
    pub fn new(
        frame: Frame,
        with_rows: bool,
        icons: Vec<(u32, Icon)>,
        notice: Option<Notice>,
        perf: Option<PerfUi>,
    ) -> Self {
        Self {
            perf,
            diagnostics: None,
            badge: 0,
            process: None,
            history_memory: SharedString::new(),
            perf_offset: None,
            process_offset: None,
            tools: ToolsUi::default(),
            alerts: Vec::new(),
            tooltip: None,
            mini: None,
            rows: with_rows.then(|| frame.rows.iter().map(proc_row).collect()),
            process_count: frame.process_count.to_string().into(),
            matching_count: frame.matching_count.to_string().into(),
            self_cpu: format_cpu(frame.self_cpu_percent),
            total_cpu: SharedString::new(),
            total_memory: SharedString::new(),
            self_memory: format_memory(frame.self_memory_bytes),
            notice,
            icons: icons
                .into_iter()
                .map(|(id, icon)| {
                    let buffer = SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(
                        &icon.rgba,
                        icon.width,
                        icon.height,
                    );
                    (id, buffer)
                })
                .collect(),
        }
    }
}

/// The outcome of an action, for the status bar. `kind` codes match `notice-text()` in
/// `ui/app.slint`.
pub struct Notice {
    pub kind: i32,
    pub name: SharedString,
    pub pid: i32,
    pub count: i32,
}

impl Notice {
    pub const ENDED: i32 = 1;
    pub const ENDED_TREE: i32 = 2;
    pub const SUSPENDED: i32 = 3;
    pub const RESUMED: i32 = 4;
    pub const PRIORITY: i32 = 5;
    pub const COPIED: i32 = 6;
    pub const GONE: i32 = 7;
    pub const DENIED: i32 = 8;
    pub const FAILED: i32 = 9;
    pub const STARTUP_ENABLED: i32 = 10;
    pub const STARTUP_DISABLED: i32 = 11;
    pub const SERVICE_STARTED: i32 = 12;
    pub const SERVICE_STOPPED: i32 = 13;
    pub const DEPENDENTS: i32 = 14;
    /// Access denied to a change that is not about a process (a service, a startup item).
    pub const CHANGE_DENIED: i32 = 15;
    pub const CHANGE_FAILED: i32 = 16;
}
fn proc_row(row: &Row) -> ProcRow {
    let pid = i32::try_from(row.pid).unwrap_or(i32::MAX);
    // A stable identity per row, so patching moves rows instead of rewriting them.
    let (kind, group, key) = match row.kind {
        RowKind::Group(group) => {
            let index = match group {
                Group::Apps => 0,
                Group::Background => 1,
                Group::System => 2,
                Group::All => 3,
            };
            (KIND_GROUP, index, -1 - index)
        }
        RowKind::Process => (KIND_PROCESS, 0, pid.wrapping_mul(2)),
        RowKind::Itself => (KIND_ITSELF, 0, pid.wrapping_mul(2) + 1),
    };
    let figures = kind != KIND_GROUP;
    ProcRow {
        key,
        kind,
        group,
        depth: i32::from(row.depth),
        count: i32::try_from(row.count).unwrap_or(i32::MAX),
        expandable: row.kind == RowKind::Process && row.count > 0,
        expanded: row.expanded,
        icon: row.icon.and_then(|i| i32::try_from(i).ok()).unwrap_or(-1),
        name: row.name.as_str().into(),
        pid,
        start: (row.start_time & 0x7fff_ffff) as i32,
        publisher: row.publisher.as_deref().unwrap_or_default().into(),
        tree_size: i32::try_from(row.tree_size).unwrap_or(i32::MAX),
        own: row.own,
        cpu: if figures {
            format_cpu(row.cpu_percent)
        } else {
            SharedString::new()
        },
        memory: if figures {
            format_memory(row.memory_bytes)
        } else {
            SharedString::new()
        },
        cpu_heat: i32::from(row.cpu_heat),
        memory_heat: i32::from(row.memory_heat),
    }
}
/// Runs on the UI thread: patch the models, nothing else.
pub fn apply(window: &slint::Weak<AppWindow>, frame: UiFrame) {
    let Some(app) = window.upgrade() else {
        return;
    };
    // Icons first, so rows never point past the end of the icon model.
    let icons = app.get_icons();
    if let Some(model) = icons.as_any().downcast_ref::<VecModel<Image>>() {
        for (id, buffer) in frame.icons {
            // Ids are handed out in order, once each.
            debug_assert_eq!(id as usize, model.row_count());
            model.push(Image::from_rgba8(buffer));
        }
    }
    let rows = app.get_rows();
    if let (Some(model), Some(new_rows)) = (
        rows.as_any().downcast_ref::<VecModel<ProcRow>>(),
        frame.rows,
    ) {
        patch(model, new_rows);
        refresh_selection(&app, model);
    }
    if let Some(perf) = frame.perf {
        app.set_perf(perf.view);
        patch_values(&app.get_perf_cores(), perf.cores);
        patch_values(&app.get_perf_disks(), perf.disks);
        patch_values(&app.get_perf_gpus(), perf.gpus);
        if let Some(graphs) = perf.graphs {
            app.set_perf_cpu_graph(graphs.cpu.into_slint());
            app.set_perf_memory_graph(graphs.memory.into_slint());
            app.set_perf_network_graph(graphs.network.into_slint());
            let models = |graphs: Vec<RawGraph>| {
                ModelRc::new(VecModel::from(
                    graphs
                        .into_iter()
                        .map(RawGraph::into_slint)
                        .collect::<Vec<_>>(),
                ))
            };
            app.set_perf_disk_graphs(models(graphs.disks));
            app.set_perf_gpu_graphs(models(graphs.gpus));
        }
    }
    if let Some(diagnostics) = frame.diagnostics {
        // Patched, not replaced: replacing would rebuild every card each time and lose a
        // click in progress.
        patch_values(&app.get_findings(), diagnostics.findings);
        app.set_checks(diagnostics.checks);
        app.set_diagnostics_available(diagnostics.available);
        app.set_diagnostics_ready(true);
    }
    if let Some(process) = frame.process {
        app.set_process_cpu_graph(process.cpu.into_slint());
        app.set_process_memory_graph(process.memory.into_slint());
        app.set_process_tracked(process.tracked);
        app.set_process_history_age(process.age_s);
        app.set_process_memory_scale(process.memory_scale);
    }
    if let Some(offset) = frame.perf_offset {
        app.set_perf_offset(offset);
    }
    if let Some(offset) = frame.process_offset {
        app.set_process_offset(offset);
    }
    apply_tools(&app, frame.tools);
    crate::desktop::on_frame(&app, frame.alerts, frame.tooltip, frame.mini);
    app.set_diagnostics_badge(frame.badge);
    app.set_history_memory(frame.history_memory);
    app.set_process_count(frame.process_count);
    app.set_matching_count(frame.matching_count);
    app.set_self_cpu(frame.self_cpu);
    app.set_total_cpu(frame.total_cpu);
    app.set_total_memory(frame.total_memory);
    app.set_self_memory(frame.self_memory);
    if let Some(notice) = frame.notice {
        app.set_notice_name(notice.name);
        app.set_notice_pid(notice.pid);
        app.set_notice_count(notice.count);
        app.set_notice_kind(notice.kind);
    }
}

/// Makes a small positional model equal to `values`, touching only what changed.
fn patch_values<T: Clone + PartialEq + 'static>(model: &slint::ModelRc<T>, values: Vec<T>) {
    let Some(model) = model.as_any().downcast_ref::<VecModel<T>>() else {
        return;
    };
    for (i, value) in values.iter().enumerate() {
        match model.row_data(i) {
            Some(old) if old == *value => {}
            Some(_) => model.set_row_data(i, value.clone()),
            None => model.push(value.clone()),
        }
    }
    while model.row_count() > values.len() {
        model.remove(model.row_count() - 1);
    }
}

/// The window graphs show: `span_s` seconds ending `offset_s` seconds ago.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Range {
    pub span_s: u64,
    pub offset_s: u64,
}

impl Default for Range {
    fn default() -> Self {
        Self::new(0, 0)
    }
}

impl Range {
    /// Spans offered by the range bar (`Spans.seconds` in `ui/timeline.slint`).
    pub const SPANS: [u64; 6] = [60, 600, 3600, 21_600, 86_400, 604_800];

    pub fn new(span_index: i32, offset_s: i32) -> Self {
        let span = usize::try_from(span_index)
            .unwrap_or(0)
            .min(Self::SPANS.len() - 1);
        Self {
            span_s: Self::SPANS[span],
            offset_s: u64::try_from(offset_s).unwrap_or(0),
        }
    }
}

/// A range as the user set it. Panned back, its end stays at the same moment, so what the user
/// looks at does not slide away as time passes; at "now" it follows the present.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Anchored {
    range: Range,
    end_ns: Option<u64>,
}

impl Anchored {
    pub fn new(range: Range, now_ns: u64) -> Self {
        Self {
            range,
            end_ns: (range.offset_s > 0).then(|| now_ns.saturating_sub(range.offset_s * SECOND)),
        }
    }

    /// The range at `now_ns`: the offset grows with time while anchored.
    pub fn at(&self, now_ns: u64) -> Range {
        match self.end_ns {
            Some(end) => Range {
                span_s: self.range.span_s,
                offset_s: now_ns.saturating_sub(end) / SECOND,
            },
            None => self.range,
        }
    }

    /// The offset the UI should show, while anchored.
    pub fn shown_offset(&self, now_ns: u64) -> Option<i32> {
        self.end_ns
            .map(|_| i32::try_from(self.at(now_ns).offset_s).unwrap_or(i32::MAX))
    }

    /// What decides whether graphs must be drawn again: the newest bucket at "now"; nothing
    /// but the anchor itself in the past.
    fn key(&self, newest_bucket: u64) -> (u64, Range) {
        match self.end_ns {
            Some(end) => (end, self.range),
            None => (newest_bucket, self.range),
        }
    }
}

/// A graph as plain data, so it can cross threads; `GraphData` holds models, made on the UI
/// thread.
#[derive(Clone, Debug, Default)]
pub struct RawGraph {
    pub line: SharedString,
    pub area: SharedString,
    pub second_line: SharedString,
    pub values: Vec<f32>,
    pub second_values: Vec<f32>,
}

impl RawGraph {
    fn into_slint(self) -> GraphData {
        GraphData {
            line: self.line,
            area: self.area,
            second_line: self.second_line,
            values: ModelRc::new(VecModel::from(self.values)),
            second_values: ModelRc::new(VecModel::from(self.second_values)),
        }
    }
}

/// Graph paths fill a 300 × 100 box (see `ui/timeline.slint`), one column per bucket up to 300.
const GRAPH_W: f32 = 300.0;
const GRAPH_H: f32 = 100.0;
const COLUMNS: usize = 300;

/// Builds graphs on the sampler thread and remembers what it last built, so a range whose
/// newest bucket has not changed is not drawn again.
#[derive(Default)]
pub struct Timeline {
    key: Option<(u64, Range, usize)>,
    series: Series,
    second: Series,
    line: String,
    area: String,
    /// Network scale of the last build, bytes per second.
    net_max: f32,
}

impl Timeline {
    /// Draw again on the next frame, whatever changed.
    pub fn invalidate(&mut self) {
        self.key = None;
    }

    /// `true` when `key` differs from the last build, which it then becomes.
    fn changed(&mut self, key: (u64, Range, usize)) -> bool {
        if self.key == Some(key) {
            return false;
        }
        self.key = Some(key);
        true
    }

    /// One history over the range, scaled to `max`, or to its own peak (with headroom and at
    /// least `floor`) when `max` is `None`. Returns the graph and the scale used.
    fn graph(
        &mut self,
        history: &History,
        now: u64,
        range: Range,
        max: Option<f32>,
        floor: f32,
    ) -> (RawGraph, f32) {
        history.window(now, range.span_s, range.offset_s, COLUMNS, &mut self.series);
        let max = max.unwrap_or_else(|| (self.series.max() * 1.15).max(floor));
        graph_paths(
            &self.series,
            max,
            GRAPH_W,
            GRAPH_H,
            &mut self.line,
            &mut self.area,
        );
        let graph = RawGraph {
            line: self.line.as_str().into(),
            area: self.area.as_str().into(),
            values: self.series.means.clone(),
            ..RawGraph::default()
        };
        (graph, max)
    }

    /// Received (line and area) and sent (second line) on one scale, at least 1 Mbit/s so
    /// idle links stay flat.
    fn network(&mut self, perf: &Performance, now: u64, range: Range) -> RawGraph {
        perf.received
            .window(now, range.span_s, range.offset_s, COLUMNS, &mut self.series);
        perf.sent
            .window(now, range.span_s, range.offset_s, COLUMNS, &mut self.second);
        self.net_max = (self.series.max().max(self.second.max()) * 1.15).max(125_000.0);
        graph_paths(
            &self.second,
            self.net_max,
            GRAPH_W,
            GRAPH_H,
            &mut self.line,
            &mut self.area,
        );
        let second_line = self.line.as_str().into();
        graph_paths(
            &self.series,
            self.net_max,
            GRAPH_W,
            GRAPH_H,
            &mut self.line,
            &mut self.area,
        );
        RawGraph {
            line: self.line.as_str().into(),
            area: self.area.as_str().into(),
            second_line,
            values: self.series.means.clone(),
            second_values: self.second.means.clone(),
        }
    }
}

/// The Performance view's graphs, rebuilt only when they change.
pub struct PerfGraphs {
    pub cpu: RawGraph,
    pub memory: RawGraph,
    pub network: RawGraph,
    pub disks: Vec<RawGraph>,
    pub gpus: Vec<RawGraph>,
}

/// Formats the Performance view. `history_age_s` is how long Vigie has been recording.
pub fn perf_ui(
    frame: &PerfFrame,
    perf: &Performance,
    info: &SystemInfo,
    window: Anchored,
    history_age_s: u64,
    timeline: &mut Timeline,
) -> PerfUi {
    let now = frame.at_ns;
    let range = window.at(now);
    let tier = perf.cpu.tier_for(range.span_s);
    let shape = perf.disks.len() << 8 | perf.gpus.len();
    let (bucket, key_range) = window.key(perf.cpu.newest_bucket(tier).unwrap_or(0));
    let graphs = timeline.changed((bucket, key_range, shape)).then(|| {
        let network = timeline.network(perf, now, range);
        PerfGraphs {
            cpu: timeline.graph(&perf.cpu, now, range, Some(100.0), 0.0).0,
            memory: timeline.graph(&perf.memory, now, range, Some(100.0), 0.0).0,
            network,
            disks: perf
                .disks
                .iter()
                .map(|h| timeline.graph(h, now, range, Some(100.0), 0.0).0)
                .collect(),
            gpus: perf
                .gpus
                .iter()
                .map(|h| timeline.graph(h, now, range, Some(100.0), 0.0).0)
                .collect(),
        }
    });
    let memory = &frame.memory;
    let used = memory.total.saturating_sub(memory.available);
    let slots = info.memory_slots.unwrap_or_default();
    let view = PerfView {
        ready: true,
        available: true,
        cpu_value: format_percent(frame.cpu_percent),
        cpu_model: info.cpu_model.as_str().into(),
        logical_cores: i32::try_from(info.logical_cores).unwrap_or(0),
        memory_value: format_percent((memory.total > 0).then_some(frame.memory_used_percent)),
        memory_used: format_memory(used),
        memory_total: format_memory(memory.total),
        memory_cached: format_memory(memory.cached),
        committed_used: format_memory(memory.committed),
        committed_limit: format_memory(memory.commit_limit),
        commit_percent: (commit_share(memory) * 100.0).round() as i32,
        memory_compressed: format_memory(memory.compressed),
        slots_used: slots.used as i32,
        slots_total: slots.total as i32,
        slots_speed: slots.speed_mts.unwrap_or(0) as i32,
        pressure: pressure_code(frame.pressure),
        pressure_cause: match frame.pressure_cause {
            PressureCause::Available => 0,
            PressureCause::Commit => 1,
        },
        network_value: format_bits(
            frame
                .received_per_s
                .zip(frame.sent_per_s)
                .map(|(r, s)| r + s),
        ),
        network_received: format_bits(frame.received_per_s),
        network_sent: format_bits(frame.sent_per_s),
        network_scale: format_bits(Some(f64::from(timeline.net_max))),
        uptime_days: (frame.uptime_s / 86_400) as i32,
        uptime_hours: (frame.uptime_s / 3600 % 24) as i32,
        uptime_minutes: (frame.uptime_s / 60 % 60) as i32,
        history_age: i32::try_from(history_age_s).unwrap_or(i32::MAX),
    };
    let disks = frame
        .disks
        .iter()
        .map(|disk| DiskCard {
            index: disk.index as i32,
            drives: drive_letters(disk.letters).into(),
            active: format_percent(disk.active_percent),
            read: format_bytes_per_s(disk.read_per_s),
            write: format_bytes_per_s(disk.write_per_s),
        })
        .collect();
    let gpus = frame
        .gpus
        .iter()
        .map(|gpu| GpuCard {
            index: gpu.index as i32,
            name: info
                .gpus
                .get(gpu.index as usize)
                .map(|n| n.as_str().into())
                .unwrap_or_default(),
            value: format_percent(gpu.percent),
            memory_used: format_memory(gpu.dedicated_used),
            memory_total: if gpu.dedicated_total > 0 {
                format_memory(gpu.dedicated_total)
            } else {
                SharedString::new()
            },
        })
        .collect();
    PerfUi {
        view,
        cores: frame.core_percents.clone(),
        disks,
        gpus,
        graphs,
    }
}

fn pressure_code(pressure: Pressure) -> i32 {
    match pressure {
        Pressure::Low => 0,
        Pressure::Medium => 1,
        Pressure::High => 2,
    }
}

/// The history panel of the selected process.
pub struct ProcessUi {
    pub cpu: RawGraph,
    pub memory: RawGraph,
    pub tracked: bool,
    pub age_s: i32,
    pub memory_scale: SharedString,
}

/// Graphs of a tracked process over `range`; `None` when nothing changed since the last build.
pub fn process_ui(
    tracked: Option<&Tracked>,
    now: u64,
    window: Anchored,
    timeline: &mut Timeline,
) -> Option<ProcessUi> {
    let range = window.at(now);
    let Some(t) = tracked else {
        // Not tracked (yet): empty graphs, once.
        return timeline.changed((0, range, 0)).then(|| ProcessUi {
            cpu: RawGraph::default(),
            memory: RawGraph::default(),
            tracked: false,
            age_s: 0,
            memory_scale: "—".into(),
        });
    };
    let tier = t.cpu.tier_for(range.span_s);
    let (bucket, key_range) = window.key(t.cpu.newest_bucket(tier).unwrap_or(0));
    // The key includes the process, so a new selection draws at once.
    let identity = (t.key.0 as usize) << 1 | 1;
    if !timeline.changed((bucket, key_range, identity)) {
        return None;
    }
    let (cpu, _) = timeline.graph(&t.cpu, now, range, None, 5.0);
    // At least 16 MB high, so a tiny process does not draw noise full-height.
    let (memory, scale) = timeline.graph(&t.memory, now, range, None, 16.0 * 1024.0 * 1024.0);
    Some(ProcessUi {
        cpu,
        memory,
        tracked: true,
        age_s: i32::try_from(now.saturating_sub(t.since_ns) / SECOND).unwrap_or(i32::MAX),
        memory_scale: format_memory(scale as u64),
    })
}

/// The Diagnostics view, and the rail badge.
pub struct DiagUi {
    /// False where the probe has no machine-wide figures to diagnose from.
    pub available: bool,
    pub findings: Vec<FindingRow>,
    pub checks: CheckSummary,
}

impl DiagUi {
    pub fn unavailable() -> Self {
        Self {
            available: false,
            findings: Vec::new(),
            checks: CheckSummary::default(),
        }
    }
}

/// Highest severity worth a badge: 0 none, 1 warning, 2 critical.
pub fn badge(findings: &[Finding]) -> i32 {
    match findings.iter().map(Finding::severity).max() {
        Some(Severity::Critical) => 2,
        Some(Severity::Warning) => 1,
        _ => 0,
    }
}

/// Formats findings for `ui/diagnostics.slint`, and the checks shown when there are none.
pub fn diagnostics_ui(
    findings: &[Finding],
    perf: &Performance,
    last: &PerfFrame,
    tracker: &Tracker,
    watched_s: u64,
) -> DiagUi {
    let now = last.at_ns;
    let rows = findings.iter().map(finding_row).collect();
    let disk = perf
        .disks
        .iter()
        .filter_map(|h| h.mean_over(now, 60).map(|(m, _)| m))
        .fold(None, |a: Option<f32>, m| Some(a.map_or(m, |a| a.max(m))));
    DiagUi {
        available: true,
        findings: rows,
        checks: CheckSummary {
            cpu: format_percent(perf.cpu.mean_over(now, 120).map(|(m, _)| m)),
            pressure: pressure_code(last.pressure),
            disk: format_percent(disk),
            watched_minutes: i32::try_from(watched_s / 60).unwrap_or(i32::MAX),
            tracked: tracker.iter().count() as i32,
        },
    }
}

fn finding_row(finding: &Finding) -> FindingRow {
    // Evidence shows the figure the finding is about: CPU or memory.
    let cpu_finding = matches!(
        finding,
        Finding::CpuSaturated { .. } | Finding::CpuHog { .. }
    );
    let culprit_line = |c: &Culprit| {
        let figure = if cpu_finding {
            format_cpu(c.cpu)
        } else {
            format_memory(c.memory)
        };
        format!("{} · PID {} · {figure}", c.name, c.target.pid)
    };
    let lines = |culprits: &[Culprit]| -> SharedString {
        culprits
            .iter()
            .map(culprit_line)
            .collect::<Vec<_>>()
            .join("\n")
            .into()
    };
    let mut row = FindingRow {
        severity: match finding.severity() {
            Severity::Info => 0,
            Severity::Warning => 1,
            Severity::Critical => 2,
        },
        ..FindingRow::default()
    };
    match finding {
        Finding::CpuSaturated {
            mean,
            window_s,
            culprits,
        } => {
            row.kind = 0;
            row.a = format_percent(Some(*mean));
            row.b = (window_s / 60).to_string().into();
            row.evidence = lines(culprits);
        }
        Finding::CpuHog {
            culprit,
            cores,
            window_s,
        } => {
            row.kind = 1;
            row.a = decimal(f64::from(*cores), 1).into();
            row.b = (window_s / 60).to_string().into();
            row.evidence = culprit_line(culprit).into();
        }
        Finding::MemoryPressure {
            level,
            cause,
            available_percent,
            commit_percent,
            culprits,
        } => {
            row.kind = 2;
            row.level = pressure_code(*level);
            row.cause = match cause {
                PressureCause::Available => 0,
                PressureCause::Commit => 1,
            };
            row.a = format!("{available_percent:.0}").into();
            row.b = format!("{commit_percent:.0}").into();
            row.evidence = lines(culprits);
        }
        Finding::MemoryGrowth {
            culprit,
            from,
            to,
            window_s,
        } => {
            row.kind = 3;
            row.a = format_memory(*from);
            row.b = format_memory(*to);
            row.c = format_duration(*window_s);
            row.evidence = culprit_line(culprit).into();
        }
        Finding::DiskBusy {
            index,
            letters,
            mean,
            ..
        } => {
            row.kind = 4;
            row.a = format_percent(Some(*mean));
            row.b = index.to_string().into();
            row.c = drive_letters(*letters).into();
        }
        Finding::LongUptime { days } => {
            row.kind = 5;
            row.a = days.to_string().into();
        }
    }
    let target = |row: &mut FindingRow, c: &Culprit| {
        row.name = c.name.as_str().into();
        row.pid = i32::try_from(c.target.pid).unwrap_or(i32::MAX);
        row.start = (c.target.start_time & 0x7fff_ffff) as i32;
    };
    row.suggestion = match finding.suggestion() {
        Suggestion::End(c) => {
            target(&mut row, &c);
            0
        }
        Suggestion::LeaveSystem(c) => {
            target(&mut row, &c);
            1
        }
        Suggestion::ClosePrograms => 2,
        Suggestion::ShowPerformance => 3,
        Suggestion::Restart => 4,
    };
    // Findings about one process name it in their title.
    if let Finding::CpuHog { culprit, .. } | Finding::MemoryGrowth { culprit, .. } = finding {
        target(&mut row, culprit);
    }
    row
}

/// "45 min", "2 h 10 min".
fn format_duration(seconds: u64) -> SharedString {
    let (hours, minutes) = (seconds / 3600, seconds / 60 % 60);
    if hours == 0 {
        format!("{minutes} min").into()
    } else if minutes == 0 {
        format!("{hours} h").into()
    } else {
        format!("{hours} h {minutes} min").into()
    }
}

/// "C: D:", the drive letters of a disk (bit 0 is A:).
fn drive_letters(letters: u32) -> String {
    (0..26u8)
        .filter(|bit| letters & (1 << bit) != 0)
        .map(|bit| format!("{}:", char::from(b'A' + bit)))
        .collect::<Vec<_>>()
        .join(" ")
}

fn format_percent(value: Option<f32>) -> SharedString {
    match value {
        Some(v) => format!("{v:.0}\u{a0}%").into(),
        None => "—".into(),
    }
}

/// Network speeds read in bits per second, as link speeds are sold.
fn format_bits(bytes_per_s: Option<f64>) -> SharedString {
    let Some(bytes) = bytes_per_s else {
        return "—".into();
    };
    let bits = bytes * 8.0;
    if bits >= 1e9 {
        format!("{}\u{a0}{}", decimal(bits / 1e9, 1), unit("Gbps", "Gbit/s")).into()
    } else if bits >= 1e6 {
        format!("{}\u{a0}{}", decimal(bits / 1e6, 1), unit("Mbps", "Mbit/s")).into()
    } else {
        format!("{:.0}\u{a0}{}", bits / 1e3, unit("kbps", "kbit/s")).into()
    }
}

fn format_bytes_per_s(bytes_per_s: Option<f64>) -> SharedString {
    let Some(bytes) = bytes_per_s else {
        return "—".into();
    };
    const KB: f64 = 1024.0;
    if bytes >= KB * KB {
        format!(
            "{}\u{a0}{}",
            decimal(bytes / (KB * KB), 1),
            unit("MB/s", "Mo/s")
        )
        .into()
    } else {
        format!("{:.0}\u{a0}{}", bytes / KB, unit("KB/s", "Ko/s")).into()
    }
}

/// The Files view's answer.
pub struct LocksUi {
    pub rows: Vec<LockRowUi>,
    /// 1 answered, 2 no such file, 3 a folder, 4 failed (see `ui/tools.slint`).
    pub status: i32,
    pub path: SharedString,
}

/// What the developer tools views get in a frame; `None` leaves a view as it is.
#[derive(Default)]
pub struct ToolsUi {
    pub ports: Option<Vec<PortRowUi>>,
    pub locks: Option<LocksUi>,
    pub startup: Option<Vec<StartupRowUi>>,
    pub services: Option<Vec<ServiceRowUi>>,
    /// `Some(false)` once a read said the system has no such source.
    pub available: Option<bool>,
}

fn icon_index(icon: Option<u32>) -> i32 {
    icon.and_then(|i| i32::try_from(i).ok()).unwrap_or(-1)
}

fn start_bits(start_time: u64) -> i32 {
    (start_time & 0x7fff_ffff) as i32
}

pub fn port_rows_ui(rows: &[PortRow]) -> Vec<PortRowUi> {
    rows.iter()
        .map(|r| PortRowUi {
            key: format!(
                "{:?} {} {} {}",
                r.protocol,
                r.local,
                r.remote.map(|a| a.to_string()).unwrap_or_default(),
                r.pid
            )
            .into(),
            protocol: match r.protocol {
                Protocol::Tcp => "TCP",
                Protocol::Udp => "UDP",
            }
            .into(),
            address: r.local.ip().to_string().into(),
            port: i32::from(r.local.port()),
            remote: r.remote.map(|a| a.to_string()).unwrap_or_default().into(),
            state: match r.state {
                None => -1,
                Some(TcpState::Listen) => 0,
                Some(TcpState::Established) => 1,
                Some(TcpState::CloseWait) => 2,
                Some(_) => 3,
            },
            pid: i32::try_from(r.pid).unwrap_or(i32::MAX),
            start: r.target.map_or(0, |t| start_bits(t.start_time)),
            name: r.name.as_str().into(),
            icon: icon_index(r.icon),
            can_end: r.can_end,
            exact: r.exact,
        })
        .collect()
}

pub fn lock_rows_ui(rows: &[LockRow]) -> Vec<LockRowUi> {
    rows.iter()
        .map(|r| LockRowUi {
            name: r.name.as_str().into(),
            pid: i32::try_from(r.target.pid).unwrap_or(i32::MAX),
            start: start_bits(r.target.start_time),
            service: r.service.as_deref().unwrap_or_default().into(),
            kind: match r.kind {
                LockerKind::App => 0,
                LockerKind::Service => 1,
                LockerKind::Explorer => 2,
                LockerKind::Console => 3,
                LockerKind::Critical => 4,
                LockerKind::Other => 5,
            },
            icon: icon_index(r.icon),
            can_end: r.can_end,
        })
        .collect()
}

/// A notification, worded by `NotifyText` in `ui/app.slint` so it follows the language.
pub struct AlertUi {
    /// 0 CPU threshold (a: mean, b: minutes), 1 memory threshold (a: percent), 2 CPU saturated
    /// (a: mean, b: minutes), 3 memory running low (a: available percent).
    pub kind: i32,
    pub a: SharedString,
    pub b: SharedString,
    /// The busiest process, if one is named.
    pub name: SharedString,
}

pub fn alert_ui(alert: &Alert) -> AlertUi {
    let name = |c: &Option<Culprit>| {
        c.as_ref()
            .map(|c| c.name.as_str().into())
            .unwrap_or_default()
    };
    match alert {
        Alert::CpuThreshold {
            mean,
            window_s,
            culprit,
        } => AlertUi {
            kind: 0,
            a: format_percent(Some(*mean)),
            b: (window_s / 60).max(1).to_string().into(),
            name: name(culprit),
        },
        Alert::MemoryThreshold { percent, culprit } => AlertUi {
            kind: 1,
            a: format_percent(Some(*percent)),
            b: SharedString::new(),
            name: name(culprit),
        },
        Alert::Critical(Finding::MemoryPressure {
            available_percent,
            culprits,
            ..
        }) => AlertUi {
            kind: 3,
            a: format!("{available_percent:.0}").into(),
            b: SharedString::new(),
            name: name(&culprits.first().cloned()),
        },
        Alert::Critical(Finding::CpuSaturated {
            mean,
            window_s,
            culprits,
        }) => AlertUi {
            kind: 2,
            a: format_percent(Some(*mean)),
            b: (window_s / 60).max(1).to_string().into(),
            name: name(&culprits.first().cloned()),
        },
        // Only the two findings above are critical; anything else reads as CPU.
        Alert::Critical(_) => AlertUi {
            kind: 2,
            a: SharedString::new(),
            b: SharedString::new(),
            name: SharedString::new(),
        },
    }
}

/// The tray mini view: machine figures and the busiest processes.
pub struct MiniUi {
    pub cpu: SharedString,
    pub memory: SharedString,
    /// 0 to 100, for the bars.
    pub cpu_share: f32,
    pub memory_share: f32,
    pub top: Vec<MiniRow>,
}

pub fn mini_ui(core: &Core, last: Option<&PerfFrame>) -> MiniUi {
    let mut busiest: Vec<_> = core
        .tracker()
        .iter()
        .filter_map(|t| t.cpu.last().map(|cpu| (cpu, t)))
        .collect();
    busiest.sort_by(|a, b| b.0.total_cmp(&a.0));
    let cpu = last.and_then(|l| l.cpu_percent);
    let memory = last
        .filter(|l| l.memory.total > 0)
        .map(|l| l.memory_used_percent);
    MiniUi {
        cpu: format_percent(cpu),
        memory: format_percent(memory),
        cpu_share: cpu.unwrap_or(0.0),
        memory_share: memory.unwrap_or(0.0),
        top: busiest
            .into_iter()
            .take(4)
            .map(|(cpu, t)| MiniRow {
                name: t.name.as_str().into(),
                cpu: format_cpu(Some(cpu)),
                memory: format_memory(t.memory.last().unwrap_or(0.0) as u64),
            })
            .collect(),
    }
}

/// Location codes shared with `ui/tools.slint`.
pub fn location_code(location: StartupLocation) -> i32 {
    match location {
        StartupLocation::UserRun => 0,
        StartupLocation::MachineRun => 1,
        StartupLocation::MachineRun32 => 2,
        StartupLocation::UserFolder => 3,
        StartupLocation::CommonFolder => 4,
    }
}

pub fn location_from_code(code: i32) -> Option<StartupLocation> {
    Some(match code {
        0 => StartupLocation::UserRun,
        1 => StartupLocation::MachineRun,
        2 => StartupLocation::MachineRun32,
        3 => StartupLocation::UserFolder,
        4 => StartupLocation::CommonFolder,
        _ => return None,
    })
}

/// Startup items with what their programs say about themselves (`details`, by item).
pub fn startup_rows_ui(items: &[StartupItem], details: &[Option<Details>]) -> Vec<StartupRowUi> {
    items
        .iter()
        .zip(details)
        .map(|(item, details)| {
            let details = details.as_ref();
            StartupRowUi {
                key: format!("{} {}", location_code(item.location), item.name).into(),
                title: details
                    .and_then(|d| d.description.as_deref())
                    .unwrap_or(&item.name)
                    .into(),
                name: item.name.as_str().into(),
                publisher: details
                    .and_then(|d| d.publisher.as_deref())
                    .unwrap_or_default()
                    .into(),
                command: item.command.as_str().into(),
                location: location_code(item.location),
                enabled: item.enabled,
                icon: icon_index(details.and_then(|d| d.icon)),
            }
        })
        .collect()
}

pub fn service_rows_ui(rows: &[&Service]) -> Vec<ServiceRowUi> {
    rows.iter()
        .map(|s| ServiceRowUi {
            name: s.name.as_str().into(),
            display: s.display_name.as_str().into(),
            state: match s.state {
                ServiceState::Running => 0,
                ServiceState::Starting => 1,
                ServiceState::Stopping => 2,
                ServiceState::Paused => 3,
                ServiceState::Stopped => 4,
                ServiceState::Other => 5,
            },
            start_type: match s.start_type {
                None => -1,
                Some(StartType::Automatic) => 0,
                Some(StartType::AutomaticDelayed) => 1,
                Some(StartType::Manual) => 2,
                Some(StartType::Disabled) => 3,
            },
            pid: s.pid.and_then(|p| i32::try_from(p).ok()).unwrap_or(0),
        })
        .collect()
}

fn apply_tools(app: &AppWindow, tools: ToolsUi) {
    if let Some(available) = tools.available {
        app.set_tools_available(available);
    }
    if let Some(ports) = tools.ports {
        // A selection whose socket closed would offer to end a process that no longer has it.
        let selected = app.get_ports_selected();
        if !selected.key.is_empty() && !ports.iter().any(|p| p.key == selected.key) {
            app.set_ports_selected(PortRowUi::default());
        }
        patch_values(&app.get_ports(), ports);
        app.set_ports_ready(true);
    }
    if let Some(locks) = tools.locks {
        patch_values(&app.get_locks(), locks.rows);
        app.set_locks_status(locks.status);
        app.set_locks_path(locks.path);
    }
    if let Some(items) = tools.startup {
        patch_values(&app.get_startup_items(), items);
        app.set_startup_ready(true);
    }
    if let Some(services) = tools.services {
        patch_values(&app.get_services(), services);
        app.set_services_ready(true);
    }
}

/// Keeps the selected row's figures current, and drops the selection once its process has left
/// the table (exited, filtered out, or folded into a collapsed parent).
fn refresh_selection(app: &AppWindow, model: &VecModel<ProcRow>) {
    if !app.get_has_selection() {
        return;
    }
    let key = app.get_selected().key;
    match model.iter().find(|r| r.kind != KIND_GROUP && r.key == key) {
        Some(row) => app.set_selected(row),
        None => app.set_has_selection(false),
    }
}

/// Index of the process row `delta` rows away from the selection (headers skipped, clamped to
/// the table). With no selection, the first process row.
pub fn step_selection(
    model: &VecModel<ProcRow>,
    selected_key: Option<i32>,
    delta: i32,
) -> Option<usize> {
    let processes: Vec<usize> = (0..model.row_count())
        .filter(|&i| model.row_data(i).is_some_and(|r| r.kind != KIND_GROUP))
        .collect();
    if processes.is_empty() {
        return None;
    }
    let current = selected_key.and_then(|key| {
        processes
            .iter()
            .position(|&i| model.row_data(i).is_some_and(|r| r.key == key))
    });
    let target = match current {
        Some(at) => (at as i64 + i64::from(delta)).clamp(0, processes.len() as i64 - 1) as usize,
        None => 0,
    };
    processes.get(target).copied()
}

/// Numbers and units follow the interface language (decision 0008): French uses a decimal
/// comma and octets (Mo, Go). Set on the UI thread, read on the sampler thread.
static FRENCH: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn set_french(french: bool) {
    FRENCH.store(french, std::sync::atomic::Ordering::Relaxed);
}

fn french() -> bool {
    FRENCH.load(std::sync::atomic::Ordering::Relaxed)
}

/// `value` with `places` decimals, with the language's decimal separator.
fn decimal(value: f64, places: usize) -> String {
    let text = format!("{value:.places$}");
    if french() {
        text.replace('.', ",")
    } else {
        text
    }
}

fn unit(english: &'static str, french_unit: &'static str) -> &'static str {
    if french() { french_unit } else { english }
}

/// "—" when there is no value yet, never a misleading 0.
pub fn format_cpu(percent: Option<f32>) -> SharedString {
    match percent {
        Some(p) => format!("{}\u{a0}%", decimal(f64::from(p), 1)).into(),
        None => "—".into(),
    }
}

/// Megabytes up to 1 GB, then gigabytes, so large values stay short and readable.
pub fn format_memory(bytes: u64) -> SharedString {
    const MB: f64 = 1024.0 * 1024.0;
    let mb = bytes as f64 / MB;
    if mb < 1024.0 {
        format!("{}\u{a0}{}", decimal(mb, 1), unit("MB", "Mo")).into()
    } else {
        format!("{}\u{a0}{}", decimal(mb / 1024.0, 2), unit("GB", "Go")).into()
    }
}

/// A machine-wide share for a column header: "34 %", or nothing while unknown.
pub fn format_total(percent: Option<f32>) -> SharedString {
    percent.map_or_else(SharedString::new, |p| format!("{p:.0} %").into())
}

/// "412 KB", "1.1 MB": small amounts such as the history's memory.
pub fn format_kilobytes(bytes: usize) -> SharedString {
    let kb = bytes as f64 / 1024.0;
    if kb < 1024.0 {
        format!("{kb:.0}\u{a0}{}", unit("KB", "Ko")).into()
    } else {
        format!("{}\u{a0}{}", decimal(kb / 1024.0, 1), unit("MB", "Mo")).into()
    }
}

/// What a patch did, for tests and diagnostics.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct PatchStats {
    pub updated: usize,
    pub inserted: usize,
    pub removed: usize,
}

/// Makes `model` equal to `rows` with targeted edits, keyed by `key`: rows that vanished are
/// removed, new rows are inserted, changed rows get `set_row_data`, unchanged rows are untouched.
pub fn patch(model: &VecModel<ProcRow>, rows: Vec<ProcRow>) -> PatchStats {
    let mut stats = PatchStats::default();
    let new_keys: HashSet<i32> = rows.iter().map(|r| r.key).collect();
    let mut old_keys: HashSet<i32> = model.iter().map(|r| r.key).collect();

    let mut i = 0;
    for row in rows {
        // Drop old rows at this position that are gone.
        while let Some(old) = model.row_data(i) {
            if new_keys.contains(&old.key) {
                break;
            }
            model.remove(i);
            old_keys.remove(&old.key);
            stats.removed += 1;
        }
        match model.row_data(i) {
            Some(old) if old.key != row.key && !old_keys.contains(&row.key) => {
                model.insert(i, row);
                stats.inserted += 1;
            }
            Some(old) => {
                if old != row {
                    model.set_row_data(i, row);
                    stats.updated += 1;
                }
            }
            None => {
                model.push(row);
                stats.inserted += 1;
            }
        }
        i += 1;
    }
    while model.row_count() > i {
        model.remove(model.row_count() - 1);
        stats.removed += 1;
    }
    stats
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(pid: i32, cpu: &str) -> ProcRow {
        ProcRow {
            key: pid * 2,
            kind: KIND_PROCESS,
            name: format!("p{pid}").into(),
            pid,
            cpu: cpu.into(),
            memory: "1.0 MB".into(),
            ..ProcRow::default()
        }
    }

    #[test]
    fn rows_have_distinct_keys() {
        let process = |kind| Row {
            kind,
            pid: 7,
            start_time: 0,
            name: "p".into(),
            cpu_percent: Some(1.0),
            memory_bytes: 1,
            cpu_heat: 0,
            memory_heat: 0,
            depth: 0,
            count: 2,
            expanded: true,
            icon: None,
            publisher: None,
            tree_size: 1,
            own: false,
        };
        let group = proc_row(&process(RowKind::Group(Group::Apps)));
        let parent = proc_row(&process(RowKind::Process));
        let itself = proc_row(&process(RowKind::Itself));
        assert_eq!((group.kind, group.key), (KIND_GROUP, -1));
        assert_eq!(group.cpu, "", "group headers show no figures");
        assert!(parent.expandable && !itself.expandable);
        assert_ne!(parent.key, itself.key);
    }

    fn rows(model: &VecModel<ProcRow>) -> Vec<ProcRow> {
        model.iter().collect()
    }

    #[test]
    fn fills_an_empty_model() {
        let model = VecModel::default();
        let next = vec![row(1, "—"), row(2, "—")];
        let stats = patch(&model, next.clone());
        assert_eq!(rows(&model), next);
        assert_eq!(
            stats,
            PatchStats {
                updated: 0,
                inserted: 2,
                removed: 0
            }
        );
    }

    #[test]
    fn updates_only_changed_rows() {
        let model = VecModel::from(vec![row(1, "1.0 %"), row(2, "2.0 %"), row(3, "3.0 %")]);
        let next = vec![row(1, "1.0 %"), row(2, "9.0 %"), row(3, "3.0 %")];
        let stats = patch(&model, next.clone());
        assert_eq!(rows(&model), next);
        assert_eq!(
            stats,
            PatchStats {
                updated: 1,
                inserted: 0,
                removed: 0
            }
        );
    }

    #[test]
    fn inserts_and_removes_in_the_middle() {
        let model = VecModel::from(vec![row(1, "a"), row(2, "a"), row(3, "a"), row(4, "a")]);
        // Process 2 exited, process 9 started between 3 and 4.
        let next = vec![row(1, "a"), row(3, "a"), row(9, "a"), row(4, "a")];
        let stats = patch(&model, next.clone());
        assert_eq!(rows(&model), next);
        assert_eq!(
            stats,
            PatchStats {
                updated: 0,
                inserted: 1,
                removed: 1
            }
        );
    }

    #[test]
    fn handles_reordering_and_shrinking() {
        let model = VecModel::from(vec![row(1, "a"), row(2, "b"), row(3, "c"), row(4, "d")]);
        let next = vec![row(3, "c"), row(1, "a")];
        patch(&model, next.clone());
        assert_eq!(rows(&model), next);

        let next = vec![row(5, "e"), row(1, "a"), row(3, "c"), row(6, "f")];
        patch(&model, next.clone());
        assert_eq!(rows(&model), next);
    }

    #[test]
    fn keyboard_steps_skip_headers_and_stay_in_bounds() {
        let header = ProcRow {
            key: -1,
            kind: KIND_GROUP,
            ..ProcRow::default()
        };
        let model = VecModel::from(vec![
            header.clone(),
            row(1, "a"),
            header,
            row(2, "b"),
            row(3, "c"),
        ]);
        assert_eq!(
            step_selection(&model, None, 1),
            Some(1),
            "first process row"
        );
        assert_eq!(
            step_selection(&model, Some(2), 1),
            Some(3),
            "header skipped"
        );
        assert_eq!(step_selection(&model, Some(4), -1), Some(1));
        assert_eq!(
            step_selection(&model, Some(6), 1_000_000),
            Some(4),
            "clamped"
        );
        assert_eq!(step_selection(&model, Some(2), -1_000_000), Some(1));
        assert_eq!(step_selection(&VecModel::default(), None, 1), None);
    }
    #[test]
    fn formats_values() {
        assert_eq!(format_cpu(None), "—");
        assert_eq!(format_cpu(Some(12.345)), "12.3\u{a0}%");
        assert_eq!(format_memory(3 * 1024 * 1024 / 2), "1.5\u{a0}MB");
        assert_eq!(format_memory(3_800_000_000), "3.54\u{a0}GB");
        set_french(true);
        assert_eq!(format_memory(3_800_000_000), "3,54\u{a0}Go");
        assert_eq!(format_cpu(Some(12.345)), "12,3\u{a0}%");
        set_french(false);
        assert_eq!(drive_letters(0b1100), "C: D:");
        assert_eq!(drive_letters(0), "");
        assert_eq!(format_duration(45 * 60), "45 min");
        assert_eq!(format_duration(130 * 60), "2 h 10 min");
        assert_eq!(
            Range::new(9, -5),
            Range {
                span_s: 604_800,
                offset_s: 0
            }
        );
        // Panned back 5 minutes at t = 1000 s: ten seconds later it looks 310 s back.
        let panned = Anchored::new(Range::new(1, 300), 1000 * SECOND);
        assert_eq!(panned.at(1010 * SECOND).offset_s, 310);
        assert_eq!(panned.shown_offset(1010 * SECOND), Some(310));
        assert_eq!(
            Anchored::new(Range::new(1, 0), 5).shown_offset(99 * SECOND),
            None
        );
    }
}
