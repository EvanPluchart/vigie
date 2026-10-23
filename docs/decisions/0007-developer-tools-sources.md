# 0007 — Sources for ports, file locks, startup items and services

Status: accepted, 2026-10-23 (M4).

## Context

M4 adds four views: ports, who holds a file, startup items and services. None of them is needed on every tick, and the budget is already tight (M3: 0.85 % of one core, 29.8 MB of working set). Every source must therefore:

* cost nothing while its view is hidden;
* add no DLL at startup;
* be readable without administrator rights.

## Decision

| View | Source (Windows) | When it is read | Cost measured |
|---|---|---|---|
| Ports | `GetExtendedTcpTable` and `GetExtendedUdpTable`, owner PID tables, IPv4 and IPv6 (iphlpapi, already loaded for the network counters) | Every 2 s while the view is shown and the window is visible | Under 1 ms (see the M4 audit) |
| Who holds a file | Restart Manager: one session per question, always ended. `rstrtmgr.dll` is loaded with `LoadLibraryExW` (System32 only) on the first question | When a file is dropped or a path is entered | Tens of ms per question |
| Startup items | The Run keys (HKCU, HKLM, HKLM WOW6432Node) and the startup folders of the user and of all users. The enabled state comes from `Explorer\StartupApproved` | When the view opens, on Refresh, after a change | A few ms |
| Services | `EnumServicesStatusExW` for state and PID, plus `QueryServiceConfigW` per service for the start type, and the delayed flag | When the view opens, on Refresh, after an action | About 70 ms for 324 services (debug build) |

### Reading the startup approval state

These facts were read on the author's machine before any write code existed.

* An approval value is 12 bytes. Its first byte is even when the item is enabled (2, or 6 for some system items) and odd when it is disabled (3). Bytes 4 to 11 then hold the time the item was disabled, as a FILETIME.
* An item without an approval value is enabled.
* Each Run key has its approval in its own hive:

| Item location | Approval key |
|---|---|
| HKCU `Run` | HKCU `StartupApproved\Run` |
| HKLM `Run` | HKLM `StartupApproved\Run` |
| HKLM WOW6432Node `Run` | HKLM `StartupApproved\Run32` |
| Startup folders | `StartupApproved\StartupFolder` in the matching hive, by file name |

* Enabling writes `02` followed by zeros. Disabling writes `03` followed by the current time. That is what Task Manager does.
* The Run value and the shortcut are never touched, so a change is reversible. Changing a machine-wide item needs administrator rights, and Vigie reports "access denied" when it runs without them.

### Actions

| Action | Confirmation | Identity check |
|---|---|---|
| Ending a process from Ports or Files | Same confirmation as the table, naming the process | PID and start time. The Restart Manager start time matches the process probe to the second (tested) |
| Stopping a service | Its own confirmation, naming the service | — |
| Starting a service | None: it is not destructive | — |
| Enabling or disabling a startup item | None: it is reversible. A notice names the item | — |

* PID 0 and PID 4 own sockets on behalf of the system (TIME_WAIT, http.sys). They are never offered for ending.
* Neither are processes the Restart Manager calls critical, nor processes of the system section.
* Starting and stopping services usually needs administrator rights. Access denied is reported with the existing "restart as administrator" notice.
* Real services are never started or stopped in tests. Control is covered with a fake probe.

### Limits

* The Restart Manager tracks files, not folders. A folder gets a clear message.
* An elevated Vigie cannot receive files dropped from a non-elevated Explorer, as Windows blocks the drop. The path field always works.
* Startup items are limited to Run keys and startup folders. Task Manager also lists packaged apps' startup tasks and some scheduled tasks, so its list can be longer. The view says so.
* On Linux and macOS, every new view shows that it is not available on that system yet.

## Consequences

* With all four views hidden, the per-tick cost and the set of loaded modules do not change. Checked: `rstrtmgr.dll` is absent from a running Vigie until the first question.
* Ports refresh at 2 s, slower than the process table: a socket that lives under 2 s can be missed. That is acceptable for "who listens on 3000".
