#!/usr/bin/env bash
# crates/ubel_llvm/scripts/run_equiv.sh
#
# Stage 1B check: compile every program in examples/ to LLVM IR and make sure the native
# build behaves like the interpreter.
#
#   bash crates/ubel_llvm/scripts/run_equiv.sh
#
# Per example, by file name:
#   NN_name.ubl   interpreter output and native output must be identical, with the native
#                 build at -O0 and -O2 (and under ASan + UBSan when the toolchain has them),
#                 run with UBEL_CHECK_LEAKS=1 so an arena left alive by any exit path fails.
#   fail_name.ubl both must fail (non-zero exit) and print the same before failing.
#   u_name.ubl    the emitter must REFUSE it with a "not supported yet" error. This guards the
#                 subset boundary: unsupported code is never silently miscompiled.
#
# Besides the console output it writes, into OUT_DIR, two files the CI report is built from
# (scripts/ci_report_emit.sh): results.tsv (status, name, kind, detail per check, plus a few
# META rows) and metrics.tsv (per compiled program: IR size, allocation sites, timings, and the
# collector's statistics from a run with UBEL_STATS=1).
#
# Environment: CLANG (default clang, needs LLVM 15+), EXTRA_FLAGS (e.g. -m32),
#              OUT_DIR (default ${TMPDIR:-/tmp}/ubel_llvm_equiv), CARGO (default cargo),
#              GC_STRESS (e.g. 1: every compiled example runs with the collector forced before
#              every N-th allocation, UBEL_GC_STRESS; slow, so the per-run timeout becomes
#              STRESS_TIMEOUT, default 300 s; the runtime tests are not affected),
#              STRESS_TIMEOUT.
# Exit status is non-zero if any example fails.
set -uo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CLANG="${CLANG:-clang}"
CARGO="${CARGO:-cargo}"
out="${OUT_DIR:-${TMPDIR:-/tmp}/ubel_llvm_equiv}"
read -r -a extra <<< "${EXTRA_FLAGS:-}"
mkdir -p "$out"

command -v "$CLANG" >/dev/null 2>&1 || { echo "error: compiler '$CLANG' not found" >&2; exit 1; }
command -v "$CARGO" >/dev/null 2>&1 || { echo "error: '$CARGO' not found; the emitter is a Rust crate" >&2; exit 1; }

export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$out/target}"
# The emitter is a member of the main workspace, whose release profile is LTO with one codegen unit: slow
# and memory-hungry to build. These scripts do not need the optimised compiler, so they ask for a plain build.
# (Set the variables yourself to override; a normal `cargo build --release` is unaffected.)
export CARGO_PROFILE_RELEASE_LTO="${CARGO_PROFILE_RELEASE_LTO:-false}"
export CARGO_PROFILE_RELEASE_CODEGEN_UNITS="${CARGO_PROFILE_RELEASE_CODEGEN_UNITS:-16}"
echo "compiler: $("$CLANG" --version | head -n 1)" >&2
echo "cargo:    $("$CARGO" --version)" >&2

# unicode-tables-begin
# The tables behind `to_upper`, `to_lower` and `trim` come from the Rust standard library, and the interpreter
# these examples are checked against uses the one of the toolchain that builds it. When this toolchain's
# Unicode version differs from the one recorded in runtime/ubel_unicode.h, regenerate the header and
# examples/29_string_unicode.ubl with this toolchain first, so the check compares like with like (commit the
# regenerated files to keep them). See scripts/gen_unicode.rs.
if command -v rustc >/dev/null 2>&1; then
  gen="$out/gen_unicode"
  if rustc --edition 2021 -O "$here/scripts/gen_unicode.rs" -o "$gen" >"$out/gen_unicode.log" 2>&1; then
    have="$("$gen" --version)"
    want="$(sed -n 's/^#define UBEL_UNICODE_VERSION "\(.*\)"$/\1/p' "$here/runtime/ubel_unicode.h")"
    if [ "$have" != "$want" ]; then
      echo "unicode:  tables are Unicode ${want:-?}, this toolchain has $have: regenerating" >&2
      "$gen" "$here/runtime/ubel_unicode.h" "$here/examples/29_string_unicode.ubl" >"$out/gen_unicode.log" 2>&1 \
        || { tail -n 20 "$out/gen_unicode.log" >&2; exit 1; }
    else
      echo "unicode:  tables match this toolchain (Unicode $have)" >&2
    fi
  else
    echo "warning: scripts/gen_unicode.rs did not compile (see $out/gen_unicode.log); using the committed Unicode tables" >&2
  fi
