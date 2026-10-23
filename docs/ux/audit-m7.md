# Audit: first-use feedback (M7)

Build of 2026-10-23. The author tested 0.1.0 by hand and sent about twenty remarks with screenshots. Each one is listed below with what changed, and how it was checked. Sources and costs are in decision 0010.

## The remarks

| # | Remark | Change | Checked |
|---|---|---|---|
| 1 | The command bar buttons are misaligned | They sat 4 px under the top rule and 8 px over the bottom one (32 px buttons in a 44 px bar with 4 px padding). Now 6 px each side, also in the Ports and Services bars. | Enlarged captures, dark and light |
| 2 | The ··· button and every other button need two clicks; the first one only shows a green ring | A `FocusScope` above each control took the focus on the first press and swallowed it (Slint 1.18 `focus-on-click`). Off for buttons, switches and rail items; Tab still reaches them. | Cause read in Slint's source; to try by hand |
| 3 | Sort by name, ascending, by default | `Sort::default` is name A to Z. Details keeps its own sort. | Unit test, captures |
| 4 | Dividers to resize columns | A divider before each optional column; drag it. Widths are saved. "Reset column widths" in the header menu. | Captures; dragging to try by hand |
| 5 | More columns from a right click on the header: status, network, disk… | Added: status (suspended), user name, description, threads, handles, I/O per second. Network per process is left out (decision 0010: it needs an elevated kernel trace). I/O counts all I/O, so it is not labelled "Disk". | Unit tests (rates, sorting, suspended child), captures |
| 6 | Resize the history panel's height | Drag its top edge; the height is saved and the graphs grow with it. | Capture; dragging to try by hand |
| 7 | History starts when Vigie opens? Brave has been open 6 h and shows nothing | Yes: history lives in memory (decision 0006), and Windows keeps no per-process history to fill it from. The part before Vigie started is now shaded and labelled. Keeping history across restarts is a separate decision, open. | Captures |
| 8 | No two Performance cards are the same size | A grid: every card the same size, with the same graph height and figures block; one column under 760 px. | Captures at 1300 px |
| 9 | Diagnostics could say more, Windows updates for example | "A restart is waiting to finish installing updates" (registry) and "Drive C: is almost full" (free space), with hysteresis and tests. Counting available updates would need a network query: left out. | Unit tests; the restart finding shows on the author's machine |
| 10 | No right-click menu; Task Manager's shortcuts are missing | Vigie's own menu in every view, with Task Manager's process entries. Del ends a task, the Menu key and Shift+F10 open the menu, arrows and Enter work inside it. | Captures (process menu, ··· menu, header menu); actions to try by hand |
| 11 | The Users tab is missing | Users: each signed-in account with its CPU and memory, expandable to its processes, with the process menu on each one. | Capture |
| 12 | The Details tab is missing | Details: one flat list named by executable file, with its own columns (status, user name, description by default). | Captures |
| 13 | The switches have an odd colour | The knob is white on the lagoon track, and the "off" track is a little darker. | Capture |
| 14 | An image open in an app: "no program is using it" | Reproduced with Photos: Photos reads the file and closes it, and renaming works while it shows. Vigie was right. The empty answer now says the file can be deleted or renamed, and why a viewer can still show it. | Hand test (rename while open in Photos) |
| — | (Seen while fixing 2) The ··· menu was a light native menu on the dark theme | Replaced by Vigie's menu, like every context menu. | Captures |

## To try by hand

These need real input, which was not sent while the author was using the machine:

* the first click on buttons, the switch and the rail;
* dragging a column divider and the history panel's top edge, then restarting: the sizes come back;
* in the process menu, Properties, Search online, Open file location, and Copy in the Ports menu (the status bar says what was copied);
* the arrows, Enter and Escape in the menu, and the Menu key on a selected process;
* "Go to details" from a port or a service: Details opens with the process selected.

## Open

| Item | Status |
|---|---|
| Network per process | Left out without elevation (decision 0010). Could come as an "elevated only" column later. |
| History across restarts | To decide: saving the 1 min tier to disk when Vigie closes. |
| User name of services without elevation | Empty: Windows does not give it to a non-elevated process. |
| Steady-state allocation test | Still fails now and then when many processes start during its 20 ticks, as before M7; passes on its own. |
