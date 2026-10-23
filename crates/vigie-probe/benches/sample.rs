//! `Probe::sample` cost. Budget: < 3 ms for 500 processes (docs/ARCHITECTURE.md).
//! `Probe::system` runs on top of it while the Performance view is shown.

use criterion::{Criterion, criterion_group, criterion_main};
use vigie_probe::{Probe, Sample, SysinfoProbe, SystemProbe, SystemSample};

fn bench<P: Probe>(c: &mut Criterion, name: &str, mut probe: P) {
    let mut sample = Sample::default();
    probe.sample(&mut sample).expect("sample");
    let label = format!("{name} ({} processes)", sample.processes.len());
    c.bench_function(&label, |b| {
        b.iter(|| probe.sample(&mut sample).expect("sample"))
    });
}

fn sample(c: &mut Criterion) {
    bench(c, "sysinfo", SysinfoProbe::new().expect("probe"));
    bench(c, "system", SystemProbe::new().expect("probe"));
}

fn system(c: &mut Criterion) {
    let mut probe = SystemProbe::new().expect("probe");
    let mut sample = Sample::default();
    probe.sample(&mut sample).expect("sample");
    let mut system = SystemSample::default();
    if probe.system(&mut system).is_err() {
        return;
    }
    c.bench_function("machine-wide figures", |b| {
        b.iter(|| probe.system(&mut system).expect("system"))
    });
    // Steady state: after warm-up, idle engines are skipped between rechecks.
    if probe.gpus(&mut system).is_ok() {
        for _ in 0..40 {
            probe.gpus(&mut system).expect("gpus");
        }
        c.bench_function("gpus", |b| {
            b.iter(|| probe.gpus(&mut system).expect("gpus"))
        });
    }
}

criterion_group!(benches, sample, system);
criterion_main!(benches);
