```
      ___           ___       ___           ___           ___     
     /\  \         /\__\     /\  \         /\  \         /\  \    
    /::\  \       /:/  /    /::\  \       /::\  \        \:\  \   
   /:/\:\  \     /:/  /    /:/\:\  \     /:/\:\  \        \:\  \  
  /::\~\:\  \   /:/  /    /::\~\:\  \   /::\~\:\  \       /::\  \ 
 /:/\:\ \:\__\ /:/__/    /:/\:\ \:\__\ /:/\:\ \:\__\     /:/\:\__\
 \/__\:\/:/  / \:\  \    \:\~\:\ \/__/ \/__\:\/:/  /    /:/  \/__/
      \::/  /   \:\  \    \:\ \:\__\        \::/  /    /:/  /     
       \/__/     \:\  \    \:\ \/__/        /:/  /     \/__/      
                  \:\__\    \:\__\         /:/  /                 
                   \/__/     \/__/         \/__/                  
```

[![license](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue)](#license)

Ribbon filters with **pleated construction** — a one-pass, cache-window reordering that builds
space-optimal filters at close to Bloom-filter speed, and roughly twice as fast as building in
arrival order at scale. An Integrallis project; companion to the paper *"Ribbon Catches Bloom:
Pleated Construction at Bloom Speed."*

Two filter families — homogeneous `RibbonFilter` (w=64) and standard `StdRibbon` (w=128, the
RocksDB shape) — each with arrival / pleated / parallel construction (all bit-identical),
tunable false-positive rate, arbitrary hashable keys, batch queries, and serialization. Every
kernel component is differentially gated byte-for-byte against the reference C++ implementation.

A ribbon filter answers approximate set membership — "is this key possibly in the set?" — in
about 7.6 bits per key at a ~0.8% false-positive rate, well under a Bloom filter's memory for
the same accuracy. The cost has always been construction: ribbon filters are built by solving a
banded linear system, and doing that in arrival order makes every key a random jump through a
table far larger than cache. Pleating groups keys into cache-sized windows with a single
counting pass first, so the banding stays local.

```rust
use pleat::filter::RibbonFilter;

let keys: Vec<u64> = /* your 64-bit key hashes */ vec![10, 20, 30];
let filter = RibbonFilter::from_keys_pleated(&keys);

assert!(filter.contains(20));           // members always present
// filter.contains(999) is false with ~99.2% probability

let bytes = filter.to_bytes();          // persist
let restored = RibbonFilter::from_bytes(&bytes).unwrap();
```

Construction variants, all producing the **bit-identical** filter (banding is order-independent):

```rust
RibbonFilter::from_keys(&keys);              // arrival order (reference default)
RibbonFilter::from_keys_pleated(&keys);      // pleated: ~2x faster at scale
RibbonFilter::from_keys_parallel(&keys, 8);  // slot-range parallel (feature "parallel")
RibbonFilter::from_hashable(&["a", "b"]);    // any Hash type (strings, tuples, ...)
```

The `*_hashable` helpers use Rust's `Hash` trait and are intended for values produced and
consumed by the same crate/toolchain. For durable or cross-language filters, define a stable
application encoding and hash it to `u64`, then use the `from_keys`/`contains` APIs.

Query one key or a batch (batch prefetches for throughput):

```rust
f.contains(key);
f.contains_hashable(&"some string");
let mut out = vec![false; probes.len()];
f.contains_batch(&probes, &mut out);
```

Tune the false-positive rate with the result-width parameter `R` (~2^-R):

```rust
use pleat::filter::Ribbon;
let low_fpr = Ribbon::<10>::from_keys_pleated(&keys);   // ~0.1% FPR, ~10.9 bits/key
// RibbonFilter is the alias Ribbon<7> (~0.8% FPR, ~7.6 bits/key)
```

## Measured construction cost

On an Intel i9-14900HX, ns/key (means of 10, `cargo bench --bench construct`; raw criterion
output committed under `benches/`):

| n keys | arrival | pleated | parallel 8t |
|---|---|---|---|
| 1,000,000 (table fits in L3) | 23.8 | 26.1 | 21.9 |
| 10,000,000 (table exceeds L3) | 60.4 | **26.0** | 24.4 |

Pleating pays off once the banding table exceeds cache (2.3x at 10M keys here); below that
crossover it costs slightly more than arrival order, as expected. These are this crate's own
numbers; the paper's headline figures are measured on the reference C++ kernel.

## Query, serialization, and accuracy

Same machine, 10M-key filter (`cargo bench --bench query --bench serde`):

- **Query (miss-dominated), ns per lookup:** homogeneous 17.4 scalar / 15.4 batch; standard 24.2 scalar.
  `contains_batch` prefetches, giving ~12% over scalar for homogeneous.
- **Serialization:** validated `from_bytes` (checksum + full field validation) runs at ~1.2 GB/s —
  validation is not a bottleneck.
- **False-positive rate** at r=7, measured over 2M absent probes: 0.783% (theory 2⁻⁷ = 0.781%);
  a statistical test asserts this on every `cargo test` run.

## Correctness

Every kernel component is **differentially gated** against the reference ribbon implementation
(`fastfilter_cpp`, by the ribbon authors, RocksDB-derived). `tools/vecgen.cc` emits committed
test vectors — per-key hash/start/coefficient values, a full-build solution fingerprint, and
query outcomes — and the Rust tests assert byte-exact agreement:

- hashing reproduces the reference per-key values on 1000 vectors;
- banding + back-substitution reproduce the reference's serialized solution byte-for-byte;
- queries reproduce the reference's present/absent outcomes;
- pleated and parallel builds are verified bit-identical to the sequential build.

Nothing in the kernel is written from memory; several subtle transcription errors were
caught, including the fact that homogeneous back-substitution fills unconstrained rows with
pseudorandom data, not zero, which preserves the false-positive rate.

## Scope

This release implements **homogeneous ribbon** at w=64 with a tunable result width R (columns):
the false-positive rate is ~2^-R and the size is ~1.09·R bits per key, so R=7 gives ~0.8% FPR
at ~7.6 bits/key — the variant used as the reference benchmark — and R=10 gives ~0.1% at
~10.9 bits/key. Gated against reference vectors for R in {5, 7, 8, 10}. Keys are 64-bit; hash
your keys to `u64` first.

**Standard ribbon (w=128)** — the RocksDB production shape — is also available via `StdRibbon`,
with the same pleated and arrival construction (bit-identical, seed-retry included) and slightly
tighter space:

```rust
use pleat::filter::StdRibbon;
let f = StdRibbon::<8>::from_keys_pleated(&keys).expect("solves");  // ~0.4% FPR
assert!(f.contains(keys[0]));
```

## Reproduce

```bash
cargo test --all-features         # differential gate + property tests
cargo test --no-default-features # serial-only feature configuration
cargo clippy --all-features --all-targets -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --all-features
cargo deny --all-features check   # advisories, licenses, sources, dependency policy
cargo bench --bench construct     # construction benchmark
cargo +nightly fuzz run decode -- -max_total_time=30
./reproduce.sh                    # regenerate reference vectors + run gate + bench
```

## License

MIT OR Apache-2.0. The ribbon kernel is ported from RocksDB / fastfilter_cpp (Apache-2.0);
ported files carry attribution.
