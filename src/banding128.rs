// Portions of this file are a Rust port of the ribbon filter kernel from
// fastfilter_cpp (https://github.com/FastFilter/fastfilter_cpp), whose ribbon
// implementation derives from RocksDB (https://github.com/facebook/rocksdb),
// both Apache-2.0. Algorithm, constants, and layout are transcribed from those
// sources (see PORT_NOTES.md and the per-item file:line references); this port
// is licensed MIT OR Apache-2.0 with attribution to the upstream authors.
//! Standard (non-homogeneous) ribbon banding + back-substitution + query for w=128, the
//! RocksDB production shape. Unlike homogeneous ribbon, banding stores result rows and can
//! fail; construction retries across ordinal seeds until one solves. Ported from the reference
//! `BandingAdd` / `ResetAndFindSeedToSolve` / `InterleavedBackSubst` and gated against
//! `tests/vectors/std_w128_r7.json` (solution fingerprint, chosen seed, and query outcomes).

use crate::hash128::{coeff_row_128, ordinal_to_raw_seed, result_row, ribbon_hash, start};

pub const W128: usize = 128;
/// Ordinal seeds tried during construction (reference uses mask 63 => 64 seeds).
pub const SEED_COUNT: u32 = 64;

/// Standard ribbon banding matrix over `R` result columns: a u128 coefficient row and a u32
/// result row per slot.
pub struct Banding128<const R: usize> {
    coeff_rows: Vec<u128>,
    result_rows: Vec<u32>,
    num_starts: u64,
    raw_seed: u32,
    ordinal_seed: u32,
}

impl<const R: usize> Banding128<R> {
    fn new(num_slots: usize) -> Self {
        assert_eq!(num_slots % W128, 0, "num_slots must be a multiple of 128");
        Self {
            coeff_rows: vec![0u128; num_slots],
            result_rows: vec![0u32; num_slots],
            num_starts: (num_slots - W128 + 1) as u64,
            raw_seed: 0,
            ordinal_seed: 0,
        }
    }

    fn reset(&mut self, ordinal: u32) {
        self.coeff_rows.iter_mut().for_each(|c| *c = 0);
        self.result_rows.iter_mut().for_each(|r| *r = 0);
        self.ordinal_seed = ordinal;
        self.raw_seed = ordinal_to_raw_seed(ordinal);
    }

    #[cfg(feature = "parallel")]
    fn coeff_result_mut(&mut self) -> (&mut [u128], &mut [u32]) {
        (&mut self.coeff_rows, &mut self.result_rows)
    }

    /// One key by Gaussian reduction (BandingAdd). Returns false on an inconsistent dependence
    /// (cr reduces to 0 with a nonzero result row) — the construction-failure signal.
    fn add(&mut self, key: u64) -> bool {
        let h = ribbon_hash(key, self.raw_seed as u64);
        let mut i = start(h, self.num_starts) as usize;
        let mut cr = coeff_row_128(h);
        let mut rr = result_row(h);
        loop {
            let cr_at_i = self.coeff_rows[i];
            if cr_at_i == 0 {
                self.coeff_rows[i] = cr;
                self.result_rows[i] = rr;
                return true;
            }
            cr ^= cr_at_i;
            rr ^= self.result_rows[i];
            if cr == 0 {
                return rr == 0;
            }
            let tz = cr.trailing_zeros() as usize;
            i += tz;
            cr >>= tz;
        }
    }

    /// Try ordinal seeds 0..SEED_COUNT (Reset+AddRange each) until one solves the whole set,
    /// mirroring ResetAndFindSeedToSolve. Returns the solving [`Banding128`], or `None` if no
    /// seed in range works (astronomically unlikely at the standard load factor).
    fn find_seed(num_slots: usize, keys: &[u64]) -> Option<Self> {
        let mut b = Self::new(num_slots);
        for ordinal in 0..SEED_COUNT {
            b.reset(ordinal);
            if keys.iter().all(|&k| b.add(k)) {
                return Some(b);
            }
        }
        None
    }

