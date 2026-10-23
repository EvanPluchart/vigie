# 0008 — Desktop integration: tray, notifications, settings, alerts, language

Status: accepted, 2026-10-23 (M5).

## Context

M5 asks for alerts with system notifications, a tray mini view, settings, and a command palette with full keyboard use. The first three talk to the desktop, and the rules say `vigie-app` never calls the OS.

The budget was settled on the same day by the author:

* memory is measured in **private bytes**;
* CPU is measured on an **idle machine**.

The working set is still reported, but it depends on the window size and on shared system pages.

## Decision

### A `vigie-shell` crate for the desktop

`vigie-shell` holds what talks to the desktop rather than to the processes:

* the tray icon and its menu;
* notifications;
* the settings file;
* placing a window near the taskbar.

Like `vigie-probe`, it never depends on Slint. `vigie-app` uses it and still makes no OS calls of its own.

**Tray (Windows): own Win32 code.**

* **Why not Slint's tray:** Slint 1.18's `SystemTrayIcon` (feature `system-tray`) gives an icon and a menu on all three systems, but no notifications. A second icon only for balloons would clutter the tray. WinRT toasts need an AppUserModelID and a Start-menu shortcut, which belong to the M6 installer.
* **Its own thread:** the shell runs the icon on a thread with a hidden message window, using `Shell_NotifyIconW` with `NOTIFYICON_VERSION_4`. `shell32` is already loaded.
* **Notifications** are balloons on the same icon (`NIF_INFO`). Windows 10 and 11 show them as toasts. Clicking one opens the view it is about.
* **Classic pitfalls handled:**
  * `SetForegroundWindow` before `TrackPopupMenu`, then `WM_NULL` after it, so the menu closes;
  * the icon is added again on `TaskbarCreated`, when Explorer restarts;
  * the small icon is loaded at the size for the current DPI, from resource 1 (the ID `winresource` uses for the application icon).
* **Ghost icon:** the icon is removed on exit. With `panic = "abort"` a crash cannot remove it, and Windows clears it when the pointer next passes over it.
* **Linux and macOS:** no tray yet. Slint's `SystemTrayIcon` is the candidate there, with notifications through the desktop's own service. Enabling it pulls ksni and D-Bus into Linux builds, so it waits until it can be measured.

**Mini view.**

* A left click on the icon opens a small window near the taskbar, with CPU, memory and the busiest processes.
* The window is created on demand and dropped when it hides, which also happens when it loses focus, so its frame buffer is freed.
* The shell places it in the monitor's work area, near the click.

### Settings

* Settings live in `%APPDATA%\Vigie\settings.ini`: plain `key = value` lines.
* `vigie-core::settings` parses and writes them with pure functions. An unknown key or a bad value keeps its default, and a corrupt file gives the defaults, never a crash.
* The shell reads the file and writes it atomically: a temporary file, then a rename.
* **Settings:**
  * refresh interval: 0.5, 1, 2 or 5 s;
  * theme: system, dark or light;
  * language: system, English or French;
  * optional columns;
  * alerts on or off, with thresholds;
  * whether closing hides to the tray (off by default, like Task Manager).
* **Command-line flags override settings for the session and are never written back.** `--no-settings` reads and writes nothing. `--no-tray` creates no icon. Benchmarks and screenshot scripts pass both, so they never change the user's setup and leave no icon behind when they are killed.

### Alerts

* Alerts are pure functions in `vigie-core::alerts`, run at the 5 s diagnostics cadence, never every tick.
* **One source per fact:** the diagnostics already decide what is critical (CPU saturated, high memory pressure). An alert is sent once a critical finding has lasted a minute, and never in the first two minutes after launch, so a machine already under pressure does not notify at every start.
* **Thresholds of their own:** the user can set them for CPU and memory, on the finest history tier.
* **Avoiding false alerts:**
  * a threshold rule needs its whole window measured, so nothing fires at launch;
  * hysteresis;
  * at most one notification per rule every 15 minutes.
* **When CPU or memory crosses a threshold**, one notification names it. The critical finding about the same resource is not notified again within the cooldown.

### Language

* Strings go through `@tr()`, with English as the source.
* French is bundled as `translations/fr/LC_MESSAGES/vigie.po`, compiled in by `slint-build` without contexts.
* A test fails when a `@tr` string has no French entry.
* Numbers and units follow the language: "1,54 Go" and "12,3 %" in French.
* The language can change without restarting.

### Keyboard

* A command palette (Ctrl+K) lists commands: views, theme, actions on the selected process, settings, quit.
* The list is tiny, so it is filtered on the UI thread. Destructive commands still ask for confirmation.
* Rail items and controls are focusable, and lists move with the arrow keys.
* The confirmation dialog opens with Cancel focused. Escape closes dialogs and the palette.

## Consequences

* `vigie-app` depends on `vigie-shell`, `vigie-core` and `vigie-probe`. Only `vigie-shell` and `vigie-probe` call the OS.
* The tray thread sleeps in `GetMessageW` and costs nothing between events. The tooltip is updated at most every 2 s, and only when its text changes.
