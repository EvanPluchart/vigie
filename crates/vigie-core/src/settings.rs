//! User settings and their file format (decision 0008): plain `key = value` lines. Parsing never
//! fails: an unknown key or a bad value keeps its default. No file access here.

use std::fmt::Write as _;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Theme {
    #[default]
    System,
    Dark,
    Light,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Language {
    #[default]
    System,
    English,
    French,
}

/// Optional columns of the process table (Name is always shown).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Columns {
    pub publisher: bool,
    pub pid: bool,
    pub cpu: bool,
    pub memory: bool,
    /// Running or suspended.
    pub status: bool,
    /// The account the process runs as.
    pub user: bool,
    /// What the executable says it is.
    pub description: bool,
    /// All I/O, per second.
    pub io: bool,
    pub threads: bool,
    pub handles: bool,
}

impl Default for Columns {
    /// The Processes view, as Task Manager opens it.
    fn default() -> Self {
        Self {
            publisher: false,
            pid: true,
            cpu: true,
            memory: true,
            status: true,
            user: false,
            description: false,
            io: false,
            threads: false,
            handles: false,
        }
    }
}

impl Columns {
    /// The Details view, as Task Manager opens it.
    pub fn details() -> Self {
        Self {
            publisher: false,
            pid: true,
            cpu: true,
            memory: true,
            status: true,
            user: true,
            description: true,
            io: false,
            threads: false,
            handles: false,
        }
    }

    /// Each column with its key in the settings file.
    fn fields(&mut self) -> [(&'static str, &mut bool); 10] {
        [
            ("publisher", &mut self.publisher),
            ("pid", &mut self.pid),
            ("cpu", &mut self.cpu),
            ("memory", &mut self.memory),
            ("status", &mut self.status),
            ("user", &mut self.user),
            ("description", &mut self.description),
            ("io", &mut self.io),
            ("threads", &mut self.threads),
            ("handles", &mut self.handles),
        ]
    }
}

/// Column widths in logical pixels; Name takes what is left.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Widths {
    pub publisher: u16,
    pub pid: u16,
    pub cpu: u16,
    pub memory: u16,
    pub status: u16,
    pub user: u16,
    pub description: u16,
    pub io: u16,
    pub threads: u16,
    pub handles: u16,
}

impl Default for Widths {
    fn default() -> Self {
        Self {
            publisher: 160,
            pid: 72,
            cpu: 88,
            memory: 120,
            status: 96,
            user: 120,
            description: 200,
            io: 96,
            threads: 72,
            handles: 80,
        }
    }
}

impl Widths {
    /// Narrowest and widest a column may be dragged to.
    pub const MIN: u16 = 40;
    pub const MAX: u16 = 600;

    fn fields(&mut self) -> [(&'static str, &mut u16); 10] {
        [
            ("publisher", &mut self.publisher),
            ("pid", &mut self.pid),
            ("cpu", &mut self.cpu),
            ("memory", &mut self.memory),
            ("status", &mut self.status),
            ("user", &mut self.user),
            ("description", &mut self.description),
            ("io", &mut self.io),
            ("threads", &mut self.threads),
            ("handles", &mut self.handles),
        ]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Settings {
    /// Refresh interval while focused, in milliseconds: one of `INTERVALS_MS`.
    pub interval_ms: u32,
    pub theme: Theme,
    pub language: Language,
    /// Columns of the Processes view and of the Details view.
    pub columns: Columns,
    pub detail_columns: Columns,
    /// Widths shared by both views.
    pub widths: Widths,
    /// Height of the process history panel, in logical pixels.
    pub history_height: u16,
    /// Notifications for critical findings and thresholds.
    pub alerts: bool,
    /// Total CPU threshold in percent, 0 for none, over `cpu_alert_minutes`.
    pub cpu_alert: u8,
    pub cpu_alert_minutes: u8,
    /// Memory in use threshold in percent, 0 for none, over one minute.
    pub memory_alert: u8,
    /// Closing the window hides it to the tray instead of quitting.
    pub close_to_tray: bool,
    /// Ask GitHub for a newer release at launch and every 6 hours (decision 0012).
    pub check_updates: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            interval_ms: 1000,
            theme: Theme::System,
            language: Language::System,
            columns: Columns::default(),
            detail_columns: Columns::details(),
            widths: Widths::default(),
            history_height: 280,
            alerts: true,
            cpu_alert: 90,
            cpu_alert_minutes: 2,
            memory_alert: 90,
            close_to_tray: false,
            check_updates: true,
        }
    }
}

/// Choices offered by the Settings view, in display order.
pub const INTERVALS_MS: [u32; 4] = [500, 1000, 2000, 5000];
pub const CPU_ALERTS: [u8; 4] = [0, 80, 90, 95];
pub const CPU_ALERT_MINUTES: [u8; 3] = [1, 2, 5];
pub const MEMORY_ALERTS: [u8; 4] = [0, 85, 90, 95];

impl Settings {
    pub fn parse(text: &str) -> Settings {
        let mut s = Settings::default();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let (key, value) = (key.trim(), value.trim());
            let flag = || match value {
                "true" | "on" | "1" => Some(true),
                "false" | "off" | "0" => Some(false),
                _ => None,
            };
            let number = || value.parse::<u32>().ok();
            match key {
                "interval_ms" => {
                    if let Some(v) = number().filter(|v| INTERVALS_MS.contains(v)) {
                        s.interval_ms = v;
                    }
                }
                "theme" => {
                    s.theme = match value {
                        "dark" => Theme::Dark,
                        "light" => Theme::Light,
                        "system" => Theme::System,
                        _ => s.theme,
                    }
                }
                "language" => {
                    s.language = match value {
                        "en" => Language::English,
                        "fr" => Language::French,
                        "system" => Language::System,
                        _ => s.language,
                    }
                }
                "history_height" => {
                    if let Some(v) = number().filter(|v| (150..=2000).contains(v)) {
                        s.history_height = v as u16;
                    }
                }
                "alerts" => s.alerts = flag().unwrap_or(s.alerts),
                "cpu_alert" => {
                    if let Some(v) = number().and_then(|v| u8::try_from(v).ok())
                        && CPU_ALERTS.contains(&v)
                    {
                        s.cpu_alert = v;
                    }
                }
                "cpu_alert_minutes" => {
                    if let Some(v) = number().and_then(|v| u8::try_from(v).ok())
                        && CPU_ALERT_MINUTES.contains(&v)
                    {
                        s.cpu_alert_minutes = v;
                    }
                }
                "memory_alert" => {
                    if let Some(v) = number().and_then(|v| u8::try_from(v).ok())
                        && MEMORY_ALERTS.contains(&v)
                    {
                        s.memory_alert = v;
                    }
                }
                "close_to_tray" => s.close_to_tray = flag().unwrap_or(s.close_to_tray),
                "check_updates" => s.check_updates = flag().unwrap_or(s.check_updates),
                _ => {
                    // column_NAME, detail_column_NAME and width_NAME.
                    let set_flag = |columns: &mut Columns, name: &str| {
                        for (key, field) in columns.fields() {
                            if key == name {
                                *field = flag().unwrap_or(*field);
                            }
                        }
                    };
                    if let Some(name) = key.strip_prefix("detail_column_") {
                        set_flag(&mut s.detail_columns, name);
                    } else if let Some(name) = key.strip_prefix("column_") {
                        set_flag(&mut s.columns, name);
                    } else if let Some(name) = key.strip_prefix("width_") {
                        for (key, field) in s.widths.fields() {
                            if key == name
                                && let Some(v) = number()
                                    .and_then(|v| u16::try_from(v).ok())
                                    .filter(|v| (Widths::MIN..=Widths::MAX).contains(v))
                            {
                                *field = v;
                            }
                        }
                    }
                }
            }
        }
        s
    }

