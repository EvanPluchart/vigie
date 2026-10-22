# Roadmap

Each milestone ends with something you can run and show. Estimates are evenings of side-project time.

## M0 — Skeleton (1 week)

* Workspace compiles on Windows, macOS and Linux (GitHub Actions matrix).
* `vigie` opens a Slint window with a live process table (name, PID, CPU %, memory), refreshed every second by the sampler thread.
* Status bar shows Vigie's own CPU % and memory.
* `cargo xtask bench-overhead` measures Vigie over 60 s and prints the result.

**Done when:** the window runs on the three systems and the overhead numbers are printed (the budget is enforced from M1).

## Design track (runs alongside the milestones)

Vigie should look as good as it is light. Design work has its own deliverables, reviewed with screenshots.

* **D1 — Identity:** logo and app icon (readable at 16 px in the taskbar and the tray), palette, typography, art direction, short brand guide in `docs/BRAND.md`, masters in `assets/brand/`.
* **D2 — Interface art direction (before M1 ends):** design tokens as a Slint global (colours for dark and light, type scale, spacing, radii), table density, heat colours, status bar; applied over the native styles without fighting them.
* **D3 — UX pass per milestone:** each milestone ends with a screenshot review of its views (empty, loading, error and privilege-limited states included) and a list of fixes.

**Done when:** each item has its screenshots and its decision recorded; D2 lands before M1 is called done.

## M1 — Processes (2 weeks)

* Apps / background processes / system grouping, expandable process tree.
* Sort by any column, search by name, PID or publisher, column chooser.
* Heat colouring of hot cells (like Windows, but readable in dark mode).
* End task, end tree, suspend and resume, priority, open file location, copy command line.
* Icons per app, cached.
* Performance budget enforced in CI.

**Done when:** it replaces the Windows Task Manager "Processes" tab for daily use, within budget.

## M2 — Performance (2 weeks)

* CPU (total and per core), memory (used, cached, compressed, pressure), disks, network, GPU (Windows first).
* Smooth graphs drawn with Slint `Path`, no chart library.
* System info: uptime, CPU model, RAM slots when available.

## M3 — History and diagnostics (2–3 weeks)

* Ring-buffer history, scrollable timeline on every graph and per process.
* "Why is my computer slow?": findings with evidence and one suggested action each.

## M4 — Developer tools (2–3 weeks)

* Ports: which process listens on or connects to which port, kill from there.
* Who locks this file: drop a file, get the processes holding it (Windows Restart Manager, `lsof`-style on macOS and Linux).
* Startup apps and services.

## M5 — Polish

* Alerts with system notifications.
* Command palette, full keyboard navigation.
* Tray / menu bar mini view.
* Settings: interval, theme, columns, language.

## M6 — Distribution

* Windows installer, signed macOS `.dmg`, AppImage and Flatpak.
* "Replace Task Manager" option on Windows (HKCU, reversible).
* Landing page on evanpluchart.fr.

## Later, maybe

Network traffic per process on all systems, energy impact on laptops, remote monitoring of another machine, a CLI (`vigie top`).
