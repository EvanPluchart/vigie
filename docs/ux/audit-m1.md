# UX and UI audit: Processes view (M1)

Design track D3. Audited on Windows 11, dark and light themes, from the build of 2026-10-22 and the
user's first hands-on test. Severity: **P1** blocks daily use, **P2** costs time or trust,
**P3** polish.

## Findings

| # | Sev. | Finding | Fix | Status |
|---|---|---|---|---|
| 1 | P1 | Rows are inert: a click does nothing, there is no selection and no hover. The table feels broken. | Click selects a row (highlight and accent bar, as in the art direction); hover tints the row; the selection follows the process across sorts and refreshes. | Fixed |
| 2 | P1 | Every action hides behind a right click. Nothing on screen says actions exist. | A command bar above the table acts on the selection: **End task** (primary), **End tree**, a **More** menu with the rest. Disabled with no selection. Right click also selects the row first. | Fixed |
| 3 | P1 | No keyboard: no arrows, no Delete, no Enter, no shortcut to search. | ↑/↓ move the selection, Home/End jump, → / ← expand and collapse, Enter toggles, Delete asks to end the task, Ctrl+F focuses search, Escape clears it. | Fixed |
| 4 | P2 | Expanding needs a 16 px chevron hit; the name does nothing. | Double-click a row toggles it; → and ← too. | Fixed |
| 5 | P2 | A search with no match shows an empty table and nothing else. | Empty state: "No process matches “…”", with a button to clear the search. | Fixed |
| 6 | P2 | While searching, the status bar still says "434 processes". | "1 of 434 processes" while a search is active. | Fixed |
| 7 | P2 | Apps show their file name (Notepad.exe) where users know them by name (Bloc-notes, Brave). | Apps section shows the executable's description when it has one; the file name stays in the Name column for other sections and in search. | Fixed |
| 8 | P2 | The column chooser is a right click on the header: undiscoverable. | Also in the More menu of the command bar ("Columns"). | Fixed |
| 9 | P2 | Memory reached 40.8 MB during the hands-on test (budget 30 MB), against 28 MB idle. | Measured: private memory stays at 9 MB; the extra working set is shared Windows pages (text input framework, UI Automation) that any focused window touches. `bench-overhead` now reports private memory too. Whether the budget counts the working set or private memory is a decision to take. | Explained, decision pending |
| 10 | P2 | "About" sits in the status bar, the most valuable strip of the window. | Moves to the More menu. | Fixed |
| 11 | P3 | Section headers cannot collapse. | Click a section header to collapse it. | Fixed |
| 12 | P3 | "—" (no value yet, or not readable without elevation) has no explanation. | Tooltip once Slint offers one; meanwhile documented in the About window. | Open |
| 13 | P3 | Large memory values read poorly ("2240.2 MB"). | GB above 1 GB ("2.67 GB"). Digit grouping per locale comes with translations. | Fixed |
| 14 | P3 | Errors (access denied) appear in the status bar and are easy to miss. | Failures show in the command bar, next to the buttons, in the danger colour. | Fixed |
| 15 | P3 | The search field's focus line uses the Windows accent (orange here), not the brand lagoon. | Native style choice; keep unless it clashes in user tests. | Won't fix for now |

## Principles applied

* **Visible first, shortcuts second.** Every action reachable with the mouse without knowing a gesture; keyboard and right click make it faster.
* **The selection is the subject.** Commands act on the selected process and say its name.
* **Calm on every tick.** The selection, scroll position and hover survive refreshes; rows do not flash.
