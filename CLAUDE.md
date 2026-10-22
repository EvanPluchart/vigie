# Vigie — agent guide

Cross-platform task manager in Rust with Slint. Read these before changing code:

1. `docs/ARCHITECTURE.md`: layers, threading, performance budget, conventions.
2. `docs/ROADMAP.md`: current milestone and its exit criteria. Work on the current milestone only.
3. `docs/decisions/`: settled choices. Do not reopen them without a new decision record.

## Hard rules

* `vigie-probe` and `vigie-core` never depend on `slint`. `vigie-core` and `vigie-app` never call the OS or `sysinfo` directly.
* Only the sampler thread owns the `Probe`. The UI thread only patches models.
* Respect the performance budget in `docs/ARCHITECTURE.md`. A change that breaks it does not merge.
* Destructive actions on processes ask first and name the target. Never test them on processes you did not start yourself.
* Slint is pinned. Upgrade it only when asked, in a dedicated commit.
* Code, comments, commits in English. UI strings through `@tr()`.
* Work on `dev`. Never push to `main`. No `Co-Authored-By` or AI attribution lines in commits or PRs.

## Commands

```sh
cargo check --workspace
cargo test --workspace
cargo clippy --workspace -- -D warnings
cargo run -p vigie -- --tab=processes --sort=cpu
cargo xtask bench-overhead
```

## Seeing the app

Launch with test flags (`--tab`, `--sort`, `--filter`, `--interval`, `--dark`/`--light`) to reach a state directly. On Windows, capture the window with PrintWindow, not a screen grab. Never send synthetic input unless the Vigie window is verified to be in the foreground.

## Skills

`.claude/skills/logo-design` (local, not committed) designs the logo and app icon. Brief: a lookout post or spyglass, simple enough to read at 16 px in the taskbar and the tray.
