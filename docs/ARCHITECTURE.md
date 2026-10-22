# Architecture

Vigie is a Cargo workspace with two library layers and one binary. Two rules shape everything:

1. **The UI never asks the OS for anything, and the probes never know about the UI.**
2. **Vigie's own cost is a feature.** Every design choice is judged by what it costs per refresh.

```
crates/
  vigie-probe   per-OS sampling behind one trait: processes, CPU, memory, disk, network, GPU   (no UI)
  vigie-core    process tree, app grouping, CPU% from deltas, history ring buffers, alerts,     (no UI, no OS calls)
                diagnostics
  vigie-app     Slint application, binary "vigie"; ui/*.slint holds every view
xtask/          bench-overhead, bundle, release                                                 (M0)
```

## Layers

### vigie-probe: sampling

```rust
pub trait Probe: Send {
    /// One pass over the system. Cheap fields only: called every tick.
    fn sample(&mut self, out: &mut Sample) -> Result<()>;
    /// Expensive, rarely changing fields (exe path, command line, publisher, icon key) for some PIDs.
    fn details(&mut self, pids: &[Pid], out: &mut Vec<ProcDetails>) -> Result<()>;
}
```

* `sample` fills a caller-owned `Sample` so buffers are reused between ticks: **no allocation in the steady state.**
* Cheap vs expensive split: `sample` returns PID, parent PID, name, CPU time, memory, I/O counters, start time. `details` is only called for PIDs that are new or currently visible, and its results are cached by `(pid, start_time)` (a PID can be reused, a pair cannot).
* Backends:
  * **Windows:** one `NtQuerySystemInformation(SystemProcessInformation)` call per tick returns every process with its times and memory. No `OpenProcess` per process in the hot path.
  * **Linux:** `/proc/<pid>/stat` and `statm`, read with reused buffers. `/proc/stat`, `/proc/meminfo` for the system.
  * **macOS:** `proc_listallpids` + `proc_pid_rusage`, `host_statistics64` for the system.
* Start with `sysinfo` behind the trait where a native backend is not written yet (decision 0002). Replace a path with native code only when `cargo xtask bench-overhead` shows it matters.

### vigie-core: the model

* **CPU %** comes from deltas between two samples (CPU time / wall time / logical cores). The first sample of a process shows no CPU % rather than a wrong one.
* **Process tree and app grouping:** children are attached to their parent; processes are grouped into apps by executable path and window ownership (Windows), bundle (macOS) or `.desktop` file (Linux).
* **History:** fixed-size ring buffers per metric, in three tiers: 1 s for 10 min, 10 s for 6 h, 1 min for 7 days. Per-process history only for the top consumers of each tier, to keep memory bounded. Memory cost of the history is computed and shown in the About window.
* **Diagnostics** ("why is my computer slow?") are pure functions over the current sample and the history: memory pressure, a process growing without bound, sustained CPU, disk saturation. Each one returns a finding with evidence and a suggested action.
* **Alerts** are rules (`metric`, `threshold`, `duration`, `scope`) evaluated on each tick.

### vigie-app: the interface

* All views live in `ui/*.slint`. Rust only feeds models and handles callbacks.
* **One sampler thread** owns the `Probe`. It samples, runs `vigie-core`, and sends a compact `Frame` (only what the visible views need) to the UI thread with `slint::invoke_from_event_loop`.
* **Row models are patched, never rebuilt.** The process table uses a `VecModel` updated with `set_row_data` for changed rows and targeted inserts and removals. Slint then redraws only those rows.
* **Sorting and filtering happen in Rust** on the sampler thread, so the UI thread does almost nothing per tick.
* **Adaptive refresh:** the chosen interval when focused, 4× slower when unfocused, paused when minimised (history keeps a slow 10 s sample).
* **Self-cost:** Vigie samples itself with the same probe and shows CPU % and memory in the status bar.
* **Text** uses Slint's `@tr()` with bundled translations. Source strings are English; French is the first translation.

## Data flow

```
sampler thread:  tick ──> Probe.sample() ──> core.update(sample) ──> diagnostics, alerts, history
                                                    │
                                                    └─> Frame (visible rows, graph points, findings)
                                                               │ invoke_from_event_loop
UI thread:                                                     └─> patch models ──> Slint redraws changed rows
UI callbacks (sort, filter, kill, suspend) ──> command channel ──> sampler thread
```

## Privileges

* Vigie never asks for elevation at start. What it cannot read without privileges is shown as "—" with a tooltip, never as 0.
* Windows: a "Restart as administrator" action. macOS: some details of other users' processes stay unavailable; no root helper before M6. Linux: `/proc` gives most data without root.
* Destructive actions (end task, end tree, change priority of a system process) ask first and name the process.

## Conventions

* Code, comments and commit messages in English.
* No `unwrap()` on anything that depends on the OS. A probe failure for one process skips that process, it never fails the tick.
* Test flags on the binary, for screenshots and agents: `vigie [--tab=processes|performance|history|ports] [--sort=cpu|memory|name|pid[:asc]] [--filter=TEXT] [--interval=MS] [--dark|--light] [--about]`.

## Performance budget

| Metric | Budget | How it is checked |
|---|---|---|
| CPU while focused, 1 s interval, ~300 processes | < 1 % of one core on average | `cargo xtask bench-overhead` (60 s run, reads Vigie's own CPU time) |
| Memory (RSS) | < 30 MB | same run |
| `Probe::sample` | < 3 ms for 500 processes | criterion benchmark in `vigie-probe` |
| Steady-state allocations per tick | 0 in `sample` | counting allocator in a test |

A change that breaks the budget does not merge.
