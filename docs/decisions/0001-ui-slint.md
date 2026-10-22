# 0001 — UI toolkit: Slint

**Status:** accepted, 2026-10-22

## Context

Vigie is mostly tables, tabs, graphs and settings, on three systems, with a tight CPU and memory budget. The author already knows GPUI (Windows Finder, Recto) and wants to learn another toolkit.

## Options

* **Slint**: declarative `.slint` language with live preview, built-in Fluent, Cupertino, Material and Cosmic styles, renders only what changed, small binaries. Licensing: GPLv3, royalty-free (with attribution), or commercial.
* **GPUI**: known, fast, but every widget is drawn by hand and it is already used by the author's other projects.
* **egui**: immediate mode redraws every frame while anything animates; harder to keep under the CPU budget.
* **Tauri**: a webview costs more memory than the whole budget.

## Decision

Slint, pinned to an exact version. Vigie's code is MIT; Slint is used under its **royalty-free license**, which requires an attribution: the About window shows Slint's `AboutSlint` element.

## Consequences

* No built-in tray icon: use the `tray-icon` crate at M5.
* Graphs are drawn with Slint `Path` elements fed by Rust; no chart library.
* Slint upgrades happen on purpose, in their own commit.
