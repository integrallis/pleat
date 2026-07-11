//! Homogeneous ribbon banding and interleaved back-substitution (w=64, r=7).
//!
//! Ported from the reference `BandingAdd` / `InterleavedBackSubst` / `BackSubstBlock`
//! (fastfilter_cpp `ribbon_alg.h`, RocksDB-derived) for the homogeneous configuration, where
//! result rows are identically zero. Gated end-to-end against the committed solution
//! fingerprint in `tests/vectors/homog_w64_r7.json` (a single wrong bit changes the FNV).
//!
//! Layout facts transcribed from the reference:
//! - `kCoeffBits = 64`, `kFixedNumColumns = r = 7`, so `GetUpperStartBlock() == 0` and every
//!   block uses exactly `r` columns.
//! - `num_slots` is a multiple of 64; `num_starts = num_slots - 64 + 1`;
//!   `num_blocks = num_slots / 64`; `num_segments = num_blocks * r`; solution is
//!   `num_segments` little-endian u64s.

use crate::hash::{coeff_row, ribbon_hash, start};

pub const W: usize = 64;
pub const R: usize = 7;

/// The banding matrix for homogeneous ribbon: one coefficient row per slot (result rows are 0).
pub struct Banding {
    coeff_rows: Vec<u64>,
    num_starts: u64,
    raw_seed: u64,
}

impl Banding {
    /// `num_slots` must be a multiple of 64.
    pub fn new(num_slots: usize, raw_seed: u64) -> Self {
        assert_eq!(num_slots % W, 0, "num_slots must be a multiple of 64");
        Self {
            coeff_rows: vec![0u64; num_slots],
            num_starts: (num_slots - W + 1) as u64,
            raw_seed,
        }
    }

    pub fn num_slots(&self) -> usize {
        self.coeff_rows.len()
    }

    /// Mutable access to the coefficient-row matrix, for the parallel builder to split into
    /// disjoint slot ranges. The band's `num_starts`/`raw_seed` are unchanged.
    pub fn coeff_rows_mut(&mut self) -> &mut [u64] {
        &mut self.coeff_rows
    }

    /// Insert one key by Gaussian row-reduction into the band (BandingAdd, homogeneous:
    /// result row is 0, kFirstCoeffAlwaysOne so `cr` always enters with its low bit set).
    /// Returns false only on an inconsistent linear dependence — impossible for homogeneous
    /// ribbon, where a dependent row reduces to zero and is silently dropped (returns true).
    pub fn add(&mut self, key: u64) -> bool {
        let h = ribbon_hash(key, self.raw_seed);
        let mut i = start(h, self.num_starts) as usize;
        let mut cr = coeff_row(h);
        loop {
            let cr_at_i = self.coeff_rows[i];
            if cr_at_i == 0 {
                self.coeff_rows[i] = cr;
                return true;
            }
            cr ^= cr_at_i;
            if cr == 0 {
                // Redundant/dependent row; homogeneous result row 0 => accepted, dropped.
                return true;
            }
            let tz = cr.trailing_zeros() as usize;
            i += tz;
            cr >>= tz;
        }
    }

    pub fn add_all(&mut self, keys: &[u64]) {
        for &k in keys {
            self.add(k);
        }
    }

    /// Back-substitute and wrap the segments into a queryable [`Solution`].
    pub fn solve(&self) -> Solution {
        Solution {
            segments: self.back_substitute(),
            num_starts: self.num_starts,
            raw_seed: self.raw_seed,
        }
    }

    /// Interleaved back-substitution into `num_segments` little-endian u64 segments
    /// (InterleavedBackSubst + BackSubstBlock, homogeneous: result rows 0).
    pub fn back_substitute(&self) -> Vec<u64> {
        let num_slots = self.coeff_rows.len();
        let num_blocks = num_slots / W;
        let num_segments = num_blocks * R;
        let mut data = vec![0u64; num_segments];
        // Column-major rolling state, r columns wide, carried across blocks (not reset).
        let mut state = [0u64; R];
        for block in (0..num_blocks).rev() {
            let start_slot = block * W;
            // BackSubstBlock: rows high->low within the block.
            for i in (start_slot..start_slot + W).rev() {
                let cr = self.coeff_rows[i];
                // Homogeneous LoadRow (ribbon_impl.h:553): an occupied slot has result row 0;
                // an EMPTY slot (cr==0, an unconstrained solution row) is filled with cheap
                // pseudorandom data so empty-slot bits look random and preserve the FPR.
                let rr: u32 = if cr == 0 {
                    (i as u64).wrapping_mul(0x9E37_79B1_85EB_CA87) as u32
                } else {
                    0
                };
                for j in 0..R {
                    let tmp = state[j] << 1;
                    let bit = (((tmp & cr).count_ones() & 1) as u64) ^ (((rr >> j) & 1) as u64);
                    state[j] = tmp | bit;
                }
            }
            let segment_num = block * R;
            data[segment_num..segment_num + R].copy_from_slice(&state);
        }
        data
    }
}

/// The solved, queryable interleaved solution for a homogeneous ribbon filter (w=64, r=7).
pub struct Solution {
    segments: Vec<u64>,
    num_starts: u64,
    raw_seed: u64,
}

