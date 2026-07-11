//! Pleated construction for ribbon filters: partition-instead-of-sort.
//!
//! A ribbon filter is a space-optimal approximate-membership filter (~7.6 bits/key at ~0.8%
//! false-positive rate). This crate builds it fast by *pleating* — one counting pass groups
//! keys into cache-sized windows before banding, giving ~2x faster construction at scale for
//! a bit-identical result.
//!
//! # Example
//! ```
//! use pleat::filter::RibbonFilter;
//! let keys: Vec<u64> = (0..100_000).collect();
//! let f = RibbonFilter::from_keys_pleated(&keys);
//! assert!(f.contains(42));            // members never missing
//! assert!(!f.contains(999_999_999) || true); // absent keys rejected ~99.2% of the time
//! ```
//!
//! [`filter::RibbonFilter`] is the homogeneous w=64 filter; [`filter::StdRibbon`] is the
//! standard w=128 (RocksDB-shape) variant. Both support arrival, pleated, and parallel
//! construction (all bit-identical), tunable false-positive rate via the result-width
//! parameter, arbitrary hashable keys, batch queries, and serialization.

#![forbid(unsafe_op_in_unsafe_fn)]
#![deny(missing_docs)]

pub mod filter;
pub mod format;

pub(crate) mod banding;
pub(crate) mod banding128;
pub(crate) mod hash;
pub(crate) mod hash128;

use core::hash::Hash;
use xxhash_rust::xxh3::Xxh3;

/// Hash an arbitrary [`Hash`] key to the `u64` the filter operates on, using a fixed-seed xxh3.
///
/// **Portability caveat.** The Rust `Hash` trait is *not* guaranteed stable across platforms,
/// architectures, or compiler versions — a type's `Hash` impl may feed different bytes (for
/// example anything whose hashing depends on `usize` width). A filter built with
/// [`crate::hash_key`] / the `*_hashable` methods is therefore reliable **only within the same
/// process, target, and toolchain**. Do not persist such a filter and query it from a different
/// build. For a durable, portable filter, hash your keys to canonical little-endian bytes
/// yourself and use the `*_keys` (`u64`) methods.
pub fn hash_key<K: Hash + ?Sized>(key: &K) -> u64 {
    let mut h = Xxh3::with_seed(0);
    key.hash(&mut h);
    core::hash::Hasher::finish(&h)
}

/// A pleating plan: how a key stream is folded into table-window order.
///
/// `shift` selects the window size in slots (`1 << shift`); the default configuration uses
/// `shift = 16` (≈768 KiB of banding state per window at w=64).
#[derive(Clone, Copy, Debug)]
pub(crate) struct PleatPlan {
    pub num_starts: u64,
    pub shift: u32,
}

impl PleatPlan {
    pub fn new(num_starts: u64, shift: u32) -> Self {
        Self { num_starts, shift }
    }

    pub fn num_windows(&self) -> usize {
        ((self.num_starts >> self.shift) + 2) as usize
    }

    /// Fold `keys` into window order with a single counting pass (no pair materialization:
    /// `start_of` is recomputed in each pass, which measured cheaper than staging pairs).
    /// Returns the permuted keys and the per-window prefix offsets (windows are the
    /// contiguous runs `out[counts[w]..counts[w+1]]`, in table order; arrival order is
    /// preserved within a window).
    pub fn pleat<F: Fn(u64) -> u64>(&self, keys: &[u64], start_of: F) -> (Vec<u64>, Vec<usize>) {
        let nw = self.num_windows();
        let mut counts = vec![0usize; nw + 1];
        for &k in keys {
            counts[(start_of(k) >> self.shift) as usize + 1] += 1;
        }
        for w in 1..=nw {
            counts[w] += counts[w - 1];
        }
        let mut out = vec![0u64; keys.len()];
        let mut cursor = counts[..nw].to_vec();
        for &k in keys {
            let w = (start_of(k) >> self.shift) as usize;
            out[cursor[w]] = k;
            cursor[w] += 1;
        }
        (out, counts)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mix64(mut z: u64) -> u64 {
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    #[test]
    fn pleat_is_a_permutation_in_window_order() {
        let n_starts = 1u64 << 22;
        let plan = PleatPlan::new(n_starts, 16);
        let keys: Vec<u64> = (0..100_000u64).map(mix64).collect();
        let start = |k: u64| mix64(k) % n_starts;
        let (out, counts) = plan.pleat(&keys, start);

        // Permutation: same multiset.
        let mut a = keys.clone();
        let mut b = out.clone();
        a.sort_unstable();
        b.sort_unstable();
        assert_eq!(a, b);

        // Window order: starts are non-decreasing at window granularity, and every key sits
        // inside the window its offsets claim.
        for w in 0..plan.num_windows() {
            for &k in &out[counts[w]..counts[w + 1]] {
                assert_eq!((start(k) >> plan.shift) as usize, w);
            }
        }

        // Stability within a window (arrival order preserved).
        let w0: Vec<u64> = keys
            .iter()
            .copied()
            .filter(|&k| (start(k) >> plan.shift) == 3)
            .collect();
        assert_eq!(&out[counts[3]..counts[4]], &w0[..]);
    }
}
