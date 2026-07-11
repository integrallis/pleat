// w=128 standard (non-homogeneous) ribbon vectors — the RocksDB production shape.
// Emits per-key hash/start/coeff(u128 hi:lo)/result-row at ordinal seed 0, plus a build that
// uses the reference seed-finding loop and records the chosen ordinal seed + solution
// fingerprint + query outcomes. Source of truth for the Rust w=128 port's differential gate.

#include <cstdint>
#include <cstdio>
#include <memory>
#include <vector>

#include "ribbon_impl.h"

template <uint32_t kNumColumns>
struct Std128TS {
  static constexpr bool kIsFilter = true;
  static constexpr bool kHomogeneous = false;
  static constexpr bool kFirstCoeffAlwaysOne = true;
  static constexpr bool kUseSmash = false;
  using CoeffRow = ribbon::Unsigned128;
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

static inline uint64_t hi64(ribbon::Unsigned128 v) { return ribbon::Upper64of128(v); }
static inline uint64_t lo64(ribbon::Unsigned128 v) { return ribbon::Lower64of128(v); }

static const size_t BUILD_N = 10000;
static constexpr uint32_t R = 7;

int main() {
  using TS = Std128TS<R>;
  using Banding = ribbon::StandardBanding<TS>;
  using InterleavedSoln = ribbon::SerializableInterleavedSolution<TS>;

  printf("{\n");
  printf("  \"config\": {\"w\":128, \"r\":%u, \"homogeneous\":false, \"raw_seed\":0},\n", R);

  // Per-key vectors at ordinal seed 0 (raw_seed 0).
  ribbon::StandardHasher<TS> h0;
  const double overhead = 1.0 + (4.0 + R * 0.25) / (8.0 * 16.0);  // sizeof(CoeffRow)=16
  const size_t num_slots = InterleavedSoln::RoundUpNumSlots((size_t)(overhead * BUILD_N));
  const size_t num_starts = num_slots - 128 + 1;
  printf("  \"num_starts_seed0\": %zu,\n", num_starts);
  printf("  \"hash_vectors\": [\n");
  auto hv = keys(1000, UINT64_C(0xA11CE));
  for (size_t i = 0; i < hv.size(); i++) {
    uint64_t hh = h0.GetHash(hv[i]);
    ribbon::Unsigned128 cr = h0.GetCoeffRow(hh);
    printf("    {\"key\": %llu, \"hash\": %llu, \"start\": %zu, \"coeff_hi\": %llu, "
           "\"coeff_lo\": %llu, \"result\": %u}%s\n",
           (unsigned long long)hv[i], (unsigned long long)hh, h0.GetStart(hh, num_starts),
           (unsigned long long)hi64(cr), (unsigned long long)lo64(cr),
           (unsigned)h0.GetResultRowFromHash(hh), i + 1 < hv.size() ? "," : "");
  }
  printf("  ],\n");

  // Build with the reference seed-finding loop.
  Banding banding;
  auto bk = keys(BUILD_N, UINT64_C(0xA11CE));
  bool ok = banding.ResetAndFindSeedToSolve(num_slots, bk.begin(), bk.end(), 0U, 63U);
  uint32_t chosen = banding.GetOrdinalSeed();
  const size_t bytes = (size_t)((num_slots * (double)R + 7) / 8);
  std::unique_ptr<char[]> ptr(new char[bytes]);
  InterleavedSoln soln(ptr.get(), bytes);
  soln.BackSubstFrom(banding);
  uint64_t fnv = UINT64_C(0xcbf29ce484222325);
  for (size_t i = 0; i < bytes; i++) fnv = (fnv ^ (uint8_t)ptr[i]) * UINT64_C(0x100000001b3);

  // Query hasher must use the chosen seed.
  ribbon::StandardHasher<TS> hq;
  hq.SetOrdinalSeed(chosen);

  printf("  \"build\": {\"n\": %zu, \"num_slots\": %zu, \"bytes\": %zu, \"banding_ok\": %s, "
         "\"chosen_ordinal_seed\": %u, \"soln_fnv\": \"%016llx\",\n",
         BUILD_N, num_slots, bytes, ok ? "true" : "false", chosen, (unsigned long long)fnv);
  printf("    \"present\": [");
  for (size_t i = 0; i < 200; i++)
    printf("%d%s", soln.FilterQuery(bk[i * 37 % BUILD_N], hq) ? 1 : 0, i + 1 < 200 ? "," : "");
  printf("],\n    \"absent\": [");
  auto ak = keys(200, UINT64_C(0xD15EA5E));
  size_t fp = 0;
  for (size_t i = 0; i < 200; i++) {
    int r = soln.FilterQuery(ak[i] ^ UINT64_C(0x5555555555555555), hq) ? 1 : 0;
    fp += r;
    printf("%d%s", r, i + 1 < 200 ? "," : "");
  }
  printf("],\n    \"absent_false_positives\": %zu\n  }\n}\n", fp);
  return 0;
}
