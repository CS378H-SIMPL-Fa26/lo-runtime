#!/usr/bin/env bash
# Host compiler emits x86 assembly; a Linux amd64 container builds/links/runs it.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/.." && pwd)
compiler=${LO_COMPILER:-"$root/../Liveoak/_build/default/bin/main.exe"}
context=${DOCKER_CONTEXT:-colima-gc-validation}
image=${GC_DOCKER_IMAGE:-rust:1-bookworm}
sweep_heaps=${GC_SWEEP_HEAPS:-}
for size in $sweep_heaps; do
  case "$size" in ''|*[!0-9]*) echo "GC_SWEEP_HEAPS requires space-separated positive bytes" >&2; exit 1;; esac
  if [ "$size" -le 0 ]; then echo "Heap size must be positive" >&2; exit 1; fi
done
work=$(mktemp -d)
container=
cleanup() {
  if [ -n "$container" ]; then docker --context "$context" rm -f "$container" >/dev/null; fi
  rm -rf "$work"
}
trap cleanup EXIT
for src in "$root"/tests/lo_programs/${GC_CASE_GLOB:-gc_meta_*.lo}; do
  if [ ! -f "$src" ]; then echo "No programs match GC_CASE_GLOB" >&2; exit 1; fi
  name=$(basename "$src" .lo)
  "$compiler" check "$src"
  "$compiler" native-asm "$src" > "$work/$name.s"
  cp "$src" "$work/$name.lo"
  cp "$root/tests/expected/$name.expected.out" "$work/$name.expected.out"
done
cat > "$work/run-gc-native.sh" <<'CONTAINER'
set -euo pipefail
cargo build --manifest-path /runtime/Cargo.toml --release --lib --offline
failures=0
for src in /tmp/gc_meta_*.lo; do
  name=$(basename "$src" .lo)
  cc -no-pie "/tmp/$name.s" /tmp/runtime-target/release/liblo_runtime.a -lpthread -ldl -lm -o "/tmp/$name.native"
  expected=$(sed -n 's|^// expected exit code: ||p' "$src")
  expected=${expected:-0}
  heap_size=$(sed -n 's|^// gc stress heap: ||p' "$src")
  trials="default $heap_size"
  if [ "$expected" = 0 ]; then
    for size in $GC_SWEEP_HEAPS; do trials="$trials sweep:$size"; done
  fi
  for trial in $trials; do
    heap=${trial#sweep:}
    status=0
    if [ "$heap" = default ]; then
      env -u LO_HEAP_SIZE timeout 120 "/tmp/$name.native" >/tmp/actual.out 2>/tmp/actual.err || status=$?
    else
      LO_HEAP_SIZE="$heap" timeout 120 "/tmp/$name.native" >/tmp/actual.out 2>/tmp/actual.err || status=$?
    fi
    if [[ "$trial" = sweep:* ]] && [ "$status" = 137 ] && grep -Fq 'lo_alloc: out of memory' /tmp/actual.err && head -c "$(wc -c < /tmp/actual.out)" "/tmp/$name.expected.out" | cmp -s /tmp/actual.out -; then
      echo "$name native@$heap sweep: OOM (exit 137)"
    elif [ "$status" = "$expected" ] && cmp -s /tmp/actual.out "/tmp/$name.expected.out" && { [ "$expected" != 137 ] || grep -Fq 'lo_alloc: out of memory' /tmp/actual.err; }; then
      if [[ "$trial" = sweep:* ]]; then
        echo "$name native@$heap sweep: SUCCESS (exit $status)"
      else
        echo "$name native@$heap: PASS (exit $status)"
      fi
    else
      echo "$name native@$heap: FAIL (exit $status)"
      head -c 400 /tmp/actual.err
      failures=$((failures + 1))
    fi
  done
done
echo "Failures: $failures"
test "$failures" = 0
CONTAINER
container=$(docker --context "$context" create --platform linux/amd64 \
  --mount "type=bind,src=$root/rust,dst=/runtime,readonly" \
  -e CARGO_TARGET_DIR=/tmp/runtime-target -e GC_SWEEP_HEAPS="$sweep_heaps" "$image" bash /tmp/run-gc-native.sh)
docker --context "$context" cp "$work/." "$container:/tmp/"
docker --context "$context" start -a "$container"
# docker start -a does not reliably propagate the container's exit code.
status=$(docker --context "$context" inspect --format '{{.State.ExitCode}}' "$container")
exit "$status"