fi
# unicode-tables-end

echo "building the emitter..." >&2
"$CARGO" build --release --manifest-path "$here/Cargo.toml" >"$out/cargo.log" 2>&1 || { tail -n 30 "$out/cargo.log" >&2; exit 1; }
bin="$CARGO_TARGET_DIR/release/ubel-llvm"

stress_to=30
if [ -n "${GC_STRESS:-}" ]; then
  stress_to="${STRESS_TIMEOUT:-300}"
  echo "gc stress: UBEL_GC_STRESS=${GC_STRESS} for every example (a collection before every ${GC_STRESS}th allocation; timeout ${stress_to}s)" >&2
fi
# A subshell, so UBEL_GC_STRESS reaches the example being run and nothing else (the runtime
# tests count collections themselves and must not get it).
run_timeout() {
  (
    if [ -n "${GC_STRESS:-}" ]; then export UBEL_GC_STRESS="$GC_STRESS"; fi
    if command -v timeout >/dev/null 2>&1; then exec timeout "$stress_to" "$@"; else exec "$@"; fi
  )
}

# Build one native binary: build_native <ll> <exe> <flags...>
build_native() {
  local ll="$1" exe="$2"; shift 2
  "$CLANG" "$@" "${extra[@]}" -Wno-override-module -I "$here/runtime" \
    "$ll" "$here/runtime/ubel_rt.c" "$here/runtime/ubel_main.c" -o "$exe" 2>"$exe.build.log"
}

pass=0; fail=0; skipped_san=0
: > "$out/results.tsv"
: > "$out/metrics.tsv"

# Milliseconds since the epoch. `date +%s%N` is GNU-only (macOS prints a literal N), so fall
# back to perl, then to whole seconds.
now_ms() {
  local t
  t="$(date +%s%N 2>/dev/null)"
  case "$t" in
    ""|*[!0-9]*)
      if command -v perl >/dev/null 2>&1; then
        perl -MTime::HiRes=time -e 'printf "%d\n", time() * 1000'
      else
        echo $(( $(date +%s) * 1000 ))
      fi ;;
    *) echo $(( t / 1000000 )) ;;
  esac
}

kind_of() {
  case "$1" in
    u_*) echo refusal ;;
    fail_*) echo failing ;;
    "ast_builder (target)") echo target ;;
    rt_*) echo runtime ;;
    *) echo runnable ;;
  esac
}
record_result() { # status name detail
  local d="${3//$'\t'/ }"
  printf '%s\t%s\t%s\t%s\n' "$1" "$2" "$(kind_of "$2")" "$d" >> "$out/results.tsv"
}
ok()  { printf '  PASS  %-22s %s\n' "$1" "$2"; pass=$((pass + 1)); record_result PASS "$1" "$2"; }
bad() { printf '  FAIL  %-22s %s\n' "$1" "$2"; fail=$((fail + 1)); record_result FAIL "$1" "$2"; }

# Per-program numbers for the report: static facts from the IR, one timed -O2 run, and the
# collector's statistics (UBEL_STATS=1 makes the runtime print them to stderr at exit).
record_metrics() { # name ll interp_ms
  local name="$1" ll="$2" interp_ms="$3" exe="$out/$1.O2"
  local lines funcs gc bump arenas t0 t1 native_ms line
  lines="$(wc -l < "$ll" | tr -d ' ')"
  funcs="$(grep '^define ' "$ll" | grep -vc ' internal ' || true)"
  gc="$(grep -c 'call ptr @ubel_gc_alloc' "$ll" || true)"
  bump="$(grep -c 'call ptr @ubel_arena_bump' "$ll" || true)"
  arenas="$(grep -c 'call ptr @ubel_arena_create' "$ll" || true)"
  t0="$(now_ms)"
  UBEL_STATS=1 run_timeout "$exe" >/dev/null 2>"$out/$name.stats"
  t1="$(now_ms)"
  native_ms=$(( t1 - t0 ))
  line="$(grep -m1 '^ubel_stats:' "$out/$name.stats" || true)"
  local c=0 n=0 b=0 m=0 sw=0 mp=0 ph=0 lv=0 kv
  for kv in $line; do
    case "$kv" in
      collections=*) c="${kv#*=}" ;;
      alloc_count=*) n="${kv#*=}" ;;
      alloc_bytes=*) b="${kv#*=}" ;;
      mark_ns=*) m="${kv#*=}" ;;
      sweep_ns=*) sw="${kv#*=}" ;;
      max_pause_ns=*) mp="${kv#*=}" ;;
      peak_heap_bytes=*) ph="${kv#*=}" ;;
      live_bytes_last=*) lv="${kv#*=}" ;;
    esac
  done
  printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
    "$name" "$interp_ms" "$native_ms" "$lines" "$funcs" "$gc" "$bump" "$arenas" \
    "$c" "$n" "$b" "$m" "$sw" "$mp" "$ph" "$lv" >> "$out/metrics.tsv"
}

