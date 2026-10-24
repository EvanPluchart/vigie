//! Command-line flags. Test flags reach a state directly, for screenshots and agents.

use std::time::Duration;

use vigie_core::{Sort, SortKey};

pub const USAGE: &str = "usage: vigie [--tab=processes|performance|diagnostics] [--sort=cpu|memory|name|pid[:asc]] \
[--filter=TEXT] [--interval=MS] [--dark|--light] [--about] [--expand=PID]... [--ask=ACTION:PID] [--columns=status,publisher,user,description,pid,threads,handles,cpu,memory,io] [--select=PID] [--menu=1|2|3] [--no-adaptive] \
[--range=1m|10m|1h|6h|24h|7d] [--history] [--tab=details|users|ports|files|startup|services|settings] [--locks=PATH] [--no-settings] [--no-tray] [--pretend-version=X.Y.Z] [--updated] [--notify-test] [--mini] [--palette[=TEXT]] [--lang=en|fr]";

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Options {
    pub sort: Sort,
    pub filter: String,
    /// `None` keeps the interval from the settings.
    pub interval: Option<Duration>,
    /// View at start: 0 processes, 1 performance, 2 diagnostics, 3 ports, 4 files, 5 startup,
    /// 6 settings, 7 details, 8 users.
    pub tab: i32,
    /// Range of the Performance graphs at start, as an index into `Range::SPANS`.
    pub range: i32,
    /// Opens the history panel of the selected process at start.
    pub history: bool,
    /// Shows who holds this file at start (opens the Files view).
    pub locks: Option<std::path::PathBuf>,
    /// Opens the Services pane of the Startup view (`--tab=services`).
    pub services: bool,
    /// Reads and writes no settings file: defaults plus flags (benchmarks, screenshots).
    pub no_settings: bool,
    /// Creates no tray icon (benchmarks, screenshots: nothing left behind when killed).
    pub no_tray: bool,
    /// Sends one test notification at start.
    pub notify_test: bool,
    /// Opens the tray mini view at start, above the bottom right corner (screenshots).
    pub mini: bool,
    /// Opens the command palette at start, with this text typed (screenshots).
    pub palette: Option<String>,
    /// Interface language for this session: "en" or "fr".
    pub lang: Option<String>,
    /// Started by Windows in place of Task Manager.
    pub from_taskmgr: bool,
    /// Elevated helper: turn replacing Task Manager on or off, then exit (decision 0009).
    pub set_replace_taskmgr: Option<bool>,
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
    /// Process to select at start, for screenshots.
    pub select: Option<u32>,
    /// Opens a context menu at start, for screenshots: 1 the selected process (with
    /// `--select`), 2 the table header, 3 the ··· menu. Nothing runs without a click.
    pub menu: Option<i32>,
    /// Keep the chosen interval in the background too (benchmarks measure the focused cost).
    pub no_adaptive: bool,
    /// Tests updates (decision 0012): behaves as this older version and checks after 2 s,
    /// even with --no-settings; "Update" stops once the installer is checked.
    pub pretend_version: Option<String>,
    /// Started again by the installer after an update from the app (decision 0012): the
    /// status bar says so, with a link to what is new.
    pub updated: bool,
}

