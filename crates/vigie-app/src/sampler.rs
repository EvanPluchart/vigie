//! The sampler thread: owns the probe, runs the core, sends frames to the UI thread.

use std::collections::HashMap;
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};

use vigie_core::alerts::{Alert, Alerts, Rules};
use vigie_core::diagnose::{Diagnostics, Finding, Input};
use vigie_core::history::SECOND;
use vigie_core::perf::{PerfFrame, Performance};
use vigie_core::{Core, Details, Group, Pid, Sort, tools};
use vigie_probe::{
    Action, Icon, Locker, Probe, ProbeError, ProcDetails, Sample, Service, Socket, StartupItem,
    StartupLocation, SystemInfo, SystemProbe, SystemSample,
};

use crate::AppWindow;
use crate::model::{self, Anchored, Notice, Range, Timeline, UiFrame};

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
    /// Expand or collapse a user of the Users view, by login session.
    ToggleUser(u32),
    /// An I/O column is on screen: read I/O counters every tick.
    IoShown(bool),
    /// The window closed: stop now. Not waiting for every sender to drop, which any new
    /// callback holding one would prevent (a Vigie left running unseen, holding the single
    /// instance, so neither Vigie nor Task Manager would open again).
    Quit,
    /// The Processes view groups by type (sections and trees) or shows one flat list.
    Grouped(bool),
    ExpandAll,
    CollapseAll,
    /// The shown view changed: 0 processes, 1 performance, 2 diagnostics.
    Tab(i32),
    /// The range of the Performance graphs.
    Range(Range),
    /// The range of the process history panel.
    ProcessRange(Range),
    /// The selected process (PID and low 31 bits of its start time), or none.
    Select(Option<(Pid, u32)>),
    /// The process history panel opened or closed.
    History(bool),
    /// The Ports view's search.
    PortsFilter(String),
    /// Who holds this file?
    CheckFile(PathBuf),
    /// The Startup view's pane: 0 startup apps, 1 services.
    StartupPane(i32),
    /// Read the shown pane of the Startup view again.
    StartupRefresh,
    SetStartup {
        location: StartupLocation,
        name: String,
        enabled: bool,
    },
    ServiceFilter(String),
    /// Start (`true`) or stop a service, by short name. The UI confirmed stopping already.
    ControlService {
        name: String,
        start: bool,
    },
    /// The refresh interval while focused changed (settings).
    Interval(Duration),
    /// Alert thresholds, or `None` with alerts turned off.
    Alerts(Option<Rules>),
    /// The tray mini view opened or closed: it needs the busiest processes.
    Mini(bool),
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
    /// View at start: 0 processes, 1 performance, 2 diagnostics.
    pub tab: i32,
    pub range: Range,
    pub process_range: Range,
    pub history_open: bool,
    pub alerts: Option<Rules>,
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
    core.set_flat(settings.tab == 7);
    core.set_file_names(settings.tab == 7);
    let mut sample = Sample::default();
    let mut exes = ExeCache {
        icon_size: settings.icon_size,
        ..ExeCache::default()
    };
    let mut interval = settings.interval;
    let mut next_tick = Instant::now();
    let mut pace = Pace {
        adaptive: settings.adaptive,
        focused: true,
        visible: true,
    };
    let mut views = Views {
        tab: settings.tab,
        grouped: true,
        info: probe.system_info(),
        range: Anchored::new(settings.range, 0),
        process_range: Anchored::new(settings.process_range, 0),
        history_open: settings.history_open,
        // Started on the Diagnostics view: send its state with the first frame.
        diagnosis_fresh: settings.tab == 2,
        alert_rules: settings.alerts,
        tools: Tools {
            startup_dirty: settings.tab == 5,
            ..Tools::default()
        },
        ..Views::default()
    };
    loop {
        match probe.sample(&mut sample) {
            Ok(()) => {
                let mut icons = exes.fill(&mut probe, &mut core, &sample);
                // Machine-wide figures every tick (about 0.25 ms): they feed the history.
                // GPU counters cost driver round trips: every other tick while the Performance
                // view is shown, every 10 s otherwise, for the history (decisions 0005, 0006).
                views.ticks = views.ticks.wrapping_add(1);
                let gpu_due = if views.tab == 1 {
                    views.ticks % 2 == 1
                } else {
                    views
                        .gpus_at
                        .is_none_or(|at| at.elapsed() >= GPU_BACKGROUND)
                };
                if gpu_due {
                    let _ = probe.gpus(&mut views.system);
                    views.gpus_at = Some(Instant::now());
                }
                views.last = match probe.system(&mut views.system) {
                    Ok(()) => Some(views.perf.update(&views.system)),
                    Err(_) => None,
                };
                views.started_ns.get_or_insert(sample.at_ns);
                views.read_session_users(&mut probe, &sample);
                if views
                    .maintenance_at
                    .is_none_or(|at| at.elapsed() >= MAINTENANCE_EVERY)
                {
                    views.maintenance = probe.maintenance();
                    views.maintenance_at = Some(Instant::now());
                }
                views.diagnose(&core, sample.logical_cores, false);
                views.tools.read(
                    &mut probe,
                    &core,
                    &sample,
                    &mut exes,
                    views.tab,
                    pace.visible,
                    &mut icons,
                );
                if !send(&window, &mut core, &sample, &mut views, icons, None) {
                    return;
                }
            }
            Err(err) => eprintln!("vigie: sampling failed: {err}"),
        }
        next_tick += pace.interval(interval);
        // A late tick (machine asleep, debugger) starts a fresh schedule instead of catching up.
        if next_tick < Instant::now() {
            next_tick = Instant::now() + pace.interval(interval);
        }
        // Between ticks, answer commands right away by reusing the last sample.
        loop {
            let wait = next_tick.saturating_duration_since(Instant::now());
            match commands.recv_timeout(wait) {
                Ok(first) => {
                    let mut notice = None;
                    let mut resample = false;
                    // Commands come in bursts (a drag sends one per mouse move): take them all,
                    // then send one frame.
                    for command in std::iter::once(first).chain(commands.try_iter()) {
                        match command {
                            Command::Sort(sort) => core.set_sort(sort),
                            Command::Filter(filter) => core.set_filter(&filter),
                            Command::Toggle(pid) => core.toggle(pid),
                            Command::ToggleGroup(group) => core.toggle_group(group),
                            Command::Grouped(grouped) => {
                                views.grouped = grouped;
                                core.set_flat(views.flat());
                            }
                            Command::ExpandAll => core.expand_all(),
                            Command::IoShown(shown) => probe.set_io_every_tick(shown),
                            Command::Quit => return,
                            Command::ToggleUser(session) => {
                                if !views.users_expanded.remove(&session) {
                                    views.users_expanded.insert(session);
                                }
                            }
                            Command::CollapseAll => core.collapse_all(),
                            Command::Tab(tab) => {
                                views.tab = tab;
                                core.set_flat(views.flat());
                                core.set_file_names(tab == 7);
                                views.read_session_users(&mut probe, &sample);
                                views.timeline.invalidate();
                                views.tools.opened(tab);
                                if tab == 2 {
                                    views.diagnose(&core, sample.logical_cores, true);
                                    views.diagnosis_fresh = true;
                                }
                            }
                            Command::Range(range) => {
                                views.range = Anchored::new(range, sample.at_ns);
                            }
                            Command::ProcessRange(range) => {
                                views.process_range = Anchored::new(range, sample.at_ns);
                            }
                            Command::Select(process) => {
                                views.selected = process;
                                core.select(process);
                                views.process_timeline.invalidate();
                            }
                            Command::History(open) => {
                                views.history_open = open;
                                views.process_timeline.invalidate();
                            }
                            Command::PortsFilter(filter) => {
                                views.tools.ports_filter = filter;
                                views.tools.ports_fresh = true;
                            }
                            Command::CheckFile(path) => {
                                views.tools.check(&mut probe, &core, &sample, path);
                            }
                            Command::StartupPane(pane) => {
                                views.tools.pane = pane;
                                if pane == 1 && !views.tools.services_loaded {
                                    views.tools.services_dirty = true;
                                }
                            }
                            Command::StartupRefresh => {
                                if views.tools.pane == 1 {
                                    views.tools.services_dirty = true;
                                } else {
                                    views.tools.startup_dirty = true;
                                }
                            }
                            Command::SetStartup {
                                location,
                                name,
                                enabled,
                            } => {
                                notice = views
                                    .tools
                                    .set_startup(&mut probe, location, &name, enabled);
                            }
                            Command::ServiceFilter(filter) => {
                                views.tools.service_filter = filter;
                                views.tools.services_fresh = true;
                            }
                            Command::ControlService { name, start } => {
                                notice = views.tools.control(&mut probe, &name, start);
                            }
                            Command::Interval(chosen) => {
                                interval = chosen;
                                // Start the new pace now.
                                resample = true;
                            }
                            Command::Alerts(rules) => views.alert_rules = rules,
                            Command::Mini(open) => views.mini_open = open,
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
                                notice = execute(
                                    &mut probe,
                                    &core,
                                    &sample,
                                    pid,
                                    start_low_bits,
                                    action,
                                );
                                // Show the effect (a process gone, say) without waiting for the tick.
                                resample = true;
                                views.tools.after_action();
                            }
                        }
                    }
                    let mut icons = Vec::new();
                    views.tools.read(
                        &mut probe,
                        &core,
                        &sample,
                        &mut exes,
                        views.tab,
                        pace.visible,
                        &mut icons,
                    );
                    if !send(&window, &mut core, &sample, &mut views, icons, notice) {
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
        ProbeError::Critical => notice(Notice::CRITICAL, 0),
        _ => notice(Notice::FAILED, 0),
    };
    // The PID must still name the process the user saw.
    let Some(target) = core.identify(pid, start_low_bits) else {
        return notice(Notice::GONE, 0);
    };
    // Suspending Vigie would freeze this thread too, with nothing left to resume it.
    if target.pid == sample.self_pid && matches!(action, UserAction::Probe(Action::Suspend)) {
        return notice(Notice::SELF_SUSPEND, 0);
    }
    match action {
        UserAction::End => match probe.act(target, Action::End) {
            Ok(()) => notice(Notice::ENDED, 0),
            Err(error) => failure(error),
        },
        UserAction::EndTree => {
            // Refused before any child goes: a critical root would only be refused last.
            if probe.is_critical(target) {
                return notice(Notice::CRITICAL, 0);
            }
            // Children first, so none is orphaned and restarted by its parent meanwhile.
            let tree = core.subtree(target);
            let mut first_error = None;
            let mut children_ended = 0;
            let mut target_gone = false;
            for member in tree.iter().filter(|m| m.pid != sample.self_pid) {
                match probe.act(*member, Action::End) {
                    Ok(()) if *member != target => children_ended += 1,
                    Ok(()) => {}
                    // A child may exit on its own while its siblings are ended.
                    // Neither do critical children (left running) stop the rest.
                    Err(ProbeError::Gone | ProbeError::Critical) if *member != target => {}
                    Err(ProbeError::Gone) => target_gone = true,
                    Err(error) => {
                        first_error.get_or_insert(error);
                    }
                }
            }
            match first_error {
                Some(error) => failure(error),
                None if target_gone && children_ended == 0 => notice(Notice::GONE, 0),
                None => notice(Notice::ENDED_TREE, children_ended),
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

impl Views {
    /// Who is signed in to each session, while the Users view is shown: on opening, for new
    /// sessions, and every 10 s for connection changes.
    fn read_session_users(&mut self, probe: &mut impl Probe, sample: &Sample) {
        if self.tab != 8 {
            return;
        }
        let stale = self
            .session_users_at
            .is_none_or(|at| at.elapsed() >= Duration::from_secs(10));
        let sessions: std::collections::BTreeSet<u32> = sample
            .processes
            .iter()
            .filter_map(|p| p.session)
            .filter(|&s| s != 0)
            .collect();
        let new = sessions.iter().any(|s| !self.session_users.contains_key(s));
        if !stale && !new {
            return;
        }
        self.session_users.clear();
        for session in sessions {
            if let Some(user) = probe.session_user(session) {
                self.session_users.insert(session, user);
            }
        }
        self.session_users_at = Some(Instant::now());
    }

    /// The process table is on screen: Processes or Details.
    fn table_shown(&self) -> bool {
        self.tab == 0 || self.tab == 7
    }

    /// Details lists every process flat; Processes too when not grouped by type.
    fn flat(&self) -> bool {
        self.tab == 7 || (self.tab == 0 && !self.grouped)
    }
}

/// How often GPU counters are read while the Performance view is hidden, for the history.
const GPU_BACKGROUND: Duration = Duration::from_secs(10);
/// How often the pending restart and free disk space are read (decision 0010).
const MAINTENANCE_EVERY: Duration = Duration::from_secs(60);
/// How often the diagnostics run (decision 0006).
const DIAGNOSE_EVERY_NS: u64 = 5 * SECOND;

/// The views, the histories and the diagnostics.
#[derive(Default)]
struct Views {
    /// Shown view: 0 processes, 1 performance, 2 diagnostics, 7 details, 8 users.
    tab: i32,
    /// The Processes view groups by type; Details is always one flat list.
    grouped: bool,
    /// Users view: expanded sessions, and who is signed in to each session (read again every
    /// few seconds while the view is shown).
    users_expanded: std::collections::HashSet<u32>,
    session_users: HashMap<u32, vigie_probe::SessionUser>,
    session_users_at: Option<Instant>,
    /// Upkeep signals for the diagnostics, read every minute.
    maintenance: Option<vigie_probe::Maintenance>,
    maintenance_at: Option<Instant>,
    perf: Performance,
    system: SystemSample,
    info: SystemInfo,
    /// The last tick's machine-wide figures; `None` where the probe has none.
    last: Option<PerfFrame>,
    ticks: u32,
    gpus_at: Option<Instant>,
    /// First sample, for how long history has been recorded.
    started_ns: Option<u64>,
    range: Anchored,
    timeline: Timeline,
    process_range: Anchored,
    process_timeline: Timeline,
    history_open: bool,
    selected: Option<(Pid, u32)>,
    diagnostics: Diagnostics,
    findings: Vec<Finding>,
    diagnosed_ns: Option<u64>,
    /// The findings changed since the Diagnostics view last got them.
    diagnosis_fresh: bool,
    tools: Tools,
    alert_rules: Option<Rules>,
    alerts: Alerts,
    /// Notifications for the next frame.
    pending_alerts: Vec<Alert>,
    /// The tray mini view is open.
    mini_open: bool,
    /// When the tray tooltip was last refreshed.
    tooltip_at: Option<Instant>,
}

impl Views {
    /// Runs the rules every 5 s, or now when `force`d (the view just opened).
    fn diagnose(&mut self, core: &Core, logical_cores: u32, force: bool) {
        let Some(last) = &self.last else { return };
        let due = self
            .diagnosed_ns
            .is_none_or(|at| last.at_ns.saturating_sub(at) >= DIAGNOSE_EVERY_NS);
        if !(due || force) {
            return;
        }
        // The first ranking of processes happens after the first diagnosis: until processes
        // are tracked, run again on the next tick so findings name their culprits early.
        if core.tracker().iter().next().is_some() {
            self.diagnosed_ns = Some(last.at_ns);
        }
        self.diagnosis_fresh = true;
        self.findings = self.diagnostics.run(&Input {
            now_ns: last.at_ns,
            perf: &self.perf,
            last,
            tracker: core.tracker(),
            logical_cores,
            maintenance: self.maintenance.as_ref(),
        });
        if let Some(rules) = self.alert_rules {
            let due = self
                .alerts
                .evaluate(last.at_ns, &self.perf, &self.findings, rules);
            self.pending_alerts.extend(due);
        }
    }

    /// Seconds of history recorded so far.
    fn history_age_s(&self) -> u64 {
        match (&self.last, self.started_ns) {
            (Some(last), Some(start)) => last.at_ns.saturating_sub(start) / SECOND,
            _ => 0,
        }
    }
}

/// Builds a frame with what the shown view needs and hands it to the UI thread. False once the
/// event loop has quit.
fn send(
    window: &slint::Weak<AppWindow>,
    core: &mut Core,
    sample: &Sample,
    views: &mut Views,
    icons: Vec<(u32, Icon)>,
    notice: Option<Notice>,
) -> bool {
    let age = views.history_age_s();
    let perf = (views.tab == 1).then(|| match &views.last {
        Some(last) => model::perf_ui(
            last,
            &views.perf,
            &views.info,
            views.range,
            age,
            &mut views.timeline,
        ),
        None => model::PerfUi::unavailable(),
    });
    let mut frame = UiFrame::new(
        core.update(sample),
        views.table_shown(),
        icons,
        notice,
        perf,
    );
    if views.tab == 8 {
        frame.users = Some(model::user_rows_ui(
            &core.users(sample),
            &views.session_users,
            &views.users_expanded,
            sample.memory_total_bytes,
        ));
    }
    frame.badge = model::badge(&views.findings);
    if let Some(last) = &views.last {
        frame.total_cpu = model::format_total(last.cpu_percent);
        frame.total_memory =
            model::format_total((last.memory.total > 0).then_some(last.memory_used_percent));
    }
    frame.history_memory =
        model::format_kilobytes(views.perf.history_bytes() + core.tracker().bytes());
    // Only when the rules ran (or the view opened): the cards are left alone otherwise.
    if views.tab == 2 && views.diagnosis_fresh {
        views.diagnosis_fresh = false;
        frame.diagnostics = Some(match &views.last {
            Some(last) => {
                model::diagnostics_ui(&views.findings, &views.perf, last, core.tracker(), age)
            }
            None => model::DiagUi::unavailable(),
        });
    }
    if views.tab == 1 {
        frame.perf_offset = views.range.shown_offset(sample.at_ns);
    }
    frame.tools = std::mem::take(&mut views.tools.ui);
    frame.alerts = views
        .pending_alerts
        .drain(..)
        .map(|alert| model::alert_ui(&alert))
        .collect();
    // The tray tooltip, at most every 2 s.
    if views
        .tooltip_at
        .is_none_or(|at| at.elapsed() >= Duration::from_secs(2))
    {
        views.tooltip_at = Some(Instant::now());
        frame.tooltip = Some((frame.total_cpu.clone(), frame.total_memory.clone()));
    }
    if views.mini_open {
        frame.mini = Some(model::mini_ui(core, views.last.as_ref()));
    }
    if views.table_shown() && views.history_open {
        let tracked = views
            .selected
            .and_then(|(pid, bits)| core.identify(pid, bits))
            .and_then(|t| core.tracker().get((t.pid, t.start_time)));
        frame.process = model::process_ui(
            tracked,
            sample.at_ns,
            views.process_range,
            &mut views.process_timeline,
        );
        frame.process_offset = views.process_range.shown_offset(sample.at_ns);
    }
    let window = window.clone();
    slint::invoke_from_event_loop(move || model::apply(&window, frame)).is_ok()
}

/// What each executable says about itself, read once per path. Icons get sequential ids, the
/// index of the image in the UI's icon model.
#[derive(Default)]
struct ExeCache {
    by_path: HashMap<PathBuf, Details>,
    icon_ids: IconIds,
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
            let Some(&start) = starts.get(&found.pid) else {
                continue;
            };
            // What the executable says is shared by its processes; the account is per process.
            let mut details = match &found.exe {
                Some(exe) => Self::lookup(
                    &mut self.by_path,
                    &mut self.icon_ids,
                    self.icon_size,
                    probe,
                    exe,
                    &mut icons,
                ),
                None => Details::default(),
            };
            details.user = found.user.as_deref().map(Into::into);
            core.set_details((found.pid, start), details);
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

/// Icon ids, one per distinct image: the many executables sharing one icon (build outputs,
/// versioned copies of an app) share one image in the UI's model, which never shrinks.
#[derive(Default)]
struct IconIds {
    next: u32,
    by_pixels: HashMap<u64, u32>,
}

impl IconIds {
    /// The id of `icon`, pushed to `icons` (new to the UI) when no earlier image matches.
    fn id_of(&mut self, icon: Icon, icons: &mut Vec<(u32, Icon)>) -> u32 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        (icon.width, icon.height, &icon.rgba).hash(&mut hasher);
        *self.by_pixels.entry(hasher.finish()).or_insert_with(|| {
            let id = self.next;
            self.next += 1;
            icons.push((id, icon));
            id
        })
    }
}

impl ExeCache {
    /// What `exe` says about itself, read once; a new icon is added to `icons`.
    fn lookup(
        by_path: &mut HashMap<PathBuf, Details>,
        icon_ids: &mut IconIds,
        icon_size: u32,
        probe: &mut impl Probe,
        exe: &std::path::Path,
        icons: &mut Vec<(u32, Icon)>,
    ) -> Details {
        if let Some(details) = by_path.get(exe) {
            return details.clone();
        }
        let info = probe.exe_info(exe, icon_size);
        let icon = info.icon.map(|icon| icon_ids.id_of(icon, icons));
        let details = Details {
            icon,
            publisher: info.publisher.map(Into::into),
            description: info.description.map(Into::into),
            path: Some(exe.to_string_lossy().into()),
            user: None,
        };
        by_path.insert(exe.to_path_buf(), details.clone());
        details
    }

    fn details_of(
        &mut self,
        probe: &mut impl Probe,
        exe: &std::path::Path,
        icons: &mut Vec<(u32, Icon)>,
    ) -> Details {
        Self::lookup(
            &mut self.by_path,
            &mut self.icon_ids,
            self.icon_size,
            probe,
            exe,
            icons,
        )
    }
}

/// How often the Ports view reads the socket tables (decision 0007).
const PORTS_EVERY: Duration = Duration::from_secs(2);

/// State of the developer tools views (decision 0007). Every read happens on demand: while
/// Ports is shown, when a file is checked, when Startup opens or is refreshed.
#[derive(Default)]
struct Tools {
    sockets: Vec<Socket>,
    ports_filter: String,
    ports_at: Option<Instant>,
    /// The search changed: rebuild the rows from the last read.
    ports_fresh: bool,
    lock_path: Option<PathBuf>,
    lockers: Vec<Locker>,
    /// An action may have ended a holder: ask again then, once the process is gone.
    locks_again_at: Option<Instant>,
    startup: Vec<StartupItem>,
    startup_dirty: bool,
    /// 0 startup apps, 1 services.
    pane: i32,
    services: Vec<Service>,
    services_loaded: bool,
    services_dirty: bool,
    services_fresh: bool,
    service_filter: String,
    /// A started or stopped service takes a moment to settle: read again then.
    services_again_at: Option<Instant>,
    /// What the next frame carries.
    ui: model::ToolsUi,
}

impl Tools {
    fn opened(&mut self, tab: i32) {
        match tab {
            // Read at once, not 2 s later.
            3 => self.ports_at = None,
            5 => {
                if self.startup.is_empty() {
                    self.startup_dirty = true;
                }
                if self.pane == 1 && !self.services_loaded {
                    self.services_dirty = true;
                }
            }
            _ => {}
        }
    }

    /// After ending a process: the views that list processes change.
    fn after_action(&mut self) {
        self.ports_at = None;
        self.services_dirty |= self.services_loaded;
        if self.lock_path.is_some() {
            self.locks_again_at = Some(Instant::now() + Duration::from_millis(500));
        }
    }

    fn unsupported(&mut self, error: &ProbeError) -> bool {
        let unsupported = matches!(error, ProbeError::Unsupported);
        if unsupported {
            self.ui.available = Some(false);
        }
        unsupported
    }

    /// Reads what the shown view needs now and stages it in `ui`.
    #[allow(clippy::too_many_arguments)]
    fn read(
        &mut self,
        probe: &mut impl Probe,
        core: &Core,
        sample: &Sample,
        exes: &mut ExeCache,
        tab: i32,
        visible: bool,
        icons: &mut Vec<(u32, Icon)>,
    ) {
        if tab == 3 && visible {
            let due = self.ports_at.is_none_or(|at| at.elapsed() >= PORTS_EVERY);
            if due {
                self.ports_at = Some(Instant::now());
                if let Err(error) = probe.sockets(&mut self.sockets) {
                    self.unsupported(&error);
                    self.sockets.clear();
                }
            }
            if due || self.ports_fresh {
                self.ports_fresh = false;
                let rows = tools::port_rows(
                    &self.sockets,
                    core.labels(sample),
                    sample.self_pid,
                    &self.ports_filter,
                );
                self.ui.ports = Some(model::port_rows_ui(&rows));
            }
        }
        if tab == 5 && self.startup_dirty {
            self.startup_dirty = false;
            match probe.startup_items(&mut self.startup) {
                Ok(()) => tools::sort_startup(&mut self.startup),
                Err(error) => {
                    self.unsupported(&error);
                    self.startup.clear();
                }
            }
            let details: Vec<Option<Details>> = self
                .startup
                .iter()
                .map(|item| {
                    item.exe
                        .as_ref()
                        .map(|exe| exes.details_of(probe, exe, icons))
                })
                .collect();
            self.ui.startup = Some(model::startup_rows_ui(&self.startup, &details));
        }
        if tab == 5
            && self
                .services_again_at
                .is_some_and(|at| Instant::now() >= at)
        {
            self.services_again_at = None;
            self.services_dirty = true;
        }
        if tab == 5 && self.services_dirty {
            self.services_dirty = false;
            self.services_fresh = true;
            self.services_loaded = true;
            if let Err(error) = probe.services(&mut self.services) {
                self.unsupported(&error);
                self.services.clear();
            }
        }
        if tab == 5 && self.services_fresh {
            self.services_fresh = false;
            let rows = tools::service_rows(&self.services, &self.service_filter);
            self.ui.services = Some(model::service_rows_ui(&rows));
        }
        if tab == 4
            && self.locks_again_at.is_some_and(|at| Instant::now() >= at)
            && let Some(path) = self.lock_path.clone()
        {
            self.check(probe, core, sample, path);
        }
    }

    /// Who holds `path`: stages the answer for the Files view.
    fn check(&mut self, probe: &mut impl Probe, core: &Core, sample: &Sample, path: PathBuf) {
        let status = match probe.lockers(&path, &mut self.lockers) {
            Ok(()) => 1,
            Err(ProbeError::NotFound) => 2,
            Err(ProbeError::IsFolder) => 3,
            Err(error) => {
                self.unsupported(&error);
                4
            }
        };
        if status != 1 {
            self.lockers.clear();
        }
        let rows = tools::lock_rows(&self.lockers, core.labels(sample), sample.self_pid);
        self.ui.locks = Some(model::LocksUi {
            rows: model::lock_rows_ui(&rows),
            status,
            path: path.to_string_lossy().as_ref().into(),
        });
        self.lock_path = Some(path);
        self.locks_again_at = None;
    }

    fn set_startup(
        &mut self,
        probe: &mut impl Probe,
        location: StartupLocation,
        name: &str,
        enabled: bool,
    ) -> Option<Notice> {
        let kind = match probe.set_startup(location, name, enabled) {
            Ok(()) if enabled => Notice::STARTUP_ENABLED,
            Ok(()) => Notice::STARTUP_DISABLED,
            Err(ProbeError::Denied) => Notice::CHANGE_DENIED,
            Err(_) => Notice::CHANGE_FAILED,
        };
        self.startup_dirty = true;
        Some(Notice {
            kind,
            name: name.into(),
            pid: 0,
            count: 0,
        })
    }

    fn control(&mut self, probe: &mut impl Probe, name: &str, start: bool) -> Option<Notice> {
        let display = self
            .services
            .iter()
            .find(|s| s.name == name)
            .map_or(name, |s| s.display_name.as_str())
            .to_owned();
        let kind = match probe.control_service(name, start) {
            Ok(()) if start => Notice::SERVICE_STARTED,
            Ok(()) => Notice::SERVICE_STOPPED,
            Err(ProbeError::Denied) => Notice::CHANGE_DENIED,
            Err(ProbeError::DependentsRunning) => Notice::DEPENDENTS,
            Err(_) => Notice::CHANGE_FAILED,
        };
        self.services_dirty = true;
        self.services_again_at = Some(Instant::now() + Duration::from_millis(1500));
        Some(Notice {
            kind,
            name: display.into(),
            pid: 0,
            count: 0,
        })
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

    /// Records actions instead of touching any process, service or startup item.
    #[derive(Default)]
    struct FakeProbe {
        acted: Vec<(Pid, Action)>,
        refuse: Option<Pid>,
        controlled: Vec<(String, bool)>,
    }

    impl Probe for FakeProbe {
        fn sample(&mut self, _out: &mut Sample) -> Result<()> {
            Ok(())
        }
        fn details(&mut self, _pids: &[Pid], out: &mut Vec<ProcDetails>) -> Result<()> {
            out.clear();
            Ok(())
        }
        fn control_service(&mut self, name: &str, start: bool) -> Result<()> {
            self.controlled.push((name.to_owned(), start));
            match name {
                "Locked" => Err(ProbeError::Denied),
                "Needed" => Err(ProbeError::DependentsRunning),
                _ => Ok(()),
            }
        }

        fn set_startup(&mut self, location: StartupLocation, _name: &str, _on: bool) -> Result<()> {
            match location {
                StartupLocation::MachineRun => Err(ProbeError::Denied),
                _ => Ok(()),
            }
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
    fn service_and_startup_changes_say_what_happened() {
        let mut probe = FakeProbe::default();
        let mut tools = Tools::default();
        tools.services.push(Service {
            name: "Spooler".into(),
            display_name: "Print Spooler".into(),
            state: vigie_probe::ServiceState::Running,
            start_type: None,
            pid: None,
        });
        let notice = tools.control(&mut probe, "Spooler", false).unwrap();
        assert_eq!(
            (notice.kind, notice.name.as_str()),
            (Notice::SERVICE_STOPPED, "Print Spooler")
        );
        assert!(tools.services_dirty, "read again to show the new state");
        assert_eq!(
            tools.control(&mut probe, "Locked", true).unwrap().kind,
            Notice::CHANGE_DENIED
        );
        assert_eq!(
            tools.control(&mut probe, "Needed", false).unwrap().kind,
            Notice::DEPENDENTS
        );
        assert_eq!(probe.controlled.len(), 3);
        let off = tools
            .set_startup(&mut probe, StartupLocation::UserRun, "App", false)
            .unwrap();
        assert_eq!(
            (off.kind, off.name.as_str()),
            (Notice::STARTUP_DISABLED, "App")
        );
        let denied = tools
            .set_startup(&mut probe, StartupLocation::MachineRun, "Tool", true)
            .unwrap();
        assert_eq!(denied.kind, Notice::CHANGE_DENIED);
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
