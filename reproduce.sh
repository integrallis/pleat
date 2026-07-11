#!/usr/bin/env bash
# Regenerate reference test vectors, run the differential gate, and benchmark.
set -e
cd "$(dirname "$0")"
echo "== regenerate reference vectors (requires the pinned fastfilter_cpp headers) =="
if [ -d ../transposed-filters/harness/fastfilter_cpp ]; then
  make -C tools vectors
else
  echo "  (reference headers not present; using committed tests/vectors/*.json)"
fi
echo "== differential gate + property tests =="
cargo test --all-features -q
echo "== construction benchmark =="
cargo bench --bench construct
echo "== done =="
