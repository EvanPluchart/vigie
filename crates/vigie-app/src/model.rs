//! Frames to Slint rows, and patching the table model in place.

use std::collections::HashSet;

use slint::{Model, SharedString, VecModel};
use vigie_core::{Frame, Row};

use crate::ProcRow;

/// A frame converted on the sampler thread, so the UI thread only patches models.
pub struct UiFrame {
    pub rows: Vec<ProcRow>,
    pub process_count: SharedString,
    pub self_cpu: SharedString,
    pub self_memory: SharedString,
}

impl From<Frame> for UiFrame {
    fn from(frame: Frame) -> Self {
        Self {
            rows: frame.rows.iter().map(proc_row).collect(),
            process_count: frame.process_count.to_string().into(),
            self_cpu: format_cpu(frame.self_cpu_percent),
            self_memory: format_memory(frame.self_memory_bytes),
        }
    }
}

fn proc_row(row: &Row) -> ProcRow {
    ProcRow {
        name: row.name.as_str().into(),
        pid: i32::try_from(row.pid).unwrap_or(i32::MAX),
        cpu: format_cpu(row.cpu_percent),
        memory: format_memory(row.memory_bytes),
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

/// Makes `model` equal to `rows` with targeted edits, keyed by PID: rows that vanished are
/// removed, new rows are inserted, changed rows get `set_row_data`, unchanged rows are untouched.
pub fn patch(model: &VecModel<ProcRow>, rows: Vec<ProcRow>) -> PatchStats {
    let mut stats = PatchStats::default();
    let new_pids: HashSet<i32> = rows.iter().map(|r| r.pid).collect();
    let mut old_pids: HashSet<i32> = model.iter().map(|r| r.pid).collect();

    let mut i = 0;
    for row in rows {
        // Drop old rows at this position whose process is gone.
        while let Some(old) = model.row_data(i) {
            if new_pids.contains(&old.pid) {
                break;
            }
            model.remove(i);
            old_pids.remove(&old.pid);
            stats.removed += 1;
        }
        match model.row_data(i) {
            Some(old) if old.pid != row.pid && !old_pids.contains(&row.pid) => {
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
            name: format!("p{pid}").into(),
            pid,
            cpu: cpu.into(),
            memory: "1.0 MB".into(),
        }
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
