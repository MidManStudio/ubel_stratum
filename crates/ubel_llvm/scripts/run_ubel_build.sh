#!/usr/bin/env bash
# crates/ubel_llvm/scripts/run_ubel_build.sh
#
# The `ubel build` command, end to end, on every program the repository has for it. It is what
# .github/workflows/ubel-build.yml runs, and it runs by hand as well:
#
#   cargo build --release -p ubel_stratum_cli
#   bash crates/ubel_llvm/scripts/run_ubel_build.sh [path/to/ubel]
#
# Everything goes through the real `ubel` command (not through `ubel-llvm`), in three groups:
#
#   1. The supported programs: crates/ubel_llvm/examples/NN_*.ubl, the fail_*.ubl ones, and the target program
#      examples/arena_demo/ast_builder.ubl. `ubel build` must succeed, and the program it makes must print exactly
#      what `ubel run` (the interpreter) prints. A fail_*.ubl program must fail in both, with the same output before.
#   2. The programs that are refused on purpose (crates/ubel_llvm/examples/u_*.ubl): `ubel build` must exit 1, print
#      the message of their `// expect-refusal:` line, and write no executable.
#   3. The fixture corpus (tests/fixtures), a robustness sweep, nothing is run: every err_* fixture must exit 1 and
#      write nothing; every ok_* fixture must either build (exit 0 and an executable exists) or be refused with
#      "not supported yet" (exit 1). Anything else (exit 2, a crash, a clang failure on our IR, a refusal without that
#      message) is a failure, because it would be a bug in the backend or the command.
#
# Environment (all optional):
#   CLANG        the compiler to use (passed as --cc); default: let `ubel build` search
#   OUT_DIR      where the executables and logs go (default: ${TMPDIR:-/tmp}/ubel_build_check)
#   OPT          the -O level, 0 1 2 3 or s (default 2)
#   EXTRA_FLAGS  extra clang flags after `--`, for example -m32
#
# Exit status 0 when everything passed, 1 otherwise. A markdown summary goes to stdout; the details are in
# $OUT_DIR/results.tsv (group, name, status, detail). Portable bash (3.2 and newer), no bash 4 features.

set -uo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/../../.." && pwd)"

ubel="${1:-${CARGO_TARGET_DIR:-$root/target}/release/ubel}"
if [ ! -x "$ubel" ]; then
  echo "ubel binary not found: $ubel (build it with: cargo build --release -p ubel_stratum_cli)" >&2
  exit 2
fi

out="${OUT_DIR:-${TMPDIR:-/tmp}/ubel_build_check}"
opt="${OPT:-2}"
rm -rf "$out"
mkdir -p "$out"
results="$out/results.tsv"
: >"$results"

cc=()
if [ -n "${CLANG:-}" ]; then cc=(--cc "$CLANG"); fi
read -r -a extra <<<"${EXTRA_FLAGS:-}"

run_timeout() {
  if command -v timeout >/dev/null 2>&1; then timeout 120 "$@"; else "$@"; fi
}

# ubel build FILE EXE: output goes to $EXE.build.out and $EXE.build.err, the exit status is returned.
ubel_build() {
  "$ubel" build "$1" -o "$2" -O "$opt" ${cc[@]+"${cc[@]}"} -- ${extra[@]+"${extra[@]}"} \
    >"$2.build.out" 2>"$2.build.err"
}

pass=0
fail=0
failed_list=""
record() { # group name status detail
  printf '%s\t%s\t%s\t%s\n' "$1" "$2" "$3" "$4" >>"$results"
  if [ "$3" = "PASS" ]; then
    pass=$((pass + 1))
  else
    fail=$((fail + 1))
    failed_list="$failed_list
- ${1}/${2}: ${4}"
    echo "  FAIL  $2  $4" >&2
  fi
}

# One line of a log, as printable ASCII only (the command prints some non-ASCII markers; cutting those in half would
# leave invalid text in the summary).
first_line() { head -n 1 "$1" 2>/dev/null | LC_ALL=C tr -c '[:print:]' '?' | cut -c1-160; }

started=$(date +%s)

