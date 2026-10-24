# 0012 — Updates from the app

Status: accepted, 2026-10-24 (M8).

## Context

Updating Vigie meant downloading the new installer by hand. The author asked for Vigie to check for a new release on its own, at launch and from time to time, and to offer an "Update" button.

Until now Vigie went online only when the user asked (decision 0010, "Search online"). A background check changes that: it is stated here, and it can be turned off.

## Decision

### Checking

* **Source:** `https://github.com/EvanPluchart/vigie/releases/latest`, without following the redirect. GitHub answers `302` to `/releases/tag/vX.Y.Z`, which gives the newest published version in one small request. This uses no API, so there is no rate limit and nothing to parse but a URL.
* **When:** about 45 s after Vigie starts, then every 6 hours, on a thread of its own (never the sampler). Nothing else runs on that thread, and the cost is one HTTPS request.
* **Setting:** "Check for updates", on by default, in Settings. Never active with `--no-settings` (benchmarks, tests, CI), so that no measurement and no test goes online.
* **What counts as newer:** `vigie-core::update` compares versions numerically, so `0.1.10` is newer than `0.1.9`. An equal or older version is never offered. A tag Vigie cannot read means no update.
* **Windows only for now.** macOS and Linux show nothing yet.

### Updating

* **The button installs only when it is safe to:**
  * when Vigie runs from the installed path (`%LOCALAPPDATA%\Programs\Vigie\vigie.exe`): a portable copy opens the release page instead;
  * when Vigie is not elevated: Ctrl+Shift+Esc may start an elevated Vigie (decision 0011), and running a file from a user-writable folder with those rights would reopen the hole 0011 describes. That Vigie says to open Vigie normally to update.
* **Steps:**
  1. Download `Vigie-X.Y.Z-setup.exe` and `SHA256SUMS` from the release into a new folder under `%TEMP%`.
  2. Check the installer's SHA-256 against its line in `SHA256SUMS`. A missing line or a mismatch refuses the update; releases from 0.1.2 on publish `SHA256SUMS`.
  3. Release the single instance.
  4. Start the installer with `/SILENT /SUPPRESSMSGBOXES /NORESTART /relaunch=1`, then quit.
  5. The installer replaces the file and starts Vigie again with `--updated` (its `[Run]` entry for `/relaunch=1`). The status bar then says "Vigie X is installed · See what's new", which opens the release notes.
* **A failure is never silent:** the reason shows in Settings, and the release page stays one click away.
* **Integrity:** HTTPS to GitHub, plus the checksum from the same release. The checksum catches a truncated or corrupted download, not a compromised release. Signing the binaries (decision 0009) remains the real answer.
* **0.1.1 and older have no updater:** their users install the next version by hand, once.
* **If decision 0011's option A is accepted,** updating must also refresh the administrator-owned copy (one UAC prompt).
