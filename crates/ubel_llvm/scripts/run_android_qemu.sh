#!/usr/bin/env bash
# crates/ubel_llvm/scripts/run_android_qemu.sh
#
# Builds the supported examples with a CROSS compiler (in CI: the Android NDK's clang for one ARM target, linked
# statically) and runs each under QEMU user mode, comparing stdout and exit status with the interpreter on this
# machine. It checks that the generated code and the C runtime build and behave with the target's C library; for
# Android that is bionic. It is not the same as running on a phone: QEMU user mode runs the program on this machine's
# kernel, so what Android's own loader, SELinux or memory limits do is not exercised.
#
#   EMITTER=target/release/ubel-llvm CC=/path/to/aarch64-linux-android24-clang QEMU=qemu-aarch64 \
#     bash crates/ubel_llvm/scripts/run_android_qemu.sh
#
# Environment: EMITTER (the ubel-llvm binary for THIS machine; default target/release/ubel-llvm), CC (the cross
# compiler, required), QEMU (required, for example qemu-aarch64 or qemu-arm), CC_FLAGS (flags for clang after `--`;
# default -static), LABEL (a name for the report), OUT_DIR, TIMEOUT (seconds per program; default 120).
# When $GITHUB_STEP_SUMMARY is set a table is appended to it. Exit status: 0 all agree, 1 a difference or a failed
# build, 2 a tool is missing. Provisional.

set -uo pipefail

here="$(cd "$(dirname "$0")/.." && pwd)"
EMITTER="${EMITTER:-target/release/ubel-llvm}"
CC="${CC:-}"
QEMU="${QEMU:-}"
CC_FLAGS="${CC_FLAGS--static}"
LABEL="${LABEL:-cross}"
TIMEOUT="${TIMEOUT:-120}"
out="${OUT_DIR:-${TMPDIR:-/tmp}/ubel_cross}"

[ -n "$CC" ] && [ -n "$QEMU" ] || { echo "run_android_qemu: set CC (the cross compiler) and QEMU (for example qemu-aarch64)" >&2; exit 2; }
[ -x "$EMITTER" ] || { echo "run_android_qemu: '$EMITTER' is not an executable; build it with: cargo build --release -p ubel_llvm" >&2; exit 2; }
command -v "$QEMU" >/dev/null 2>&1 || { echo "run_android_qemu: '$QEMU' not found" >&2; exit 2; }
mkdir -p "$out"
read -r -a cc_flags <<< "$CC_FLAGS"

pass=0; fail=0
rows=""
progs=()
for f in "$here"/examples/[0-9][0-9]_*.ubl "$here"/examples/fail_*.ubl "$here/../../examples/arena_demo/ast_builder.ubl"; do
  [ -f "$f" ] && progs+=("$f")
done

for src in "${progs[@]}"; do
  name="$(basename "$src" .ubl)"
  exe="$out/$name"
  "$EMITTER" interp "$src" >"$exe.expected" 2>/dev/null
  want_status=$?
  if ! "$EMITTER" build "$src" -o "$exe" --cc "$CC" -O2 -- ${cc_flags[@]+"${cc_flags[@]}"} >"$exe.build.log" 2>&1; then
    echo "FAIL  $name: did not build: $(head -n 2 "$exe.build.log" | tr '\n' ' ')"
    rows="$rows| $name | build failed | $(head -n 1 "$exe.build.log" | tr '|' '/') |
"
    fail=$((fail + 1))
    continue
  fi
  if command -v timeout >/dev/null 2>&1; then
    timeout "$TIMEOUT" "$QEMU" "$exe" >"$exe.actual" 2>"$exe.stderr"
  else
    "$QEMU" "$exe" >"$exe.actual" 2>"$exe.stderr"
  fi
  got_status=$?
  if cmp -s "$exe.expected" "$exe.actual" && [ "$want_status" = "$got_status" ]; then
    echo "PASS  $name"
    pass=$((pass + 1))
  else
    why="output differs from the interpreter"
    [ "$want_status" != "$got_status" ] && why="exit status $got_status, the interpreter's is $want_status"
    echo "FAIL  $name: $why: $(diff "$exe.expected" "$exe.actual" | head -n 3 | tr '\n' ' ' | cut -c1-200)"
    rows="$rows| $name | $why | $(head -n 1 "$exe.stderr" | tr '|' '/' | cut -c1-120) |
"
    fail=$((fail + 1))
  fi
done

echo
echo "$LABEL: $pass passed, $fail failed (of $((pass + fail)) programs built with $(basename "$CC") ${CC_FLAGS} and run under $QEMU)"
if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then
  {
    echo "### $LABEL under $QEMU"
    echo
    echo "$pass passed, $fail failed, of $((pass + fail)) programs (cross compiler: \`$(basename "$CC") $CC_FLAGS\`)."
    if [ "$fail" != "0" ]; then
      echo
      echo "| program | what differs | first stderr line |"
      echo "|---|---|---|"
      printf '%s' "$rows"
    fi
  } >> "$GITHUB_STEP_SUMMARY"
fi
[ "$fail" = "0" ]