# ---- 1. supported programs against the interpreter -------------------------------------------------------------
g1=0
for f in "$root"/crates/ubel_llvm/examples/[0-9]*.ubl "$root"/crates/ubel_llvm/examples/fail_*.ubl \
         "$root/examples/arena_demo/ast_builder.ubl"; do
  [ -f "$f" ] || continue
  g1=$((g1 + 1))
  name="$(basename "$f" .ubl)"
  exe="$out/$name"

  "$ubel" run "$f" >"$exe.expected" 2>"$exe.interp.err"
  istat=$?
  if ! ubel_build "$f" "$exe"; then
    record supported "$name" FAIL "ubel build failed: $(first_line "$exe.build.err")"
    continue
  fi
  UBEL_CHECK_LEAKS=1 run_timeout "$exe" >"$exe.actual" 2>"$exe.run.err"
  nstat=$?

  if ! cmp -s "$exe.expected" "$exe.actual"; then
    record supported "$name" FAIL "output differs from ubel run: $(diff "$exe.expected" "$exe.actual" | head -n 3 | LC_ALL=C tr -c '[:print:]' '?' | cut -c1-140)"
  elif [ "$istat" -eq 0 ] && [ "$nstat" -ne 0 ]; then
    record supported "$name" FAIL "the interpreter succeeded but the native program exited with status $nstat"
  elif [ "$istat" -ne 0 ] && [ "$nstat" -eq 0 ]; then
    record supported "$name" FAIL "the interpreter failed but the native program succeeded"
  else
    case "$name" in
      fail_*) record supported "$name" PASS "failed the same way in both (status $istat and $nstat), same output before it" ;;
      *)      record supported "$name" PASS "identical to ubel run" ;;
    esac
  fi
done

# ---- 2. refusals that are on purpose -----------------------------------------------------------------------------
g2=0
for f in "$root"/crates/ubel_llvm/examples/u_*.ubl; do
  [ -f "$f" ] || continue
  g2=$((g2 + 1))
  name="$(basename "$f" .ubl)"
  exe="$out/$name"
  want="$(sed -n 's#.*expect-refusal: ##p' "$f" | head -n 1)"
  if [ -z "$want" ]; then
    record refused "$name" FAIL "no '// expect-refusal:' line in the example"
    continue
  fi
  ubel_build "$f" "$exe"
  status=$?
  if [ "$status" -ne 1 ]; then
    record refused "$name" FAIL "expected exit status 1, got $status"
  elif [ -e "$exe" ]; then
    record refused "$name" FAIL "refused, but an executable was written"
  elif ! grep -qF -- "$want" "$exe.build.err" "$exe.build.out"; then
    record refused "$name" FAIL "the message '$want' is missing: $(first_line "$exe.build.err")"
  else
    record refused "$name" PASS "refused: $want"
  fi
done

# ---- 3. the fixture corpus --------------------------------------------------------------------------------------
g3_err=0
g3_built=0
g3_refused=0
for f in "$root"/tests/fixtures/err_*.ubl; do
  [ -f "$f" ] || continue
  g3_err=$((g3_err + 1))
  name="$(basename "$f" .ubl)"
  exe="$out/fx_$name"
  ubel_build "$f" "$exe"
  status=$?
  if [ "$status" -ne 1 ]; then
    record fixtures "$name" FAIL "an err_ fixture must exit 1, got $status"
  elif [ -e "$exe" ]; then
    record fixtures "$name" FAIL "an err_ fixture produced an executable"
  fi
  rm -f "$exe" "$exe.build.out" "$exe.build.err"
done
for f in "$root"/tests/fixtures/ok_*.ubl; do
  [ -f "$f" ] || continue
  name="$(basename "$f" .ubl)"
  exe="$out/fx_$name"
  ubel_build "$f" "$exe"
  status=$?
  if [ "$status" -eq 0 ] && [ -x "$exe" ]; then
    g3_built=$((g3_built + 1))
  elif [ "$status" -eq 1 ] && grep -qF "not supported yet" "$exe.build.err" "$exe.build.out"; then
    g3_refused=$((g3_refused + 1))
  else
    record fixtures "$name" FAIL "an ok_ fixture must build or be refused with 'not supported yet'; exit $status: $(first_line "$exe.build.err")"
  fi
  rm -f "$exe" "$exe.build.out" "$exe.build.err"
done
# the sweep adds one PASS line per group so the table is complete
record fixtures "sweep" PASS "$g3_err err_ fixtures rejected, $g3_built ok_ fixtures built, $g3_refused refused as not supported yet"

elapsed=$(($(date +%s) - started))

# ---- summary ----------------------------------------------------------------------------------------------------
{
  echo "## ubel build"
  echo
  echo "- ubel: \`$("$ubel" --version 2>/dev/null | head -n 1)\`"
  if [ -n "${CLANG:-}" ]; then echo "- compiler: \`$("$CLANG" --version 2>/dev/null | head -n 1)\`"; else echo "- compiler: found by \`ubel build\`"; fi
  echo "- options: \`-O $opt\` \`${EXTRA_FLAGS:-}\`"
  echo
  echo "| Group | Result |"
  echo "|---|---|"
  echo "| supported programs against \`ubel run\` | $g1 checked |"
  echo "| refusals that are on purpose | $g2 checked |"
  echo "| fixtures | $g3_err err_ rejected, $g3_built ok_ built, $g3_refused ok_ refused |"
  echo
  if [ "$fail" -eq 0 ]; then
    echo "**All checks passed** ($pass lines, ${elapsed}s)."
  else
    echo "**$fail failed**, $pass passed (${elapsed}s):"
    echo "$failed_list"
  fi
} | tee "$out/summary.md"

[ "$fail" -eq 0 ]
