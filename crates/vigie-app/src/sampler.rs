//! The sampler thread: owns the probe, runs the core, sends frames to the UI thread.

use std::collections::HashMap;
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};

use vigie_core::{Core, Details, Pid, Sort};
use vigie_probe::{Icon, Probe, ProcDetails, Sample, SystemProbe};

use crate::AppWindow;
use crate::model::{self, UiFrame};

/// What the UI asks of the sampler. Sorting and filtering run here, not on the UI thread.
#[derive(Debug)]
pub enum Command {
    Sort(Sort),
    Filter(String),
    /// Expand or collapse the subtree of this PID.
    Toggle(Pid),
}

pub struct Settings {
    pub sort: Sort,
    pub filter: String,
    pub interval: Duration,
    /// Icon side in physical pixels.
    pub icon_size: u32,
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
    loop {
        match probe.sample(&mut sample) {
            Ok(()) => {
                let icons = exes.fill(&mut probe, &mut core, &sample);
                if !send(&window, &mut core, &sample, icons) {
                    return;
                }
            }
            Err(err) => eprintln!("vigie: sampling failed: {err}"),
        }
        next_tick += settings.interval;
        // A late tick (machine asleep, debugger) starts a fresh schedule instead of catching up.
        if next_tick < Instant::now() {
            next_tick = Instant::now() + settings.interval;
        }
        // Between ticks, answer commands right away by reusing the last sample.
        loop {
            let wait = next_tick.saturating_duration_since(Instant::now());
            match commands.recv_timeout(wait) {
                Ok(command) => {
                    match command {
                        Command::Sort(sort) => core.set_sort(sort),
                        Command::Filter(filter) => core.set_filter(&filter),
                        Command::Toggle(pid) => core.toggle(pid),
                    }
                    if !send(&window, &mut core, &sample, Vec::new()) {
                        return;
                    }
                }
                Err(flume::RecvTimeoutError::Timeout) => break,
                Err(flume::RecvTimeoutError::Disconnected) => return,
            }
        }
    }
}

/// Builds a frame and hands it to the UI thread. False once the event loop has quit.
fn send(
    window: &slint::Weak<AppWindow>,
    core: &mut Core,
    sample: &Sample,
    icons: Vec<(u32, Icon)>,
) -> bool {
    let frame = UiFrame::new(core.update(sample), icons);
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
