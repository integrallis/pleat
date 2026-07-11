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
- OPEN QUESTION (do NOT guess): how the homogeneous FILTER query derives its expected result
  bits (GetResultRowFromHash returns 0 for homogeneous, yet FPR ≈ 2^-r). Read
  InterleavedSolutionStorage FilterQuery / PhsfQuery path in ribbon_impl.h + backsubst before
  porting queries. The differential vectors will arbitrate.
- num_slots sizing (filterapi.h HomogRibbonFilter): overhead = 1 + (4 + 7*0.25)/(8*8);
  num_slots = InterleavedSoln::RoundUpNumSlots(overhead * n); read RoundUpNumSlots before
  porting.
- Banding loop: ribbon_alg.h BandingAdd (:546-596) + BandingAddRange prefetch pipeline
  (:608-700). Backtracking unused for homogeneous.

Next actions (test-first ladder step 1):
1. tools/vecgen.cc — compile against the pinned headers with the e1b TS; emit JSON vectors:
   per-key (key, h, start, coeff, resultrow) for 1000 keys (splitmix64 seed 0xA11CE), plus a
   10K-key build's solution bytes FNV and query outcomes for 200 present/200 absent keys.
   Commit vectors as tests/vectors/*.json.
2. src/hash.rs written against those vectors (tests first, then impl).
3. Then banding (gate: solution FNV), then queries (gate: outcomes + FPR), per README ladder.
