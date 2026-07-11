//! Query throughput: scalar vs. batch, both filter families, plus a measured false-positive
//! rate. Complements the construction benchmark.
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use pleat::filter::{RibbonFilter, StdRibbon};

fn mix64(mut z: u64) -> u64 {
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}
fn keys(n: usize, seed: u64) -> Vec<u64> {
    let mut s = seed;
    (0..n)
        .map(|_| {
            s = s.wrapping_add(0x9e37_79b9_7f4a_7c15);
            mix64(s)
        })
        .collect()
}

fn bench(c: &mut Criterion) {
    let n = 10_000_000usize;
    let members = keys(n, 0xA11CE);
    let probes = keys(1_000_000, 0xD15EA5E); // absent (miss-dominated)
    let hf = RibbonFilter::from_keys_pleated(&members);
    let sf = StdRibbon::<7>::from_keys_pleated(&members).unwrap();

    // Report measured FPR alongside (printed once).
    let fp_h = probes.iter().filter(|&&k| hf.contains(k)).count();
    let fp_s = probes.iter().filter(|&&k| sf.contains(k)).count();
    eprintln!(
        "measured FPR @ r=7: homogeneous {:.3}%, standard {:.3}% (theory 0.781%)",
        100.0 * fp_h as f64 / probes.len() as f64,
        100.0 * fp_s as f64 / probes.len() as f64
    );

    let mut out = vec![false; probes.len()];
    let mut g = c.benchmark_group("query_miss");
    g.throughput(Throughput::Elements(probes.len() as u64));
    g.sample_size(20);
    g.bench_function(BenchmarkId::from_parameter("homog/scalar"), |b| {
        b.iter(|| {
            let mut acc = 0u64;
            for &k in &probes {
                acc += hf.contains(std::hint::black_box(k)) as u64;
            }
            acc
        })
    });
    g.bench_function(BenchmarkId::from_parameter("homog/batch"), |b| {
        b.iter(|| hf.contains_batch(std::hint::black_box(&probes), &mut out))
    });
    g.bench_function(BenchmarkId::from_parameter("std/scalar"), |b| {
        b.iter(|| {
            let mut acc = 0u64;
            for &k in &probes {
                acc += sf.contains(std::hint::black_box(k)) as u64;
            }
            acc
        })
    });
    g.bench_function(BenchmarkId::from_parameter("std/batch"), |b| {
        b.iter(|| sf.contains_batch(std::hint::black_box(&probes), &mut out))
    });
    g.finish();
}
criterion_group!(benches, bench);
criterion_main!(benches);
