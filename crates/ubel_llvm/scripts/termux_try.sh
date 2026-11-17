#!/usr/bin/env bash
# crates/ubel_llvm/scripts/termux_try.sh
#
# For the phone. Run it in Termux (or any shell) next to the `ubel` binary you downloaded from the "Ubel for Android"
# workflow, and paste the whole output back:
#
#   chmod +x ubel
#   bash termux_try.sh            # or: bash termux_try.sh path/to/ubel
#
# It says what the device is, then tries `ubel run` on two small programs and, when clang is installed
# (`pkg install clang`), `ubel build` on them and runs what was built. It writes only to a temporary directory and
# removes it. Nothing here needs root. Portable bash (3.2 and newer). Provisional, like the rest.

set -u

ubel="${1:-./ubel}"
pass=0; fail=0; skip=0

say() { printf '%s\n' "$*"; }

tmp="$(mktemp -d "${TMPDIR:-/tmp}/ubel_try.XXXXXX")" || { say "cannot make a temporary directory in ${TMPDIR:-/tmp}"; exit 2; }
trap 'rm -rf "$tmp"' EXIT

say "== this device"
say "uname -m     : $(uname -m)   (aarch64 = 64-bit userspace; armv8l or armv7l = 32-bit userspace)"
if command -v getprop >/dev/null 2>&1; then
  say "android      : $(getprop ro.build.version.release) (API $(getprop ro.build.version.sdk))"
  say "model        : $(getprop ro.product.manufacturer) $(getprop ro.product.model)"
  say "cpu abilist  : $(getprop ro.product.cpu.abilist)"
fi
if command -v clang >/dev/null 2>&1; then
  say "clang        : $(clang --version | head -n 1)"
  have_clang=1
else
  say "clang        : not installed (run: pkg install clang), so only 'ubel run' is tried"
  have_clang=0
fi
say "free memory  : $(grep -E '^(MemAvailable|MemTotal)' /proc/meminfo 2>/dev/null | tr -s ' ' | tr '\n' ' ')"
say "ubel         : $ubel"
if [ ! -f "$ubel" ]; then say "no such file: $ubel"; exit 2; fi
chmod +x "$ubel" 2>/dev/null
say "version      : $("$ubel" --version 2>&1 | head -n 1)"
say

cat > "$tmp/hello.ubl" <<'UBL'
fn main() void {
    println("hello from ubel", 6 * 7)
}
UBL
cat > "$tmp/float.ubl" <<'UBL'
fn main() void {
    println(0.1 + 0.2, sqrt(2.0), 1.0 / 3.0, 1e21, 16777217 as float)
}
UBL
want_hello="hello from ubel 42"
want_float="0.30000000000000004 1.4142135623730951 0.3333333333333333 1000000000000000000000 16777216"

check() { # name, wanted, command...
  local name="$1" want="$2"; shift 2
  local got
  got="$("$@" 2>"$tmp/err")"
  local code=$?
  if [ "$code" = "0" ] && [ "$got" = "$want" ]; then
    say "PASS  $name"
    pass=$((pass + 1))
  else
    say "FAIL  $name (exit $code)"
    say "      wanted: $want"
    say "      got   : $got"
    head -n 5 "$tmp/err" | sed 's/^/      stderr: /'
    fail=$((fail + 1))
  fi
}

say "== ubel run (the interpreter)"
check "run hello" "$want_hello" "$ubel" run "$tmp/hello.ubl"
check "run float" "$want_float" "$ubel" run "$tmp/float.ubl"

say
say "== ubel build (native programs through clang)"
if [ "$have_clang" = "1" ]; then
  for n in hello float; do
    if "$ubel" build "$tmp/$n.ubl" -o "$tmp/$n" >"$tmp/build.log" 2>&1; then
      want="$want_hello"; [ "$n" = "float" ] && want="$want_float"
      check "build $n, run the result" "$want" "$tmp/$n"
    else
      say "FAIL  build $n"
      head -n 12 "$tmp/build.log" | sed 's/^/      /'
      fail=$((fail + 1))
    fi
  done
else
  say "SKIP  needs clang"
  skip=$((skip + 1))
fi

say
say "RESULT: $pass passed, $fail failed, $skip skipped"
[ "$fail" = "0" ]
