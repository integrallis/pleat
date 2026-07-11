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

    /// Minimal JSON pull for the fields we need (avoids a serde dep in the test).
    fn load_vectors() -> serde_lite::Vectors {
        serde_lite::parse(
            &std::fs::read_to_string(
                Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/vectors/homog_w64_r7.json"),
            )
            .expect("vectors present"),
        )
    }

    #[test]
    fn hash_start_coeff_match_reference_exactly() {
        let v = load_vectors();
        assert_eq!(v.raw_seed, 0);
        for e in &v.hash_vectors {
            let h = ribbon_hash(e.key, v.raw_seed);
            assert_eq!(h, e.hash, "hash mismatch for key {}", e.key);
            assert_eq!(
                start(h, v.num_starts),
                e.start,
                "start mismatch for key {}",
                e.key
            );
            assert_eq!(coeff_row(h), e.coeff, "coeff mismatch for key {}", e.key);
        }
        assert!(!v.hash_vectors.is_empty());
    }

    #[test]
    fn coeff_first_bit_always_one() {
        for e in &load_vectors().hash_vectors {
            assert_eq!(coeff_row(e.hash) & 1, 1);
        }
    }

    // Tiny hand-rolled parser for exactly this vector file — no external JSON crate needed
    // in the port's early stages. Replaced by serde if the crate later takes a serde dep.
    mod serde_lite {
        pub struct Entry {
            pub key: u64,
            pub hash: u64,
            pub start: u64,
            pub coeff: u64,
        }
        pub struct Vectors {
            pub raw_seed: u64,
            pub num_starts: u64,
            pub hash_vectors: Vec<Entry>,
        }
        fn field(obj: &str, name: &str) -> Option<u64> {
            let pat = format!("\"{name}\":");
            let i = obj.find(&pat)? + pat.len();
            let rest = obj[i..].trim_start();
            let end = rest
                .find(|c: char| !c.is_ascii_digit())
                .unwrap_or(rest.len());
            rest[..end].parse().ok()
        }
        pub fn parse(s: &str) -> Vectors {
            let raw_seed = field(s, "raw_seed").unwrap();
            let num_starts = field(s, "_num_starts_for_start_field").unwrap();
            let mut hash_vectors = Vec::new();
            // Each key entry is an object containing "key":
            for chunk in s.split("{\"key\":").skip(1) {
                let obj = format!("{{\"key\":{chunk}");
                hash_vectors.push(Entry {
                    key: field(&obj, "key").unwrap(),
                    hash: field(&obj, "hash").unwrap(),
                    start: field(&obj, "start").unwrap(),
                    coeff: field(&obj, "coeff").unwrap(),
                });
            }
            Vectors {
                raw_seed,
                num_starts,
                hash_vectors,
            }
        }
    }
}