# Is ASan + UBSan usable here?
san_flags=(-fsanitize=address,undefined -fno-sanitize-recover=undefined)
printf 'int main(void){return 0;}\n' > "$out/san_probe.c"
if "$CLANG" "${san_flags[@]}" "${extra[@]}" "$out/san_probe.c" -o "$out/san_probe" >/dev/null 2>&1 && "$out/san_probe" >/dev/null 2>&1; then
  have_san=1
else
  have_san=0
  echo "note: sanitizers unavailable with this toolchain, skipping the ASan/UBSan runs" >&2
fi
printf 'META\thave_san\t-\t%s\n' "$have_san" >> "$out/results.tsv"
printf 'META\textra_flags\t-\t%s\n' "${EXTRA_FLAGS:-}" >> "$out/results.tsv"

# The examples, then the target program (examples/arena_demo/ast_builder.ubl): since A8 the emitter
# compiles it, so it is held to the same standard as every example, at -O0, -O2 and under the sanitizers.
for src in "$here"/examples/*.ubl "$here/../../examples/arena_demo/ast_builder.ubl"; do
  [ -f "$src" ] || continue
  name="$(basename "$src" .ubl)"
  ll="$out/$name.ll"

  case "$name" in
    u_*)
      if "$bin" emit "$src" -o "$ll" 2>"$out/$name.err"; then
        bad "$name" "was compiled, but it uses something the emitter does not support"
      else
        want="$(sed -n 's|^// expect-refusal: *||p' "$src" | head -n 1)"
        if ! grep -q "not supported yet" "$out/$name.err"; then
          bad "$name" "failed for a reason other than 'not supported yet': $(head -n 2 "$out/$name.err" | tr '\n' ' ')"
        elif [ -n "$want" ] && ! grep -qF "$want" "$out/$name.err"; then
          bad "$name" "refused, but not for the expected feature (wanted '$want'): $(grep -m1 'not supported yet' "$out/$name.err" | sed 's/^ *//')"
        else
          ok "$name" "refused: $(grep -m1 'not supported yet' "$out/$name.err" | sed 's/^ *//')"
        fi
      fi
      continue
      ;;
  esac

  t_interp0="$(now_ms)"
  "$bin" interp "$src" >"$out/$name.expected" 2>"$out/$name.interp.err"; interp_status=$?
  interp_ms=$(( $(now_ms) - t_interp0 ))
  if ! "$bin" emit "$src" -o "$ll" 2>"$out/$name.err"; then
    bad "$name" "emit failed: $(head -n 3 "$out/$name.err" | tr '\n' ' ')"
    continue
  fi

  # `// expect-ir: <text>` and `// expect-no-ir: <text>` in an example say that the emitted IR must contain, or
  # must not contain, that fixed string. They pin down choices the output cannot show, such as where a string
  # is allocated (a null arena argument means the GC heap).
  ir_problem=""
  sed -n 's|^// expect-ir: *||p' "$src" > "$out/$name.expect-ir"
  while IFS= read -r want_ir; do
    [ -z "$want_ir" ] && continue
    grep -qF -- "$want_ir" "$ll" || ir_problem="the emitted IR lacks: $want_ir"
  done < "$out/$name.expect-ir"
  sed -n 's|^// expect-no-ir: *||p' "$src" > "$out/$name.expect-no-ir"
  while IFS= read -r nope; do
    [ -z "$nope" ] && continue
    if grep -qF -- "$nope" "$ll"; then ir_problem="the emitted IR contains: $nope"; fi
  done < "$out/$name.expect-no-ir"
  if [ -n "$ir_problem" ]; then
    bad "$name" "$ir_problem"
    continue
  fi

  case "$name" in
    fail_*)
      if [ "$interp_status" -eq 0 ]; then bad "$name" "the interpreter was expected to fail but succeeded"; continue; fi
      if ! build_native "$ll" "$out/$name.O2" -O2; then bad "$name" "native build failed: $(head -n 3 "$out/$name.O2.build.log" | tr '\n' ' ')"; continue; fi
      UBEL_CHECK_LEAKS=1 run_timeout "$out/$name.O2" >"$out/$name.actual" 2>"$out/$name.native.err"; native_status=$?
      if [ "$native_status" -eq 0 ]; then bad "$name" "the native build was expected to fail but succeeded"
      elif ! diff -q "$out/$name.expected" "$out/$name.actual" >/dev/null; then bad "$name" "output before the failure differs"
      else ok "$name" "both fail the same way (interpreter exit $interp_status, native exit $native_status), same output before it"
      fi
      continue
      ;;
  esac

  if [ "$interp_status" -ne 0 ]; then
    bad "$name" "the interpreter failed: $(head -n 2 "$out/$name.interp.err" | tr '\n' ' ')"
    continue
  fi

  variants=("O0:-O0" "O2:-O2")
  [ "$have_san" -eq 1 ] && variants+=("san:-O1 ${san_flags[*]}")
  all_ok=1; detail=""
  for v in "${variants[@]}"; do
    tag="${v%%:*}"; flags="${v#*:}"
    # shellcheck disable=SC2086
    if ! build_native "$ll" "$out/$name.$tag" $flags; then
      all_ok=0; detail="native build ($tag) failed: $(head -n 3 "$out/$name.$tag.build.log" | tr '\n' ' ')"; break
    fi
    UBEL_CHECK_LEAKS=1 run_timeout "$out/$name.$tag" >"$out/$name.$tag.actual" 2>"$out/$name.$tag.run.err"; st=$?
    if [ "$st" -ne 0 ]; then
      all_ok=0; detail="native run ($tag) exited $st: $(head -n 2 "$out/$name.$tag.run.err" | tr '\n' ' ')"; break
    fi
    if ! diff -q "$out/$name.expected" "$out/$name.$tag.actual" >/dev/null; then
      all_ok=0; detail="output differs from the interpreter ($tag): $(diff "$out/$name.expected" "$out/$name.$tag.actual" | head -n 4 | tr '\n' ' ')"; break
    fi
  done
  if [ "$all_ok" -eq 1 ]; then
    ok "$name" "identical to the interpreter at -O0, -O2$([ "$have_san" -eq 1 ] && echo ', ASan+UBSan'), no arena left alive"
    record_metrics "$name" "$ll" "$interp_ms"
  else
    bad "$name" "$detail"
  fi
