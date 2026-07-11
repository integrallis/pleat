// Portions of this file are a Rust port of the ribbon filter kernel from
// fastfilter_cpp (https://github.com/FastFilter/fastfilter_cpp), whose ribbon
// implementation derives from RocksDB (https://github.com/facebook/rocksdb),
// both Apache-2.0. Algorithm, constants, and layout are transcribed from those
// sources (see PORT_NOTES.md and the per-item file:line references); this port
// is licensed MIT OR Apache-2.0 with attribution to the upstream authors.
//! Ribbon hashing layer — start position and coefficient row derivation.
//!
//! Ported from the reference `StandardHasher` (fastfilter_cpp `ribbon_impl.h`, RocksDB-derived)
//! for the homogeneous w=64 configuration, and gated against committed reference vectors
//! (`tests/vectors/homog_w64_r7.json`, produced by `tools/vecgen.cc`). Every constant here is
//! transcribed from that source with a file reference; none is chosen from memory.

/// Key pre-hash: `RibbonTS::HashFn` (filterapi.h) — murmur-style finalizer of `key + raw_seed`.
/// Our configuration uses `raw_seed = 0`.
#[inline]
pub fn ribbon_hash(key: u64, raw_seed: u64) -> u64 {
    let mut h = key.wrapping_add(raw_seed);
    h ^= h >> 33;
    h = h.wrapping_mul(0xff51_afd7_ed55_8ccd);
    h ^= h >> 33;
    h = h.wrapping_mul(0xc4ce_b9fe_1a85_ec53);
    h ^= h >> 33;
    h
}

/// Start slot: `GetStart` (!smash path, ribbon_impl.h:221) = `FastRangeGeneric(h, num_starts)`
/// = high 64 bits of the 128-bit product `h * num_starts`.
#[inline]
pub fn start(h: u64, num_starts: u64) -> u64 {
    (((h as u128) * (num_starts as u128)) >> 64) as u64
}

/// Coefficient row for w=64, !smash: `GetCoeffRow` (ribbon_impl.h:265,383) =
/// `(h * kCoeffAndResultFactor) | 1` (the `| 1` is kFirstCoeffAlwaysOne).
#[inline]
pub fn coeff_row(h: u64) -> u64 {
    const K_COEFF_AND_RESULT_FACTOR: u64 = 0xc28f_8282_2b65_0bed;
    h.wrapping_mul(K_COEFF_AND_RESULT_FACTOR) | 1
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn load_vectors() -> serde_json::Value {
        let text = std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/vectors/homog_w64_r7.json"),
        )
        .expect("vectors present");
        serde_json::from_str(&text).expect("reference vectors must be valid JSON")
    }

    fn field(value: &serde_json::Value, name: &str) -> u64 {
        value[name]
            .as_u64()
            .unwrap_or_else(|| panic!("missing or non-u64 reference field {name}"))
    }

    #[test]
    fn hash_start_coeff_match_reference_exactly() {
        let v = load_vectors();
        let raw_seed = field(&v["config"], "raw_seed");
        assert_eq!(raw_seed, 0);
        let vectors = v["hash_vectors"]
            .as_array()
            .expect("hash_vectors must be an array");
        let num_starts = field(&vectors[0], "_num_starts_for_start_field");
        for e in &vectors[1..] {
            let key = field(e, "key");
            let h = ribbon_hash(key, raw_seed);
            assert_eq!(h, field(e, "hash"), "hash mismatch for key {key}");
            assert_eq!(
                start(h, num_starts),
                field(e, "start"),
                "start mismatch for key {key}"
            );
            assert_eq!(
                coeff_row(h),
                field(e, "coeff"),
                "coeff mismatch for key {key}"
            );
        }
        assert!(vectors.len() >= 1001);
    }

    #[test]
    fn coeff_first_bit_always_one() {
        let v = load_vectors();
        let vectors = v["hash_vectors"]
            .as_array()
            .expect("hash_vectors must be an array");
        for e in &vectors[1..] {
            assert_eq!(coeff_row(field(e, "hash")) & 1, 1);
        }
    }
}
