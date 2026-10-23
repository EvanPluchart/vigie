# 0004 — Windows probe: basic list every tick, handles for what we may open, full snapshot for the rest

**Status:** accepted, 2026-10-22. Refines decision 0002 for Windows.

## Context

Decision 0002 planned one `NtQuerySystemInformation(SystemProcessInformation)` call per tick. Implemented and measured on Windows 11 (about 460 processes, about 7,000 threads, not elevated):

* 7.7 ms per sample, almost all of it inside the kernel call, which also describes every thread (about 1 MB per call).
* Whole app: 1.44 % of one core. Budget: `sample` < 3 ms, app < 1 %.

Measured alternatives:

| Approach | Cost per tick |
|---|---|
| `SystemProcessInformation` (class 5) | 8.1 ms |
| `SystemBasicProcessInformation` (class 252: PID, parent, sequence number, name, no threads) | 0.07 ms |
| Class 252 + `GetProcessTimes` + `K32GetProcessMemoryInfo` on cached handles | 1.2 ms |

Without elevation, `OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION)` fails for about 175 of 460 processes (services, protected processes); for those, class 5 is the only source.

## Decision

The Windows probe is hybrid:

* Every tick, class 252 lists the processes. A changed sequence number for a PID means a new process.
* A process gets a handle once, when first seen. Each tick reads its times and working set from that handle.
* Processes that cannot be opened are read from a full class 5 snapshot every 5 ticks, and immediately when such a process appears (so it always has a start time and a first value).
* Each `ProcSample` carries `measured_at_ns`. The core computes CPU % over the real interval between two measurements and keeps the last value between refreshes, instead of dropping to 0.
* Windows versions without class 252 (before recent Windows 11 builds) fall back to one class 5 snapshot per tick.

## Results

* `Probe::sample`: 7.7 ms to 2.0 ms average (criterion, 448 processes).
* App: 1.44 % to 0.94 % of one core in steady state, 25.9 MB RSS (`cargo xtask bench-overhead`, 60 s at 1 s).
* Zero allocations per tick in the steady state (test with a counting allocator).

## Consequences

* For processes that cannot be opened, CPU % and memory refresh every 5 s instead of every second. The value is still an exact average over that interval. Run elevated, almost every process gets a handle and refreshes every second.
* Class 252 and the full `SYSTEM_PROCESS_INFORMATION` layout are undocumented (taken from the reverse-engineered NT headers). Both are covered by tests that compare against the full snapshot and against `sysinfo`.
* Vigie keeps one handle per process it can open (about 300). Handles close when the process exits.