    /// As [`find_seed`], but pleats the keys into start-windows (for that seed) before banding.
    /// Because banding — and the solvability that picks the seed — is order-independent, this
    /// lands on the same seed and the same solution as [`find_seed`], faster at scale.
    fn find_seed_pleated(num_slots: usize, keys: &[u64], window_shift: u32) -> Option<Self> {
        let mut b = Self::new(num_slots);
        let num_starts = b.num_starts;
        let mut scratch: Vec<u64> = vec![0; keys.len()];
        for ordinal in 0..SEED_COUNT {
            b.reset(ordinal);
            let raw = b.raw_seed as u64;
            let plan = crate::PleatPlan::new(num_starts, window_shift);
            let _counts = plan.pleat_into(
                keys,
                |k| start(ribbon_hash(k, raw), num_starts),
                &mut scratch,
            );
            if scratch.iter().all(|&k| b.add(k)) {
                return Some(b);
            }
        }
        None
    }

    /// Interleaved back-substitution into `num_blocks * R` little-endian u128 segments. Uses
    /// stored result rows (0 for empty slots — non-homogeneous LoadRow).
    fn back_substitute(&self) -> Vec<u128> {
        let num_slots = self.coeff_rows.len();
        let num_blocks = num_slots / W128;
        let mut data = vec![0u128; num_blocks * R];
        let mut state = [0u128; R];
        for block in (0..num_blocks).rev() {
            let base = block * W128;
            for i in (base..base + W128).rev() {
                let cr = self.coeff_rows[i];
                let rr = self.result_rows[i]; // 0 for empty slots
                for (j, sj) in state.iter_mut().enumerate() {
                    let tmp = *sj << 1;
                    let bit = ((tmp & cr).count_ones() & 1) ^ ((rr >> j) & 1);
                    *sj = tmp | bit as u128;
                }
            }
            let seg = block * R;
            data[seg..seg + R].copy_from_slice(&state);
        }
        data
    }

    fn into_solution(self) -> Solution128<R> {
        Solution128 {
            segments: self.back_substitute(),
            num_starts: self.num_starts,
            raw_seed: self.raw_seed,
            ordinal_seed: self.ordinal_seed,
        }
    }
}

/// Solved, queryable w=128 standard ribbon solution.
pub struct Solution128<const R: usize> {
    segments: Vec<u128>,
    num_starts: u64,
    raw_seed: u32,
    ordinal_seed: u32,
}

impl<const R: usize> Solution128<R> {
    pub fn segments(&self) -> &[u128] {
        &self.segments
    }
    #[cfg(test)]
    pub fn ordinal_seed(&self) -> u32 {
        self.ordinal_seed
    }

    /// Batch membership query with software prefetch (see `Solution::contains_batch`).
    pub fn contains_batch(&self, keys: &[u64], out: &mut [bool]) {
        assert_eq!(keys.len(), out.len());
        const STRIDE: usize = 32;
        for (kc, oc) in keys.chunks(STRIDE).zip(out.chunks_mut(STRIDE)) {
            for &k in kc {
                let s = start(ribbon_hash(k, self.raw_seed as u64), self.num_starts) as usize;
                let seg = (s / W128) * R;
                // Bounds-checked: prefetch only a pointer that is genuinely in-bounds, so no
                // wild pointer arithmetic even on a (validated but defensively re-checked) buffer.
                #[cfg(all(target_arch = "x86_64", not(miri)))]
                if let Some(p) = self.segments.get(seg) {
                    // SAFETY: `p` came from `slice::get`, so it is a valid in-bounds address;
                    // `_mm_prefetch` only uses it as a cache hint and does not dereference in Rust.
                    unsafe {
                        core::arch::x86_64::_mm_prefetch(
                            p as *const _ as *const i8,
                            core::arch::x86_64::_MM_HINT_T0,
                        );
                    }
                }
                #[cfg(any(not(target_arch = "x86_64"), miri))]
                let _ = seg;
            }
            for (k, o) in kc.iter().zip(oc.iter_mut()) {
                *o = self.contains(*k);
            }
        }
    }

