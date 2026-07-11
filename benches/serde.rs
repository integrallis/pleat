//! Serialization / deserialization throughput (including validation on decode).
use criterion::{criterion_group, criterion_main, Criterion, Throughput};
use pleat::filter::{RibbonFilter, StdRibbon};

fn keys(n: usize) -> Vec<u64> {
    (0..n as u64)
        .map(|i| i.wrapping_mul(0x9e3779b97f4a7c15))
        .collect()
}

fn bench(c: &mut Criterion) {
    let k = keys(10_000_000);
    let hf = RibbonFilter::from_keys_pleated(&k);
    let sf = StdRibbon::<7>::from_keys_pleated(&k).unwrap();
    let hb = hf.to_bytes();
    let sb = sf.to_bytes();

    let mut g = c.benchmark_group("serde");
    g.throughput(Throughput::Bytes(hb.len() as u64));
    g.sample_size(30);
    g.bench_function("homog/to_bytes", |b| {
        b.iter(|| std::hint::black_box(&hf).to_bytes())
    });
    g.bench_function("homog/from_bytes_validated", |b| {
        b.iter(|| RibbonFilter::from_bytes(std::hint::black_box(&hb)).unwrap())
    });
    g.bench_function("std/to_bytes", |b| {
        b.iter(|| std::hint::black_box(&sf).to_bytes())
    });
    g.bench_function("std/from_bytes_validated", |b| {
        b.iter(|| StdRibbon::<7>::from_bytes(std::hint::black_box(&sb)).unwrap())
    });
    g.finish();
}
criterion_group!(benches, bench);
criterion_main!(benches);
