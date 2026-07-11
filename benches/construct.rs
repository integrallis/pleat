//! Reproduces the shape of the paper's Table 1 on this crate: construction throughput for
//! arrival, pleated, and parallel builds. Cross-validates that the Rust crate delivers the
//! technique's benefit (the differential gate already proved it computes the same filter).
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use pleat::filter::RibbonFilter;

fn mix64(mut z: u64) -> u64 {
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}
fn keys(n: usize) -> Vec<u64> {
    let mut s = 0xA11CEu64;
    (0..n).map(|_| { s = s.wrapping_add(0x9e37_79b9_7f4a_7c15); mix64(s) }).collect()
}

fn bench(c: &mut Criterion) {
    for &n in &[1_000_000usize, 10_000_000] {
        let k = keys(n);
        let mut g = c.benchmark_group(format!("construct/{}", n));
        g.throughput(Throughput::Elements(n as u64));
        g.sample_size(10);
        g.bench_function(BenchmarkId::from_parameter("arrival"), |b| {
            b.iter(|| RibbonFilter::from_keys(std::hint::black_box(&k)))
        });
        g.bench_function(BenchmarkId::from_parameter("pleated"), |b| {
            b.iter(|| RibbonFilter::from_keys_pleated(std::hint::black_box(&k)))
        });
        #[cfg(feature = "parallel")]
        for t in [4usize, 8] {
            g.bench_function(BenchmarkId::from_parameter(format!("parallel_{t}t")), |b| {
                b.iter(|| RibbonFilter::from_keys_parallel(std::hint::black_box(&k), t))
            });
        }
        g.finish();
    }
}
criterion_group!(benches, bench);
criterion_main!(benches);
