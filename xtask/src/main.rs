//! Project tasks: `cargo xtask <task>`.

use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};

const USAGE: &str = "usage: cargo xtask bench-overhead [--seconds=N] [--check] [--tab=processes|performance|diagnostics|ports|files|startup]
       cargo xtask dist";
/// Performance budget from docs/ARCHITECTURE.md: CPU on an idle machine, private memory.
const CPU_BUDGET_PERCENT: f64 = 1.0;
const MEMORY_BUDGET_MB: f64 = 30.0;

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("bench-overhead") => {
            let mut seconds = 60;
            let mut check = false;
            let mut tab = String::from("processes");
            for arg in args {
                if let Some(name) = arg.strip_prefix("--tab=") {
                    tab = name.to_owned();
                    continue;
                }
                if arg == "--check" {
                    check = true;
                    continue;
                }
                match arg.strip_prefix("--seconds=").map(str::parse) {
                    Some(Ok(n)) if n > 5 => seconds = n,
                    _ => bail!("invalid argument `{arg}`\n{USAGE}"),
                }
            }
            bench_overhead(Duration::from_secs(seconds), check, &tab)
        }
        Some("dist") => dist(),
        _ => bail!("{USAGE}"),
    }
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask lives in the workspace")
        .to_path_buf()
}

/// The workspace version, from the root `Cargo.toml`.
fn version(root: &Path) -> Result<String> {
    let manifest = std::fs::read_to_string(root.join("Cargo.toml"))?;
    manifest
        .lines()
        .find_map(|l| l.trim().strip_prefix("version = \"")?.strip_suffix('"'))
        .map(str::to_owned)
        .context("no version in Cargo.toml")
}

/// Builds the release binary and the packages for this system into `dist/` (decision 0009):
/// on Windows, the Inno Setup installer and a portable zip.
fn dist() -> Result<()> {
    let root = workspace_root();
    let version = version(&root)?;
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let status = Command::new(cargo)
        .current_dir(&root)
        .args(["build", "--release", "-p", "vigie"])
        .status()
        .context("running cargo build")?;
    if !status.success() {
        bail!("release build failed");
    }
    let out = root.join("dist");
    std::fs::create_dir_all(&out)?;
    if !cfg!(windows) {
        println!(
            "dist: packages for this system are built by the release workflow (decision 0009)"
        );
        return Ok(());
    }
    let exe = root.join("target/release/vigie.exe");
    // Inno Setup 6, as winget or Chocolatey install it.
    let iscc = std::env::var_os("ISCC")
        .map(PathBuf::from)
        .into_iter()
        .chain(
            std::env::var_os("LOCALAPPDATA")
                .map(|d| PathBuf::from(d).join("Programs/Inno Setup 6/ISCC.exe")),
        )
        .chain([PathBuf::from(
            r"C:\Program Files (x86)\Inno Setup 6\ISCC.exe",
        )])
        .find(|p| p.exists())
        .context("Inno Setup 6 not found: install it, or set ISCC to ISCC.exe")?;
    let status = Command::new(&iscc)
        .current_dir(root.join("packaging/windows"))
        .arg("/Q")
        .arg(format!("/DAppVersion={version}"))
        .arg(format!("/DSourceExe={}", exe.display()))
        .arg(format!("/DOutputDir={}", out.display()))
        .arg("vigie.iss")
        .status()
        .context("running ISCC")?;
    if !status.success() {
        bail!("the installer did not build");
    }
    // A portable zip next to it: the exe alone runs anywhere, settings in %APPDATA%.
    let zip = out.join(format!("Vigie-{version}-portable.zip"));
    let _ = std::fs::remove_file(&zip);
    let status = Command::new("tar")
        .current_dir(exe.parent().context("exe folder")?)
        .arg("-a")
        .arg("-c")
        .arg("-f")
        .arg(&zip)
        .arg("vigie.exe")
        .status()
        .context("running tar")?;
    if !status.success() {
        bail!("the portable zip did not build");
    }
    println!(
        "dist: {}",
        out.join(format!("Vigie-{version}-setup.exe")).display()
    );
    println!("dist: {}", zip.display());
    Ok(())
}

