# UX and UI audit: Performance view (M2)

Design track D3. Windows 11, dark and light themes, default window (820 × 600) and a tall one
(980 × 1040), build of 2026-10-22. Severity: **P1** blocks daily use, **P2** costs time or trust,
**P3** polish.

## Findings

| # | Sev. | Finding | Fix | Status |
|---|---|---|---|---|
| 1 | P2 | Graphs stopped at about 93 % of their width: `Path` kept the 3:1 viewbox aspect. | `fit: fill` on every graph path. | Fixed |
| 2 | P2 | Disks were "Disk 0", "Disk 1": nobody knows their disks by number. | Drive letters from the volumes on each disk: "Disk 0 (C:)". | Fixed |
| 3 | P2 | No keyboard way between views. | Ctrl+1 Processes, Ctrl+2 Performance (rail order). | Fixed |
| 4 | P2 | A virtual display adapter showed as "GPU 2" with no memory. | Unnamed adapters (remote desktop, capture drivers) and Microsoft Basic Render Driver are not listed. | Fixed |
| 5 | P2 | On a 600 px tall window, network, disks and GPUs sit below the fold; nothing says there is more. | Scroll bar shows; consider a compact two-column layout or a resource list on the left (Task Manager style) at M5. | Open |
| 6 | P3 | Graphs have no time axis: "one minute" is implicit. | A discreet "60 s" caption under the first graph, or on hover once Slint has tooltips. | Open |
| 7 | P3 | Memory pressure reads "Medium" while RAM looks fine: it comes from commit (81 % of the limit). | Say why: "Medium (commit 81 %)". Diagnostics (M3) explain it fully. | Open |
| 8 | P3 | GPU cards show the busiest engine only; no 3D / video / copy breakdown. | Engine types from D3DKMT node metadata, as a small legend. | Open |
| 9 | P3 | Per-core bars have no labels; with many cores they get thin. | Tooltip per core later; switch to a heat grid above 32 cores. | Open |
| 10 | P3 | CPU and memory cards end at different heights; empty space in the CPU card. | Balance the cards (share the row height, move uptime to a footer). | Open |
| 11 | P3 | Linux and macOS show "Performance figures are not available on this system yet." | Native probes (sysinfo bridge or `/proc`, `host_statistics64`). | Open (platform work) |

## Performance of the view

* `bench-overhead`, 60 s at 1 s, ~434 processes, same machine and load: Performance view 0.82 %, Processes view 0.94 % of one core; 29.1 MB working set, 8.9 MB private.
* Area fills under the graphs cost about 0.13 % (kept, as the art direction asks).
* The working set grows with the window: a 980 × 1040 window reached 30.9 MB (larger frame buffer). Private memory does not change.
* GPU engines are read only while the view is shown, every other tick (decision 0005).
