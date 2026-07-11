#![no_main]
//! Build a filter from arbitrary keys, serialize and deserialize it, and verify no false
//! negatives and byte-stable round-tripping.
use libfuzzer_sys::fuzz_target;
use pleat::filter::{RibbonFilter, StdRibbon};

fuzz_target!(|keys: Vec<u64>| {
    if keys.is_empty() || keys.len() > 100_000 {
        return;
    }
    let f = RibbonFilter::from_keys_pleated(&keys);
    let bytes = f.to_bytes();
    let g = RibbonFilter::from_bytes(&bytes).expect("valid buffer must round-trip");
    for &k in &keys {
        assert!(g.contains(k), "false negative after homogeneous round-trip");
    }
    assert_eq!(f.to_bytes(), g.to_bytes(), "round-trip not byte-stable");

    if let Some(s) = StdRibbon::<7>::from_keys_pleated(&keys) {
        let sb = s.to_bytes();
        let s2 = StdRibbon::<7>::from_bytes(&sb).expect("std buffer must round-trip");
        for &k in &keys {
            assert!(s2.contains(k), "false negative after standard round-trip");
        }
    }
});
