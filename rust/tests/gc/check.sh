#!/usr/bin/env bash
# From any directory. Requires Rust's wasm32-unknown-unknown target, clang,
# wasm-ld, and the course wasmrun host. Runs identical cases on both targets.
set -euo pipefail
crate=$(cd "$(dirname "$0")/../.." && pwd)
build=$(mktemp -d)
trap 'rm -rf "$build"' EXIT
export CARGO_TARGET_DIR="$build/target"
cargo build --manifest-path "$crate/Cargo.toml" --release --lib --features gc-stress
cargo build --manifest-path "$crate/Cargo.toml" --release --lib --features gc-stress --target wasm32-unknown-unknown
expected=(
  ''
  'unknown descriptor'
  'reference is not an object start'
  'unregistered non-heap reference'
  'free space is not poisoned'
  'object extends beyond allocation frontier|string size overflow'
  'unregistered non-heap reference'
  'reference into free heap'
  'unexpected GC header bits'
)
for mode in native wasm; do
  for case_id in {0..9}; do
    if [[ $mode == native ]]; then
      cc -std=c11 -O2 -DCASE="$case_id" "$crate/tests/gc/probe.c" \
        "$build/target/release/liblo_runtime.a" -lpthread -ldl -lm -o "$build/probe"
      command=("$build/probe")
    else
      clang --target=wasm32-unknown-unknown -std=c11 -O2 -DCASE="$case_id" \
        -c "$crate/tests/gc/probe.c" -o "$build/probe.o"
      wasm-ld --no-entry --export=lo_entry --allow-undefined "$build/probe.o" \
        "$build/target/wasm32-unknown-unknown/release/liblo_runtime.a" -o "$build/probe.wasm"
      command=(wasmrun "$build/probe.wasm")
    fi
    status=0
    "${command[@]}" >"$build/out" 2>"$build/err" || status=$?
    if [[ $case_id == 0 ]]; then
      [[ $status == 0 ]] || { cat "$build/err"; echo "$mode preservation probe failed: $status"; exit 1; }
    elif [[ $case_id == 9 ]]; then
      [[ $status == 137 ]] && grep -q 'lo_alloc: out of memory' "$build/err" \
        || { cat "$build/err"; echo "$mode live-set overflow failed: $status"; exit 1; }
    else
      [[ $status != 0 ]] && grep -q 'lo_gc_verify: before collection:' "$build/err" \
        && grep -Eq "${expected[$case_id]}" "$build/err" \
        || { cat "$build/err"; echo "$mode corruption $case_id not diagnosed: $status"; exit 1; }
      if [[ $mode == native && $status != 134 ]]; then
        echo "unexpected native diagnostic exit: $status"; exit 1
      fi
    fi
    echo "$mode case $case_id passed"
  done
done
