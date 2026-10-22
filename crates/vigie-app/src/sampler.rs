//! The sampler thread: owns the probe, runs the core, sends frames to the UI thread.

use std::collections::HashMap;
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};

use vigie_core::perf::{PerfFrame, Performance};
use vigie_core::{Core, Details, Group, Pid, Sort};
use vigie_probe::{
    Action, Icon, Probe, ProbeError, ProcDetails, Sample, SystemInfo, SystemProbe, SystemSample,
};

use crate::AppWindow;
use crate::model::{self, Notice, UiFrame};

/// What the UI asks of the sampler. Sorting and filtering run here, not on the UI thread.
#[derive(Debug)]
pub enum Command {
    Sort(Sort),
    Filter(String),
    /// Expand or collapse the subtree of this PID.
    Toggle(Pid),
    /// The window gained or lost focus, or was minimised or covered (`visible: false`).
    Window {
        focused: bool,
        visible: bool,
    },
    /// Fold or unfold a section of the table.
    ToggleGroup(Group),
    /// The shown view changed: 0 processes, 1 performance.
    Tab(i32),
    /// Act on a process, named by its PID and the low 31 bits of its start time. The UI has
    /// already asked the user to confirm when the action is destructive.
    Act {
        pid: Pid,
        start_low_bits: u32,
        action: UserAction,
    },
}

/// The actions of the process menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UserAction {
    End,
    EndTree,
    Probe(Action),
}

pub struct Settings {
    pub sort: Sort,
    pub filter: String,
    pub interval: Duration,
    /// Icon side in physical pixels.
    pub icon_size: u32,
    /// Slow down when the window is in the background (see `Pace`).
    pub adaptive: bool,
    /// View at start: 0 processes, 1 performance.
    pub tab: i32,
}

/// Starts the sampler. It stops when every `Command` sender is dropped or the event loop quits.
pub fn spawn(
    window: slint::Weak<AppWindow>,
    settings: Settings,
    commands: flume::Receiver<Command>,
) -> thread::JoinHandle<()> {
    thread::Builder::new()
        .name("vigie-sampler".into())
        .spawn(move || run(window, settings, commands))
        .expect("spawning the sampler thread")
}

fn run(window: slint::Weak<AppWindow>, settings: Settings, commands: flume::Receiver<Command>) {
    let mut probe = match SystemProbe::new() {
        Ok(probe) => probe,
        Err(err) => {
            eprintln!("vigie: {err}");
            return;
        }
    };
    let mut core = Core::new(settings.sort, settings.filter);
    let mut sample = Sample::default();
    let mut exes = ExeCache {
        icon_size: settings.icon_size,
        ..ExeCache::default()
    };
    let mut next_tick = Instant::now();
    let mut pace = Pace {
        adaptive: settings.adaptive,
        focused: true,
        visible: true,
    };
    let mut views = Views {
        tab: settings.tab,
        info: probe.system_info(),
        ..Views::default()
    };
    loop {
        match probe.sample(&mut sample) {
            Ok(()) => {
                let icons = exes.fill(&mut probe, &mut core, &sample);
                // Machine-wide figures every tick (about 0.25 ms), so the graphs are full when
                // the Performance view opens.
                // GPU counters cost driver round trips: only while the Performance view is
                // shown, every other tick (decision 0005).
                views.ticks = views.ticks.wrapping_add(1);
                if views.tab == 1 && views.ticks % 2 == 1 {
                    let _ = probe.gpus(&mut views.system);
                }
                views.last = match probe.system(&mut views.system) {
                    Ok(()) => Some(views.perf.update(&views.system)),
                    Err(_) => None,
                };
                if !send(&window, &mut core, &sample, &views, icons, None) {
                    return;
                }
            }
            Err(err) => eprintln!("vigie: sampling failed: {err}"),
        }
        next_tick += pace.interval(settings.interval);
        // A late tick (machine asleep, debugger) starts a fresh schedule instead of catching up.
        if next_tick < Instant::now() {
            next_tick = Instant::now() + pace.interval(settings.interval);
        }
        // Between ticks, answer commands right away by reusing the last sample.
        loop {
            let wait = next_tick.saturating_duration_since(Instant::now());
            match commands.recv_timeout(wait) {
                Ok(command) => {
                    let mut notice = None;
                    let mut resample = false;
                    match command {
                        Command::Sort(sort) => core.set_sort(sort),
                        Command::Filter(filter) => core.set_filter(&filter),
                        Command::Toggle(pid) => core.toggle(pid),
                        Command::ToggleGroup(group) => core.toggle_group(group),
                        Command::Tab(tab) => views.tab = tab,
                        Command::Window { focused, visible } => {
                            let woke = (focused && !pace.focused) || (visible && !pace.visible);
                            pace.focused = focused;
                            pace.visible = visible;
                            // Back in front: refresh now rather than at the end of a slow interval.
                            if woke {
                                resample = true;
                            }
                        }
                        Command::Act {
                            pid,
                            start_low_bits,
                            action,
                        } => {
                            notice =
                                execute(&mut probe, &core, &sample, pid, start_low_bits, action);
                            // Show the effect (a process gone, say) without waiting for the tick.
                            resample = true;
                        }
                    }
                    if !send(&window, &mut core, &sample, &views, Vec::new(), notice) {
                        return;
                    }
                    if resample {
                        next_tick = Instant::now();
                        break;
                    }
                }
                Err(flume::RecvTimeoutError::Timeout) => break,
                Err(flume::RecvTimeoutError::Disconnected) => return,
            }
        }
    }
}

