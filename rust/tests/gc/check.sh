#!/usr/bin/env bash
# From any directory. Requires Rust's wasm32-unknown-unknown target, clang,
# wasm-ld, and the course wasmrun host. Runs identical cases on both targets.
set -euo pipefail
crate=$(cd "$(dirname "$0")/../.." && pwd)
build=$(mktemp -d)
trap 'rm -rf "$build"' EXIT
export CARGO_TARGET_DIR="$build/target"
# MODES=native permits local testing without the WASM tools.
# SEEDS controls deterministic random runs; failures print their reproducer.
read -r -a modes <<< "${MODES:-native wasm}"
read -r -a seeds <<< "${SEEDS:-1 7 42 12345 3735928559}"
export LO_HEAP_SIZE=4096
build_runtime() {
  for mode in "${modes[@]}"; do
    if [[ $mode == native ]]; then
      cargo build --manifest-path "$crate/Cargo.toml" --release --lib --features "$1"
    elif [[ $mode == wasm ]]; then
      cargo build --manifest-path "$crate/Cargo.toml" --release --lib --features "$1" --target wasm32-unknown-unknown
    else
      echo "Unknown mode: $mode" >&2; exit 2
    fi
  done
}
compile_probe() {
  local source=$1 define=$2
  if [[ $mode == native ]]; then
    "${CC:-cc}" -std=c11 -O2 "$define" "$crate/tests/gc/$source" \
      "$build/target/release/liblo_runtime.a" -lpthread -ldl -lm -o "$build/probe"
    command=("$build/probe")
  else
    "${WASM_CC:-clang}" --target=wasm32-unknown-unknown -std=c11 -O2 "$define" \
      -c "$crate/tests/gc/$source" -o "$build/probe.o"
    "${WASM_LD:-wasm-ld}" --no-entry --export=lo_entry --allow-undefined "$build/probe.o" \
      "$build/target/wasm32-unknown-unknown/release/liblo_runtime.a" -o "$build/probe.wasm"
    command=("${WASMRUN:-wasmrun}" "$build/probe.wasm")
  fi
}
positive() {
  local status=0
  "${command[@]}" >"$build/out" 2>"$build/err" || status=$?
  if [[ $status != 0 ]]; then
    cat "$build/out" "$build/err"
    echo "$mode $1 failed: status $status (C assertion line modulo 256)" >&2; exit 1
  fi
  echo "$mode $1 passed"
}
build_runtime gc-stress
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
for mode in "${modes[@]}"; do
  for case_id in {0..9}; do
    compile_probe probe.c "-DCASE=$case_id"
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

for mode in "${modes[@]}"; do
  compile_probe boundaries.c -DCASE=0
  positive boundaries
  for seed in "${seeds[@]}"; do
    compile_probe graph.c "-DSEED=$seed"
    positive "graph seed=$seed gc-stress"
  done
done
build_runtime gc-every-alloc
for mode in "${modes[@]}"; do
  compile_probe probe.c -DCASE=0
  positive "preservation gc-every-alloc"
  for seed in "${seeds[@]}"; do
    compile_probe graph.c "-DSEED=$seed"
    positive "graph seed=$seed gc-every-alloc"
  done
done
