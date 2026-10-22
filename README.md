<img src="assets/brand/vigie-lockup-horizontal.svg" alt="Vigie" height="64">

# Vigie

A fast, good-looking task manager for Windows, macOS and Linux, written in Rust with [Slint](https://slint.dev).

*Vigie* is the lookout post at the top of a ship's mast (*vigía* in Spanish, *vigia* in Portuguese): the one who watches everything. Vigie watches your machine and tells you what is going on, while using as little of it as possible.

> **Status:** milestone M0 (skeleton): a live process table with Vigie's own cost in the status bar. See [docs/ROADMAP.md](docs/ROADMAP.md).

## Goals

* **Light.** Under 1 % CPU and 30 MB of memory while refreshing every second. Vigie shows its own cost at all times.
* **Clear.** Processes grouped by app, a "why is my computer slow?" view, history you can scroll back through.
* **Useful for developers.** Which process holds port 3000, which process locks this file, suspend and resume, kill a whole tree.
* **Native on every system.** Fluent style on Windows, Cupertino on macOS, Cosmic-like on Linux.

The full feature matrix is in [docs/FEATURES.md](docs/FEATURES.md).

## Building

```sh
cargo run -p vigie -- --sort=cpu          # also --filter=TEXT, --interval=MS, --dark|--light
cargo xtask bench-overhead              # Vigie's own CPU and memory over 60 s
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
