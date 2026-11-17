#!/usr/bin/env bash
# crates/ubel_llvm/scripts/float_diff.sh
#
# Checks the way the native runtime prints `double` and `float` (runtime/ubel_float.h) against Rust's own `{}`, which
# is what the interpreter uses. scripts/float_diff_gen.rs (compiled with rustc) writes a corpus, one number per line
# with the text Rust prints for it; harness/float_diff_check.c (compiled with clang, against the runtime header)
# prints the same numbers and reports every difference. The corpus has every power of two and of ten and their
# neighbours (the asymmetric rounding interval at a power of two is the classic trap), integers, short decimals,
# NaN, infinities and both zeros, and random bit patterns: about 1.6 million values at scale 1.
#
#   bash crates/ubel_llvm/scripts/float_diff.sh [scale]
#
# `scale` (default 1, or $SCALE) multiplies the random part: scale 15 is about 17 million values (half a minute).
# Environment: CLANG (default clang), RUSTC (default rustc), EXTRA_FLAGS (for example -m32), SAN=1 (build the checker
# with ASan and UBSan). Exit status: 0 no difference, 1 a difference or a failed step, 2 a tool is missing or bad usage.
# Portable bash (3.2 and newer), no bash 4 features.

set -uo pipefail

here="$(cd "$(dirname "$0")/.." && pwd)"
CLANG="${CLANG:-clang}"
RUSTC="${RUSTC:-rustc}"
scale="${1:-${SCALE:-1}}"
case "$scale" in
  ''|*[!0-9]*) echo "usage: float_diff.sh [scale]" >&2; exit 2 ;;
esac
for tool in "$CLANG" "$RUSTC"; do
  command -v "$tool" >/dev/null 2>&1 || { echo "float_diff: '$tool' not found" >&2; exit 2; }
done
read -r -a extra <<< "${EXTRA_FLAGS:-}"
if [ "${SAN:-}" = "1" ]; then
  opt=(-O1 -g -fsanitize=address,undefined -fno-sanitize-recover=undefined)
else
  opt=(-O2)
fi

tmp="$(mktemp -d "${TMPDIR:-/tmp}/ubel_float_diff.XXXXXX")" || exit 2
trap 'rm -rf "$tmp"' EXIT

if ! "$RUSTC" --edition 2021 -O "$here/scripts/float_diff_gen.rs" -o "$tmp/gen" 2>"$tmp/rustc.log"; then
  tail -n 20 "$tmp/rustc.log" >&2
  echo "float_diff: the corpus generator did not compile" >&2
  exit 1
fi
if ! "$CLANG" "${opt[@]}" ${extra[@]+"${extra[@]}"} -std=c11 -Wall -Wextra -I "$here/runtime" \
     "$here/harness/float_diff_check.c" -o "$tmp/check" 2>"$tmp/clang.log"; then
  tail -n 20 "$tmp/clang.log" >&2
  echo "float_diff: the checker did not compile" >&2
  exit 1
fi

"$tmp/gen" "$scale" | "$tmp/check"
status=("${PIPESTATUS[@]}")
if [ "${status[0]}" != "0" ]; then
  echo "float_diff: the corpus generator failed" >&2
  exit 1
fi
exit "${status[1]}"
