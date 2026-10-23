//! Frames to Slint rows, and patching the table model in place.

use std::collections::HashSet;

use slint::{Image, Model, Rgba8Pixel, SharedPixelBuffer, SharedString, VecModel};
use vigie_core::history::{History, Series, graph_paths};
use vigie_core::perf::{PerfFrame, Performance, Pressure};
use vigie_core::{Frame, Group, Row, RowKind};
use vigie_probe::{Icon, SystemInfo};

use crate::{AppWindow, DiskCard, GpuCard, PerfView, ProcRow};

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
    pub self_memory: SharedString,
    /// Icons the UI has not seen yet, with their index in its icon model. Pixel buffers cross
    /// threads; `Image`s are made on the UI thread.
    pub icons: Vec<(u32, SharedPixelBuffer<Rgba8Pixel>)>,
    pub notice: Option<Notice>,
    /// `Some` only while the Performance view is shown.
    pub perf: Option<PerfUi>,
}

/// The Performance view's data, ready for its models.
pub struct PerfUi {
    pub view: PerfView,
    pub cores: Vec<f32>,
    pub disks: Vec<DiskCard>,
    pub gpus: Vec<GpuCard>,
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
            rows: with_rows.then(|| frame.rows.iter().map(proc_row).collect()),
            process_count: frame.process_count.to_string().into(),
            matching_count: frame.matching_count.to_string().into(),
            self_cpu: format_cpu(frame.self_cpu_percent),
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
    }
    app.set_process_count(frame.process_count);
    app.set_matching_count(frame.matching_count);
    app.set_self_cpu(frame.self_cpu);
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

/// Formats the Performance view. Graph paths fill a 300 × 100 box (see `ui/performance.slint`).
pub fn perf_ui(frame: &PerfFrame, perf: &Performance, info: &SystemInfo) -> PerfUi {
    const W: f32 = 300.0;
    const H: f32 = 100.0;
    const SPAN_S: u64 = 60;
    const COLUMNS: usize = 300;
    let now = frame.at_ns;
    let window = |history: &History| {
        let mut series = Series::default();
        history.window(now, SPAN_S, 0, COLUMNS, &mut series);
        series
    };
    let paths = |series: &Series, max: f32| {
        let (mut line, mut area) = (String::new(), String::new());
        graph_paths(series, max, W, H, &mut line, &mut area);
        (line, area)
    };
    let (cpu_line, cpu_area) = paths(&window(&perf.cpu), 100.0);
    let (memory_line, memory_area) = paths(&window(&perf.memory), 100.0);
    // Network: both directions share one scale, at least 1 Mbit/s so idle links stay flat.
    let (received, sent) = (window(&perf.received), window(&perf.sent));
    let net_max = (received.max().max(sent.max()) * 1.15).max(125_000.0);
    let (received_line, received_area) = paths(&received, net_max);
    let (sent_line, _) = paths(&sent, net_max);
    let memory = &frame.memory;
    let used = memory.total.saturating_sub(memory.available);
    let view = PerfView {
        ready: true,
        available: true,
        cpu_value: format_percent(frame.cpu_percent),
        cpu_model: info.cpu_model.as_str().into(),
        cpu_detail: format!("{} logical processors", info.logical_cores).into(),
        cpu_line: cpu_line.into(),
        cpu_area: cpu_area.into(),
        memory_value: format_percent((memory.total > 0).then_some(frame.memory_used_percent)),
        memory_detail: format!("{} of {}", format_memory(used), format_memory(memory.total)).into(),
        memory_cached: format_memory(memory.cached),
        memory_committed: format!(
            "{} of {}",
            format_memory(memory.committed),
            format_memory(memory.commit_limit)
        )
        .into(),
        memory_compressed: format_memory(memory.compressed),
        memory_slots: match info.memory_slots {
            Some(slots) => match slots.speed_mts {
                Some(speed) => format!("{} of {} · {speed} MT/s", slots.used, slots.total),
                None => format!("{} of {}", slots.used, slots.total),
            }
            .into(),
            None => "—".into(),
        },
        pressure: match frame.pressure {
            Pressure::Low => 0,
            Pressure::Medium => 1,
            Pressure::High => 2,
        },
        memory_line: memory_line.into(),
        memory_area: memory_area.into(),
        network_value: format_bits(
            frame
                .received_per_s
                .zip(frame.sent_per_s)
                .map(|(r, s)| r + s),
        ),
        network_received: format_bits(frame.received_per_s),
        network_sent: format_bits(frame.sent_per_s),
        network_scale: format_bits(Some(f64::from(net_max))),
        received_line: received_line.into(),
        received_area: received_area.into(),
        sent_line: sent_line.into(),
        uptime: format_uptime(frame.uptime_s),
    };
    let disks = frame
        .disks
        .iter()
        .zip(&perf.disks)
        .map(|(disk, history)| {
            let (line, area) = paths(&window(history), 100.0);
            DiskCard {
                title: disk_title(disk.index, disk.letters).into(),
                active: format_percent(disk.active_percent),
                read: format_bytes_per_s(disk.read_per_s),
                write: format_bytes_per_s(disk.write_per_s),
                line: line.into(),
                area: area.into(),
            }
        })
        .collect();
    let gpus = frame
        .gpus
        .iter()
        .map(|gpu| {
            let history = perf.gpus.get(gpu.index as usize);
            let (line, area) =
                history.map_or_else(Default::default, |h| paths(&window(h), 100.0));
            let title = info
                .gpus
                .get(gpu.index as usize)
                .map_or_else(|| format!("GPU {}", gpu.index), Clone::clone);
            GpuCard {
                title: title.into(),
                value: format_percent(gpu.percent),
                memory: if gpu.dedicated_total > 0 {
                    format!(
                        "{} of {}",
                        format_memory(gpu.dedicated_used),
                        format_memory(gpu.dedicated_total)
                    )
                    .into()
                } else {
                    "—".into()
                },
                line: line.into(),
                area: area.into(),
            }
        })
        .collect();
    PerfUi {
        view,
        cores: frame.core_percents.clone(),
        disks,
        gpus,
    }
}

