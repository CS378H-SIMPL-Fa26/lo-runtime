// I/O round-trip driver, mirroring the Rust and Zig probes. Reads one integer
// from stdin and echoes it, then prints the literal 42, each on its own line.
// Driven by tests/io_roundtrip.sh. Not part of the shipped runtime.

#include "lo_runtime/lo_runtime.h"

int main() {
  lo_runtime_init();

  const std::int32_t n = lo_read_int();
  lo_print_int(n, 0);
  lo_println(0);

  lo_print_int(42, 0);
  lo_println(0);

  lo_runtime_shutdown();
  return 0;
}
