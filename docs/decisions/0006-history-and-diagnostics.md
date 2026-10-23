# 0006 — History tiers and diagnostics

Status: accepted, 2026-10-23 (M3).

## Context

M3 asks for a scrollable history on every graph and per process, and for a "why is my computer slow?" view whose findings have evidence and one suggested action each. The budget leaves almost no room: at the end of M2, Vigie used 0.94 % of one core and 29.1 MB of working set against 1 % and 30 MB.

## Decision

### Buckets are keyed by time, not by tick

The tick interval changes: 1 s with focus, 4 s in the background, 10 s minimised, anything with `--interval`. A history that stores one point per tick shows a false time axis: the one-minute graph of M2 covered four minutes in the background.

Each tier therefore keeps one bucket per period, indexed by `time / period`:

* A value recorded in an open bucket adds to its mean and, on coarse tiers, to its peak.
* When ticks are farther apart than a period, the buckets in between take the value that ends the gap. Rates are measured over the whole gap, so that value is right for every second of it.
* Gaps longer than 60 s, such as sleep, stay empty (NaN) and break the graph line. Nothing is made up.
* Every tier is fed with the raw values. Coarse tiers are not fed from finer ones.

### Tiers

| Series | Tiers | Peaks |
|---|---|---|
| CPU, network in and out, each disk, each GPU | 1 s × 600 (10 min), 10 s × 2160 (6 h), 1 min × 10 080 (7 days) | on the 10 s and 1 min tiers |
| Memory in use (a gauge) | same | none |
| Each tracked process, CPU and memory | 1 s × 600 (10 min), 1 min × 360 (6 h) | on the 1 min tier |

* Graphs draw the mean as a line and the peak as the filled area, so short spikes stay visible on long ranges.
* Values are `f32`.
* Buffers grow in steps until a tier is full, then turn into a ring, so memory follows uptime.
* History lives in memory only. It starts empty at each launch: the 7-day range fills only if Vigie keeps running.

### Per-process history: 32 slots

* Every 10 s, Vigie ranks processes by CPU and by memory. The top 12 of each list get a slot when there is one free. When the slots are all taken, a newcomer replaces the tracked process that matters least, measured as the larger of its CPU share and its memory share.
* The process the user selects is pinned. It gets a slot at once and keeps it while selected.
* A slot is freed when its process exits.
* Vigie itself is never tracked.
* Ranking uses a partial select over the processes of the tick, and runs only when due.

### Memory cost, computed

| Part | After 10 min | After 7 days |
|---|---|---|
| One metric with peaks | 2.4 KB (+ a few coarse buckets) | 100 KB |
| Memory gauge | 2.4 KB | 51 KB |
| One tracked process (CPU and memory) | 4.8 KB | 10.6 KB |
| This machine: 3 metrics, 2 disks, 2 GPUs, memory, 32 slots | about 175 KB | about 1.1 MB |

* This is heap: it counts in private bytes and, once touched, in the working set.
* The About window shows the live figure.

### Diagnostics

The rules are pure functions over the last figures and the histories (`vigie-core/src/diagnose.rs`).

* **Hysteresis:** each rule raises at one threshold and clears at a lower one.
* **Cadence:** the sampler runs the rules every 5 s, and as soon as the Diagnostics view opens.
* **Translation:** findings are data (kind, figures, processes). The UI words them through `@tr()`.

| Finding | Raised when | Clears when | Evidence | Suggestion |
|---|---|---|---|---|
| CPU saturated (critical) | mean ≥ 90 % over 2 min, half the window measured | mean < 75 % | top 3 tracked processes by mean CPU over the window | end the first one (asks first); leave it alone if it belongs to the system |
| One process on a full core (warning) | mean ≥ 0.9 core over 5 min, 80 % of the window watched | < 0.6 core | the process | same |
| Memory pressure (warning, critical when high) | `Pressure` medium or high (decision 0005 thresholds), with its cause: available memory or commit | available ≥ 25 % and commit ≤ 75 % | figures and top 3 by memory | same, or "close programs" when no process is tracked |
| Memory keeps growing (warning) | linear fit over ≥ 30 one-minute buckets (up to 6 h): growth ≥ 256 MB, ≥ 25 % of the start, R² ≥ 0.8, last value ≥ 95 % of the peak | growth < 128 MB or R² < 0.6 | from and to, and over how long | restart the app (end it) |
| Disk busy (warning) | mean active ≥ 90 % over 1 min | < 70 % | the disk and its drive letters | open the Performance view; per-process disk I/O is not read, so no process is named |
| Long uptime (info) | up for 14 days or more | — | days | restart (a real restart: fast startup keeps the kernel running across shutdowns) |

* Suggestions never act by themselves.
* Ending a process goes through the same confirmation as the table. It names the target and checks its identity (PID and start time).
* System processes are never offered for ending.

## Consequences

* The time axis is true in every refresh mode, and the M2 graphs get this fix for free.
* Diagnostics can only name processes that are tracked. A burst from an untracked process shows up in the machine figures first, and the next ranking round, within 10 s, picks the process up.
* Graph paths are rebuilt only when the visible range has a new bucket, or when the user changes or pans the range.
* Disk findings cannot name a process until the probes read per-process I/O.
