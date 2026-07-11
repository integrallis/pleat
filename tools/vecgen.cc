// Vector generator for the pleat differential gate. Compiles against the PINNED reference
// ribbon headers (fastfilter_cpp @ 924e560) and emits JSON test vectors that the Rust kernel
// port must reproduce exactly. This is the source of truth; the Rust port is written and
// gated against these, never the other way around.
//
// Config mirrors HomogRibbon64_7 (filterapi.h): Hash=CoeffRow=u64 (w=64), ResultRow=u32,
// r=7, homogeneous, kFirstCoeffAlwaysOne, !smash, filter. Key pre-hash = the RibbonTS::HashFn
// murmur mix of (key + raw_seed=0).
//
// Build: see tools/Makefile. Emits:
//   1. hash_vectors: for N1 keys, (key, hash, start, coeff) from the reference StandardHasher.
//   2. build: over N2 keys, num_slots + FNV-1a of the raw solution bytes.
//   3. queries: for the same build, membership (0/1) of N3 present + N3 absent keys.

#include <cstdint>
#include <cstdio>
#include <memory>
#include <vector>

#include "ribbon_impl.h"

template <typename CoeffType, bool kHomog, uint32_t kNumColumns, bool kSmash = false>
struct RibbonTS {
  static constexpr bool kIsFilter = true;
  static constexpr bool kHomogeneous = kHomog;
  static constexpr bool kFirstCoeffAlwaysOne = true;
  static constexpr bool kUseSmash = kSmash;
  using CoeffRow = CoeffType;
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
using TS = RibbonTS<uint64_t, true, 7>;
IMPORT_RIBBON_IMPL_TYPES(TS);
static constexpr double kFractionalCols = 7.0;

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

int main() {
  ribbon::StandardHasher<TS> hasher;  // default seed 0

  printf("{\n");

  // 1. Per-key hash/start/coeff vectors (1000 keys, seed 0xA11CE).
  printf("  \"config\": {\"w\":64, \"r\":7, \"homogeneous\":true, \"first_coeff_always_one\":true, \"raw_seed\":0},\n");
  printf("  \"hash_vectors\": [\n");
  auto hv = keys(1000, UINT64_C(0xA11CE));
  // num_starts must be known to compute start; use the build's slot count so starts are valid.
  const double overhead = 1.0 + (4.0 + kFractionalCols * 0.25) / (8.0 * sizeof(uint64_t));
  const size_t build_n = 10000;
  const size_t num_slots = InterleavedSoln::RoundUpNumSlots((size_t)(overhead * build_n));
  const size_t num_starts = num_slots - 64 + 1;  // banding.GetNumStarts() for w=64
  printf("    {\"_num_starts_for_start_field\": %zu},\n", num_starts);
  for (size_t i = 0; i < hv.size(); i++) {
    uint64_t k = hv[i];
    uint64_t h = hasher.GetHash(k);
    size_t st = hasher.GetStart(h, num_starts);
    uint64_t cr = (uint64_t)hasher.GetCoeffRow(h);
    printf("    {\"key\": %llu, \"hash\": %llu, \"start\": %zu, \"coeff\": %llu}%s\n",
           (unsigned long long)k, (unsigned long long)h, st, (unsigned long long)cr,
           i + 1 < hv.size() ? "," : "");
  }
  printf("  ],\n");

  // 2. Build over 10000 keys; FNV-1a of the raw solution bytes.
  Banding banding(num_slots);
  auto bk = keys(build_n, UINT64_C(0xA11CE));
  bool ok = banding.AddRange(bk.begin(), bk.end());
  const size_t bytes = (size_t)((num_slots * kFractionalCols + 7) / 8);
  std::unique_ptr<char[]> ptr(new char[bytes]);
  InterleavedSoln soln(ptr.get(), bytes);
  soln.BackSubstFrom(banding);
  uint64_t fnv = UINT64_C(0xcbf29ce484222325);
  for (size_t i = 0; i < bytes; i++) fnv = (fnv ^ (uint8_t)ptr[i]) * UINT64_C(0x100000001b3);
  printf("  \"build\": {\"n\": %zu, \"num_slots\": %zu, \"bytes\": %zu, \"banding_ok\": %s, \"soln_fnv\": \"%016llx\"},\n",
         build_n, num_slots, bytes, ok ? "true" : "false", (unsigned long long)fnv);

  // 3. Query outcomes: 200 present (from the built set) + 200 absent.
  printf("  \"queries\": {\n    \"present\": [");
  for (size_t i = 0; i < 200; i++)
    printf("%d%s", soln.FilterQuery(bk[i * 37 % build_n], hasher) ? 1 : 0, i + 1 < 200 ? "," : "");
  printf("],\n    \"absent\": [");
  auto ak = keys(200, UINT64_C(0xD15EA5E));
  size_t fp = 0;
  for (size_t i = 0; i < 200; i++) {
    int r = soln.FilterQuery(ak[i] ^ UINT64_C(0x5555555555555555), hasher) ? 1 : 0;
    fp += r;
    printf("%d%s", r, i + 1 < 200 ? "," : "");
  }
  printf("],\n    \"absent_false_positives\": %zu\n  }\n", fp);
  printf("}\n");
  return 0;
}