/// "Disk 0 (C: D:)", as Windows users know their disks.
fn disk_title(index: u32, letters: u32) -> String {
    let drives: Vec<String> = (0..26u8)
        .filter(|bit| letters & (1 << bit) != 0)
        .map(|bit| format!("{}:", char::from(b'A' + bit)))
        .collect();
    if drives.is_empty() {
        format!("Disk {index}")
    } else {
        format!("Disk {index} ({})", drives.join(" "))
    }
}

fn format_percent(value: Option<f32>) -> SharedString {
    match value {
        Some(v) => format!("{v:.0} %").into(),
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
        format!("{:.1} Gbps", bits / 1e9).into()
    } else if bits >= 1e6 {
        format!("{:.1} Mbps", bits / 1e6).into()
    } else {
        format!("{:.0} kbps", bits / 1e3).into()
    }
}

fn format_bytes_per_s(bytes_per_s: Option<f64>) -> SharedString {
    let Some(bytes) = bytes_per_s else {
        return "—".into();
    };
    const KB: f64 = 1024.0;
    if bytes >= KB * KB {
        format!("{:.1} MB/s", bytes / (KB * KB)).into()
    } else {
        format!("{:.0} KB/s", bytes / KB).into()
    }
}

fn format_uptime(seconds: u64) -> SharedString {
    let (days, hours, minutes) = (seconds / 86_400, seconds / 3600 % 24, seconds / 60 % 60);
    if days > 0 {
        format!("up {days} d {hours} h").into()
    } else if hours > 0 {
        format!("up {hours} h {minutes} min").into()
    } else {
        format!("up {minutes} min").into()
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

/// "—" when there is no value yet, never a misleading 0.
pub fn format_cpu(percent: Option<f32>) -> SharedString {
    match percent {
        Some(p) => format!("{p:.1} %").into(),
        None => "—".into(),
    }
}

/// Megabytes up to 1 GB, then gigabytes, so large values stay short and readable.
pub fn format_memory(bytes: u64) -> SharedString {
    const MB: f64 = 1024.0 * 1024.0;
    let mb = bytes as f64 / MB;
    if mb < 1024.0 {
        format!("{mb:.1} MB").into()
    } else {
        format!("{:.2} GB", mb / 1024.0).into()
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
        assert_eq!(format_cpu(Some(12.345)), "12.3 %");
        assert_eq!(format_memory(3 * 1024 * 1024 / 2), "1.5 MB");
        assert_eq!(format_memory(3_800_000_000), "3.54 GB");
        assert_eq!(disk_title(0, 0b1100), "Disk 0 (C: D:)");
        assert_eq!(disk_title(1, 0), "Disk 1");
    }
}
