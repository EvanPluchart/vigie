# Initial prompt: milestone M0

Paste this into a new agent session at the repository root.

---

You are building **Vigie**, a cross-platform task manager in Rust with Slint. Read `CLAUDE.md`, `docs/ARCHITECTURE.md`, `docs/ROADMAP.md` and `docs/decisions/` first. Then deliver **milestone M0** exactly as the roadmap defines it, and nothing beyond it.

## Deliverables

1. **Workspace dependencies.** In the root `Cargo.toml`, pin `slint` (and `slint-build` for `vigie-app`) to an exact version, plus `sysinfo`, `flume`, `anyhow`, `thiserror`. Check crates.io for the current versions before pinning.
2. **`vigie-probe`**: the `Probe` trait and `Sample` types from ARCHITECTURE.md, implemented with `sysinfo` (decision 0002). `sample` reuses its buffers. Include Vigie's own process in the sample.
3. **`vigie-core`**: CPU % from deltas between samples, the first sample of a process showing no value; a `Frame` holding the sorted rows the UI needs.
4. **`vigie-app`**: `vigie [--sort=…] [--filter=…] [--interval=MS] [--dark|--light]` opens a Slint window (style chosen per OS: fluent on Windows, cupertino on macOS, cosmic or fluent on Linux) with the process table (name, PID, CPU %, memory) and a status bar with Vigie's own CPU % and memory. The sampler thread sends frames with `slint::invoke_from_event_loop`; the table model is patched (`set_row_data`, inserts, removals), never replaced. An About window shows `AboutSlint`.
5. **`xtask`** with `cargo xtask bench-overhead`: builds in release, starts Vigie for 60 s, reads its CPU time and peak memory, prints CPU % of one core and MB. Add the `xtask` alias in `.cargo/config.toml`.
6. **CI**: `.github/workflows/ci.yml` with a matrix on `windows-latest`, `macos-latest`, `ubuntu-latest` running `cargo check`, `cargo test`, `cargo clippy -- -D warnings`. Install the Linux system packages Slint needs. Keep it lean (cargo cache, no scheduled runs): the repository is private.
7. **Tests**: CPU % computation from two synthetic samples; first-sample case; PID reuse (same PID, different start time) is treated as a new process; model patching produces the expected rows.

## Rules

* Respect the hard rules in `CLAUDE.md`.
* If a decision record turns out to be wrong in practice, stop and explain instead of working around it.
* Work on `dev`. Commit in small steps with conventional messages (`feat(probe): …`), push to `origin/dev` when done. No AI attribution lines.

## Done when

`cargo run -p vigie` shows the live process table on this machine, `cargo xtask bench-overhead` prints its numbers, CI is green on all three systems, and you report what works, what you could not verify, the overhead numbers, and anything you changed in the docs.
