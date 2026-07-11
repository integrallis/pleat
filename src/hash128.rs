//! Hashing layer for w=128 standard (non-homogeneous) ribbon — the RocksDB production shape.
//!
//! Ported from the reference `StandardHasher` (fastfilter_cpp `ribbon_impl.h`) for w=128, and
//! gated against committed reference vectors (`tests/vectors/std_w128_r7.json`). Constants
//! transcribed from that source by file reference; none from memory. The start position reuses
//! the 64-bit fastrange from [`crate::hash`]; the coefficient row and result row expand the
//! 64-bit key hash, and the ordinal-seed mapping drives the construction retry loop.

pub use crate::hash::{ribbon_hash, start};

const K_COEFF_AND_RESULT_FACTOR: u64 = 0xc28f_8282_2b65_0bed; // ribbon_impl.h:383
const K_COEFF_XOR64: u64 = 0xc367_844a_6e52_731d; // ribbon_impl.h:389
                                                  // Ordinal <-> raw seed mixing (ribbon_impl.h:392-397).
const K_SEED_MIX_MASK: u64 = 0xf0f0_f0f0_f0f0_f0f0;
const K_SEED_MIX_SHIFT: u32 = 4;
const K_TO_RAW_SEED_FACTOR: u64 = 0xc782_19a2_3eea_dd03;

/// 128-bit coefficient row for w=128, !smash (GetCoeffRow, ribbon_impl.h:265-303):
/// `a = h * factor`; `c = (a << 64) ^ (a ^ kCoeffXor64)`; then set the low bit
/// (kFirstCoeffAlwaysOne).
#[inline]
pub fn coeff_row_128(h: u64) -> u128 {
    let a = h.wrapping_mul(K_COEFF_AND_RESULT_FACTOR);
    let c = ((a as u128) << 64) ^ ((a ^ K_COEFF_XOR64) as u128);
    c | 1
}

/// Result row for a filter, non-homogeneous (GetResultRowFromHash, ribbon_impl.h:309-330):
/// `byteswap(h * factor)` truncated to u32. The interleaved query uses only its low `R` bits.
#[inline]
pub fn result_row(h: u64) -> u32 {
    let a = h.wrapping_mul(K_COEFF_AND_RESULT_FACTOR);
    a.swap_bytes() as u32 // EndianSwapValue(a) then truncate to ResultRow (low 32 bits)
}

/// Map an ordinal seed (0,1,2,...) to the raw seed used in the key pre-hash
/// (SetOrdinalSeed, ribbon_impl.h:349-367). Seed is 32-bit, mixed via a 64-bit intermediate.
#[inline]
pub fn ordinal_to_raw_seed(ordinal: u32) -> u32 {
    let mut tmp = (ordinal as u64).wrapping_mul(K_TO_RAW_SEED_FACTOR);
    tmp ^= (tmp & K_SEED_MIX_MASK) >> K_SEED_MIX_SHIFT;
    tmp as u32
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn load() -> String {
        std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/vectors/std_w128_r7.json"),
        )
        .unwrap()
    }
    fn num(json: &str, name: &str) -> u64 {
        let pat = format!("\"{name}\":");
        let i = json.find(&pat).unwrap() + pat.len();
        let rest = json[i..].trim_start();
        let end = rest
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(rest.len());
        rest[..end].parse().unwrap()
    }

    #[test]
    fn w128_hash_coeff_result_seed_match_reference() {
        let j = load();
        let num_starts = num(&j, "num_starts_seed0");
        // Iterate hash_vectors objects (each begins with "key":).
        let mut n = 0;
        for chunk in j.split("{\"key\":").skip(1) {
            let obj = format!("{{\"key\":{chunk}");
            if !obj.contains("\"coeff_hi\":") {
                continue;
            }
            let key = num(&obj, "key");
            let h = ribbon_hash(key, 0);
            assert_eq!(h, num(&obj, "hash"), "hash mismatch key {key}");
            assert_eq!(
                start(h, num_starts),
                num(&obj, "start"),
                "start mismatch key {key}"
            );
            let cr = coeff_row_128(h);
            assert_eq!(
                (cr >> 64) as u64,
                num(&obj, "coeff_hi"),
                "coeff_hi mismatch key {key}"
            );
            assert_eq!(
                cr as u64,
                num(&obj, "coeff_lo"),
                "coeff_lo mismatch key {key}"
            );
            assert_eq!(
                result_row(h) as u64,
                num(&obj, "result"),
                "result mismatch key {key}"
            );
            n += 1;
        }
        assert!(n >= 1000, "expected >=1000 vectors, got {n}");
    }

    #[test]
    fn seed_mapping_is_injective_and_zero_fixed() {
        assert_eq!(ordinal_to_raw_seed(0), 0);
        let seeds: std::collections::HashSet<u32> = (0..64).map(ordinal_to_raw_seed).collect();
        assert_eq!(
            seeds.len(),
            64,
            "ordinal->raw seed must be injective over 0..64"
        );
    }
}
