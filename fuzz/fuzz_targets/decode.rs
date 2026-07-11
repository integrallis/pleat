#![no_main]
//! Arbitrary bytes must never panic or trigger UB in any decoder — they must only ever return
//! Ok(valid filter) or Err(DecodeError).
use libfuzzer_sys::fuzz_target;
use pleat::filter::{Ribbon, RibbonFilter, StdRibbon};

fuzz_target!(|data: &[u8]| {
    // If a buffer decodes, the resulting filter must be queryable without panicking.
    if let Ok(f) = RibbonFilter::from_bytes(data) {
        let _ = f.contains(0);
        let mut out = [false; 4];
        f.contains_batch(&[1, 2, 3, 4], &mut out);
    }
    let _ = Ribbon::<10>::from_bytes(data);
    if let Ok(f) = StdRibbon::<7>::from_bytes(data) {
        let _ = f.contains(0);
        let mut out = [false; 4];
        f.contains_batch(&[1, 2, 3, 4], &mut out);
    }
    let _ = StdRibbon::<8>::from_bytes(data);
});
