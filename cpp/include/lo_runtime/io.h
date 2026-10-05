// I/O surface (runtime-abi.md §3.7).
#pragma once

#include "lo_runtime/object.h"

extern "C" {
// The print family's trailing `to_stderr` selects the destination stream:
// 0 = stdout, 1 = stderr (runtime-abi.md §3.7; other values reserved).
void lo_print_int(std::int32_t n, std::int32_t to_stderr);
void lo_print_bool(bool b, std::int32_t to_stderr);
void lo_print_string(Object *s, std::int32_t to_stderr);
void lo_println(std::int32_t to_stderr);

std::int32_t lo_read_int();
bool lo_read_bool();
Object *lo_read_string();
bool lo_eof();
}
