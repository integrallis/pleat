//! Public ribbon filter API with pleated construction.
//!
//! [`RibbonFilter`] is a homogeneous ribbon filter (w=64, default R=7 result bits, ~0.8%
//! false-positive rate at ~7.6 bits/key; tune R via [`Ribbon`] for other rates). It offers
//! three construction orders that all produce the **same filter, bit for bit** (the ribbon
//! banding solution is invariant to insertion order):
//! - [`RibbonFilter::from_keys`] — arrival order (the reference default).
//! - [`RibbonFilter::from_keys_pleated`] — one counting pass groups keys into cache-sized
//!   windows before banding; ~2x faster to build at scale for the same output.
//! - [`RibbonFilter::from_keys_parallel`] — slot-range parallel banding with boundary deferral
//!   (requires the `parallel` feature).

use crate::banding::{Banding, Solution, W};
use crate::hash::{ribbon_hash, start};
use crate::PleatPlan;

/// Default pleating window: 2^16 slots (~768 KiB of banding state, under half an L2).
pub const DEFAULT_WINDOW_SHIFT: u32 = 16;

/// Space overhead factor for w=64 with `r` result bits (filterapi.h `GetBestOverheadFactor`).
fn overhead(r: usize) -> f64 {
    1.0 + (4.0 + r as f64 * 0.25) / (8.0 * 8.0)
}

/// Number of banding slots for `n` keys at result width `r`, rounded up to a multiple of 64
/// (never fewer than 128 for the non-smash configuration). Mirrors `RoundUpNumSlots`.
pub fn num_slots_for(n: usize, r: usize) -> usize {
    let raw = (overhead(r) * n as f64) as usize;
    let mut s = raw.div_ceil(W) * W;
    if s == W {
        s += W;
    }
    s.max(2 * W)
}

/// A homogeneous ribbon filter over 64-bit keys with `R` result bits (false-positive rate
/// ~2^-R). Use the [`RibbonFilter`] alias for the default (R=7, ~0.8% FPR), or pick another
/// width, e.g. `Ribbon::<10>::from_keys(&keys)` for ~0.1%.
pub struct Ribbon<const R: usize> {
    soln: Solution<R>,
}

/// Homogeneous ribbon filter at the default R=7 (~0.8% false-positive rate, ~7.6 bits/key).
pub type RibbonFilter = Ribbon<7>;

impl<const R: usize> Ribbon<R> {
    /// Build from keys in arrival order (seed 0).
    pub fn from_keys(keys: &[u64]) -> Self {
        Self::from_keys_seeded(keys, 0)
    }

    pub fn from_keys_seeded(keys: &[u64], seed: u64) -> Self {
        let mut b = Banding::<R>::new(num_slots_for(keys.len(), R), seed);
        b.add_all(keys);
        Self { soln: b.solve() }
    }

    /// Build with pleated construction: one counting pass folds keys into window order, then
    /// bands. Produces the identical filter to [`RibbonFilter::from_keys`], faster at scale.
    pub fn from_keys_pleated(keys: &[u64]) -> Self {
        Self::from_keys_pleated_seeded(keys, 0, DEFAULT_WINDOW_SHIFT)
    }

    pub fn from_keys_pleated_seeded(keys: &[u64], seed: u64, window_shift: u32) -> Self {
        let num_slots = num_slots_for(keys.len(), R);
        let num_starts = (num_slots - W + 1) as u64;
        let plan = PleatPlan::new(num_starts, window_shift);
        let (ordered, _counts) = plan.pleat(keys, |k| start(ribbon_hash(k, seed), num_starts));
        let mut b = Banding::<R>::new(num_slots, seed);
        b.add_all(&ordered);
        Self { soln: b.solve() }
    }

    /// Is `key` possibly in the set? Never a false negative; ~0.8% false-positive rate.
    #[inline]
    pub fn contains(&self, key: u64) -> bool {
        self.soln.contains(key)
    }

    /// Serialized solution size in bytes (the queryable payload; keys are not stored).
    pub fn size_bytes(&self) -> usize {
        self.soln.segments().len() * 8
    }

    /// Bits per key for `n` inserted keys (diagnostic).
    pub fn bits_per_key(&self, n: usize) -> f64 {
        self.size_bytes() as f64 * 8.0 / n as f64
    }

    /// Serialize to a portable little-endian byte buffer: `[num_starts u64][raw_seed u64]`
    /// followed by the solution segments. Keys are not stored.
    pub fn to_bytes(&self) -> Vec<u8> {
        let (num_starts, raw_seed, segs) = self.soln.parts();
        let mut out = Vec::with_capacity(16 + segs.len() * 8);
        out.extend_from_slice(&num_starts.to_le_bytes());
        out.extend_from_slice(&raw_seed.to_le_bytes());
        for &s in segs {
            out.extend_from_slice(&s.to_le_bytes());
        }
        out
    }