    /// Membership query (InterleavedFilterQuery, w=128). Present iff every column parity
    /// matches the corresponding bit of the key's expected result row.
    pub fn contains(&self, key: u64) -> bool {
        let h = ribbon_hash(key, self.raw_seed as u64);
        let start_slot = start(h, self.num_starts) as usize;
        let seg = (start_slot / W128) * R;
        let start_bit = start_slot % W128;
        let cr = coeff_row_128(h);
        let expected = result_row(h);
        let cr_left = cr << start_bit;
        let cr_right = cr >> ((W128 - start_bit) % W128);
        let maybe = if start_bit != 0 { R } else { 0 };
        for i in 0..R {
            let soln =
                (self.segments[seg + i] & cr_left) | (self.segments[seg + maybe + i] & cr_right);
            if (soln.count_ones() & 1) != ((expected >> i) & 1) {
                return false;
            }
        }
        true
    }
}

/// Overhead factor for w=128 with `r` columns (sizeof(CoeffRow)=16).
fn overhead128(r: usize) -> f64 {
    1.0 + (4.0 + r as f64 * 0.25) / (8.0 * 16.0)
}

/// Slots for `n` keys at w=128, rounded up to a multiple of 128.
pub fn num_slots_for_128(n: usize, r: usize) -> usize {
    let raw = (overhead128(r) * n as f64) as usize;
    let mut s = raw.div_ceil(W128) * W128;
    if s == W128 {
        s += W128;
    }
    s.max(2 * W128)
}

/// Build a w=128 standard ribbon filter in arrival order, finding a solving seed. Returns
/// `None` only if no seed in 0..64 solves (not observed at the standard load factor).
pub fn build_std128<const R: usize>(keys: &[u64]) -> Option<Solution128<R>> {
    let num_slots = num_slots_for_128(keys.len(), R);
    Banding128::<R>::find_seed(num_slots, keys).map(Banding128::into_solution)
}

/// Build a w=128 standard ribbon filter with pleated construction. Bit-identical to
/// [`build_std128`] (same seed, same solution), faster at scale.
pub fn build_std128_pleated<const R: usize>(
    keys: &[u64],
    window_shift: u32,
) -> Option<Solution128<R>> {
    let _ = crate::filter::window_size(window_shift);
    let num_slots = num_slots_for_128(keys.len(), R);
    Banding128::<R>::find_seed_pleated(num_slots, keys, window_shift).map(Banding128::into_solution)
}

