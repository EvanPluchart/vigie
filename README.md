<img src="assets/brand/vigie-lockup-horizontal.svg" alt="Vigie" height="64">

# Vigie

A fast, good-looking task manager for Windows, macOS and Linux, written in Rust with [Slint](https://slint.dev).

*Vigie* is the lookout post at the top of a ship's mast (*vigía* in Spanish, *vigia* in Portuguese): the one who watches everything. Vigie watches your machine and tells you what is going on, while using as little of it as possible.

> **Status:** version 0.1.2. Windows has every feature (processes, performance with history, diagnostics, ports, file locks, startup and services, tray, French); macOS and Linux build and show processes, with the rest to come. See [docs/ROADMAP.md](docs/ROADMAP.md).

## Installing

* **Windows:** run `Vigie-0.1.2-setup.exe`: it installs for your account only, without administrator rights, and uninstalls from Settings > Apps. From 0.1.2 on, Vigie offers new versions itself (Settings > Updates). A portable zip is also built. The files are not signed yet, so SmartScreen warns on first launch.
* **macOS and Linux:** a `.dmg` and an AppImage are built by the release workflow (unsigned for now).

## Goals

* **Light.** Under 1 % of a core and 30 MB of private memory while refreshing every second (about 9 MB measured). Vigie shows its own cost at all times.
* **Clear.** Processes grouped by app, a "why is my computer slow?" view, history you can scroll back through.
* **Useful for developers.** Which process holds port 3000, which process locks this file, suspend and resume, kill a whole tree.
* **Native on every system.** Fluent style on Windows, Cupertino on macOS, Cosmic-like on Linux.

The full feature matrix is in [docs/FEATURES.md](docs/FEATURES.md).

## Building

```sh
cargo run -p vigie -- --sort=cpu          # also --filter=TEXT, --interval=MS, --dark|--light
cargo xtask bench-overhead              # Vigie's own CPU and memory over 60 s
cargo xtask dist                        # release build, then the Windows installer and zip in dist/
```

On Linux, Slint needs the system packages listed in `.github/workflows/ci.yml`.

## Documentation

* [Architecture](docs/ARCHITECTURE.md)
* [Roadmap](docs/ROADMAP.md)
* [Feature matrix](docs/FEATURES.md)
* [Brand and art direction](docs/BRAND.md)
* [Decision records](docs/decisions)

## License

MIT. See [LICENSE](LICENSE). The interface is built with Slint under its [royalty-free license](https://slint.dev/terms-and-conditions#royalty-free), which asks for an attribution shown in the About window.
