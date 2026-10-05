#include "lo_runtime/io.h"

#include "lo_runtime/abort.h"
#include "lo_runtime/alloc.h"

#include <cctype>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <string>

#ifndef __wasm__
namespace {

// Peek the next input byte without consuming it (the ABI's read semantics need a
// one-byte lookahead). Mirrors the Rust/Zig skeletons' peek-based parsing.
int peek_char() {
  const int c = std::getchar();
  if (c != EOF) {
    std::ungetc(c, stdin);
  }
  return c;
}

void skip_whitespace() {
  int c = peek_char();
  while (c != EOF && (std::isspace(static_cast<unsigned char>(c)) != 0)) {
    std::getchar();
    c = peek_char();
  }
}

// The stream the print family's selector names: 0 = stdout, 1 = stderr. Other
// values are reserved (runtime-abi.md §3.7); codegen never emits them, and they
// fall back to stdout here.
std::FILE *print_stream(std::int32_t to_stderr) { return to_stderr == 1 ? stderr : stdout; }

// Flush right after every print-family write, matching the Rust and Zig
// skeletons: a program interleaving `out` and `err` must land each byte on its
// stream as it is issued, not at exit.
void flush_stream(std::FILE *f) { std::fflush(f); }

} // namespace
#endif

// Native I/O uses <cstdio> (C++ reaches stdin/stdout directly). lo_eof peeks with
// getc/ungetc rather than feof, which only reports EOF after a failed read — the
// ABI's lo_eof is "at end-of-input without consuming". WASM forwards to host
// imports the harness wires.

#ifdef __wasm__
extern "C" {
void host_print_int(std::int32_t n, std::int32_t to_stderr);
void host_print_bool(std::int32_t b, std::int32_t to_stderr);
void host_print_bytes(const std::uint8_t *p, std::int32_t len, std::int32_t to_stderr);
void host_println(std::int32_t to_stderr);
std::int32_t host_read_int();
std::int32_t host_read_bool();
std::int32_t host_read_line_len();
std::int32_t host_read_line_into(std::uint8_t *p, std::int32_t max);
std::int32_t host_eof();
}
#endif

extern "C" void lo_print_int(std::int32_t n, std::int32_t to_stderr) {
#ifdef __wasm__
  host_print_int(n, to_stderr);
#else
  std::FILE *f = print_stream(to_stderr);
  std::fprintf(f, "%d", n);
  flush_stream(f);
#endif
}

extern "C" void lo_print_bool(bool b, std::int32_t to_stderr) {
#ifdef __wasm__
  host_print_bool(b ? 1 : 0, to_stderr);
#else
  std::FILE *f = print_stream(to_stderr);
  std::fputs(b ? "true" : "false", f);
  flush_stream(f);
#endif
}

extern "C" void lo_print_string(Object *s, std::int32_t to_stderr) {
  if (s == nullptr) {
    return;
  }
  auto *so = reinterpret_cast<StringObject *>(s);
  const auto *data = reinterpret_cast<const std::uint8_t *>(s) + lo::string_data_offset();
#ifdef __wasm__
  host_print_bytes(data, static_cast<std::int32_t>(so->length), to_stderr);
#else
  std::FILE *f = print_stream(to_stderr);
  std::fwrite(data, 1, so->length, f);
  flush_stream(f);
#endif
}

extern "C" void lo_println(std::int32_t to_stderr) {
#ifdef __wasm__
  host_println(to_stderr);
#else
  std::FILE *f = print_stream(to_stderr);
  std::fputc('\n', f);
  flush_stream(f);
#endif
}

extern "C" std::int32_t lo_read_int() {
#ifdef __wasm__
  return host_read_int();
#else
  skip_whitespace();
  // EOF before any integer characters -> 111. A non-whitespace byte present means
  // input exists; failure to parse it is malformed (110), not EOF.
  if (peek_char() == EOF) {
    lo::runtime_abort("lo_read_int: end of input", 111);
  }
  std::string token;
  int c = peek_char();
  if (c == '-' || c == '+') {
    token.push_back(static_cast<char>(c));
    std::getchar();
  }
  bool saw_digit = false;
  while ((c = peek_char()) != EOF && (std::isdigit(static_cast<unsigned char>(c)) != 0)) {
    token.push_back(static_cast<char>(c));
    std::getchar();
    saw_digit = true;
  }
  if (!saw_digit) {
    lo::runtime_abort("lo_read_int: malformed token", 110);
  }
  char *end = nullptr;
  const long value = std::strtol(token.c_str(), &end, 10);
  if (end == token.c_str() || *end != '\0' || value < INT32_MIN || value > INT32_MAX) {
    lo::runtime_abort("lo_read_int: malformed token", 110);
  }
  return static_cast<std::int32_t>(value);
#endif
}

extern "C" bool lo_read_bool() {
#ifdef __wasm__
  return host_read_bool() != 0;
#else
  skip_whitespace();
  std::string token;
  int c = 0;
  while ((c = peek_char()) != EOF && (std::isspace(static_cast<unsigned char>(c)) == 0)) {
    token.push_back(static_cast<char>(c));
    std::getchar();
  }
  if (token == "true") {
    return true;
  }
  if (token == "false") {
    return false;
  }
  lo::runtime_abort("lo_read_bool: invalid token", 112);
#endif
}

extern "C" Object *lo_read_string() {
#ifdef __wasm__
  const std::int32_t raw = host_read_line_len();
  const std::uint32_t len = raw < 0 ? 0 : static_cast<std::uint32_t>(raw);
  Object *o = lo::bump_alloc_string(len);
  if (len > 0) {
    auto *d = reinterpret_cast<std::uint8_t *>(o) + lo::string_data_offset();
    host_read_line_into(d, static_cast<std::int32_t>(len));
  }
  return o;
#else
  std::string line;
  int c = 0;
  while ((c = std::getchar()) != EOF && c != '\n') {
    line.push_back(static_cast<char>(c));
  }
  Object *o = lo::bump_alloc_string(static_cast<std::uint32_t>(line.size()));
  if (!line.empty()) {
    auto *d = reinterpret_cast<std::uint8_t *>(o) + lo::string_data_offset();
    // LO strings are length-prefixed (StringObject::length), not null-terminated.
    // NOLINTNEXTLINE(bugprone-not-null-terminated-result)
    std::memcpy(d, line.data(), line.size());
  }
  return o;
#endif
}

extern "C" bool lo_eof() {
#ifdef __wasm__
  return host_eof() != 0;
#else
  const int c = std::getchar();
  if (c == EOF) {
    return true;
  }
  std::ungetc(c, stdin);
  return false;
#endif
}