/// Build a w=128 standard ribbon filter with slot-range parallel banding + boundary deferral,
/// under the seed-retry loop. Bit-identical to [`build_std128`]. Requires the `parallel`
/// feature. Falls back to sequential for `threads <= 1`.
#[cfg(feature = "parallel")]
pub fn build_std128_parallel<const R: usize>(
    keys: &[u64],
    window_shift: u32,
    threads: usize,
) -> Option<Solution128<R>> {
    use std::thread;
    const G: usize = 1 << 14;

    let num_slots = num_slots_for_128(keys.len(), R);
    if threads <= 1 || num_slots / W128 < 2 {
        return build_std128_pleated::<R>(keys, window_shift);
    }
    let mut b = Banding128::<R>::new(num_slots);
    let num_starts = b.num_starts;
    let window = crate::filter::window_size(window_shift);
    let n_windows = num_slots.div_ceil(window);
    let threads = threads.min(n_windows);

    'seed: for ordinal in 0..SEED_COUNT {
        b.reset(ordinal);
        let raw = b.raw_seed as u64;
        // Thread range bounds, window-aligned at ~equal key counts is overkill; even windows.
        let per = n_windows.div_ceil(threads);
        let mut bounds: Vec<usize> = (0..=threads)
            .map(|t| t.saturating_mul(per).saturating_mul(window).min(num_slots))
            .collect();
        bounds.dedup();
        let nt = bounds.len() - 1;

        let bucket_capacity = keys.len().div_ceil(nt);
        let mut buckets: Vec<Vec<u64>> = (0..nt)
            .map(|_| Vec::with_capacity(bucket_capacity))
            .collect();
        let mut deferred: Vec<u64> = Vec::with_capacity(keys.len() / 4);
        for &k in keys {
            let s = start(ribbon_hash(k, raw), num_starts) as usize;
            let t = bounds.partition_point(|&x| x <= s) - 1;
            if t + 1 < nt && s.saturating_add(G) >= bounds[t + 1] {
                deferred.push(k);
            } else {
                buckets[t].push(k);
            }
        }

        // Split both matrices into per-thread window-aligned slices.
        let (coeff_all, result_all) = b.coeff_result_mut();
        let mut cslices: Vec<(usize, &mut [u128])> = Vec::with_capacity(nt);
        let mut rslices: Vec<&mut [u32]> = Vec::with_capacity(nt);
        let mut crest = &mut coeff_all[..];
        let mut rrest = &mut result_all[..];
        let mut base = 0usize;
        for t in 0..nt {
            let len = bounds[t + 1] - bounds[t];
            let (ch, ct) = crest.split_at_mut(len);
            let (rh, rt) = rrest.split_at_mut(len);
            cslices.push((base, ch));
            rslices.push(rh);
            crest = ct;
            rrest = rt;
            base += len;
        }

        // Parallel band; each thread returns None on an inconsistent dependence (seed fails) or
        // Some(spilled) — keys whose reduction crossed the boundary, to be banded sequentially.
        let results: Vec<Option<Vec<u64>>> = thread::scope(|scope| {
            let handles: Vec<_> = cslices
                .into_iter()
                .zip(rslices)
                .zip(&buckets)
                .map(|(((cbase, cs), rs), bk)| {
                    scope.spawn(move || band_range_128::<R>(cs, rs, cbase, num_starts, raw, bk))
                })
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });
        if results.iter().any(Option::is_none) {
            continue 'seed;
        }
        // Sequential tail: pre-deferred boundary keys + any spilled keys, over the whole matrix.
        let mut tail_ok = true;
        for &k in &deferred {
            if !b.add(k) {
                tail_ok = false;
                break;
            }
        }
        if tail_ok {
            'tail: for spill in results.into_iter().flatten() {
                for k in spill {
                    if !b.add(k) {
                        tail_ok = false;
                        break 'tail;
                    }
                }
            }
        }
        if !tail_ok {
            continue 'seed;
        }
        return Some(b.into_solution());
    }
    None
}

/// Band `keys` into `(coeff, result)` slices for global slots `[base, base+len)`. The `G` margin
/// makes boundary crossing very unlikely, but if a reduction would leave the slice the key is
/// returned as spilled (banded sequentially later) rather than indexing out of bounds. Returns
/// `None` on an inconsistent dependence (the seed-failure signal), else `Some(spilled)`.
#[cfg(feature = "parallel")]
fn band_range_128<const R: usize>(
    coeff: &mut [u128],
    result: &mut [u32],
    base: usize,
    num_starts: u64,
    raw: u64,
    keys: &[u64],
) -> Option<Vec<u64>> {
    let len = coeff.len();
    let mut spilled = Vec::new();
    'key: for &k in keys {
        let h = ribbon_hash(k, raw);
        let mut i = start(h, num_starts) as usize - base;
        let mut cr = coeff_row_128(h);
        let mut rr = result_row(h);
        loop {
            if i >= len {
                spilled.push(k); // crossed the boundary — defer, do not index OOB
                continue 'key;
            }
            let cr_at_i = coeff[i];
            if cr_at_i == 0 {
                coeff[i] = cr;
                result[i] = rr;
                break;
            }
            cr ^= cr_at_i;
            rr ^= result[i];
            if cr == 0 {
                if rr != 0 {
                    return None; // inconsistent dependence -> this seed fails
                }
                break;
            }
            let tz = cr.trailing_zeros() as usize;
            i += tz;
            cr >>= tz;
        }
    }
    Some(spilled)
}

