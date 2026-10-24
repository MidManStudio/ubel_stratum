#!/usr/bin/env bash
# crates/llvm_poc/scripts/run_equiv.sh
#
# Stage 1B check: compile every program in examples/ to LLVM IR and make sure the native
# build behaves like the interpreter.
#
#   bash crates/llvm_poc/scripts/run_equiv.sh
#
# Per example, by file name:
#   NN_name.ubl   interpreter output and native output must be identical, with the native
#                 build at -O0 and -O2 (and under ASan + UBSan when the toolchain has them),
#                 run with UBEL_CHECK_LEAKS=1 so an arena left alive by any exit path fails.
#   fail_name.ubl both must fail (non-zero exit) and print the same before failing.
#   u_name.ubl    the emitter must REFUSE it with a "not supported yet" error. This guards the
#                 subset boundary: unsupported code is never silently miscompiled.
#
# Environment: CLANG (default clang, needs LLVM 15+), EXTRA_FLAGS (e.g. -m32),
#              OUT_DIR (default ${TMPDIR:-/tmp}/ubel_llvm_equiv), CARGO (default cargo).
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
echo "compiler: $("$CLANG" --version | head -n 1)" >&2
echo "cargo:    $("$CARGO" --version)" >&2
echo "building the emitter..." >&2
"$CARGO" build --release --manifest-path "$here/Cargo.toml" >"$out/cargo.log" 2>&1 || { tail -n 30 "$out/cargo.log" >&2; exit 1; }
bin="$CARGO_TARGET_DIR/release/ubel-llvm"

run_timeout() { if command -v timeout >/dev/null 2>&1; then timeout 30 "$@"; else "$@"; fi; }

# Build one native binary: build_native <ll> <exe> <flags...>
build_native() {
  local ll="$1" exe="$2"; shift 2
  "$CLANG" "$@" "${extra[@]}" -Wno-override-module -I "$here/runtime" \
    "$ll" "$here/runtime/ubel_rt.c" "$here/runtime/ubel_main.c" -o "$exe" 2>"$exe.build.log"
}

pass=0; fail=0; skipped_san=0
ok()  { printf '  PASS  %-22s %s\n' "$1" "$2"; pass=$((pass + 1)); }
bad() { printf '  FAIL  %-22s %s\n' "$1" "$2"; fail=$((fail + 1)); }

# Is ASan + UBSan usable here?
san_flags=(-fsanitize=address,undefined -fno-sanitize-recover=undefined)
printf 'int main(void){return 0;}\n' > "$out/san_probe.c"
if "$CLANG" "${san_flags[@]}" "${extra[@]}" "$out/san_probe.c" -o "$out/san_probe" >/dev/null 2>&1 && "$out/san_probe" >/dev/null 2>&1; then
  have_san=1
else
  have_san=0
  echo "note: sanitizers unavailable with this toolchain, skipping the ASan/UBSan runs" >&2
fi

for src in "$here"/examples/*.ubl; do
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

  "$bin" interp "$src" >"$out/$name.expected" 2>"$out/$name.interp.err"; interp_status=$?
  if ! "$bin" emit "$src" -o "$ll" 2>"$out/$name.err"; then
    bad "$name" "emit failed: $(head -n 3 "$out/$name.err" | tr '\n' ' ')"
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
  else
    bad "$name" "$detail"
  fi
done

# The target program (examples/arena_demo/ast_builder.ubl). The emitter cannot compile it yet,
# so it is not part of the equivalence loop above. What IS checked: the interpreter still
# accepts it and prints the output that was checked by hand (ast_builder.expected), so the file
# cannot go stale again. Whether the emitter compiles it is reported, never failed on: it is
# the thing the emitter is being grown toward.
target="$here/../../examples/arena_demo/ast_builder.ubl"
golden="$here/../../examples/arena_demo/ast_builder.expected"
if [ -f "$target" ] && [ -f "$golden" ]; then
  if "$bin" interp "$target" >"$out/ast_builder.actual" 2>"$out/ast_builder.err" && diff -q "$golden" "$out/ast_builder.actual" >/dev/null; then
    ok "ast_builder (target)" "the interpreter runs it and prints the checked output"
  else
    bad "ast_builder (target)" "the interpreter no longer runs it or its output changed: $(head -n 2 "$out/ast_builder.err" | tr '\n' ' ')$(diff "$golden" "$out/ast_builder.actual" 2>/dev/null | head -n 3 | tr '\n' ' ')"
  fi
  if "$bin" emit "$target" -o "$out/ast_builder.ll" >/dev/null 2>"$out/ast_builder.emit.err"; then
    echo "  note  ast_builder: the emitter COMPILES it now. Move it into the equivalence suite."
  else
    echo "  note  ast_builder: not compilable yet:$(sed -n 2p "$out/ast_builder.emit.err")"
  fi
fi

echo
echo "equivalence: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