done

# The target program (examples/arena_demo/ast_builder.ubl). The equivalence loop above checks that the
# native build prints what the interpreter prints. Here the interpreter's output is checked against
# the output that was verified by hand (ast_builder.expected), so the two together pin the native
# output to it, and the file cannot go stale. Whether the emitter still compiles it is also reported
# (a failure to compile already shows up as a FAIL in the loop above).
target="$here/../../examples/arena_demo/ast_builder.ubl"
golden="$here/../../examples/arena_demo/ast_builder.expected"
if [ -f "$target" ] && [ -f "$golden" ]; then
  if "$bin" interp "$target" >"$out/ast_builder.actual" 2>"$out/ast_builder.err" && diff -q "$golden" "$out/ast_builder.actual" >/dev/null; then
    ok "ast_builder (target)" "the interpreter runs it and prints the checked output"
  else
    bad "ast_builder (target)" "the interpreter no longer runs it or its output changed: $(head -n 2 "$out/ast_builder.err" | tr '\n' ' ')$(diff "$golden" "$out/ast_builder.actual" 2>/dev/null | head -n 3 | tr '\n' ' ')"
  fi
  if "$bin" emit "$target" -o "$out/ast_builder.ll" >/dev/null 2>"$out/ast_builder.emit.err"; then
    echo "  note  ast_builder: the emitter compiles it; it runs in the equivalence loop above."
    printf 'NOTE\ttarget_emit\ttarget\tcompiles\n' >> "$out/results.tsv"
  else
    echo "  note  ast_builder: not compilable yet:$(sed -n 2p "$out/ast_builder.emit.err")"
    printf 'NOTE\ttarget_emit\ttarget\t%s\n' "$(sed -n 2p "$out/ast_builder.emit.err" | sed 's/^ *//')" >> "$out/results.tsv"
  fi
