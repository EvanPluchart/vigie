# 0002 — System data: native probes behind a trait, sysinfo as a starting point

**Status:** accepted, 2026-10-22

## Context

The probe runs every second for the whole life of the app, so it is the main source of Vigie's own CPU use. `sysinfo` covers all three systems quickly, but it is generic: it may open each process or allocate on every refresh.

## Decision

* All sampling goes through the `Probe` trait in `vigie-probe`.
* M0 implements `Probe` with `sysinfo` on every system, to get a working app fast.
* From M1, each hot path is measured (`cargo xtask bench-overhead`, criterion benchmarks) and replaced with native calls where it pays: `NtQuerySystemInformation` on Windows, `/proc` with reused buffers on Linux, `libproc` on macOS.
* A native path replaces `sysinfo` only with a benchmark showing the gain, recorded in the commit message.

## Consequences

* `vigie-core` and `vigie-app` never import `sysinfo` or OS crates.
* Unsafe FFI is confined to `vigie-probe`, one module per OS, each `unsafe` block with a `// SAFETY:` comment.