/// Builds Vigie in release, runs it with a 1 s interval and reads its own CPU time and memory.
/// With `check`, fails when the steady-state CPU or the peak memory is over budget.
fn bench_overhead(duration: Duration, check: bool, tab: &str) -> Result<()> {
    const WARM_UP: Duration = Duration::from_secs(5);
    const POLL: Duration = Duration::from_millis(250);

    let root = workspace_root();
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let status = Command::new(cargo)
        .current_dir(&root)
        .args(["build", "--release", "-p", "vigie"])
        .status()
        .context("running cargo build")?;
    if !status.success() {
        bail!("release build failed");
    }

    let exe = root
        .join("target/release/vigie")
        .with_extension(std::env::consts::EXE_EXTENSION);
    let mut child = Command::new(&exe)
        // No settings file and no tray icon: a benchmark neither reads nor changes the user's
        // setup, and leaves nothing behind when it is killed.
        .args([
            "--interval=1000",
            "--no-adaptive",
            "--no-settings",
            "--no-tray",
            &format!("--tab={tab}"),
        ])
        .spawn()
        .with_context(|| format!("starting {}", exe.display()))?;
    let result = measure(&child, duration, WARM_UP, POLL);
    // Vigie was started by this task, so stopping it here is safe.
    let _ = child.kill();
    let _ = child.wait();
    let m = result?;

    let run_s = m.elapsed.as_secs_f64();
    let cpu_total = m.cpu_ms as f64 / 1000.0 / run_s * 100.0;
    let steady_s = run_s - WARM_UP.as_secs_f64();
    let cpu_steady =
        m.cpu_ms.saturating_sub(m.cpu_ms_at_warm_up) as f64 / 1000.0 / steady_s * 100.0;
    let mb = |bytes: u64| bytes as f64 / (1024.0 * 1024.0);

    println!();
    println!(
        "vigie overhead, {tab} view, {run_s:.0} s at a 1 s interval, {} processes on this machine",
        m.process_count
    );
    println!(
        "  CPU     {cpu_total:.2} % of one core ({cpu_steady:.2} % after a {} s warm-up)",
        WARM_UP.as_secs()
    );
    println!(
        "  memory  {:.1} MB sampled peak, {:.1} MB at the end",
        mb(m.peak_memory),
        mb(m.last_memory)
    );
    println!(
        "  private {:.1} MB sampled peak (memory not shared with other processes)",
        mb(m.peak_private)
    );
    println!(
        "  budget  < {CPU_BUDGET_PERCENT} % of one core on an idle machine, < {MEMORY_BUDGET_MB} MB private"
    );
    if check {
        let peak_mb = mb(m.peak_private);
        if cpu_steady >= CPU_BUDGET_PERCENT || peak_mb >= MEMORY_BUDGET_MB {
            bail!("over budget: {cpu_steady:.2} % of one core, {peak_mb:.1} MB");
        }
        println!("  within budget");
    }
    Ok(())
}

struct Measurement {
    elapsed: Duration,
    cpu_ms: u64,
    cpu_ms_at_warm_up: u64,
    peak_memory: u64,
    last_memory: u64,
    peak_private: u64,
    process_count: usize,
}

fn measure(
    child: &Child,
    duration: Duration,
    warm_up: Duration,
    poll: Duration,
) -> Result<Measurement> {
    let pid = Pid::from_u32(child.id());
    let mut system = System::new();
    let kind = ProcessRefreshKind::nothing().with_cpu().with_memory();
    let start = Instant::now();
    let mut m = Measurement {
        elapsed: Duration::ZERO,
        cpu_ms: 0,
        cpu_ms_at_warm_up: 0,
        peak_memory: 0,
        last_memory: 0,
        peak_private: 0,
        process_count: 0,
    };
    let mut warmed_up = false;
    loop {
        thread::sleep(poll);
        system.refresh_processes_specifics(ProcessesToUpdate::Some(&[pid]), true, kind);
        let Some(process) = system.process(pid) else {
            bail!("vigie exited during the benchmark");
        };
        m.elapsed = start.elapsed();
        m.cpu_ms = process.accumulated_cpu_time();
        m.last_memory = process.memory();
        m.peak_memory = m.peak_memory.max(m.last_memory);
        // On Windows, sysinfo reports private bytes (memory only Vigie uses) as virtual memory.
        m.peak_private = m.peak_private.max(process.virtual_memory());
        if !warmed_up && m.elapsed >= warm_up {
            warmed_up = true;
            m.cpu_ms_at_warm_up = m.cpu_ms;
        }
        if m.elapsed >= duration {
            break;
        }
    }
    system.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::nothing());
    m.process_count = system.processes().len();
    Ok(m)
}
