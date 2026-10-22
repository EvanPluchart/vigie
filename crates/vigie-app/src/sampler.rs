//! The sampler thread: owns the probe, runs the core, sends frames to the UI thread.

use std::thread;
use std::time::{Duration, Instant};

use vigie_core::{Core, Pid, Sort};
use vigie_probe::{Probe, Sample, SystemProbe};

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
    let mut next_tick = Instant::now();
    loop {
        match probe.sample(&mut sample) {
            Ok(()) => {
                if !send(&window, &mut core, &sample) {
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
                    if !send(&window, &mut core, &sample) {
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
fn send(window: &slint::Weak<AppWindow>, core: &mut Core, sample: &Sample) -> bool {
    let frame = UiFrame::from(core.update(sample));
    let window = window.clone();
    slint::invoke_from_event_loop(move || model::apply(&window, frame)).is_ok()
}
