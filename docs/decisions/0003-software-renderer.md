# 0003 — Renderer: Slint's software renderer on winit

**Status:** accepted, 2026-10-22

## Context

M0 shipped with Slint's default features: winit with the femtovg (OpenGL) renderer. `cargo xtask bench-overhead` on Windows (about 460 processes, 1 s interval, 60 s) measured:

| Renderer | RSS | Vigie CPU (whole run) |
|---|---|---|
| femtovg (OpenGL) | 136 MB at the end, 156 MB sampled peak | 5.0 % of one core |
| software | 24.5 MB at the end, 25.5 MB sampled peak | 2.8 % of one core |

The OpenGL context and driver alone cost about four times the whole memory budget (< 30 MB). Vigie redraws a few rows per second, so a GPU buys nothing visible. Most of the CPU left is the `sysinfo` probe (about 33 ms per sample), handled under decision 0002.

## Options

* **femtovg (default):** GPU-accelerated, smooth for animations, but over budget in memory on its own.
* **Skia:** better text and effects, but a large binary and a GPU context too.
* **Software renderer:** CPU rasterisation of dirty regions only, no GPU context. Supports text, images, clipping, rounded rectangles and `Path` (needed for M2 graphs).

## Decision

Build Slint with `default-features = false` and features `std`, `compat-1-2`, `backend-winit` (X11 and Wayland), `renderer-software`, `accessibility`. Text quality was checked with PrintWindow captures in dark and light themes.

The `system-tray` feature that Slint 1.18 enables by default is left out for now; the tray is decided at M5 (see decision 0001).

## Consequences

* RSS fits the budget with margin; CPU still depends on the probe (decision 0002).
* Large animated surfaces (full-window transitions, big graphs at high frame rates) cost CPU per pixel. Keep animations small and short, as the art direction already asks (`docs/BRAND.md`).
* If a later view needs the GPU, measure it with `cargo xtask bench-overhead` and record a new decision before switching.
