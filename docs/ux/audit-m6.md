# Audit: distribution (M6)

Build of 2026-10-23, version 0.1.0. M6 is mostly packaging, so this audit sorts what was **built and checked here**, what is **written and proven only by the release workflow**, and what **waits for the author**.

## Built and checked on this machine (Windows 11)

| Item | Check | Result |
|---|---|---|
| Installer (`cargo xtask dist`, Inno Setup 6.7.3) | Silent install in French, with "start with Windows" | Exit code 0, with everything in place: <ul><li>`%LOCALAPPDATA%\Programs\Vigie\vigie.exe` and its uninstaller</li><li>the Start-menu shortcut</li><li>the `Run` value</li><li>the uninstall entry "Vigie 0.1.0"</li></ul> |
| Installed Vigie | Started from its folder, closed with `WM_CLOSE` | Runs, exit code 0 |
| Uninstaller | Silent uninstall | Exit code 0. Nothing left: no folder, shortcut, `Run` value or uninstall entry, and no `Debugger` value (it was absent before and stayed absent) |
| Portable zip | Built next to the installer | `Vigie-0.1.0-portable.zip` holds `vigie.exe` |
| Single instance | A second launch, with the arguments Windows gives `taskmgr.exe` (`C:\WINDOWS\system32\taskmgr.exe /4`) | It exits with code 0 within 4 s, and one Vigie stays |
| Task Manager arguments | Unit test | Paths naming taskmgr.exe, quoted or not, and their flags are accepted |
| Debugger value | Unit test | Built and read back. A stale path is recognised, and so is another program (Process Explorer) |
| macOS build of the app | `cargo check -p vigie --target aarch64-apple-darwin` | Compiles. Not linked: that needs a Mac |
| Landing page (`site/`, French and English, light only) | Rendered with headless Chrome at 1440 and 520 px, with and without reduced motion | Laid out in both. The Ports and Files views show a test process (127.0.0.1:3000, a held file on a temporary `V:` drive), so no user path appears. The Processes view shows the author's real apps |

## Written, proven only by the release workflow

* **macOS:** `packaging/macos/build-dmg.sh` builds a universal `.app` (icon from the brand PNGs) and a `.dmg`. It signs ad hoc, since no Developer ID is configured.
* **Linux:** `packaging/linux/build-appimage.sh` builds the AppImage on Ubuntu 22.04, with its `.desktop` file and AppStream metadata.
* **Windows on CI:** Inno Setup comes from Chocolatey (6.7.3), and signing is skipped without a certificate.
* **The Linux build of the app itself:** cross-checking from Windows stops in a font dependency (`fontique` needs pkg-config), before reaching Vigie's code.

The first run of `ci.yml` and of `release.yml` (on the `release-check` branch, without publishing) is recorded below.

## Waiting for the author

| Item | What is needed |
|---|---|
| Replacing Task Manager | A real try. The switch is in Settings, on an installed Vigie, and asks for one UAC prompt each way. Then check: <ul><li>Ctrl+Shift+Esc opens Vigie, or brings the running one to the front;</li><li>turning the switch off gives Task Manager back;</li><li>uninstalling with the switch on asks to turn it off first.</li></ul> |
| Signing | A Windows code-signing certificate and an Apple Developer ID, as repository secrets (names in `release.yml`). Until then, SmartScreen and Gatekeeper warn on first launch. |
| Landing page | Publishing on GitHub Pages: enable Pages (source "GitHub Actions"), make the repository public, run `pages.yml`. See `site/README.md`. |
| App ID | `fr.evanpluchart.Vigie`, proposed and used in the Linux and macOS metadata. To confirm before any store. |
| Flatpak | Deferred: in its sandbox, Vigie would see only itself (decision 0009). |

## Findings

| # | Sev. | Finding | Fix | Status |
|---|---|---|---|---|
| 1 | P1 | Windows starts the replacement with taskmgr's path as an argument, and Vigie rejected unknown arguments (exit 2), so Ctrl+Shift+Esc would have shown nothing. | The arguments are accepted and ignored (tested). | Fixed |
| 2 | P1 | A `Debugger` value left pointing at a deleted Vigie stops Task Manager from opening at all. | <ul><li>The uninstaller turns the option off before removing files, and stays installed if the prompt is declined.</li><li>Settings shows a stale value and offers to repair it.</li><li>Only the installed path can be written.</li></ul> | Fixed |
| 3 | P2 | A second launch (Ctrl+Shift+Esc while Vigie runs, or with close-to-tray) started a second Vigie with a second tray icon. | Single instance: the second launch shows the first and exits (checked). | Fixed |
| 4 | P2 | The roadmap said "HKCU" for replacing Task Manager; the only mechanism lives in HKLM. | Decision 0009, roadmap corrected. | Fixed |
| 5 | P3 | `workflow_dispatch` only works for workflows on the default branch, and `main` is not used during development. | The release dry run runs on pushes to `release-check`. | Accepted |
| 6 | P3 | The "Task Manager" section is low in Settings: on a short screen it sits below the fold. | Fine for now; consider moving it up. | Open |
