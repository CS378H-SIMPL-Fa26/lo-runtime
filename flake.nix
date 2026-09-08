{
  description = "LO runtime (CS 378H) — wasm32 toolchain: build liblo_runtime.a for wasm-ld, plus the wasmrun host";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { self, nixpkgs, rust-overlay }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" "x86_64-darwin" "aarch64-darwin" ];
      forAllSystems = f: nixpkgs.lib.genAttrs systems (system:
        let
          pkgs = import nixpkgs { inherit system; overlays = [ rust-overlay.overlays.default ]; };
          # Stable Rust with the wasm32 target, matching CI (dtolnay/rust-toolchain@stable
          # + wasm32-unknown-unknown). rust-analyzer/rust-src are for the dev shell.
          toolchain = pkgs.rust-bin.stable.latest.default.override {
            extensions = [ "rust-src" "rust-analyzer" ];
            targets = [ "wasm32-unknown-unknown" ];
          };
        in
        f { inherit pkgs toolchain; });
    in
    {
      packages = forAllSystems ({ pkgs, toolchain }: rec {
        # The wasm32 static archive (runtime-abi.md §4.1): relocatable WASM objects
        # that a student's `student.o` is linked against with wasm-ld into one
        # module. Output: result/lib/liblo_runtime.a. The crate has no
        # dependencies, so an `--offline` cargo build needs no vendoring.
        lo-runtime-wasm = pkgs.stdenv.mkDerivation {
          pname = "lo-runtime-wasm";
          version = "0.1.0";
          src = ./rust;
          nativeBuildInputs = [ toolchain ];
          # stdenv's fixup would run `strip`/`ranlib` over $out/lib, which on
          # macOS rewrites the GNU-format archive into BSD format and corrupts
          # the wasm members (wasm-ld then fails with "section too large").
          dontStrip = true;
          buildPhase = ''
            export CARGO_HOME="$TMPDIR/cargo"
            cargo build --release --offline --lib --target wasm32-unknown-unknown
          '';
          installPhase = ''
            mkdir -p "$out/lib"
            cp target/wasm32-unknown-unknown/release/liblo_runtime.a "$out/lib/"
          '';
        };

        # The WASM host harness (tools/wasmrun): supplies the runtime's `host.*`
        # I/O imports via wasmtime and runs a linked module's `lo_entry`.
        wasmrun = pkgs.rustPlatform.buildRustPackage {
          pname = "wasmrun";
          version = "0.1.0";
          src = ./tools/wasmrun;
          cargoLock.lockFile = ./tools/wasmrun/Cargo.lock;
        };

        default = lo-runtime-wasm;
      });

      devShells = forAllSystems ({ pkgs, toolchain }: {
        default = pkgs.mkShell {
          # Unwrapped clang for `--target=wasm32`. The shell's default `clang` is
          # Nix's wrapped host compiler (kept, cargo needs a host linker), and its
          # wrapper injects host-only flags that break cross-compiling to wasm32.
          WASM_CC = "${pkgs.llvmPackages.clang-unwrapped}/bin/clang";
          packages = [
            toolchain                                    # cargo/rustc/clippy/rustfmt + wasm32 target
            pkgs.lld                                     # wasm-ld: link student.o + liblo_runtime.a
            pkgs.wabt                                    # wasm-objdump / wasm2wat / wat2wasm
            pkgs.llvmPackages.bintools-unwrapped         # llvm-nm / llvm-ar for inspecting the archive
            self.packages.${pkgs.stdenv.hostPlatform.system}.wasmrun         # run the linked module
            pkgs.nil                                     # Nix language servers, for editing this flake
            pkgs.nixd
          ];
          shellHook = ''
            echo "lo-runtime wasm shell: $(rustc --version)"
            echo "  cd rust && cargo build --target wasm32-unknown-unknown"
            echo "    -> target/wasm32-unknown-unknown/debug/liblo_runtime.a"
            echo "  wasm-ld --no-entry --export=lo_entry --allow-undefined student.o liblo_runtime.a -o program.wasm"
            echo "  wasmrun program.wasm"
            echo "  (\$WASM_CC --target=wasm32 -ffreestanding -nostdlib -c x.c   # C -> wasm object; see examples/wasm-hello)"
          '';
        };
      });
    };
}
