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

* **D1 — Identity (done, approved 2026-10-22):** logo and app icon (readable at 16 px in the taskbar and the tray), palette, typography, art direction, short brand guide in `docs/BRAND.md`, masters in `assets/brand/`.
* **D2 — Interface art direction (done 2026-10-22, `ui/theme.slint`):** design tokens as a Slint global (colours for dark and light, type scale, spacing, radii), table density, heat colours, status bar; applied over the native styles without fighting them.
* **D3 — UX pass per milestone (M1: `docs/ux/audit-m1.md`, M2: `docs/ux/audit-m2.md`, M3: `docs/ux/audit-m3.md`, M4: `docs/ux/audit-m4.md`, M5: `docs/ux/audit-m5.md`, M6: `docs/ux/audit-m6.md`, M7: `docs/ux/audit-m7.md`):** each milestone ends with a screenshot review of its views (empty, loading, error and privilege-limited states included) and a list of fixes.

**Done when:** each item has its screenshots and its decision recorded; D2 lands before M1 is called done.

## M1 — Processes (2 weeks)

* Apps / background processes / system grouping, expandable process tree.
* Sort by any column, search by name, PID or publisher, column chooser.
* Heat colouring of hot cells (like Windows, but readable in dark mode).
* End task, end tree, suspend and resume, priority, open file location, copy command line.
* Icons per app, cached.
* Performance budget enforced in CI.

**Done when:** it replaces the Windows Task Manager "Processes" tab for daily use, within budget.

**Status (2026-10-22):** features done on Windows and tried hands-on by the author; UX audit in `docs/ux/audit-m1.md`. Pending, at the end with CI: budget enforcement in CI. Open questions on the budget itself: CPU measured on an idle machine (~300 processes) or under real load, memory as working set or private bytes (see the audit, item 9).

## M2 — Performance (2 weeks)

* CPU (total and per core), memory (used, cached, compressed, pressure), disks, network, GPU (Windows first).
* Smooth graphs drawn with Slint `Path`, no chart library.
* System info: uptime, CPU model, RAM slots when available.

**Status (2026-10-22):** done on Windows (sources and costs in decision 0005, UX audit in `docs/ux/audit-m2.md`). Linux and macOS show that the figures are not available yet. Pending, at the end with CI: the three-system check.

## M3 — History and diagnostics (2–3 weeks)

* Ring-buffer history, scrollable timeline on every graph and per process.
* "Why is my computer slow?": findings with evidence and one suggested action each.

**Status (2026-10-23):** done on Windows (decision 0006, UX audit in `docs/ux/audit-m3.md`). History is in memory only: the 7-day range fills only while Vigie keeps running.

## M4 — Developer tools (2–3 weeks)

* Ports: which process listens on or connects to which port, kill from there.
* Who locks this file: drop a file, get the processes holding it (Windows Restart Manager, `lsof`-style on macOS and Linux).
* Startup apps and services.

**Status (2026-10-23):** done on Windows (decision 0007, UX audit in `docs/ux/audit-m4.md`). Vigie now draws its own controls from the design tokens. Linux and macOS show that these views are not available yet.

## M5 — Polish

* Alerts with system notifications.
* Command palette, full keyboard navigation.
* Tray / menu bar mini view.
* Settings: interval, theme, columns, language.

**Status (2026-10-23):** done on Windows (decision 0008, UX audit in `docs/ux/audit-m5.md`). French is bundled. The budget is now measured in private memory on an idle machine. Linux and macOS have no tray yet.

## M6 — Distribution

* Windows installer, signed macOS `.dmg`, AppImage and Flatpak.
* "Replace Task Manager" option on Windows (reversible; Image File Execution Options in HKLM, so one administrator prompt each way: decision 0009).
* Landing page on GitHub Pages (French and English).

**Status (2026-10-23):** version 0.1.0. The Windows installer is built and tested here (install, launch, uninstall, nothing left). The macOS `.dmg` and the Linux AppImage are built by the release workflow, unsigned until certificates exist. Replacing Task Manager is waiting for the author's manual check. The landing page is in `site/` (French and English, `site/DESIGN.md`), to publish with `pages.yml`. Flatpak is deferred (decision 0009). Audit in `docs/ux/audit-m6.md`.

## M7 — First-use feedback

* Vigie's own context menus everywhere, with Task Manager's entries and shortcuts.
* Details and Users views; group by type, expand all, collapse all.
* Resizable columns, more columns (status, user name, description, threads, handles, I/O), resizable history panel.
* Upkeep findings: restart pending for updates, system drive almost full.

**Status (2026-10-23):** done on Windows (decision 0010, audit in `docs/ux/audit-m7.md`, which maps each remark of the author's first test to its fix). Network per process and history across restarts stay open. 0.1.1 (2026-10-24) fixes a Vigie that kept running unseen after its window closed, which then blocked every later launch, Ctrl+Shift+Esc included.

## Later, maybe

Network traffic per process on all systems, energy impact on laptops, remote monitoring of another machine, a CLI (`vigie top`).