    pub fn to_text(&self) -> String {
        let mut out = String::from("# Vigie settings. Edit with Vigie's Settings view.\n");
        let theme = match self.theme {
            Theme::System => "system",
            Theme::Dark => "dark",
            Theme::Light => "light",
        };
        let language = match self.language {
            Language::System => "system",
            Language::English => "en",
            Language::French => "fr",
        };
        let _ = writeln!(out, "interval_ms = {}", self.interval_ms);
        let _ = writeln!(out, "theme = {theme}");
        let _ = writeln!(out, "language = {language}");
        let (mut columns, mut details, mut widths) =
            (self.columns, self.detail_columns, self.widths);
        for (key, value) in columns.fields() {
            let _ = writeln!(out, "column_{key} = {value}");
        }
        for (key, value) in details.fields() {
            let _ = writeln!(out, "detail_column_{key} = {value}");
        }
        for (key, value) in widths.fields() {
            let _ = writeln!(out, "width_{key} = {value}");
        }
        let _ = writeln!(out, "history_height = {}", self.history_height);
        let _ = writeln!(out, "alerts = {}", self.alerts);
        let _ = writeln!(out, "cpu_alert = {}", self.cpu_alert);
        let _ = writeln!(out, "cpu_alert_minutes = {}", self.cpu_alert_minutes);
        let _ = writeln!(out, "memory_alert = {}", self.memory_alert);
        let _ = writeln!(out, "close_to_tray = {}", self.close_to_tray);
        let _ = writeln!(out, "check_updates = {}", self.check_updates);
        out
    }
}

/// Position of `value` in `choices`, or of the default when it is not one of them.
pub fn index_of<T: PartialEq + Copy>(choices: &[T], value: T, default: usize) -> usize {
    choices.iter().position(|&c| c == value).unwrap_or(default)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        let custom = Settings {
            interval_ms: 2000,
            theme: Theme::Light,
            language: Language::French,
            columns: Columns {
                publisher: true,
                pid: false,
                threads: true,
                ..Columns::default()
            },
            detail_columns: Columns {
                io: true,
                ..Columns::details()
            },
            widths: Widths {
                description: 320,
                ..Widths::default()
            },
            history_height: 400,
            alerts: false,
            cpu_alert: 95,
            cpu_alert_minutes: 5,
            memory_alert: 0,
            close_to_tray: true,
            check_updates: false,
        };
        assert_eq!(Settings::parse(&custom.to_text()), custom);
        assert_eq!(
            Settings::parse(&Settings::default().to_text()),
            Settings::default()
        );
    }

    #[test]
    fn bad_input_keeps_defaults() {
        let parsed = Settings::parse(
            "interval_ms = 3\ntheme = purple\n\u{0}garbage\nalerts = maybe\ncpu_alert = 300\nunknown = 1\nlanguage=fr\nwidth_pid = 9000\ncolumn_nope = true\nhistory_height = 3",
        );
        assert_eq!(
            parsed,
            Settings {
                language: Language::French,
                ..Settings::default()
            }
        );
        assert_eq!(Settings::parse(""), Settings::default());
        assert_eq!(index_of(&INTERVALS_MS, 2000, 1), 2);
        assert_eq!(index_of(&INTERVALS_MS, 7, 1), 1);
    }
}