fi

# ---- the build command -----------------------------------------------------------------------
# `ubel-llvm build` is the one-command route from source to an executable (src/driver.rs). The loop
# above links by hand; here two programs go through the command itself, with the compiler and the
# extra flags of this run (so -m32 reaches clang through `--`), and must print what the interpreter
# prints. The Rust tests of the driver (tests/build.rs) are not run by this script.
build_check() { # source-file
  local src="$1" name exe
  name="$(basename "$src" .ubl)"
  exe="$out/build_$name"
  "$bin" interp "$src" >"$exe.expected" 2>/dev/null
  if ! "$bin" build "$src" -o "$exe" -O2 --cc "$CLANG" -- "${extra[@]}" >"$exe.out" 2>"$exe.err"; then
    bad "build $name" "ubel-llvm build failed: $(head -n 3 "$exe.err" | tr '\n' ' ')"
    return
  fi
  UBEL_CHECK_LEAKS=1 run_timeout "$exe" >"$exe.actual" 2>"$exe.run.err"
  if diff -q "$exe.expected" "$exe.actual" >/dev/null; then
    ok "build $name" "built by ubel-llvm build, identical to the interpreter"
  else
    bad "build $name" "output differs from the interpreter: $(diff "$exe.expected" "$exe.actual" | head -n 4 | tr '\n' ' ')"
  fi
}
echo
echo "ubel-llvm build:"
build_check "$here/examples/31_enum_unit.ubl"
if [ -f "$target" ]; then build_check "$target"; fi

# ---- collector runtime tests (C) -------------------------------------------------------------
# harness/rt_gc_test.c covers large objects, the size classes above 512 bytes and the array
# allocator. Each of its tests runs in its own process. It is built at -O0 and -O2 and, when the
# sanitizers work here, under ASan + UBSan (there with -DRT_SOFT_LIVE and without ASan's
# stack-use-after-return fake stacks, which would hide live pointers from the conservative scan:
# see the comments in the file).
rt_src="$here/harness/rt_gc_test.c"
rt_timeout() { if command -v timeout >/dev/null 2>&1; then timeout 600 "$@"; else "$@"; fi; }
run_rt() { # label flags...
  local label="$1"; shift
  local exe="$out/rt_gc_test_${label//[^A-Za-z0-9]/_}"
  if ! "$CLANG" "$@" "${extra[@]}" -Wno-unused-parameter -I "$here/runtime" \
       "$here/runtime/ubel_rt.c" "$rt_src" -o "$exe" 2>"$exe.build.log"; then
    bad "rt_gc ($label)" "did not build: $(head -n 2 "$exe.build.log" | tr '\n' ' ')"
    return
  fi
  if ASAN_OPTIONS="detect_stack_use_after_return=0${ASAN_OPTIONS:+:$ASAN_OPTIONS}" \
       rt_timeout "$exe" all >"$exe.log" 2>&1; then
    ok "rt_gc ($label)" "$(tail -n 1 "$exe.log")"
  else
    bad "rt_gc ($label)" "$(grep -E 'FAIL|abort|rror' "$exe.log" | head -n 3 | tr '\n' ' ')"
  fi
}
if [ -f "$rt_src" ]; then
  echo
  echo "collector runtime tests:"
  run_rt "-O0" -O0
  run_rt "-O2" -O2
  if [ "$have_san" = "1" ]; then
    run_rt "ASan+UBSan" -O1 -g "${san_flags[@]}" -DRT_SOFT_LIVE
  else
    echo "  skip  rt_gc (ASan+UBSan)  sanitizers unavailable"
  fi
fi

echo
echo "equivalence: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
