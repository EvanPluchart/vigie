//! Settings between the file, the flags and the Settings view (decision 0008). Flags override
//! settings for the session and are never written back: only what the user changes in the view
//! reaches the file.

use std::time::Duration;

use vigie_core::alerts::Rules;
use vigie_core::settings::{
    CPU_ALERT_MINUTES, CPU_ALERTS, Columns, INTERVALS_MS, Language, MEMORY_ALERTS, Settings, Theme,
    Widths, index_of,
};

use crate::cli::Options;
use crate::{AppWindow, ColumnSet, ColumnWidths};

/// The settings on disk, and what the view showed last (to tell what the user changed).
pub struct Prefs {
    pub saved: Settings,
    shown: Settings,
    /// False with `--no-settings`: nothing is written.
    persist: bool,
}

impl Prefs {
    /// Reads the file (unless `--no-settings`) and applies the flags on top for this session.
    pub fn load(options: &Options) -> (Prefs, Settings) {
        let persist = !options.no_settings;
        let saved = if persist {
            vigie_shell::settings_file::load()
                .map(|text| Settings::parse(&text))
                .unwrap_or_default()
        } else {
            Settings::default()
        };
        let mut session = saved;
        if let Some(interval) = options.interval {
            session.interval_ms = interval.as_millis() as u32;
        }
        match options.lang.as_deref() {
            Some("fr") => session.language = Language::French,
            Some("en") => session.language = Language::English,
            _ => {}
        }
        if let Some(dark) = options.dark {
            session.theme = if dark { Theme::Dark } else { Theme::Light };
        }
        if let Some(columns) = &options.columns {
            let has = |name: &str| columns.iter().any(|c| c == name);
            session.columns = Columns {
                publisher: has("publisher"),
                pid: has("pid"),
                cpu: has("cpu"),
                memory: has("memory"),
                status: has("status"),
                user: has("user"),
                description: has("description"),
                io: has("io"),
                threads: has("threads"),
                handles: has("handles"),
            };
        }
        (
            Prefs {
                saved,
                shown: session,
                persist,
            },
            session,
        )
    }

