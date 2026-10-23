# 0009 — Distribution, single instance, replacing Task Manager

Status: accepted, 2026-10-23 (M6). First version: 0.1.0.

## Context

M6 ships Vigie on three systems. Two things decided with the author shape it:

* **No signing certificates for now.** Windows will show SmartScreen ("unknown publisher") and macOS Gatekeeper will block the `.dmg` on other Macs until certificates exist.
* **Replacing Task Manager uses the only mechanism Windows offers**, which needs administrator rights. The roadmap said "HKCU": that was wrong.

## Decision

### Windows installer

* **Tool and pinning:** Inno Setup 6, pinned to an exact version: winget locally, Chocolatey in CI. Script: `packaging/windows/vigie.iss`. `cargo xtask dist` builds the release binary, then the installer, into `dist/`.
* **Per-user install, no elevation:** `PrivilegesRequired=lowest`, into `%LOCALAPPDATA%\Programs\Vigie`.
* **Installed:** a Start-menu shortcut, and optionally a "start with Windows" value in HKCU `Run`.
* **Upgrades:** the installer closes a running Vigie first (`CloseApplications`).
* **Uninstall:**
  * turns off "Replace Task Manager" first (elevated, only if it was on) — a value pointing at a deleted exe would leave Ctrl+Shift+Esc opening nothing;
  * then removes the files, the shortcut and its own `Run` value;
  * keeps `%APPDATA%\Vigie\settings.ini` and says so.
* **Signing:** the release workflow signs the installer and the exe when a certificate secret exists. Otherwise they ship unsigned, and the release notes say so.

### Single instance

* A second launch must not start a second process: with close-to-tray, and with Ctrl+Shift+Esc mapped to Vigie, that would add a second tray icon and a second sampler.
* `vigie-shell` holds a named mutex, `Local\Vigie.SingleInstance` (one Vigie per user session). A second launch finds the first one's window and asks it to show itself, then exits.
* Test flags (`--no-settings`) skip the check, so screenshots and benchmarks can run next to the user's Vigie.

### Replacing Task Manager (Windows)

* **Mechanism:** `HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Image File Execution Options\taskmgr.exe`, value `Debugger` = `"<installed vigie.exe>"`. It is the mechanism Process Explorer uses.
* **What Windows then does:** starting `taskmgr.exe`, including through Ctrl+Shift+Esc and the taskbar menu, starts Vigie instead, with the path of `taskmgr.exe` and its flags as arguments. Vigie accepts and ignores a leading argument that names `taskmgr.exe`, and what follows it.
* **Turning it on or off:**
  * needs one UAC prompt: Vigie relaunches itself elevated with `--set-replace-taskmgr=on|off`, which writes or deletes the value and exits;
  * Vigie never asks for elevation otherwise;
  * deleting the value restores Task Manager.
* **Safety:**
  * Only an installed Vigie can turn it on: the value always names `%LOCALAPPDATA%\Programs\Vigie\vigie.exe`, never `target\` or `dist\`.
  * At each start, Vigie checks the value. If it names an exe that does not exist, the Settings view says so and offers to remove it.
  * Real keys are never touched by tests: building and parsing the value is unit-tested, and the author checks the real switch by hand.

### macOS

* The release workflow, on a macOS runner, builds a `.app`:
  * `Info.plist` from `packaging/macos`;
  * an `.icns` made from the brand PNGs.
* It packs the `.app` in a `.dmg`.
* **Signing:** with a Developer ID secret, the app is signed and notarised. Without it, it is signed ad hoc, which runs on the build Mac but is blocked by Gatekeeper elsewhere (right click, then Open, to allow it).

### Linux

* **AppImage**, built in the release workflow, with a `.desktop` file, AppStream metainfo and icons.
* **App ID, proposed:** `fr.evanpluchart.Vigie`, to be confirmed by the author before any store submission.
* **Flatpak is deferred.** A Flatpak app runs in its own PID namespace, so a sandboxed Vigie would list only itself. System monitors on Flathub run a helper on the host (`flatpak-spawn --host`) for that. Vigie would need the same before it is worth publishing there.

### CI and releases

* `ci.yml` runs on the three systems. Until the end of M6, only by hand.
* `release.yml` builds the Windows installer, the macOS `.dmg` and the Linux AppImage, as artifacts. It runs on a `v*` tag, or by hand without publishing.
* No tag is pushed without the author's go.

### Landing page

* `site/` is a static page: French first, no tracking, no external scripts, built from the previews.
* The author deploys it to evanpluchart.fr.

## Consequences

* Until certificates exist, first launches show a warning on Windows and macOS.
* The macOS and Linux packages are written, but only proven once the release workflow runs.
* Replacing Task Manager is reversible from Vigie, and undone by the uninstaller.
