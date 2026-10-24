# 0011 — Replacing Task Manager safely

Status: proposed, 2026-10-24. Awaits the author's choice; amends decision 0009 once accepted.

## Context

Decision 0009 points the machine-wide `Debugger` value of `taskmgr.exe` (HKLM, Image File Execution Options) at the per-user install, `%LOCALAPPDATA%\Programs\Vigie\vigie.exe`. The 0.1.0 lockout (a Vigie left running unseen swallowed every Ctrl+Shift+Esc) led to an audit of this path. Its findings, each checked on the author's machine or in the code:

* **Silent elevation of a user-writable file (checked 2026-10-24).** Task Manager auto-elevates. Starting `taskmgr.exe` normally, then letting it relaunch itself, started Vigie **elevated, with no UAC prompt**: the elevated relaunch goes through the `Debugger` value too. Any program running as the user can overwrite `vigie.exe` in `%LOCALAPPDATA%`, or drop a DLL next to it, and gets administrator rights the next time anyone presses Ctrl+Shift+Esc. This is a UAC bypass.
* **Other accounts lose Task Manager.** The value is machine-wide, but only its owner can read their profile folder. For every other account on the PC, Ctrl+Shift+Esc opens nothing.
* **A removed Vigie leaves Task Manager broken** for everyone, until the value is deleted by hand, unless the uninstaller runs.
* **Falling back to Task Manager cannot be done from Vigie.** Starting `taskmgr.exe` as a debugger bypasses the value, but Task Manager then relaunches itself elevated, and that relaunch is redirected to Vigie again (checked 2026-10-24).

## Options

### A. Point the value at an administrator-owned copy (recommended)

* The elevated helper that already writes the value also copies `vigie.exe` to `%ProgramFiles%\Vigie\vigie.exe`, and the value points there. Only administrators can change that file.
* Every account can run it, so nobody loses Task Manager.
* The copy survives uninstalling the per-user Vigie. The uninstaller still turns the replacement off and deletes the copy.
* Ctrl+Shift+Esc then opens Vigie elevated, as Task Manager itself is. A Vigie already running in the session is asked to show itself instead, as today.
* **Cost:** the copy must follow updates.
  * When the per-user Vigie starts, it compares its version with the copy. If they differ, it says so in Settings, and one click refreshes the copy with one UAC prompt.
  * Until then Ctrl+Shift+Esc opens the older version, which is safe.
* Turning the replacement on still needs one UAC prompt, as today.

### B. Remove "Replace Task Manager"

* No machine-wide change at all. Users pin Vigie or give it a shortcut key.
* This gives up an M6 feature the author asked for.

### C. Keep 0009 as is

* Warn in Settings that the option weakens UAC and affects other accounts. The risks above stay.

## Already done on `dev` for the next release, whatever the choice

* A launch that the running Vigie does not answer within 2.5 s starts its own window. A hung Vigie, or a program holding the single-instance mutex, can no longer swallow Ctrl+Shift+Esc.
* A bare `taskmgr` typed in a console opens Vigie instead of exiting.
* On quit, the exit watchdog and the sampler's stop come first, and the single instance is released before the tray is torn down.
* The tray ignores other programs' `WM_CLOSE`.
