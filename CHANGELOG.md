# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/), and the project adheres to
[Semantic Versioning](https://semver.org/).

## [Unreleased]

Pre-release hardening in response to an external production-readiness audit:

### Added
- Versioned, self-describing, checksummed serialization format (`format` module) with a
  typed `DecodeError`; `from_bytes` validates every field before use.
- Compile-time enforcement of the result width `1 <= R <= 32`.
- Bounds-checked batch-query prefetch; adversarial-input parallel test; malformed-buffer tests.
- `Debug` for both filter types; `StdRibbon::bits_per_key`.

### Changed
- `from_bytes` now returns `Result<Self, DecodeError>` (was `Option`).
- Parallel builders detect boundary spills and defer them safely (no panics on adversarial input).
- Internal modules (`banding`, `hash`, raw solutions, `PleatPlan`) are now crate-private.
- `hash_key` documented as process/toolchain-local (Rust `Hash` is not portable).

### Removed
- Unused `rayon` dependency (parallel construction uses `std::thread`).

## [0.1.0] — unreleased
First release: homogeneous (w=64) and standard (w=128, RocksDB-shape) ribbon filters with
pleated construction; arrival / pleated / parallel builds (all bit-identical); tunable
false-positive rate; hashable keys; batch queries; serialization. Every kernel component is
differentially gated byte-for-byte against the reference C++ implementation.
