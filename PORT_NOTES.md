# Kernel port notes — extracted from reference source, not memory

Source of truth: transposed-filters/harness/fastfilter_cpp/src/ribbon/ribbon_impl.h (pinned
924e560; RocksDB-derived, by the ribbon author). Our config: Hash=CoeffRow=u64 (w=64),
ResultRow=u32, r=7, kHomogeneous=true, kFirstCoeffAlwaysOne=true, kUseSmash=false,
kIsFilter=true. Key pre-hash: filterapi.h RibbonTS::HashFn = murmur-style mix of
(key + raw_seed): h^=h>>33; h*=0xff51afd7ed558ccd; h^=h>>33; h*=0xc4ceb9fe1a85ec53; h^=h>>33.
Default raw_seed = 0.

Extracted semantics (file:line, verified 2026-07-10):
- GetStart (impl.h:221, !smash): FastRangeGeneric(h, num_starts) = ((u128)h * num_starts) >> 64.
- GetCoeffRow (impl.h:265..): a = h.wrapping_mul(0xc28f82822b650bed) [impl.h:383-384];
  Hash==u64==CoeffRow so cr = a; then cr |= 1 (kFirstCoeffAlwaysOne).
- GetResultRowFromHash (impl.h:322..): for kHomogeneous → **returns 0** (impl.h:330-331);
  the non-homogeneous path is bswap(h * kCoeffAndResultFactor) & mask — needed later for the
  w=128 standard config (kAltCoeffFactor1/2 = 0x876f170be4f1fcb9 / 0xf0433a4aecda4c5f for the
  smash path; kCoeffXor64 = 0xc367844a6e52731d for Hash<CoeffRow expansion).
- Homogeneous FILTER queries expect a zero result row. Back-substitution fills unconstrained
  rows deterministically from the hash, producing the expected ≈2^-r false-positive rate. This
  is covered by byte-exact reference vectors and the statistical false-positive-rate test.
- num_slots sizing (filterapi.h HomogRibbonFilter): overhead = 1 + (4 + 7*0.25)/(8*8);
  num_slots = InterleavedSoln::RoundUpNumSlots(overhead * n); read RoundUpNumSlots before
  porting.
- Banding loop: ribbon_alg.h BandingAdd (:546-596) + BandingAddRange prefetch pipeline
  (:608-700). Backtracking unused for homogeneous.

Port status: the vector generator and committed fixtures under `tests/vectors/` now cover the
per-key hash/start/coefficient/result values, complete banding solutions, and present/absent
query outcomes for both filter families. The Rust tests enforce those gates, construction-order
identity, round trips, malformed-input rejection, and measured false-positive rates.
