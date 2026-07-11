// Vector generator for the pleat differential gate. Compiles against the PINNED reference
// ribbon headers (fastfilter_cpp @ 924e560) and emits JSON test vectors that the Rust kernel
// port must reproduce exactly. This is the source of truth; the Rust port is written and
// gated against these, never the other way around.
//
// Config: homogeneous ribbon, w=64 (Hash=CoeffRow=u64), ResultRow=u32, kFirstCoeffAlwaysOne,
// !smash. Emits vectors for several result-bit widths r (columns), since the interleaved
// layout, build fingerprint, and query outcomes depend on r; per-key hash/start/coeff do not.
// Key pre-hash = RibbonTS::HashFn murmur mix of (key + raw_seed=0).
//
// Build: see tools/Makefile.

#include <cstdint>
#include <cstdio>
#include <memory>
#include <vector>

#include "ribbon_impl.h"

template <uint32_t kNumColumns>
struct HomogTS {
  static constexpr bool kIsFilter = true;
  static constexpr bool kHomogeneous = true;
  static constexpr bool kFirstCoeffAlwaysOne = true;
  static constexpr bool kUseSmash = false;
  using CoeffRow = uint64_t;
  using Hash = uint64_t;
  using Key = uint64_t;
  using Seed = uint32_t;
  using Index = size_t;
  using ResultRow = uint32_t;
  static constexpr bool kAllowZeroStarts = false;
  static constexpr uint32_t kFixedNumColumns = kNumColumns;
  static Hash HashFn(const Hash& input, Seed raw_seed) {
    uint64_t h = input + raw_seed;
    h ^= h >> 33;
    h *= UINT64_C(0xff51afd7ed558ccd);
    h ^= h >> 33;
    h *= UINT64_C(0xc4ceb9fe1a85ec53);
    h ^= h >> 33;
    return h;
  }
};

static inline uint64_t mix64(uint64_t z) {
  z = (z ^ (z >> 30)) * UINT64_C(0xbf58476d1ce4e5b9);
  z = (z ^ (z >> 27)) * UINT64_C(0x94d049bb133111eb);
  return z ^ (z >> 31);
}
static std::vector<uint64_t> keys(size_t n, uint64_t seed) {
  std::vector<uint64_t> k(n);
  uint64_t s = seed;
  for (size_t i = 0; i < n; i++) { s += UINT64_C(0x9e3779b97f4a7c15); k[i] = mix64(s); }
  return k;
}

static const size_t BUILD_N = 10000;

// Emit the r-independent per-key hash/start/coeff vectors (uses r=7's slot count for starts).
static size_t emit_hash_vectors() {
  using TS = HomogTS<7>;
  using InterleavedSoln = ribbon::SerializableInterleavedSolution<TS>;
  ribbon::StandardHasher<TS> hasher;
  const double overhead = 1.0 + (4.0 + 7.0 * 0.25) / (8.0 * 8.0);
  const size_t num_slots = InterleavedSoln::RoundUpNumSlots((size_t)(overhead * BUILD_N));
  const size_t num_starts = num_slots - 64 + 1;
  printf("  \"hash_vectors\": [\n");
  printf("    {\"_num_starts_for_start_field\": %zu},\n", num_starts);
  auto hv = keys(1000, UINT64_C(0xA11CE));
  for (size_t i = 0; i < hv.size(); i++) {
    uint64_t h = hasher.GetHash(hv[i]);
    printf("    {\"key\": %llu, \"hash\": %llu, \"start\": %zu, \"coeff\": %llu}%s\n",
           (unsigned long long)hv[i], (unsigned long long)h,
           hasher.GetStart(h, num_starts), (unsigned long long)hasher.GetCoeffRow(h),
           i + 1 < hv.size() ? "," : "");
  }
  printf("  ],\n");
  return num_slots;  // r-independent (depends only on overhead & n, both r-fixed here at r=7)
}

// Emit build fingerprint + query outcomes for a given r.
template <uint32_t R>
static void emit_for_r(bool last) {
  using TS = HomogTS<R>;
  using Banding = ribbon::StandardBanding<TS>;
  using InterleavedSoln = ribbon::SerializableInterleavedSolution<TS>;
  const double overhead = 1.0 + (4.0 + R * 0.25) / (8.0 * 8.0);
  const size_t num_slots = InterleavedSoln::RoundUpNumSlots((size_t)(overhead * BUILD_N));

  Banding banding(num_slots);
  auto bk = keys(BUILD_N, UINT64_C(0xA11CE));
  bool ok = banding.AddRange(bk.begin(), bk.end());
  const size_t bytes = (size_t)((num_slots * (double)R + 7) / 8);
  std::unique_ptr<char[]> ptr(new char[bytes]);
  InterleavedSoln soln(ptr.get(), bytes);
  soln.BackSubstFrom(banding);
  ribbon::StandardHasher<TS> hasher;
  uint64_t fnv = UINT64_C(0xcbf29ce484222325);
  for (size_t i = 0; i < bytes; i++) fnv = (fnv ^ (uint8_t)ptr[i]) * UINT64_C(0x100000001b3);

  printf("    \"%u\": {\n", R);
  printf("      \"num_slots\": %zu, \"bytes\": %zu, \"banding_ok\": %s, \"soln_fnv\": \"%016llx\",\n",
         num_slots, bytes, ok ? "true" : "false", (unsigned long long)fnv);
  printf("      \"present\": [");
  for (size_t i = 0; i < 200; i++)
    printf("%d%s", soln.FilterQuery(bk[i * 37 % BUILD_N], hasher) ? 1 : 0, i + 1 < 200 ? "," : "");
  printf("],\n      \"absent\": [");
  auto ak = keys(200, UINT64_C(0xD15EA5E));
  size_t fp = 0;
  for (size_t i = 0; i < 200; i++) {
    int r = soln.FilterQuery(ak[i] ^ UINT64_C(0x5555555555555555), hasher) ? 1 : 0;
    fp += r;
    printf("%d%s", r, i + 1 < 200 ? "," : "");
  }
  printf("],\n      \"absent_false_positives\": %zu\n    }%s\n", fp, last ? "" : ",");
}

int main() {
  printf("{\n");
  printf("  \"config\": {\"w\":64, \"homogeneous\":true, \"first_coeff_always_one\":true, \"raw_seed\":0},\n");
  emit_hash_vectors();
  printf("  \"build_n\": %zu,\n", BUILD_N);
  printf("  \"by_r\": {\n");
  emit_for_r<5>(false);
  emit_for_r<7>(false);
  emit_for_r<8>(false);
  emit_for_r<10>(true);
  printf("  }\n}\n");
  return 0;
}
