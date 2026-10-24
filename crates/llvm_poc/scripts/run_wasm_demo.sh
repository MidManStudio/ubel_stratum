#!/usr/bin/env bash
# crates/llvm_poc/scripts/run_wasm_demo.sh
#
# Feasibility demo for a browser playground: take the LLVM IR the emitter produces, compile it
# to WebAssembly, run it in a JavaScript engine (Node) and compare with the interpreter.
#
#   bash crates/llvm_poc/scripts/run_wasm_demo.sh
#
# This proves the RUN side only. It uses clang on the host to turn IR into wasm; a browser
# would need an in-browser LLVM for that step, which exists but is large (see docs/LLVM_POC.md).
# The runtime is wasm/wasm_rt.c: freestanding, no libc, printing and traps imported from JS.
# Programs that allocate (structs from the GC heap or from an arena) are SKIPPED, not failed:
# the wasm runtime has no GC and no arena memory. Everything else in the Stage 1B subset runs.
#
# Needs: clang with the wasm32 target, wasm-ld, node, cargo.
# Environment: CLANG (default clang), WASM_LD (default wasm-ld), OUT_DIR (default
# ${TMPDIR:-/tmp}/ubel_llvm_wasm), CARGO_TARGET_DIR.
set -uo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CLANG="${CLANG:-clang}"
WASM_LD="${WASM_LD:-wasm-ld}"
out="${OUT_DIR:-${TMPDIR:-/tmp}/ubel_llvm_wasm}"
mkdir -p "$out"

for tool in "$CLANG" "$WASM_LD" node cargo; do
  command -v "$tool" >/dev/null 2>&1 || { echo "error: '$tool' not found; this demo needs clang (wasm32), wasm-ld, node and cargo" >&2; exit 1; }
done
if ! "$CLANG" --print-targets 2>/dev/null | grep -q wasm32; then
  echo "error: this clang has no wasm32 target" >&2; exit 1
fi

export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$out/target}"
echo "building the emitter..." >&2
cargo build --release --manifest-path "$here/Cargo.toml" >"$out/cargo.log" 2>&1 || { tail -n 30 "$out/cargo.log" >&2; exit 1; }
bin="$CARGO_TARGET_DIR/release/ubel-llvm"

"$CLANG" --target=wasm32 -O2 -ffreestanding -nostdlib -c "$here/wasm/wasm_rt.c" -o "$out/rt.o" || exit 1

pass=0; fail=0; skip=0
for src in "$here"/examples/[0-9]*.ubl "$here"/examples/fail_*.ubl; do
  name="$(basename "$src" .ubl)"
  "$bin" interp "$src" >"$out/$name.expected" 2>/dev/null
  "$bin" emit "$src" -o "$out/$name.ll" 2>"$out/$name.err" || { echo "  FAIL  $name: emit failed"; fail=$((fail + 1)); continue; }
  if grep -qE '@ubel_gc_alloc|@ubel_arena_grow' "$out/$name.ll"; then
    echo "  SKIP  $name: allocates from the GC heap or an arena, which the wasm runtime does not implement"; skip=$((skip + 1)); continue
  fi
  "$CLANG" --target=wasm32 -O2 -Wno-override-module -c "$out/$name.ll" -o "$out/$name.o" 2>"$out/$name.cerr" \
    || { echo "  FAIL  $name: IR to wasm32 failed: $(head -n 2 "$out/$name.cerr" | tr '\n' ' ')"; fail=$((fail + 1)); continue; }
  "$WASM_LD" --no-entry --export=ubel_main --export=ubel_arena_live_count --allow-undefined \
    "$out/rt.o" "$out/$name.o" -o "$out/$name.wasm" 2>"$out/$name.lerr" \
    || { echo "  FAIL  $name: wasm-ld failed: $(head -n 2 "$out/$name.lerr" | tr '\n' ' ')"; fail=$((fail + 1)); continue; }
  node "$here/wasm/run.js" "$out/$name.wasm" >"$out/$name.actual" 2>"$out/$name.nerr"; st=$?
  size="$(wc -c <"$out/$name.wasm" | tr -d ' ')"
  case "$name" in
    fail_*) if [ "$st" -eq 1 ] && cmp -s "$out/$name.expected" "$out/$name.actual"; then
              echo "  PASS  $name: traps like the interpreter, same output before it ($size bytes)"; pass=$((pass + 1))
            else echo "  FAIL  $name: exit $st"; fail=$((fail + 1)); fi ;;
    *)      if [ "$st" -eq 0 ] && cmp -s "$out/$name.expected" "$out/$name.actual"; then
              echo "  PASS  $name: identical to the interpreter, no arena left alive ($size bytes)"; pass=$((pass + 1))
            else echo "  FAIL  $name: exit $st, $(head -n 1 "$out/$name.nerr")"; fail=$((fail + 1)); fi ;;
  esac
done
echo
echo "wasm demo: $pass passed, $fail failed, $skip skipped"
[ "$fail" -eq 0 ]
