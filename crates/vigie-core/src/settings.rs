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

/// Optional columns of the process table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Columns {
    pub publisher: bool,
    pub pid: bool,
    pub cpu: bool,
    pub memory: bool,
}

impl Default for Columns {
    fn default() -> Self {
        Self {
            publisher: false,
            pid: true,
            cpu: true,
            memory: true,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Settings {
    /// Refresh interval while focused, in milliseconds: one of `INTERVALS_MS`.
    pub interval_ms: u32,
    pub theme: Theme,
    pub language: Language,
    pub columns: Columns,
    /// Notifications for critical findings and thresholds.
    pub alerts: bool,
    /// Total CPU threshold in percent, 0 for none, over `cpu_alert_minutes`.
    pub cpu_alert: u8,
    pub cpu_alert_minutes: u8,
    /// Memory in use threshold in percent, 0 for none, over one minute.
    pub memory_alert: u8,
    /// Closing the window hides it to the tray instead of quitting.
    pub close_to_tray: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            interval_ms: 1000,
            theme: Theme::System,
            language: Language::System,
            columns: Columns::default(),
            alerts: true,
            cpu_alert: 90,
            cpu_alert_minutes: 2,
            memory_alert: 90,
            close_to_tray: false,
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
                "column_publisher" => s.columns.publisher = flag().unwrap_or(s.columns.publisher),
                "column_pid" => s.columns.pid = flag().unwrap_or(s.columns.pid),
                "column_cpu" => s.columns.cpu = flag().unwrap_or(s.columns.cpu),
                "column_memory" => s.columns.memory = flag().unwrap_or(s.columns.memory),
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
                _ => {}
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
        let _ = writeln!(out, "column_publisher = {}", self.columns.publisher);
        let _ = writeln!(out, "column_pid = {}", self.columns.pid);
        let _ = writeln!(out, "column_cpu = {}", self.columns.cpu);
        let _ = writeln!(out, "column_memory = {}", self.columns.memory);
        let _ = writeln!(out, "alerts = {}", self.alerts);
        let _ = writeln!(out, "cpu_alert = {}", self.cpu_alert);
        let _ = writeln!(out, "cpu_alert_minutes = {}", self.cpu_alert_minutes);
        let _ = writeln!(out, "memory_alert = {}", self.memory_alert);
        let _ = writeln!(out, "close_to_tray = {}", self.close_to_tray);
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
                cpu: true,
                memory: true,
            },
            alerts: false,
            cpu_alert: 95,
            cpu_alert_minutes: 5,
            memory_alert: 0,
            close_to_tray: true,
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
            "interval_ms = 3\ntheme = purple\n\u{0}garbage\nalerts = maybe\ncpu_alert = 300\nunknown = 1\nlanguage=fr",
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