    /// Where the file is, for the Settings view; empty when nothing is saved.
    pub fn path(&self) -> String {
        if !self.persist {
            return String::new();
        }
        vigie_shell::settings_file::path()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    /// The view changed: copy what the user changed into the saved settings and write them.
    /// Returns the session settings now in effect.
    pub fn edited(&mut self, now: Settings) -> Settings {
        let before = self.shown;
        let mut saved = self.saved;
        if now.interval_ms != before.interval_ms {
            saved.interval_ms = now.interval_ms;
        }
        if now.theme != before.theme {
            saved.theme = now.theme;
        }
        if now.language != before.language {
            saved.language = now.language;
        }
        if now.columns != before.columns {
            saved.columns = now.columns;
        }
        if now.detail_columns != before.detail_columns {
            saved.detail_columns = now.detail_columns;
        }
        if now.widths != before.widths {
            saved.widths = now.widths;
        }
        if now.history_height != before.history_height {
            saved.history_height = now.history_height;
        }
        if (
            now.alerts,
            now.cpu_alert,
            now.cpu_alert_minutes,
            now.memory_alert,
        ) != (
            before.alerts,
            before.cpu_alert,
            before.cpu_alert_minutes,
            before.memory_alert,
        ) {
            saved.alerts = now.alerts;
            saved.cpu_alert = now.cpu_alert;
            saved.cpu_alert_minutes = now.cpu_alert_minutes;
            saved.memory_alert = now.memory_alert;
        }
        if now.close_to_tray != before.close_to_tray {
            saved.close_to_tray = now.close_to_tray;
        }
        if now.check_updates != before.check_updates {
            saved.check_updates = now.check_updates;
        }
        self.shown = now;
        if saved != self.saved {
            self.saved = saved;
            if self.persist
                && let Err(error) = vigie_shell::settings_file::save(&saved.to_text())
            {
                eprintln!("vigie: saving the settings failed: {error}");
            }
        }
        now
    }
}

pub fn show(app: &AppWindow, s: &Settings) {
    app.set_setting_interval(index_of(&INTERVALS_MS, s.interval_ms, 1) as i32);
    app.set_setting_theme(scheme(s.theme));
    app.set_setting_language(match s.language {
        Language::System => 0,
        Language::English => 1,
        Language::French => 2,
    });
    app.set_columns(column_set(&s.columns));
    app.set_detail_columns(column_set(&s.detail_columns));
    let w = |v: u16| f32::from(v);
    app.set_widths(ColumnWidths {
        publisher: w(s.widths.publisher),
        pid: w(s.widths.pid),
        cpu: w(s.widths.cpu),
        memory: w(s.widths.memory),
        status: w(s.widths.status),
        user: w(s.widths.user),
        description: w(s.widths.description),
        io: w(s.widths.io),
        threads: w(s.widths.threads),
        handles: w(s.widths.handles),
    });
    app.set_history_height(f32::from(s.history_height));
    app.set_setting_alerts(s.alerts);
    app.set_setting_cpu_alert(index_of(&CPU_ALERTS, s.cpu_alert, 2) as i32);
    app.set_setting_cpu_minutes(index_of(&CPU_ALERT_MINUTES, s.cpu_alert_minutes, 1) as i32);
    app.set_setting_memory_alert(index_of(&MEMORY_ALERTS, s.memory_alert, 2) as i32);
    app.set_setting_close_to_tray(s.close_to_tray);
    app.set_setting_check_updates(s.check_updates);
}

fn column_set(c: &Columns) -> ColumnSet {
    ColumnSet {
        publisher: c.publisher,
        pid: c.pid,
        cpu: c.cpu,
        memory: c.memory,
        status: c.status,
        user: c.user,
        description: c.description,
        io: c.io,
        threads: c.threads,
        handles: c.handles,
    }
}

fn columns_of(c: &ColumnSet) -> Columns {
    Columns {
        publisher: c.publisher,
        pid: c.pid,
        cpu: c.cpu,
        memory: c.memory,
        status: c.status,
        user: c.user,
        description: c.description,
        io: c.io,
        threads: c.threads,
        handles: c.handles,
    }
}

/// What the view shows now.
pub fn read(app: &AppWindow, session: &Settings) -> Settings {
    fn pick<T: Copy>(choices: &[T], index: i32, current: T) -> T {
        usize::try_from(index)
            .ok()
            .and_then(|i| choices.get(i).copied())
            .unwrap_or(current)
    }
    let widths = app.get_widths();
    let w = |v: f32| (v.round() as u16).clamp(Widths::MIN, Widths::MAX);
    Settings {
        // Unchanged in the view, an interval set by a flag (500, 1500…) stays as it is.
        interval_ms: {
            let index = app.get_setting_interval();
            if index == index_of(&INTERVALS_MS, session.interval_ms, 1) as i32 {
                session.interval_ms
            } else {
                pick(&INTERVALS_MS, index, session.interval_ms)
            }
        },
        theme: match app.get_setting_theme() {
            1 => Theme::Dark,
            2 => Theme::Light,
            _ => Theme::System,
        },
        language: match app.get_setting_language() {
            1 => Language::English,
            2 => Language::French,
            _ => Language::System,
        },
        columns: columns_of(&app.get_columns()),
        detail_columns: columns_of(&app.get_detail_columns()),
        widths: Widths {
            publisher: w(widths.publisher),
            pid: w(widths.pid),
            cpu: w(widths.cpu),
            memory: w(widths.memory),
            status: w(widths.status),
            user: w(widths.user),
            description: w(widths.description),
            io: w(widths.io),
            threads: w(widths.threads),
            handles: w(widths.handles),
        },
        history_height: (app.get_history_height().round() as u16).clamp(150, 2000),
        alerts: app.get_setting_alerts(),
        cpu_alert: pick(&CPU_ALERTS, app.get_setting_cpu_alert(), session.cpu_alert),
        cpu_alert_minutes: pick(
            &CPU_ALERT_MINUTES,
            app.get_setting_cpu_minutes(),
            session.cpu_alert_minutes,
        ),
        memory_alert: pick(
            &MEMORY_ALERTS,
            app.get_setting_memory_alert(),
            session.memory_alert,
        ),
        close_to_tray: app.get_setting_close_to_tray(),
        check_updates: app.get_setting_check_updates(),
    }
}

/// 0 follows the system, 1 dark, 2 light (`set-scheme` in `ui/app.slint`).
pub fn scheme(theme: Theme) -> i32 {
    match theme {
        Theme::System => 0,
        Theme::Dark => 1,
        Theme::Light => 2,
    }
}

pub fn interval(s: &Settings) -> Duration {
    Duration::from_millis(u64::from(s.interval_ms.max(100)))
}

/// Alert thresholds, or `None` when alerts are off (or no notification can be shown).
pub fn rules(s: &Settings, can_notify: bool) -> Option<Rules> {
    (s.alerts && can_notify).then(|| Rules {
        cpu_percent: f32::from(s.cpu_alert),
        cpu_window_s: u64::from(s.cpu_alert_minutes) * 60,
        memory_percent: f32::from(s.memory_alert),
    })
}

/// Selects the bundled translation for `language`; English is the source text.
pub fn select_language(language: Language) {
    let code = match language {
        Language::English => "en",
        Language::French => "fr",
        Language::System => {
            if system_prefers_french() {
                "fr"
            } else {
                "en"
            }
        }
    };
    let _ = slint::select_bundled_translation(code);
    crate::model::set_french(code == "fr");
}

/// The user's language from the environment Slint reads; French when it starts with "fr".
fn system_prefers_french() -> bool {
    vigie_shell::user_language().is_some_and(|l| l.to_lowercase().starts_with("fr"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_are_never_written_back() {
        // Started with --dark and --interval=500 on default settings, saving disabled.
        let options = Options {
            dark: Some(true),
            interval: Some(Duration::from_millis(500)),
            no_settings: true,
            ..Options::default()
        };
        let (mut prefs, session) = Prefs::load(&options);
        assert_eq!((session.theme, session.interval_ms), (Theme::Dark, 500));
        // The user changes only the CPU alert in the view.
        let now = prefs.edited(Settings {
            cpu_alert: 95,
            ..session
        });
        assert_eq!(now.theme, Theme::Dark, "the session keeps the flag");
        assert_eq!(prefs.saved.cpu_alert, 95);
        assert_eq!(prefs.saved.theme, Theme::System, "the flag is not saved");
        assert_eq!(prefs.saved.interval_ms, 1000);
        assert_eq!(prefs.path(), "", "nothing is saved with --no-settings");
    }
}
