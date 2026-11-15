#!/usr/bin/env bash
# crates/ubel_llvm/scripts/regen_unicode.sh
#
# Refreshes the committed copies of the Unicode tables (runtime/ubel_unicode.h and the differential test
# examples/29_string_unicode.ubl) from the Rust toolchain on this machine.
#
#   bash crates/ubel_llvm/scripts/regen_unicode.sh           regenerate when the versions differ
#   bash crates/ubel_llvm/scripts/regen_unicode.sh --check   only report (writes nothing): exit 0 when they match, 3 when not
#
# Why: `to_upper`, `to_lower` and `trim` are implemented by the interpreter with Rust's own `str` methods, so
# they follow the Unicode version of the toolchain that builds `ubel`. The native runtime cannot call Rust, so it
# carries tables. The tables the DRIVER embeds (`ubel build`, `ubel-llvm build`) do not come from the committed
# header any more: crates/ubel_llvm/build.rs generates them on every build from the toolchain that compiles the
# crate, so a native build always agrees with `ubel run` and nothing has to be run for that.
#
# What this script refreshes is (1) the committed header, which the scripts that compile the C runtime directly
# still read (run_equiv.sh, run_bench.sh, the collector harness), and (2) example 29, whose strings are taken
# from the tables: it only covers the characters of the Unicode version it was generated with, so a newer Rust
# adds characters it does not test until it is regenerated. run_equiv.sh does the same by itself when its
# toolchain differs. To keep the result, commit the two files it rewrites; the CI workflow does not commit.
#
# Environment (optional): RUSTC, the compiler to use (default: rustc on PATH); TMPDIR.
# Exit status: 0 done or nothing to do, 1 the generator failed, 2 no usable rustc or bad option, 3 --check found a
# difference. Portable bash (3.2 and newer), no bash 4 features.

set -uo pipefail

here="$(cd "$(dirname "$0")/.." && pwd)"
header="$here/runtime/ubel_unicode.h"
example="$here/examples/29_string_unicode.ubl"

check=0
case "${1:-}" in
  "") ;;
  --check) check=1 ;;
  *)
    echo "usage: regen_unicode.sh [--check]" >&2
    exit 2
    ;;
esac

rustc="${RUSTC:-rustc}"
if ! command -v "$rustc" >/dev/null 2>&1; then
  echo "regen_unicode: no rustc found (put it on PATH or set RUSTC)" >&2
  exit 2
fi
if [ ! -f "$header" ] || [ ! -f "$here/scripts/gen_unicode.rs" ]; then
  echo "regen_unicode: not inside crates/ubel_llvm ($header or scripts/gen_unicode.rs is missing)" >&2
  exit 2
fi

tmp="$(mktemp -d "${TMPDIR:-/tmp}/ubel_unicode.XXXXXX")" || exit 2
trap 'rm -rf "$tmp"' EXIT

if ! "$rustc" --edition 2021 -O "$here/scripts/gen_unicode.rs" -o "$tmp/gen_unicode" >"$tmp/compile.log" 2>&1; then
  tail -n 20 "$tmp/compile.log" >&2
  echo "regen_unicode: scripts/gen_unicode.rs did not compile with $rustc" >&2
  exit 2
fi

have="$("$tmp/gen_unicode" --version)"
want="$(sed -n 's/^#define UBEL_UNICODE_VERSION "\(.*\)"$/\1/p' "$header")"

if [ "$have" = "$want" ]; then
  echo "unicode: tables are Unicode $have, the same as this toolchain: nothing to do"
  exit 0
fi

if [ "$check" = "1" ]; then
  echo "unicode: tables are Unicode ${want:-?}, this toolchain has $have"
  exit 3
fi

echo "unicode: tables are Unicode ${want:-?}, this toolchain has $have: regenerating"
if ! "$tmp/gen_unicode" "$header" "$example" >"$tmp/gen.log" 2>&1; then
  tail -n 20 "$tmp/gen.log" >&2
  echo "regen_unicode: the generator failed" >&2
  exit 1
fi
echo "unicode: rewrote ${header#"$here"/} and ${example#"$here"/} (Unicode $have)"
echo "unicode: commit both files to keep the new tables"
