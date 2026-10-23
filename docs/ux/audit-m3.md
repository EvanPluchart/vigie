# UX and UI audit: history and diagnostics (M3)

Design track D3, run on the build of 2026-10-23 under Windows 11, in the dark and light themes.

Three areas are covered:

* the Diagnostics view;
* the range bar and readout on the Performance graphs;
* the history panel of the selected process.

The machine was under real memory pressure during the review: 95 % in use, 2 to 6 % available, and commit at 93 % of its limit. The CPU was at 40 to 75 %.

Severity: **P1** blocks daily use, **P2** costs time or trust, **P3** polish.

## Findings

| # | Sev. | Finding | Fix | Status |
|---|---|---|---|---|
| 1 | P2 | The first diagnosis ran before any process was tracked. "Memory is running low" suggested "close programs" without naming one, for the first 5 s. | The rules run again on each tick until the first ranking has tracked processes. | Fixed |
| 2 | P2 | The memory finding suggested leaving "Memory Compression" alone. It was the top consumer, but it is a symptom of the pressure, not its cause. | The suggestion targets the first culprit outside the system. System processes stay listed as evidence. | Fixed |
| 3 | P3 | Evidence lines showed "—" for CPU on memory findings. | Each line shows the figure the finding is about: CPU or memory. | Fixed |
| 4 | P2 | The history toggle used the native checked style. It looked like the orange primary "End task" button next to it. | A plain button that reads "Show history" or "Hide history". | Fixed |
| 5 | P3 | The history panel clipped the bottom of its graphs, and spread its labels away from them. | Taller panel, rows aligned to the top. | Fixed |
| 6 | P3 | A history of one bucket (a process just selected) drew nothing. | Lone values draw as a short dash. | Fixed |
| 7 | P2 | The column headers did not say how busy the machine is: Task Manager shows "34 % CPU" above its columns. Raised by the author. | Machine CPU and memory in use, before the CPU and Memory labels. | Fixed |
| 8 | P3 | The readout under the pointer cannot be captured without synthetic input. It is checked by hand only. | Manual check list below. | Open |
| 9 | P3 | Long ranges (24 h, 7 days) stay mostly empty on a fresh start, because history lives in memory only. | The range bar shows what exists, and panning stops there. Persisting history is not planned (decision 0006). | Accepted |
| 10 | P3 | Wording is Windows-first: fast startup, the page file. | Reword per system when the Linux and macOS probes land. | Open |

## Cost

`bench-overhead` ran for 60 s on the Processes view, on the same busy machine (about 470 processes), alternating builds. Values are after the 5 s warm-up.

| Run | M2 build | M3 build |
|---|---|---|
| 1 | 1.05 % | 1.53 % |
| 2 | 1.16 % | 1.11 % |

* Private memory: 9.3 to 10.3 MB for M2, 10.4 to 10.7 MB for M3.
* The machine was loaded, so the 1 % budget, defined on an idle machine, cannot be judged from these runs. Re-measure on a quiet machine.
* The computed history memory shows in the About window: about 175 KB after 10 minutes, about 1.1 MB after 7 days (decision 0006).

## Manual checks

1. **Diagnostics (Ctrl+3):** the rail shows a dot when something needs a look (orange for a warning, red when critical). Every finding has an explanation and one suggestion. "End task…" opens the same confirmation as the table and names the process.
2. **Performance (Ctrl+2):** pick 10 min, then hover a graph. The readout shows the value and how long ago it was. Drag right to go back in time; "Now" returns to the present.
3. **Processes:** select a process and press "Show history" (Ctrl+H). CPU and memory are drawn over 1 min to 6 h. A process that was idle until selected starts its history at that moment.