    /// Reconstruct a filter from [`RibbonFilter::to_bytes`]. Returns `None` on a malformed buffer.
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 16 || !(bytes.len() - 16).is_multiple_of(8) {
            return None;
        }
        let num_starts = u64::from_le_bytes(bytes[0..8].try_into().ok()?);
        let raw_seed = u64::from_le_bytes(bytes[8..16].try_into().ok()?);
        let segs = bytes[16..]
            .chunks_exact(8)
            .map(|c| u64::from_le_bytes(c.try_into().unwrap()))
            .collect();
        Some(Self {
            soln: Solution::from_parts(num_starts, raw_seed, segs),
        })
    }

    #[cfg(test)]
    pub(crate) fn solution_segments(&self) -> &[u64] {
        self.soln.segments()
    }
}

#[cfg(feature = "parallel")]
mod parallel;
#[cfg(feature = "parallel")]
pub use parallel::from_keys_parallel_seeded;

#[cfg(feature = "parallel")]
impl<const R: usize> Ribbon<R> {
    /// Build with slot-range parallel banding (boundary keys deferred to a sequential tail).
    /// Produces the identical filter to [`RibbonFilter::from_keys`]. Requires the `parallel` feature.
    pub fn from_keys_parallel(keys: &[u64], threads: usize) -> Self {
        Self::from_keys_parallel_seeded(keys, 0, DEFAULT_WINDOW_SHIFT, threads)
    }

    pub fn from_keys_parallel_seeded(
        keys: &[u64],
        seed: u64,
        window_shift: u32,
        threads: usize,
    ) -> Self {
        Self {
            soln: from_keys_parallel_seeded::<R>(keys, seed, window_shift, threads),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::banding::solution_fnv;

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
    fn pleated_build_is_bit_identical_to_arrival() {
        for n in [1000usize, 50_000, 250_000] {
            let k = keys(n, 0xA11CE);
            let arrival = RibbonFilter::from_keys(&k);
            let pleated = RibbonFilter::from_keys_pleated(&k);
            assert_eq!(
                solution_fnv(arrival.soln.segments()),
                solution_fnv(pleated.soln.segments()),
                "pleated build diverges from arrival at n={n}"
            );
        }
    }

    #[test]
    fn no_false_negatives_and_plausible_fpr() {
        let n = 200_000;
        let k = keys(n, 0xA11CE);
        let f = RibbonFilter::from_keys_pleated(&k);
        assert!(k.iter().all(|&x| f.contains(x)), "false negative");
        let absent = keys(200_000, 0xD15EA5E);
        let fp = absent.iter().filter(|&&x| f.contains(x ^ 0x5555_5555_5555_5555)).count();
        let fpr = fp as f64 / 200_000.0;
        assert!(fpr < 0.02, "FPR {fpr} too high"); // r=7 => ~0.78%
        assert!(f.bits_per_key(n) < 10.0);
    }
}

#[cfg(test)]
mod prod_tests {
    use super::*;

    #[test]
    fn empty_and_tiny_inputs_do_not_panic() {
        let f = RibbonFilter::from_keys(&[]);
        assert!(!f.contains(12345) || f.contains(12345)); // no member; just must not panic
        let f2 = RibbonFilter::from_keys_pleated(&[7, 42, 1000]);
        assert!(f2.contains(7) && f2.contains(42) && f2.contains(1000));
    }

    #[test]
    fn tunable_fpr_scales_with_r() {
        use crate::filter::Ribbon;
        let k: Vec<u64> = (0..200_000u64).map(|i| i.wrapping_mul(0x9e3779b97f4a7c15)).collect();
        let absent: Vec<u64> =
            (0..200_000u64).map(|i| i.wrapping_mul(0x9e3779b97f4a7c15) ^ 0x1).collect();
        let fpr = |present: &dyn Fn(u64) -> bool| -> f64 {
            absent.iter().filter(|&&x| present(x)).count() as f64 / absent.len() as f64
        };
        let f7 = Ribbon::<7>::from_keys_pleated(&k);
        let f10 = Ribbon::<10>::from_keys_pleated(&k);
        assert!(k.iter().all(|&x| f7.contains(x)) && k.iter().all(|&x| f10.contains(x)));
        let (e7, e10) = (fpr(&|x| f7.contains(x)), fpr(&|x| f10.contains(x)));
        // Lower FPR (higher r) costs more space; ~2^-r each.
        assert!(e10 < e7, "r=10 FPR {e10} should be below r=7 {e7}");
        assert!(f10.bits_per_key(k.len()) > f7.bits_per_key(k.len()));
    }

    #[test]
    fn roundtrip_serialization_preserves_queries() {
        let k: Vec<u64> = (0..100_000u64).map(|i| i.wrapping_mul(0x9e3779b97f4a7c15)).collect();
        let f = RibbonFilter::from_keys_pleated(&k);
        let bytes = f.to_bytes();
        let g = RibbonFilter::from_bytes(&bytes).expect("valid buffer");
        assert_eq!(f.size_bytes(), g.size_bytes());
        assert!(k.iter().all(|&x| g.contains(x)), "false negative after roundtrip");
        // A few absent keys must answer identically before/after.
        for x in [1u64, 3, 999_999_999, u64::MAX] {
            assert_eq!(f.contains(x), g.contains(x));
        }
        assert!(RibbonFilter::from_bytes(&[0u8; 5]).is_none());
    }
}