impl<const R: usize> Solution128<R> {
    /// Serialization components: (num_starts, ordinal_seed, segments).
    pub(crate) fn parts(&self) -> (u64, u32, &[u128]) {
        (self.num_starts, self.ordinal_seed, &self.segments)
    }

    /// Reconstruct from validated components (used by the versioned decoder in `format`).
    /// `ordinal_seed` must be `< SEED_COUNT`; geometry is validated by the caller.
    pub(crate) fn from_parts(num_starts: u64, ordinal_seed: u32, segments: Vec<u128>) -> Self {
        Self {
            segments,
            num_starts,
            raw_seed: ordinal_to_raw_seed(ordinal_seed),
            ordinal_seed,
        }
    }
}

/// FNV-1a over the u128 segment bytes — the differential-gate fingerprint (test-only).
#[cfg(test)]
pub fn solution_fnv_128(segments: &[u128]) -> u64 {
    let mut fnv = 0xcbf2_9ce4_8422_2325u64;
    for &seg in segments {
        for b in seg.to_le_bytes() {
            fnv = (fnv ^ b as u64).wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    fnv
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

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
    fn load() -> serde_json::Value {
        let text = std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/vectors/std_w128_r7.json"),
        )
        .expect("reference vectors must be present");
        serde_json::from_str(&text).expect("reference vectors must be valid JSON")
    }
    fn num(value: &serde_json::Value, name: &str) -> u64 {
        if name == "soln_fnv" {
            let hex = value[name]
                .as_str()
                .expect("soln_fnv must be a hexadecimal string");
            u64::from_str_radix(hex, 16).expect("soln_fnv must be valid hexadecimal")
        } else {
            value[name]
                .as_u64()
                .unwrap_or_else(|| panic!("missing or non-u64 reference field {name}"))
        }
    }
    fn bits(value: &serde_json::Value, name: &str) -> Vec<u8> {
        value[name]
            .as_array()
            .unwrap_or_else(|| panic!("reference field {name} must be an array"))
            .iter()
            .map(|value| {
                u8::try_from(
                    value
                        .as_u64()
                        .unwrap_or_else(|| panic!("reference field {name} must contain integers")),
                )
                .unwrap_or_else(|_| panic!("reference field {name} contains a non-u8 value"))
            })
            .collect()
    }

    fn by_r(j: &serde_json::Value, r: usize) -> &serde_json::Value {
        &j["by_r"][r.to_string()]
    }

    fn gate<const R: usize>(j: &serde_json::Value, r: usize) {
        let obj = by_r(j, r);
        let n = num(j, "build_n") as usize;
        let bk = keys(n, 0xA11CE);
        let soln = build_std128::<R>(&bk).expect("solves");
        assert_eq!(
            soln.ordinal_seed() as u64,
            num(obj, "chosen_ordinal_seed"),
            "seed r={r}"
        );
        assert_eq!(
            solution_fnv_128(soln.segments()),
            num(obj, "soln_fnv"),
            "fingerprint r={r}"
        );
        for (i, &want) in bits(obj, "present").iter().enumerate() {
            assert_eq!(
                soln.contains(bk[i * 37 % n]) as u8,
                want,
                "present r={r} i={i}"
            );
        }
        let ak = keys(200, 0xD15EA5E);
        for (i, &want) in bits(obj, "absent").iter().enumerate() {
            assert_eq!(
                soln.contains(ak[i] ^ 0x5555_5555_5555_5555) as u8,
                want,
                "absent r={r} i={i}"
            );
        }
        assert!(bk.iter().all(|&k| soln.contains(k)), "false negative r={r}");
    }

    #[test]
    fn w128_build_and_query_match_reference_all_r() {
        let j = load();
        gate::<5>(&j, 5);
        gate::<7>(&j, 7);
        gate::<8>(&j, 8);
        gate::<10>(&j, 10);
    }
}
