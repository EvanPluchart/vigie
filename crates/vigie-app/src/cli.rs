//! Command-line flags. Test flags reach a state directly, for screenshots and agents.

use std::time::Duration;

use vigie_core::{Sort, SortKey};

pub const USAGE: &str = "usage: vigie [--tab=processes] [--sort=cpu|memory|name|pid[:asc]] \
[--filter=TEXT] [--interval=MS] [--dark|--light] [--about] [--expand=PID]... [--ask=ACTION:PID] [--columns=publisher,pid,cpu,memory]";

#[derive(Clone, Debug, PartialEq)]
pub struct Options {
    pub sort: Sort,
    pub filter: String,
    pub interval: Duration,
    /// `None` follows the system theme.
    pub dark: Option<bool>,
    /// Opens the About window at start, for screenshots.
    pub about: bool,
    /// Processes to show expanded at start, for screenshots.
    pub expand: Vec<u32>,
    /// Opens the menu action ACTION (code from ui/app.slint) on PID at start, without running
    /// it: shows the confirmation, for screenshots.
    pub ask: Option<(i32, u32)>,
    /// Optional columns to show; `None` keeps the defaults.
    pub columns: Option<Vec<String>>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            sort: Sort::default(),
            filter: String::new(),
            interval: Duration::from_secs(1),
            dark: None,
            about: false,
            expand: Vec::new(),
            ask: None,
            columns: None,
        }
    }
}

/// Parses the arguments after the program name. `Err` carries the message to print.
pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Options, String> {
    let mut options = Options::default();
    for arg in args {
        let (flag, value) = match arg.split_once('=') {
            Some((flag, value)) => (flag, Some(value)),
            None => (arg.as_str(), None),
        };
        match (flag, value) {
            ("--tab", Some("processes")) => {}
            ("--tab", Some(tab)) => return Err(format!("tab `{tab}` is not available yet")),
            ("--sort", Some(sort)) => options.sort = parse_sort(sort)?,
            ("--filter", Some(filter)) => options.filter = filter.to_owned(),
            ("--interval", Some(ms)) => {
                let ms: u64 = ms.parse().map_err(|_| format!("invalid interval `{ms}`"))?;
                if ms < 100 {
                    return Err("the interval must be at least 100 ms".into());
                }
                options.interval = Duration::from_millis(ms);
            }
            ("--dark", None) => options.dark = Some(true),
            ("--light", None) => options.dark = Some(false),
            ("--about", None) => options.about = true,
            ("--ask", Some(value)) => {
                let parsed = value
                    .split_once(':')
                    .and_then(|(code, pid)| Some((code.parse().ok()?, pid.parse().ok()?)));
                options.ask = Some(parsed.ok_or_else(|| format!("invalid --ask `{value}`"))?);
            }
            ("--columns", Some(list)) => {
                let names: Vec<String> = list.split(',').map(str::to_owned).collect();
                if let Some(bad) = names
                    .iter()
                    .find(|n| !["publisher", "pid", "cpu", "memory"].contains(&n.as_str()))
                {
                    return Err(format!("unknown column `{bad}`"));
                }
                options.columns = Some(names);
            }
            ("--expand", Some(pid)) => options
                .expand
                .push(pid.parse().map_err(|_| format!("invalid PID `{pid}`"))?),
            _ => return Err(format!("unknown argument `{arg}`")),
        }
    }
    Ok(options)
}

fn parse_sort(value: &str) -> Result<Sort, String> {
    let (key, ascending) = match value.split_once(':') {
        Some((key, "asc")) => (key, Some(true)),
        Some((key, "desc")) => (key, Some(false)),
        Some(_) => return Err(format!("invalid sort `{value}`")),
        None => (value, None),
    };
    let key = match key {
        "cpu" => SortKey::Cpu,
        "memory" => SortKey::Memory,
        "name" => SortKey::Name,
        "pid" => SortKey::Pid,
        _ => return Err(format!("invalid sort `{value}`")),
    };
    let ascending = ascending.unwrap_or(key.default_ascending());
    Ok(Sort { key, ascending })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parses_all_flags() {
        let options = parse(args(&[
            "--tab=processes",
            "--sort=memory:asc",
            "--filter=code",
            "--interval=500",
            "--dark",
        ]))
        .unwrap();
        assert_eq!(
            options,
            Options {
                sort: Sort {
                    key: SortKey::Memory,
                    ascending: true
                },
                filter: "code".into(),
                interval: Duration::from_millis(500),
                dark: Some(true),
                about: false,
                expand: Vec::new(),
                ask: None,
                columns: None,
            }
        );
    }

    #[test]
    fn rejects_unknown_flags() {
        assert!(parse(args(&["--frobnicate"])).is_err());
        assert!(parse(args(&["--sort=size"])).is_err());
        assert!(parse(args(&["--interval=10"])).is_err());
        assert!(parse(args(&["--tab=ports"])).is_err());
    }
}