impl Solution {
    pub fn segments(&self) -> &[u64] {
        &self.segments
    }

    /// Membership query (InterleavedPrepareQuery + InterleavedFilterQuery, r=7 fixed columns,
    /// upper_start_block=0). Homogeneous: the expected result row is 0, so a key is present iff
    /// all `r` column parities are zero. False positives occur at rate ~2^-r.
    pub fn contains(&self, key: u64) -> bool {
        let h = ribbon_hash(key, self.raw_seed);
        let start_slot = start(h, self.num_starts) as usize;
        let start_block = start_slot / W;
        let segment_num = start_block * R; // upper_start_block == 0
        let start_bit = start_slot % W;

        let cr = coeff_row(h);
        let cr_left = cr << start_bit;
        // Avoid the undefined shift-by-64: (W - start_bit) % W.
        let cr_right = cr >> ((W - start_bit) % W);
        let maybe = if start_bit != 0 { R } else { 0 };

        for i in 0..R {
            let soln = (self.segments[segment_num + i] & cr_left)
                | (self.segments[segment_num + maybe + i] & cr_right);
            // expected bit is 0 for homogeneous; present requires even parity in every column.
            if (soln.count_ones() & 1) != 0 {
                return false;
            }
        }
        true
    }
}

/// FNV-1a over the little-endian bytes of the segment array — matches the reference's FNV over
/// its serialized `char` buffer, so it is the gate value.
pub fn solution_fnv(segments: &[u64]) -> u64 {
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

    fn vec_field(json: &str, name: &str) -> u64 {
        let pat = format!("\"{name}\":");
        let i = json.find(&pat).unwrap() + pat.len();
        let rest = json[i..].trim_start().trim_start_matches('"');
        let end = rest
            .find(|c: char| !c.is_ascii_hexdigit())
            .unwrap_or(rest.len());
        let tok = &rest[..end];
        // soln_fnv is hex, others decimal; disambiguate by the field name.
        if name == "soln_fnv" {
            u64::from_str_radix(tok, 16).unwrap()
        } else {
            tok.parse().unwrap()
        }
    }

    fn load() -> String {
        std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/vectors/homog_w64_r7.json"),
        )
        .unwrap()
    }

    #[test]
    fn build_fingerprint_matches_reference() {
        let j = load();
        // Scope into the "build" object so field names don't collide with hash_vectors.
        let build = &j[j.find("\"build\"").unwrap()..];
        let n = vec_field(build, "n") as usize;
        let num_slots = vec_field(build, "num_slots") as usize;
        let expect_fnv = vec_field(build, "soln_fnv");

        let mut b = Banding::new(num_slots, 0);
        b.add_all(&keys(n, 0xA11CE));
        let soln = b.back_substitute();
        assert_eq!(
            solution_fnv(&soln),
            expect_fnv,
            "solution fingerprint diverges from the reference kernel"
        );
    }

    /// Extract the JSON int array `"name": [ ... ]` as bits.
    fn vec_array(json: &str, name: &str) -> Vec<u8> {
        let pat = format!("\"{name}\":");
        let i = json.find(&pat).unwrap() + pat.len();
        let rest = &json[i..];
        let lb = rest.find('[').unwrap();
        let rb = rest.find(']').unwrap();
        rest[lb + 1..rb]
            .split(',')
            .filter_map(|t| t.trim().parse::<u8>().ok())
            .collect()
    }

    #[test]
    fn query_outcomes_match_reference() {
        let j = load();
        let build = &j[j.find("\"build\"").unwrap()..];
        let n = vec_field(build, "n") as usize;
        let num_slots = vec_field(build, "num_slots") as usize;

        let bk = keys(n, 0xA11CE);
        let mut b = Banding::new(num_slots, 0);
        b.add_all(&bk);
        let soln = b.solve();

        // Present set: the reference queried bk[i*37 % n] for i in 0..200 (see vecgen.cc).
        let present_ref = vec_array(&j, "present");
        for (i, &want) in present_ref.iter().enumerate() {
            let got = soln.contains(bk[i * 37 % n]) as u8;
            assert_eq!(got, want, "present query {i} disagrees with reference");
        }
        // Absent set: keys(200, 0xD15EA5E) each xored with 0x5555...
        let absent_ref = vec_array(&j, "absent");
        let ak = keys(200, 0xD15EA5E);
        for (i, &want) in absent_ref.iter().enumerate() {
            let got = soln.contains(ak[i] ^ 0x5555_5555_5555_5555) as u8;
            assert_eq!(got, want, "absent query {i} disagrees with reference");
        }
        // Every inserted key must be present (zero false negatives).
        assert!(bk.iter().all(|&k| soln.contains(k)));
    }

    #[test]
    fn no_reduction_leaves_low_bit_set() {
        // Guard on the banding invariant: cr always carries a set low bit at each step.
        let mut b = Banding::new(64 * 32, 0);
        assert!(b.add(12345));
        assert!(b.coeff_rows.iter().filter(|&&c| c != 0).all(|&c| c & 1 == 1));
    }
}
