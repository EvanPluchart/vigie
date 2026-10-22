//! `Probe::sample` cost. Budget: < 3 ms for 500 processes (docs/ARCHITECTURE.md).

use criterion::{Criterion, criterion_group, criterion_main};
use vigie_probe::{Probe, Sample, SysinfoProbe, SystemProbe};

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

criterion_group!(benches, sample);
criterion_main!(benches);
