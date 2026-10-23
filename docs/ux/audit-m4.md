# UX and UI audit: developer tools (M4)

Design track D3. Build of 2026-10-23, Windows 11, dark and light themes.

Views covered:

* Ports;
* Files ("who is using this file?");
* Startup, with its two panes, startup apps and services.

Every destructive action was tried only on a test helper started for the review: a Python process listening on port 3000 and holding a file.

Severity: **P1** blocks daily use, **P2** costs time or trust, **P3** polish.

## Findings

| # | Sev. | Finding | Fix | Status |
|---|---|---|---|---|
| 1 | P2 | Buttons, search fields and pane switches were the native widgets: the system accent (orange here), other radii and other heights. They did not match the art direction or the rail. Raised by the author. | Vigie's own controls from the tokens (`ui/controls.slint`, rule in `docs/BRAND.md`), used in every view: radius 8 like the rail, the lagoon tint for the action a view is about, a danger tint for ending and stopping, a field border that turns lagoon on focus, a magnifying glass and a clear button. | Fixed |
| 2 | P2 | After ending the process behind a port, the selection stayed, with "End task" still enabled for a process that no longer owned anything. | The selection is cleared when its socket leaves the list. | Fixed |
| 3 | P3 | The footnotes under the startup and service lists overlapped the last row. | Laid out under a divider. | Fixed |
| 4 | P3 | "…" touched the search field. | Fixed width for the menu button. | Fixed |
| 5 | P3 | Some startup items read by their program's file description: "Update" for Discord's updater. | Show the publisher next to it (done). Consider the folder or product name when the description is generic. | Open |
| 6 | P3 | Task Manager also lists packaged apps' startup tasks and some scheduled tasks. Vigie lists Run keys and startup folders only, so its list can be shorter. | The view says so in a footnote (decision 0007). | Accepted |
| 7 | P3 | Service start and stop could not be tried for real: real services are never controlled in tests, and the shell is not elevated. | Covered with a fake probe (notices for started, stopped, denied, dependents running). The confirmation names the service. | Accepted |
| 8 | P3 | An elevated Vigie cannot receive files dropped from a non-elevated Explorer, as Windows blocks it. | The path field always works. Say it in the drop zone when Vigie runs elevated (M5, with "restart as administrator"). | Open |

## End to end, on the test helper

* **Ports:** the search "3000" puts the helper first, with the port in bold. Clicking the row and then "End task" opens the shared confirmation: "End python.exe (PID 56304)?". Confirming ends the helper, and the list says "Nothing uses 3000". The status bar says "Ended python.exe (PID 56304)".
* **Files:** the held file lists the helper as a console program, with its icon, PID and "End task".
  * A folder says that only files can be checked.
  * A missing path says that there is no file there.
* **Startup:** the toggle was tested by hand, through the probe, on a Run value created by the test and removed afterwards. No real item was changed.

## Cost

The Processes view was measured with `bench-overhead` (60 s) in alternated pairs against the M3 build, on the same busy machine (about 435 to 470 processes). CPU figures are after the 5 s warm-up.

| | M3 | M4 |
|---|---|---|
| CPU, mean of 6 pairs | 0.94 % | 1.05 % |
| CPU, range | 0.82 to 1.02 % | 0.93 to 1.24 % |
| Working set | 29.6 to 29.8 MB | 29.6 to 30.2 MB |
| Private memory | 9.2 to 9.5 MB | 9.3 to 9.7 MB |

* **CPU:** the gap of about 0.1 point is within the noise of a single build. No per-tick code was added for hidden views: they read nothing while hidden.
* **New views:** one run each. Ports: 0.91 %, sockets read every 2 s. Startup: 0.79 %.
* **Binary:** 12.5 MB, 0.87 MB more than M3. That adds to the working set.
* **Loaded modules:** `rstrtmgr.dll` is not loaded at start. It loads on the first file checked.
* **Read costs:** 324 services in about 73 ms (debug build), read on opening and on Refresh only.
* **Budget:** on this loaded machine, Vigie now sits around 1 % of a core and 30 MB of working set. The budget asks for an idle machine, and the memory metric (working set or private) is still to be decided.

## Manual checks

1. **Ctrl+4, Ports:** type a port in use, select the row; "End task" asks first and names the process.
2. **Ctrl+5, Files:** drop a file onto the window, from any view. A file open in Word, say, lists Word.
3. **Ctrl+6, Startup:**
   * Disable an item you do not need, then enable it again. The status bar names it each time.
   * In Services, "Stop" asks first. Without administrator rights it says access is denied.
