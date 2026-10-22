# 0005 — Performance view: where each figure comes from (Windows)

**Status:** accepted, 2026-10-22. Follows decision 0002: native calls, each measured.

## Context

M2 adds machine-wide figures, read every tick: CPU per core, memory, network, disks, GPU. Budget: the whole sampler under 3 ms per tick, the app under 1 % of one core, no allocation in the steady state. Measured with criterion on Windows 11 (Ryzen 5 7600X, 12 logical processors, two AMD GPUs).

## Decision

| Figure | Source | Cost per read |
|---|---|---|
| CPU per core | `NtQuerySystemInformation(SystemProcessorPerformanceInformation)`; `KernelTime` includes idle, one entry per logical processor (both checked by tests) | |
| Memory | `GlobalMemoryStatusEx`, `K32GetPerformanceInfo` (cache, commit); compressed = working set of the Memory Compression process, already sampled | |
| Uptime, CPU model | `GetTickCount64`; registry `CentralProcessor\0\ProcessorNameString` (once) | |
| CPU + memory + uptime together | | 71 µs |
| Network | Physical, non-filter interfaces listed with `GetIfTable2` every 30 reads; `GetIfEntry2` on those only in between (`GetIfTable2` alone costs ~1 ms, mostly virtual interfaces) | +150 µs |
| Disks | `IOCTL_DISK_PERFORMANCE` on `\\.\PhysicalDriveN`, opened once with no access rights (allowed without elevation), rescanned every 60 reads | +20 µs |
| **All of the above** | `Probe::system`, every tick | **~0.16–0.24 ms** |
| GPU utilisation | D3DKMT node statistics: running time per engine; utilisation = busiest engine, as Task Manager shows it | see below |
| GPU memory | D3DKMT segment statistics, non-aperture segments | ~2 µs per segment |

**GPU engines are expensive.** Each engine query is a driver round trip: about 0.4 ms of CPU on AMD drivers, 8.4 ms for 19 engines on two GPUs. So:

* `Probe::gpus` is separate from `Probe::system` and runs only while the Performance view is shown, every other tick.
* Engines idle for 10 reads are skipped and checked again every 30 reads.
* Steady state: about 0.4 ms per read in a tight loop; in real use, under 0.1 % of one core.
* Some drivers report nonsense running times (negative, or jumps longer than the interval). Counters wrap; deltas longer than 1.5 × the interval count as 0.
* Microsoft Basic Render Driver and unnamed virtual adapters are not listed.

**No PDH.** Performance counters would give GPU engines directly, but load perflib and counter providers into Vigie: more memory than the whole budget leaves, and still per-engine work underneath. Not adopted; measure again if D3DKMT turns out wrong on other drivers.

## Consequences

* The Performance view costs about 0.1–0.4 % of one core more than the Processes view, mostly for repainting the graphs (area fills about 0.13 %).
* GPU figures refresh every 2 s; an engine waking up after a long idle may show up to 30 s late. Acceptable for a view of averages; revisit if users notice.
* Linux and macOS: `Probe::system` and `Probe::gpus` return `Unsupported` until native code lands; the view says so.