/// Runs a user action and describes its outcome for the status bar (`None`: nothing to say).
fn execute(
    probe: &mut impl Probe,
    core: &Core,
    sample: &Sample,
    pid: Pid,
    start_low_bits: u32,
    action: UserAction,
) -> Option<Notice> {
    let name = sample
        .processes
        .iter()
        .find(|p| p.pid == pid)
        .map(|p| p.name.as_str())
        .unwrap_or_default();
    let notice = |kind, count: usize| {
        Some(Notice {
            kind,
            name: name.into(),
            pid: i32::try_from(pid).unwrap_or(i32::MAX),
            count: i32::try_from(count).unwrap_or(i32::MAX),
        })
    };
    let failure = |error: ProbeError| match error {
        ProbeError::Gone => notice(Notice::GONE, 0),
        ProbeError::Denied => notice(Notice::DENIED, 0),
        _ => notice(Notice::FAILED, 0),
    };
    // The PID must still name the process the user saw.
    let Some(target) = core.identify(pid, start_low_bits) else {
        return notice(Notice::GONE, 0);
    };
    match action {
        UserAction::End => match probe.act(target, Action::End) {
            Ok(()) => notice(Notice::ENDED, 0),
            Err(error) => failure(error),
        },
        UserAction::EndTree => {
            // Children first, so none is orphaned and restarted by its parent meanwhile.
            let tree = core.subtree(target);
            let mut first_error = None;
            for member in &tree {
                match probe.act(*member, Action::End) {
                    // A child may exit on its own while its siblings are ended.
                    Ok(()) | Err(ProbeError::Gone) => {}
                    Err(error) => {
                        first_error.get_or_insert(error);
                    }
                }
            }
            match first_error {
                None => notice(Notice::ENDED_TREE, tree.len().saturating_sub(1)),
                Some(error) => failure(error),
            }
        }
        UserAction::Probe(action) => match probe.act(target, action) {
            Ok(()) => match action {
                Action::Suspend => notice(Notice::SUSPENDED, 0),
                Action::Resume => notice(Notice::RESUMED, 0),
                Action::SetPriority(_) => notice(Notice::PRIORITY, 0),
                Action::CopyCommandLine => notice(Notice::COPIED, 0),
                Action::End | Action::RevealFile => None,
            },
            Err(error) => failure(error),
        },
    }
}

/// The views and what only the Performance view needs.
#[derive(Default)]
struct Views {
    /// Shown view: 0 processes, 1 performance.
    tab: i32,
    perf: Performance,
    system: SystemSample,
    info: SystemInfo,
    /// The last tick's machine-wide figures; `None` where the probe has none.
    last: Option<PerfFrame>,
    ticks: u32,
}

/// Builds a frame with what the shown view needs and hands it to the UI thread. False once the
/// event loop has quit.
fn send(
    window: &slint::Weak<AppWindow>,
    core: &mut Core,
    sample: &Sample,
    views: &Views,
    icons: Vec<(u32, Icon)>,
    notice: Option<Notice>,
) -> bool {
    let perf = (views.tab == 1).then(|| match &views.last {
        Some(last) => model::perf_ui(last, &views.perf, &views.info),
        None => model::PerfUi::unavailable(),
    });
    let frame = UiFrame::new(core.update(sample), views.tab == 0, icons, notice, perf);
    let window = window.clone();
    slint::invoke_from_event_loop(move || model::apply(&window, frame)).is_ok()
}

/// What each executable says about itself, read once per path. Icons get sequential ids, the
/// index of the image in the UI's icon model.
#[derive(Default)]
struct ExeCache {
    by_path: HashMap<PathBuf, Details>,
    next_icon: u32,
    icon_size: u32,
    details: Vec<ProcDetails>,
}

