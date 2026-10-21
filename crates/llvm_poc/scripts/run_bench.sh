#!/usr/bin/env bash
# crates/llvm_poc/scripts/run_bench.sh
#
# Builds the Stage 0 proof of concept with clang and runs it.
#
#   bash crates/llvm_poc/scripts/run_bench.sh [--iters N] [--scale X] [--only SUBSTR]
#   bash crates/llvm_poc/scripts/run_bench.sh --selftest
#
# The report covers Stage 0 (the three tiers in isolation) and Stage 1A (how the tiers talk
# to each other: see docs/LLVM_POC.md).
#
# --selftest runs a small GC stress test (collect every 16 KiB) and fails unless
# every tier agrees and the collector really ran and kept live data alive. Run it
# once on every new target before trusting any timing from that target.
#
# Environment:
#   CLANG        compiler to use (default: clang). Needs LLVM 15+ for opaque pointers.
#   EXTRA_FLAGS  extra flags for every clang call, e.g. EXTRA_FLAGS=-m32
#   OUT_DIR      where build output goes (default: ${TMPDIR:-/tmp}/ubel_llvm_poc)
#
# Everything else after the script name is passed straight to the benchmark binary.
# Progress goes to stderr; the markdown report goes to stdout.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CLANG="${CLANG:-clang}"
out="${OUT_DIR:-${TMPDIR:-/tmp}/ubel_llvm_poc}"
read -r -a extra <<< "${EXTRA_FLAGS:-}"

if ! command -v "$CLANG" >/dev/null 2>&1; then
  echo "error: compiler '$CLANG' not found on PATH (set CLANG=clang-18, for example)" >&2
  exit 1
fi

mkdir -p "$out"
echo "compiler: $("$CLANG" --version | head -n 1)" >&2
echo "target:   $("$CLANG" "${extra[@]}" -dumpmachine)" >&2

# The IR needs to know whether the target's size_t is 32 or 64 bits (malloc's parameter type).
sizeof_size_t="$("$CLANG" "${extra[@]}" -dM -E -x c /dev/null | awk '$2 == "__SIZEOF_SIZE_T__" { print $3 }')"
case "$sizeof_size_t" in
  8) size_t_ir=i64 ;;
  4) size_t_ir=i32 ;;
  *) echo "error: cannot determine size_t width for this target (got '$sizeof_size_t')" >&2; exit 1 ;;
esac
echo "size_t:   ${size_t_ir} (${sizeof_size_t} bytes)" >&2
for m in tiers boundaries; do
  sed "s/^%size_t = type i64\$/%size_t = type ${size_t_ir}/" "$here/bench/$m.ll" > "$out/$m.gen.ll"
done

cflags=(-O2 -std=gnu11 -Wall -Wextra -I "$here/runtime" "${extra[@]}")

"$CLANG" "${cflags[@]}" -c "$here/runtime/ubel_rt.c"        -o "$out/ubel_rt.o"
"$CLANG" -O2 "${extra[@]}" -Wno-override-module -c "$out/tiers.gen.ll"      -o "$out/tiers.o"
"$CLANG" -O2 "${extra[@]}" -Wno-override-module -c "$out/boundaries.gen.ll" -o "$out/boundaries.o"
"$CLANG" "${cflags[@]}" -c "$here/harness/bench_main.c"     -o "$out/bench_main.o"
"$CLANG" "${extra[@]}" "$out/ubel_rt.o" "$out/tiers.o" "$out/boundaries.o" "$out/bench_main.o" -o "$out/bench"

# Keep the optimized IR so you can see what LLVM did with each tier. tiers.opt.ll holds
# both modules one after the other, for reading; it is not meant to be reassembled.
for m in tiers boundaries; do
  "$CLANG" -O2 "${extra[@]}" -Wno-override-module -S -emit-llvm "$out/$m.gen.ll" -o "$out/$m.mod.opt.ll"
done
{
  echo "; ===== bench/tiers.ll, optimized ====="
  cat "$out/tiers.mod.opt.ll"
  echo
  echo "; ===== bench/boundaries.ll, optimized ====="
  cat "$out/boundaries.mod.opt.ll"
} > "$out/tiers.opt.ll"

echo "built: $out/bench (optimized IR: $out/tiers.opt.ll)" >&2

if [ "${1:-}" = "--selftest" ]; then
  shift
  UBEL_GC_TRIGGER_KB=16 exec "$out/bench" --iters 1 --scale 0.05 --require-gc "$@"
fi
exec "$out/bench" "$@"
