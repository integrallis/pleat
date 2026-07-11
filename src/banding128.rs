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
    pub fn ordinal_seed(&self) -> u32 {
        self.ordinal_seed
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
            let soln = (self.segments[seg + i] & cr_left) | (self.segments[seg + maybe + i] & cr_right);
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

/// Build a w=128 standard ribbon filter, finding a solving seed. Returns `None` only if no seed
/// in 0..64 solves (not observed at the standard load factor).
pub fn build_std128<const R: usize>(keys: &[u64]) -> Option<Solution128<R>> {
    let num_slots = num_slots_for_128(keys.len(), R);
    Banding128::<R>::find_seed(num_slots, keys).map(Banding128::into_solution)
}

/// FNV-1a over the little-endian bytes of the u128 segments (matches the reference serialized
/// buffer, so it is the gate value).
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
        (0..n).map(|_| { s = s.wrapping_add(0x9e37_79b9_7f4a_7c15); mix64(s) }).collect()
    }
    fn load() -> String {
        std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/vectors/std_w128_r7.json"),
        )
        .unwrap()
    }
    fn num(j: &str, name: &str) -> u64 {
        let p = format!("\"{name}\":");
        let i = j.find(&p).unwrap() + p.len();
        let r = j[i..].trim_start().trim_start_matches('"');
        let e = r.find(|c: char| !c.is_ascii_hexdigit()).unwrap_or(r.len());
        if name == "soln_fnv" { u64::from_str_radix(&r[..e], 16).unwrap() } else { r[..e].parse().unwrap() }
    }
    fn bits(j: &str, name: &str) -> Vec<u8> {
        let p = format!("\"{name}\":");
        let i = j.find(&p).unwrap() + p.len();
        let r = &j[i..];
        let (a, b) = (r.find('[').unwrap(), r.find(']').unwrap());
        r[a + 1..b].split(',').filter_map(|t| t.trim().parse().ok()).collect()
    }

    #[test]
    fn w128_build_and_query_match_reference() {
        let j = load();
        let n = num(&j, "n") as usize;
        let bk = keys(n, 0xA11CE);
        let soln = build_std128::<7>(&bk).expect("solves");

        assert_eq!(soln.ordinal_seed() as u64, num(&j, "chosen_ordinal_seed"), "seed differs");
        assert_eq!(solution_fnv_128(soln.segments()), num(&j, "soln_fnv"), "fingerprint differs");

        for (i, &want) in bits(&j, "present").iter().enumerate() {
            assert_eq!(soln.contains(bk[i * 37 % n]) as u8, want, "present {i}");
        }
        let ak = keys(200, 0xD15EA5E);
        for (i, &want) in bits(&j, "absent").iter().enumerate() {
            assert_eq!(soln.contains(ak[i] ^ 0x5555_5555_5555_5555) as u8, want, "absent {i}");
        }
        assert!(bk.iter().all(|&k| soln.contains(k)), "false negative");
    }
}
