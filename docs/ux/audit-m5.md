# UX and UI audit: polish (M5)

Design track D3. Build of 2026-10-23, Windows 11. Covered:

* the Settings view;
* the tray icon, its mini view and its notifications;
* the command palette;
* keyboard use;
* the French translation, with every view captured in French, dark and light.

Severity: **P1** blocks daily use, **P2** costs time or trust, **P3** polish.

## Findings

| # | Sev. | Finding | Fix | Status |
|---|---|---|---|---|
| 1 | P2 | In French, the CPU header read "… Processe…": "16 % Processeur ↓" does not fit 88 px. | Two-line headers like Task Manager: the machine total in bold above, the label below. | Fixed |
| 2 | P2 | The mini view cut its "Open Vigie" button. | Taller window (330 px). | Fixed |
| 3 | P2 | The palette listed 13 commands past the bottom of a 640 px window. | Scrolling list, 32 px rows: every command fits in a normal window. | Fixed |
| 4 | P3 | "0.5 s" kept a decimal point in French. | Translated: "0,5 s". Numbers and units formatted in Rust follow the language too ("1,92 Go", "12,3 %", "294 kbit/s"). | Fixed |
| 5 | P2 | A test notification was accepted by the system but was not seen during the review: a game was running, and Windows silences notifications during games. | The notification API returns success. Check by hand on a quiet desktop, with `vigie --notify-test`. | Open |
| 6 | P3 | Killing Vigie (benchmarks, crashes with `panic = "abort"`) leaves a ghost tray icon until the pointer passes over it. | Benchmarks and screenshots pass `--no-tray`. A normal quit removes the icon. | Accepted |
| 7 | P3 | Plural forms: "and {} child processes" reads awkwardly for one child in French. | Use Slint's plural syntax for the few counted strings. | Open |
| 8 | P3 | The mini view's placement was checked with `--mini`, near the bottom right corner; a real click on the tray icon was not tried. | Manual check below. | Open |

## What landed

* **Settings** (rail, bottom; Ctrl+7):
  * refresh interval, theme (system, dark, light), language (system, English, French);
  * table columns;
  * alerts and their thresholds;
  * closing to the notification area.
  Changes apply at once and go to `%APPDATA%\Vigie\settings.ini`. Command-line flags override settings for the session and are never written back (tested). `--no-settings` reads and writes nothing.
* **Tray:**
  * the icon's tooltip shows CPU and memory;
  * left click opens the mini view (CPU, memory, busiest processes);
  * right click shows the menu (Open Vigie, Quit);
  * notifications are balloons, shown as toasts.
  * Closing to the tray is off by default, like Task Manager.
* **Alerts:**
  * a critical finding (CPU saturated, high memory pressure) notifies when it is raised;
  * so does a user threshold;
  * at most one notification per resource every 15 minutes;
  * nothing fires at launch.
* **Command palette (Ctrl+K):** views, search, end the selected task (which asks first), history, theme, About, Quit, and "Search processes for …".
* **Keyboard:**
  * rail items and buttons are reachable with Tab;
  * confirmations open with Cancel focused, and Escape closes them;
  * arrows move through the Ports and Services lists;
  * Delete ends the selected port's process.
* **French:**
  * 276 strings, in the vocabulary of the French Task Manager;
  * the language switches live;
  * a test fails when a string has no translation (`python xtask/tr_fr.py` writes the `.po`).

## Cost

The budget was settled for this milestone: **private memory, on an idle machine**.

The Processes view was measured with `bench-overhead`, alternating builds, on a busy machine (a game running, about 435 processes). CPU figures are after the 5 s warm-up.

| | M4 | M5 |
|---|---|---|
| CPU (pairs) | 0.82, 0.91, 0.99 % | 0.94, 0.97, 0.88, 0.99 % |
| Private memory | 9.3 to 9.5 MB | 9.2 to 9.7 MB |
| Working set | 29.7 MB | 29.8 to 30.2 MB |

* One M4 run was left out: 4.5 %, a disturbance on the machine.
* The binary is 13.3 MB, 0.7 MB more than M4, mostly the translation.
* Measured once each, 12 s after start:

| Setup | Private memory | Working set | Threads |
|---|---|---|---|
| No tray | 9.0 MB | 29.6 MB | 12 |
| Tray | 9.2 MB | 30.5 MB | 13 |
| Tray with the mini view open | 9.8 MB | 31.2 MB | 13 |

  The mini view is dropped when it closes.
* The allocation test of the probe failed once while the whole workspace was tested in parallel, and passed alone 4 times in 4. A process started during the test is the likely cause. Keep an eye on it.

## Manual checks

1. **Settings (Ctrl+7):** switch the language to Français. Every view changes at once, numbers included. Close Vigie and start it again: the choice is kept.
2. **Notification area:**
   * Click the icon: the mini view opens above the taskbar, and clicking elsewhere closes it.
   * Right click shows Open Vigie and Quit.
3. **Notifications:** run `vigie --notify-test` on a quiet desktop. A toast says the processor is very busy, and clicking it opens Diagnostics.
4. **Closing to the tray:** turn on "Closing the window keeps Vigie in the notification area" in Settings, then close the window. Vigie stays in the notification area. Quit from its menu.
5. **Ctrl+K:** type "port" and press Enter to go to the Ports view.
