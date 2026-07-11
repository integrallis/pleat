//! Slot-range parallel banding (feature `parallel`).
//!
//! Threads own disjoint, window-aligned ranges of the banding matrix and band only the keys
//! whose Gaussian reduction is expected to stay inside their range; keys starting within a safety
//! margin `G` of a range boundary are deferred to a short sequential tail, and unexpected spills
//! are detected and handled safely. Because banding is
//! order-independent, the result is bit-identical to the sequential build — verified by the
//! same fingerprint gate. No `unsafe`: the matrix is split into non-overlapping mutable slices
//! with `split_at_mut`.

use crate::banding::{Banding, Solution, W};
use crate::hash::{coeff_row, ribbon_hash, start};
use std::thread;

/// Boundary safety margin in slots. A key whose start is within `G` of its range's upper slot
/// boundary is deferred; the spill path handles the rare reduction that crosses anyway.
const G: usize = 1 << 14;

pub fn from_keys_parallel_seeded<const R: usize>(
    keys: &[u64],
    seed: u64,
    window_shift: u32,
    threads: usize,
) -> Solution<R> {
    let mut band = Banding::<R>::new(crate::filter::num_slots_for(keys.len(), R), seed);
    let num_slots = band.num_slots();
    let num_starts = (num_slots - W + 1) as u64;
    let threads = threads.max(1).min(num_slots / W); // at least one window per thread

    if threads <= 1 {
        band.add_all(keys);
        return band.solve();
    }

    let window = crate::filter::window_size(window_shift);
    // Range boundaries in slots, aligned to windows so the coeff-row matrix splits cleanly.
    let n_windows = num_slots.div_ceil(window);
    let per = n_windows.div_ceil(threads);
    let mut bounds: Vec<usize> = (0..=threads)
        .map(|t| t.saturating_mul(per).saturating_mul(window).min(num_slots))
        .collect();
    bounds.dedup();
    let nt = bounds.len() - 1;

    // Bucket keys by which thread-range their start falls in; collect boundary keys to defer.
    let bucket_capacity = keys.len().div_ceil(nt);
    let mut buckets: Vec<Vec<u64>> = (0..nt)
        .map(|_| Vec::with_capacity(bucket_capacity))
        .collect();
    let mut deferred: Vec<u64> = Vec::with_capacity(keys.len() / 4);
    for &k in keys {
        let s = start(ribbon_hash(k, seed), num_starts) as usize;
        // Which range does slot s belong to?
        let t = bounds.partition_point(|&b| b <= s) - 1;
        let hi = bounds[t + 1];
        if t + 1 < nt && s.saturating_add(G) >= hi {
            deferred.push(k); // too close to the upper boundary — defer
        } else {
            buckets[t].push(k);
        }
    }

    // Split the coeff matrix into per-thread mutable slices at the (window-aligned) bounds.
    let coeff = band.coeff_rows_mut();
    let mut slices: Vec<(usize, &mut [u64])> = Vec::with_capacity(nt);
    let mut rest = &mut coeff[..];
    let mut consumed = 0usize;
    for t in 0..nt {
        let len = bounds[t + 1] - bounds[t];
        let (head, tail) = rest.split_at_mut(len);
        slices.push((consumed, head));
        rest = tail;
        consumed += len;
    }

    // Band each range in its own thread against its private slice; collect spilled keys.
    let spilled: Vec<Vec<u64>> = thread::scope(|scope| {
        let handles: Vec<_> = slices
            .into_iter()
            .zip(buckets.iter())
            .map(|((base, slice), bkeys)| {
                scope.spawn(move || band_slice(slice, base, num_starts, seed, bkeys))
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });

    // Sequential tail: the pre-deferred boundary keys plus any keys whose reduction actually
    // spilled past a thread's range (rare, but possible for adversarial inputs — handled
    // safely, never panicking). All banded against the whole matrix.
    band.add_all(&deferred);
    for spill in spilled {
        band.add_all(&spill);
    }
    band.solve()
}

/// Band `keys` into `slice` (global slots `[base, base+slice.len())`). The `G` margin makes
/// crossing the upper boundary very unlikely, but Gaussian reduction can in principle advance
/// past it; any key whose reduction would leave the slice is returned as *spilled* to be banded
/// sequentially later, so this never indexes out of bounds.
fn band_slice(
    slice: &mut [u64],
    base: usize,
    num_starts: u64,
    seed: u64,
    keys: &[u64],
) -> Vec<u64> {
    let len = slice.len();
    let mut spilled = Vec::new();
    'key: for &k in keys {
        let h = ribbon_hash(k, seed);
        let mut i = start(h, num_starts) as usize - base;
        let mut cr = coeff_row(h);
        loop {
            if i >= len {
                spilled.push(k); // crossed the range boundary — defer, do not index OOB
                continue 'key;
            }
            let cr_at_i = slice[i];
            if cr_at_i == 0 {
                slice[i] = cr;
                break;
            }
            cr ^= cr_at_i;
            if cr == 0 {
                break;
            }
            let tz = cr.trailing_zeros() as usize;
            i += tz;
            cr >>= tz;
        }
    }
    spilled
}

#[cfg(test)]
mod tests {

    use crate::banding::solution_fnv;
    use crate::filter::RibbonFilter;

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

    #[test]
    fn parallel_build_is_bit_identical_to_sequential() {
        let k = keys(300_000, 0xA11CE);
        let seq = RibbonFilter::from_keys(&k);
        for t in [2usize, 4, 8] {
            let par = super::from_keys_parallel_seeded::<7>(&k, 0, 16, t);
            assert_eq!(
                solution_fnv(seq_segments(&seq)),
                solution_fnv(par.segments()),
                "parallel build (t={t}) diverges from sequential"
            );
            assert!(
                k.iter().all(|&x| par.contains(x)),
                "parallel: false negative t={t}"
            );
        }
    }

    fn seq_segments(f: &RibbonFilter) -> &[u64] {
        f.solution_segments()
    }
}
