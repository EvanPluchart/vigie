//! Frames to Slint rows, and patching the table model in place.

use std::collections::HashSet;

use slint::{Image, Model, Rgba8Pixel, SharedPixelBuffer, SharedString, VecModel};
use vigie_core::{Frame, Group, Row, RowKind};
use vigie_probe::Icon;

use crate::{AppWindow, ProcRow};

/// Row kinds as `ui/app.slint` reads them.
const KIND_GROUP: i32 = 0;
const KIND_PROCESS: i32 = 1;
const KIND_ITSELF: i32 = 2;

/// A frame converted on the sampler thread, so the UI thread only patches models.
pub struct UiFrame {
    pub rows: Vec<ProcRow>,
    pub process_count: SharedString,
    pub self_cpu: SharedString,
    pub self_memory: SharedString,
    /// Icons the UI has not seen yet, with their index in its icon model. Pixel buffers cross
    /// threads; `Image`s are made on the UI thread.
    pub icons: Vec<(u32, SharedPixelBuffer<Rgba8Pixel>)>,
    pub notice: Option<Notice>,
}

impl UiFrame {
    pub fn new(frame: Frame, icons: Vec<(u32, Icon)>, notice: Option<Notice>) -> Self {
        Self {
            rows: frame.rows.iter().map(proc_row).collect(),
            process_count: frame.process_count.to_string().into(),
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
    if let Some(model) = rows.as_any().downcast_ref::<VecModel<ProcRow>>() {
        patch(model, frame.rows);
    }
    app.set_process_count(frame.process_count);
    app.set_self_cpu(frame.self_cpu);
    app.set_self_memory(frame.self_memory);
    if let Some(notice) = frame.notice {
        app.set_notice_name(notice.name);
        app.set_notice_pid(notice.pid);
        app.set_notice_count(notice.count);
        app.set_notice_kind(notice.kind);
    }
}

/// "—" when there is no value yet, never a misleading 0.
pub fn format_cpu(percent: Option<f32>) -> SharedString {
    match percent {
        Some(p) => format!("{p:.1} %").into(),
        None => "—".into(),
    }
}

pub fn format_memory(bytes: u64) -> SharedString {
    format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0)).into()
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
    fn formats_values() {
        assert_eq!(format_cpu(None), "—");
        assert_eq!(format_cpu(Some(12.345)), "12.3 %");
        assert_eq!(format_memory(3 * 1024 * 1024 / 2), "1.5 MB");
    }
}
