# pleat

Ribbon filters with **pleated construction**, so that space-optimal filters build at
Bloom-filter speed. An Integrallis project; companion to the paper *"Ribbon Catches Bloom"*.

**Status: pre-0.1. Nothing is exported until its gate passes.**

## Engineering policy

- **Production-anchored port.** The ribbon kernel (hashing, banding, back-substitution,
  interleaved solution storage) is transcribed from RocksDB's production implementation
  (`util/ribbon_impl.h`, `util/ribbon_alg.h` — the code that has guarded real databases since
  2021), cross-checked against the ribbon author's reference in fastfilter_cpp. No kernel code
  is written from memory or paraphrased from the paper.
- **Differential gate before anything ships.** The Rust kernel must reproduce, byte for byte,
  the solution fingerprints produced by the reference C++ kernel on the committed key sets
  from the paper repository (transposed-filters, results/e1b/phase1). CI fails the crate if
  the fingerprints diverge.
- **Test-first.** Every module lands as: reference test vectors first (ported from RocksDB's
  `ribbon_test.cc` where applicable), property tests (proptest) second, implementation third.
- **Built-in reproducible benchmarks.** Criterion benches reproduce the paper's Table 1 shape
  (arrival vs. pleated vs. parallel); raw outputs are committed and a `reproduce.sh`
  re-derives every README number from them. No number appears in this README without a
  committed artifact.
- **Licensing.** Crate dual-licensed MIT OR Apache-2.0 (Rust convention); RocksDB is
  Apache-2.0/GPL-2.0 dual — we port under Apache-2.0 with attribution headers in every ported
  file. License audit is a pre-publish gate.

## The technique

A ribbon filter is built by solving a banded linear system; each key's work is confined to a
short run of table slots at its hashed start position. Arrival-order insertion makes every
key a far jump through a table much larger than cache. Pleating folds the key stream into
L2-sized windows with one counting pass — 98% of the locality benefit of the full
start-position sort at roughly a quarter of its cost — and because the solved filter is
bit-identical under any insertion order, pleated and parallel builds verify themselves against
the sequential build with a single checksum.

## Road to 0.1 (each step gated)

1. Test vectors + hashing layer (RocksDB `StandardHasher` semantics).
2. Homogeneous banding kernel (w=64) → differential fingerprint gate.
3. Back-substitution + interleaved storage + queries → zero-false-negative and FPR gates.
4. `PleatPlan` integration: pleated sequential builder + checksum self-verification.
5. Parallel builder (slot-range ownership, boundary deferral) behind `parallel` feature.
6. Standard ribbon (w=128, seeds, backtracking) — the RocksDB production shape.
7. Criterion benches + committed baselines + reproduce.sh.
