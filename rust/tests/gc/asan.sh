#!/usr/bin/env bash
# Instrument BOTH Rust's collector and the C harness. Requires Rust nightly
# and clang with AddressSanitizer. Uses the host native target, not WASM.
set -euo pipefail
crate=$(cd "$(dirname "$0")/../.." && pwd)
build=$(mktemp -d)
trap 'rm -rf "$build"' EXIT
host=$(rustc +nightly -vV | sed -n 's/^host: //p')
export CARGO_TARGET_DIR="$build/target"
export LO_HEAP_SIZE=4096
export ASAN_OPTIONS="${ASAN_OPTIONS:-halt_on_error=1}"
read -r -a seeds <<< "${SEEDS:-1 7 42 12345 3735928559}"
for feature in gc-stress gc-every-alloc; do
  RUSTFLAGS='-Zsanitizer=address -Cforce-frame-pointers=yes' \
    cargo +nightly build --manifest-path "$crate/Cargo.toml" --lib \
      --target "$host" --features "$feature"
  run_probe() {
    "${CC:-clang}" -std=c11 -g -O1 -fsanitize=address -fno-omit-frame-pointer \
      "$crate/tests/gc/$1" "$2" "$build/target/$host/debug/liblo_runtime.a" \
      -lpthread -ldl -lm -o "$build/probe"
    "$build/probe"
    echo "ASan $feature $1 $2 passed"
  }
  run_probe probe.c -DCASE=0
  if [[ $feature == gc-stress ]]; then run_probe boundaries.c -DCASE=0; fi
  for seed in "${seeds[@]}"; do run_probe graph.c "-DSEED=$seed"; done
done