/// Parses the arguments after the program name. `Err` carries the message to print.
pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Options, String> {
    let mut options = Options::default();
    for arg in args {
        // Replacing Task Manager (decision 0009): Windows starts Vigie with the path of
        // taskmgr.exe and its own flags. They mean "open Vigie": ignore them.
        if names_taskmgr(&arg) {
            options.from_taskmgr = true;
            break;
        }
        let (flag, value) = match arg.split_once('=') {
            Some((flag, value)) => (flag, Some(value)),
            None => (arg.as_str(), None),
        };
        match (flag, value) {
            ("--tab", Some("processes")) => options.tab = 0,
            ("--tab", Some("performance")) => options.tab = 1,
            ("--tab", Some("diagnostics")) => options.tab = 2,
            ("--tab", Some("ports")) => options.tab = 3,
            ("--tab", Some("files")) => options.tab = 4,
            ("--tab", Some("startup")) => options.tab = 5,
            ("--tab", Some("settings")) => options.tab = 6,
            ("--tab", Some("details")) => options.tab = 7,
            ("--tab", Some("users")) => options.tab = 8,
            ("--tab", Some("services")) => {
                options.tab = 5;
                options.services = true;
            }
            ("--locks", Some(path)) => {
                options.locks = Some(path.into());
                options.tab = 4;
            }
            ("--range", Some(range)) => {
                options.range = ["1m", "10m", "1h", "6h", "24h", "7d"]
                    .iter()
                    .position(|&r| r == range)
                    .ok_or_else(|| format!("invalid range `{range}`"))?
                    as i32;
            }
            ("--history", None) => options.history = true,
            ("--tab", Some(tab)) => return Err(format!("tab `{tab}` is not available yet")),
            ("--sort", Some(sort)) => options.sort = parse_sort(sort)?,
            ("--filter", Some(filter)) => options.filter = filter.to_owned(),
            ("--interval", Some(ms)) => {
                let ms: u64 = ms.parse().map_err(|_| format!("invalid interval `{ms}`"))?;
                if ms < 100 {
                    return Err("the interval must be at least 100 ms".into());
                }
                options.interval = Some(Duration::from_millis(ms));
            }
            ("--dark", None) => options.dark = Some(true),
            ("--light", None) => options.dark = Some(false),
            ("--about", None) => options.about = true,
            ("--no-adaptive", None) => options.no_adaptive = true,
            ("--updated", None) => options.updated = true,
            ("--pretend-version", Some(version)) => {
                if vigie_core::update::Version::parse(version).is_none() {
                    return Err(format!("invalid version `{version}`"));
                }
                options.pretend_version = Some(version.to_owned());
            }
            ("--no-settings", None) => options.no_settings = true,
            ("--no-tray", None) => options.no_tray = true,
            ("--notify-test", None) => options.notify_test = true,
            ("--mini", None) => options.mini = true,
            ("--set-replace-taskmgr", Some(state @ ("on" | "off"))) => {
                options.set_replace_taskmgr = Some(state == "on");
            }
            ("--lang", Some(lang @ ("en" | "fr"))) => options.lang = Some(lang.to_owned()),
            ("--palette", value) => options.palette = Some(value.unwrap_or_default().to_owned()),
            ("--menu", Some(value)) => {
                options.menu = Some(
                    value
                        .parse::<i32>()
                        .ok()
                        .filter(|m| (1..=3).contains(m))
                        .ok_or_else(|| format!("invalid --menu `{value}`"))?,
                );
            }
            ("--ask", Some(value)) => {
                let parsed = value
                    .split_once(':')
                    .and_then(|(code, pid)| Some((code.parse().ok()?, pid.parse().ok()?)));
                options.ask = Some(parsed.ok_or_else(|| format!("invalid --ask `{value}`"))?);
            }
            ("--columns", Some(list)) => {
                let names: Vec<String> = list.split(',').map(str::to_owned).collect();
                if let Some(bad) = names.iter().find(|n| {
                    ![
                        "publisher",
                        "pid",
                        "cpu",
                        "memory",
                        "status",
                        "user",
                        "description",
                        "io",
                        "threads",
                        "handles",
                    ]
                    .contains(&n.as_str())
                }) {
                    return Err(format!("unknown column `{bad}`"));
                }
                options.columns = Some(names);
            }
            ("--select", Some(pid)) => {
                options.select = Some(pid.parse().map_err(|_| format!("invalid PID `{pid}`"))?);
            }
            ("--expand", Some(pid)) => options
                .expand
                .push(pid.parse().map_err(|_| format!("invalid PID `{pid}`"))?),
            _ => return Err(format!("unknown argument `{arg}`")),
        }
    }
    Ok(options)
}

/// An argument naming taskmgr.exe, quoted or not, with or without `.exe` (a console passes
/// `taskmgr` as typed), with any folder in front.
fn names_taskmgr(arg: &str) -> bool {
    let arg = arg.trim().trim_matches('"');
    let name = arg.rsplit(['\\', '/']).next().unwrap_or(arg);
    name.eq_ignore_ascii_case("taskmgr.exe") || name.eq_ignore_ascii_case("taskmgr")
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
                interval: Some(Duration::from_millis(500)),
                dark: Some(true),
                ..Options::default()
            }
        );
    }

    #[test]
    fn rejects_unknown_flags() {
        assert!(parse(args(&["--frobnicate"])).is_err());
        assert!(parse(args(&["--sort=size"])).is_err());
        assert!(parse(args(&["--interval=10"])).is_err());
        assert!(parse(args(&["--tab=network"])).is_err());
        let m3 = parse(args(&["--tab=diagnostics", "--range=6h", "--history"])).unwrap();
        assert_eq!((m3.tab, m3.range, m3.history), (2, 3, true));
        assert!(parse(args(&["--range=2h"])).is_err());
        // Started in place of Task Manager: its path and flags are not errors.
        let taskmgr = parse(args(&[r"C:\WINDOWS\system32\taskmgr.exe", "/4"])).unwrap();
        assert!(taskmgr.from_taskmgr);
        assert!(parse(args(&["taskmgr"])).unwrap().from_taskmgr);
        assert!(parse(args(&["TASKMGR", "/7"])).unwrap().from_taskmgr);
        assert!(
            parse(args(&[r#""C:\Windows\System32\Taskmgr.exe""#]))
                .unwrap()
                .from_taskmgr
        );
        assert_eq!(
            parse(args(&["--set-replace-taskmgr=on"]))
                .unwrap()
                .set_replace_taskmgr,
            Some(true)
        );
        assert!(parse(args(&["--set-replace-taskmgr=maybe"])).is_err());
        let m4 = parse(args(&["--tab=ports"])).unwrap();
        assert_eq!(m4.tab, 3);
        let locks = parse(args(&[r"--locks=C:\x.txt"])).unwrap();
        assert_eq!((locks.tab, locks.locks), (4, Some(r"C:\x.txt".into())));
    }
}