impl ExeCache {
    /// Gives the core the details of processes it has not seen yet. Returns the icons that are
    /// new to the UI.
    fn fill(
        &mut self,
        probe: &mut impl Probe,
        core: &mut Core,
        sample: &Sample,
    ) -> Vec<(u32, Icon)> {
        let missing = core.missing_details(sample);
        if missing.is_empty() {
            return Vec::new();
        }
        if let Err(err) = probe.details(&missing, &mut self.details) {
            eprintln!("vigie: reading process details failed: {err}");
            self.details.clear();
        }
        let starts: HashMap<Pid, u64> = sample
            .processes
            .iter()
            .map(|p| (p.pid, p.start_time))
            .collect();
        let mut icons = Vec::new();
        for found in &self.details {
            let (Some(exe), Some(&start)) = (&found.exe, starts.get(&found.pid)) else {
                continue;
            };
            let details = self.by_path.entry(exe.clone()).or_insert_with(|| {
                let info = probe.exe_info(exe, self.icon_size);
                let icon = info.icon.map(|icon| {
                    let id = self.next_icon;
                    self.next_icon += 1;
                    icons.push((id, icon));
                    id
                });
                Details {
                    icon,
                    publisher: info.publisher.map(Into::into),
                    description: info.description.map(Into::into),
                }
            });
            core.set_details((found.pid, start), details.clone());
        }
        // Processes that could not be read get empty details, so they are not asked again.
        for pid in core.missing_details(sample) {
            if let Some(&start) = starts.get(&pid) {
                core.set_details((pid, start), Details::default());
            }
        }
        icons
    }
}

/// Adaptive refresh (docs/ARCHITECTURE.md): the chosen interval while the window has focus,
/// four times slower in the background, one sample every 10 s while minimised or hidden.
struct Pace {
    adaptive: bool,
    focused: bool,
    visible: bool,
}

impl Pace {
    const HIDDEN: Duration = Duration::from_secs(10);

    fn interval(&self, chosen: Duration) -> Duration {
        match (self.adaptive, self.visible, self.focused) {
            (false, _, _) | (true, true, true) => chosen,
            (true, true, false) => chosen * 4,
            (true, false, _) => Self::HIDDEN.max(chosen),
        }
    }
}
#[cfg(test)]
mod tests {
    use vigie_core::Sort;
    use vigie_probe::{ProcSample, Result, Target};

    use super::*;

    /// Records actions instead of touching any process.
    #[derive(Default)]
    struct FakeProbe {
        acted: Vec<(Pid, Action)>,
        refuse: Option<Pid>,
    }

    impl Probe for FakeProbe {
        fn sample(&mut self, _out: &mut Sample) -> Result<()> {
            Ok(())
        }
        fn details(&mut self, _pids: &[Pid], out: &mut Vec<ProcDetails>) -> Result<()> {
            out.clear();
            Ok(())
        }
        fn act(&mut self, target: Target, action: Action) -> Result<()> {
            if self.refuse == Some(target.pid) {
                return Err(ProbeError::Denied);
            }
            self.acted.push((target.pid, action));
            Ok(())
        }
    }

    /// app(20) with children 21 and 22; 22 has child 23.
    fn family() -> Sample {
        let p = |pid, parent| ProcSample {
            pid,
            parent,
            start_time: 1_000,
            name: format!("p{pid}"),
            ..ProcSample::default()
        };
        Sample {
            logical_cores: 1,
            processes: vec![
                p(20, None),
                p(21, Some(20)),
                p(22, Some(20)),
                p(23, Some(22)),
            ],
            ..Sample::default()
        }
    }

    fn setup() -> (FakeProbe, Core, Sample) {
        let sample = family();
        let mut core = Core::new(Sort::default(), "");
        core.update(&sample);
        (FakeProbe::default(), core, sample)
    }

    #[test]
    fn end_tree_ends_children_first() {
        let (mut probe, core, sample) = setup();
        let notice =
            execute(&mut probe, &core, &sample, 20, 1_000, UserAction::EndTree).expect("notice");
        let order: Vec<Pid> = probe.acted.iter().map(|(pid, _)| *pid).collect();
        assert_eq!(order.len(), 4);
        assert_eq!(order.last(), Some(&20));
        let at = |pid| order.iter().position(|&p| p == pid).unwrap();
        assert!(at(23) < at(22));
        assert_eq!((notice.kind, notice.count), (Notice::ENDED_TREE, 3));
        assert_eq!(notice.name, "p20");
    }

    #[test]
    fn stale_identity_acts_on_nothing() {
        let (mut probe, core, sample) = setup();
        let notice = execute(&mut probe, &core, &sample, 20, 999, UserAction::End).expect("notice");
        assert!(probe.acted.is_empty());
        assert_eq!(notice.kind, Notice::GONE);
    }

    #[test]
    fn background_and_hidden_windows_sample_less() {
        let second = Duration::from_secs(1);
        let pace = |adaptive, focused, visible| {
            Pace {
                adaptive,
                focused,
                visible,
            }
            .interval(second)
        };
        assert_eq!(pace(true, true, true), second);
        assert_eq!(pace(true, false, true), second * 4);
        assert_eq!(pace(true, false, false), Duration::from_secs(10));
        assert_eq!(
            pace(false, false, false),
            second,
            "benchmarks keep the chosen interval"
        );
    }
    #[test]
    fn failures_are_reported() {
        let (mut probe, core, sample) = setup();
        probe.refuse = Some(21);
        let notice =
            execute(&mut probe, &core, &sample, 21, 1_000, UserAction::End).expect("notice");
        assert_eq!(notice.kind, Notice::DENIED);
        let reveal = UserAction::Probe(Action::RevealFile);
        assert!(execute(&mut probe, &core, &sample, 20, 1_000, reveal).is_none());
    }
}
