# 0010 — First-use feedback: menus, Details and Users, more columns, upkeep findings

Status: accepted, 2026-10-23 (M7).

## Context

The author's first full test of 0.1.0 asked for what Task Manager users expect: a right-click menu everywhere, the Details and Users tabs, more columns that can be resized, a history panel that can be resized, and findings about Windows upkeep. Each new figure needs a source that keeps the performance budget and works without elevation.

## Decision

### Menus

* **Vigie draws its own menus** (`ui/menu.slint`), from the design tokens.
  * Slint's `ContextMenuArea` uses native Win32 menus on Windows, which stay light on the dark theme. The author had already turned down native widgets for the same reason (decision 0008, "Controls").
  * One menu sits above the window. Each view hands it a context: a process, the table header, the ··· menu, a port, a process holding a file, a startup app or a service.
  * Arrows, Enter, Right and Left, and Escape work. The Menu key and Shift+F10 open the menu of the selected process.
* **Process entries follow Task Manager:**
  * expand or collapse;
  * end task and end process tree;
  * suspend, resume and priority;
  * show history;
  * go to details (or go to the process);
  * open file location, search online, properties, copy command line.
* **Desktop actions run in `vigie-shell`:** the clipboard, the file manager, the Properties dialog (on the UI thread, which has a message loop) and the browser. "Search online" opens a Bing search, as Task Manager does. It is the only page Vigie opens, and only when the user asks.

### Details and Users

* **Details** is the process table as one flat list (`Core::set_flat`), with every row named by its executable file (`Core::set_file_names`). It keeps its own sort and its own columns.
* **The Processes view** can show the same flat list: ··· › View › Group by type. Expand all and Collapse all are in the same menu.
* **Users** groups processes by login session (`Core::users`), leaving out session 0 (services and the system), as Task Manager does.
  * Account names and connection states come from `WTSQuerySessionInformationW`.
  * They are read when the view opens, when a session appears, and every 10 s while the view is shown.

### Columns and their sources

| Column | Source | Cost |
|---|---|---|
| Threads, handles | The full process snapshot the probe already takes every 5 ticks (decision 0004) | None: fields of entries already read |
| Status (suspended) | The thread entries of that snapshot: every thread waiting with reason `Suspended` | Reads the thread entries that follow each process entry |
| I/O per second | The snapshot's transfer counts (all processes, every 5 s); `GetProcessIoCounters` on the handles already held, every tick, only while an I/O column is shown | One system call per openable process and tick, only when shown |
| User name | `WTSEnumerateProcessesW` and `LookupAccountSidW`, once per batch of new processes, with names cached by SID | Only when processes appear |
| Description | The executable's version resource, already read for the icon | None |

* **Layout:** `SystemProcessInfo` now has its pool, page file, private and I/O fields. `SystemThreadInfo` was added from the same NT headers (phnt). A test pins their sizes on 64-bit: 0x100 and 0x50.
* **I/O counts all I/O:** files, devices and network, as `GetProcessIoCounters` defines it. So the column is labelled "I/O", not "Disk". Task Manager's disk-only figure comes from a kernel trace that needs administrator rights.
* **Without elevation, Windows names the owner of the user's own processes only.** It does not name services and system processes. Their user name stays empty rather than guessed. Run elevated, every row has one.
* **Network per process is left out.**
  * Task Manager reads it from a kernel network trace (ETW), which needs administrator rights and a trace session running all the time.
  * Estimating it from the sockets table would show connections, not traffic.
  * A column of zeros or guesses would mislead.
* **Columns are resizable.** A divider sits before each optional column; Name takes what is left.
* **Saved in the settings file:**
  * the columns of Processes and of Details;
  * the column widths (shared by both views);
  * the height of the history panel.

### Upkeep findings

They are read every minute, with no network access, so the page's "0 requests" stays true.

* **Restart pending for updates:** the registry keys `WindowsUpdate\Auto Update\RebootRequired` and `Component Based Servicing\RebootPending` (HKLM, readable by every user). This finding replaces "running for N days", whose advice it repeats. Asking Windows Update for available updates would mean a network query: left out.
* **System drive almost full:**
  * `GetDiskFreeSpaceExW` on `%SystemDrive%`;
  * raised under 10 % free (and under 20 GB, so a large drive with room left is fine) or under 5 GB;
  * cleared above 12 % or 24 GB, and above 8 GB;
  * critical under 2 GB, which also notifies;
  * the suggestion opens Storage Sense (`ms-settings:storagesense`).

### Smaller changes

* **First click:** buttons, switches and rail items act on the first click. A `FocusScope` on top of each control took the focus on the first press and swallowed it. Fixed with `focus-on-click: false`.
* **Default sort:** names from A to Z, as Task Manager opens.
* **Performance view:** cards are on a grid, all the same size, with the same graph height and the same block of figures below.
* **Graphs:** the part of a range before Vigie started is shaded and labelled, so an empty 6 h graph explains itself. History stays in memory (decision 0006): Windows keeps no per-process history to fill it from.

## Results

* Side by side on the same busy machine (game running, about 480 processes): the build before M7 used 1.56 % of a core, the M7 build 1.51 %. Private memory: 11.4 MB, then 12.4 MB.
* The steady-state allocation test still passes on its own. It can fail when the machine starts many processes during its 20 ticks, as before M7.

## Consequences

* For processes Vigie cannot open, threads, handles, status and I/O refresh every 5 s; for the others, I/O refreshes every second while it is shown.
* A process suspended by Vigie shows "Suspended" within 5 s.
