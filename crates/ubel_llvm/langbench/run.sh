#!/usr/bin/env bash
# Cross-language benchmark runner. Programs are written once per language, built the way each language is normally built
# for speed, checked against one expected output, then timed. Tracks (tracks.txt) group them by memory tier:
#   compute (numeric loops), gc (Ubel HIGH vs C# and Go), arena (Ubel MID vs Zig, Odin and Rust),
#   manual (Ubel LOW vs C, C++ and Rust).
#
#   bash run.sh [--track NAME|all] [--size small|bench] [--runs N] [--pin CPU] [--only PROGRAM] [--langs "c rust"]
#
#   --size small   tiny inputs: only checks that every language prints the expected output (fast)
#   --size bench   the timed inputs (default); a language's first run is a correctness run and is not timed
#   --pin CPU      run every timed process on that one CPU (taskset, Linux): single-core comparison, GC threads included
#   --langs        restrict to these language tokens (for example "ubel-high go")
# Environment: UBEL CLANG CLANGXX RUSTC DOTNET GO ZIG ODIN (default: the plain command names), OUT_DIR
#              (default ${TMPDIR:-/tmp}/ubel_langbench). A missing tool skips its language; nothing else stops.
# Portable: bash 3.2 (macOS), BSD awk, no GNU-only options. The timer is timeit.c (built here with CLANG).
# See LANGBENCH.md for the method. Numbers from a shared or busy machine are indicative only.

here="$(cd "$(dirname "$0")" && pwd)"
TRACK=all; SIZE=bench; RUNS=5; PIN=""; ONLY=""; LANGS=""
while [ $# -gt 0 ]; do
  case "$1" in
    --track) TRACK="$2"; shift 2 ;;
    --size)  SIZE="$2"; shift 2 ;;
    --runs)  RUNS="$2"; shift 2 ;;
    --pin)   PIN="$2"; shift 2 ;;
    --only)  ONLY="$2"; shift 2 ;;
    --langs) LANGS="$2"; shift 2 ;;
    -h|--help) sed -n '2,19p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "unknown option: $1 (try --help)" >&2; exit 2 ;;
  esac
done

UBEL="${UBEL:-ubel}"; CLANG="${CLANG:-clang}"; CLANGXX="${CLANGXX:-clang++}"; RUSTC="${RUSTC:-rustc}"
DOTNET="${DOTNET:-dotnet}"; GO="${GO:-go}"; ZIG="${ZIG:-zig}"; ODIN="${ODIN:-odin}"
out="${OUT_DIR:-${TMPDIR:-/tmp}/ubel_langbench}"
mkdir -p "$out"
status="$out/status.tsv"; tsv="$out/results.tsv"; summary="$out/summary.md"
: > "$status"; : > "$tsv"

have() { command -v "$1" >/dev/null 2>&1; }
have "$CLANG" || { echo "error: C compiler '$CLANG' not found (needed for the timer and for Ubel's native backend)" >&2; exit 1; }
"$CLANG" -O2 -o "$out/timeit" "$here/timeit.c" || { echo "error: could not build timeit.c" >&2; exit 1; }
pin_cmd=""
if [ -n "$PIN" ]; then
  if have taskset; then pin_cmd="taskset -c $PIN"; else echo "warning: --pin needs taskset (Linux); running unpinned" >&2; PIN=""; fi
fi

tool_of() { # the command a language needs
  case "$1" in
    ubel|ubel-high|ubel-mid|ubel-low) echo "$UBEL" ;; c) echo "$CLANG" ;; cpp) echo "$CLANGXX" ;; rust) echo "$RUSTC" ;;
    cs) echo "$DOTNET" ;; go) echo "$GO" ;; zig) echo "$ZIG" ;; odin) echo "$ODIN" ;; *) echo "" ;;
  esac
}
ext_of() {
  case "$1" in
    ubel|ubel-high|ubel-mid|ubel-low) echo ubl ;; c) echo c ;; cpp) echo cpp ;; rust) echo rs ;; cs) echo cs ;;
    go) echo go ;; zig) echo zig ;; odin) echo odin ;; *) echo "" ;;
  esac
}
version_of() {
  case "$1" in
    c) "$CLANG" --version | head -n 1 ;; cpp) "$CLANGXX" --version | head -n 1 ;; rust) "$RUSTC" --version ;;
    cs) echo ".NET $("$DOTNET" --version 2>/dev/null)" ;; go) "$GO" version ;; zig) echo "zig $("$ZIG" version)" ;;
    odin) "$ODIN" version 2>/dev/null | head -n 1 ;; ubel) echo "ubel (LLVM backend through $CLANG)" ;;
  esac
}

cpu="$( (grep -m1 'model name' /proc/cpuinfo 2>/dev/null | sed 's/.*: //') || true)"
[ -z "$cpu" ] && cpu="$(sysctl -n machdep.cpu.brand_string 2>/dev/null || echo unknown)"
ncpu="$( (nproc 2>/dev/null || sysctl -n hw.ncpu 2>/dev/null) || echo '?')"

timed_build() { # timed_build DIR LANG command...  -> 0 on success; build time in DIR/LANG.build_ms
  d="$1"; l="$2"; shift 2
  "$out/timeit" "$d/$l.build.stats" "$@" > "$d/$l.build.log" 2>&1
  read -r bw _ _ _ bcode < "$d/$l.build.stats"
  echo "$bw" > "$d/$l.build_ms"
  [ "$bcode" = 0 ]
}

# build_one TRACK PROGRAM LANG N: 0 built (CMD set), 2 no program, 3 tool missing, 1 build failed (NOTE set)
build_one() {
  track="$1"; prog="$2"; lang="$3"; n="$4"
  src="$here/programs/$prog"; w="$out/$track/$prog"; mkdir -p "$w"
  ext="$(ext_of "$lang")"
  case "$lang" in ubel-low) f="$src/$prog.low.$ext" ;; *) f="$src/$prog.$ext" ;; esac
  [ -f "$f" ] || { NOTE="no $lang program for this track yet"; return 2; }
  tool="$(tool_of "$lang")"
  have "$tool" || { NOTE="'$tool' is not installed here"; return 3; }
  sed "s/@N@/$n/" "$f" > "$w/$lang.$ext"
  bin="$w/$lang.bin"
  case "$lang" in
    ubel|ubel-high|ubel-mid|ubel-low) timed_build "$w" "$lang" "$UBEL" build "$w/$lang.ubl" -o "$bin" -O 2 --cc "$CLANG" || return 1 ;;
    c)    timed_build "$w" c "$CLANG" -O2 -ffp-contract=off -o "$bin" "$w/c.c" -lm || return 1 ;;
    cpp)  timed_build "$w" cpp "$CLANGXX" -O2 -std=c++17 -ffp-contract=off -o "$bin" "$w/cpp.cpp" || return 1 ;;
    rust) timed_build "$w" rust "$RUSTC" -C opt-level=3 -C codegen-units=1 -o "$bin" "$w/rust.rs" || return 1 ;;
    go)   GOCACHE="$out/gocache" timed_build "$w" go "$GO" build -o "$bin" "$w/go.go" || return 1 ;;
    zig)  timed_build "$w" zig "$ZIG" build-exe "$w/zig.zig" -OReleaseFast -femit-bin="$bin" --cache-dir "$out/zigcache" --global-cache-dir "$out/zigglobal" || return 1 ;;
    odin) timed_build "$w" odin "$ODIN" build "$w/odin.odin" -file -o:speed -out:"$bin" || return 1 ;;
    cs)   mkdir -p "$w/cs"; cp "$w/cs.cs" "$w/cs/Program.cs"; cp "$here/prog.csproj" "$w/cs/prog.csproj"
          timed_build "$w" cs "$DOTNET" build "$w/cs/prog.csproj" -c Release -o "$w/cs/out" -nologo -v q || return 1
          bin="$w/cs/out/prog.dll" ;;
  esac
  case "$lang" in cs) CMD=("$DOTNET" "$w/cs/out/prog.dll") ;; *) CMD=("$bin") ;; esac
  BIN="$bin"
  return 0
}

note_of_failure() { # the first useful line of a failed build
  l="$(grep -m1 'not supported yet' "$1" 2>/dev/null | sed 's/^.*not supported yet: //')"
  [ -z "$l" ] && l="$(grep -m1 -i -E 'error' "$1" 2>/dev/null)"
  [ -z "$l" ] && l="$(grep -m1 . "$1" 2>/dev/null)"
  echo "$l" | tr '\t|' '  ' | cut -c1-160
}

record() { printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$1" "$2" "$3" "$4" "$5" "$6" "$7" >> "$status"; }

selected_langs=" $LANGS "
want_lang() { [ -z "$LANGS" ] || case "$selected_langs" in *" $1 "*) return 0 ;; *) return 1 ;; esac; }

while IFS='|' read -r tname tprogs tlangs tdesc; do
  tname="$(echo "$tname" | tr -d ' ')"
  case "$tname" in ''|'#'*) continue ;; esac
  [ "$TRACK" != all ] && [ "$TRACK" != "$tname" ] && continue
  for prog in $tprogs; do
    [ -n "$ONLY" ] && [ "$ONLY" != "$prog" ] && continue
    n="$(sed -n "s/^$SIZE=//p" "$here/programs/$prog/sizes")"
    [ -z "$n" ] && { echo "$tname/$prog: no '$SIZE' size, skipped" >&2; continue; }
    w="$out/$tname/$prog"; mkdir -p "$w"; : > "$w/cmds.txt"
    echo "== $tname / $prog ($SIZE, N=$n)" >&2
    built=""
    for lang in $tlangs; do
      want_lang "$lang" || continue
      build_one "$tname" "$prog" "$lang" "$n"; rc=$?
      case $rc in
        0) "${CMD[@]}" > "$w/$lang.out" 2> "$w/$lang.err"
           echo "$lang ${CMD[*]}" >> "$w/cmds.txt"
           size_kb="-"; case "$lang" in cs) ;; *) size_kb="$(( ($(wc -c < "$BIN") + 1023) / 1024 ))" ;; esac
           bms="$(cat "$w/$lang.build_ms" 2>/dev/null || echo 0)"
           built="$built $lang"; echo "$lang $size_kb $bms" >> "$w/built.txt" ;;
        1) NOTE="$(note_of_failure "$w/$lang.build.log")"
           echo "   $lang: build failed: $NOTE" >&2; record "$tname" "$prog" "$lang" build_failed "$NOTE" 0 - ;;
        2) echo "   $lang: $NOTE" >&2; record "$tname" "$prog" "$lang" no_program "$NOTE" 0 - ;;
        3) echo "   $lang: $NOTE" >&2; record "$tname" "$prog" "$lang" skipped "$NOTE" 0 - ;;
      esac
    done
    # the reference output: the expected file for the small input, otherwise the first language that built
    ref=""
    if [ "$SIZE" = small ] && [ -f "$here/programs/$prog/expected_small.txt" ]; then ref="$here/programs/$prog/expected_small.txt"
    else for lang in $built; do ref="$w/$lang.out"; break; done; fi
    for lang in $built; do
      size_kb="$(grep "^$lang " "$w/built.txt" | tail -n 1 | cut -d' ' -f2)"
      bms="$(grep "^$lang " "$w/built.txt" | tail -n 1 | cut -d' ' -f3)"
      if ! cmp -s "$ref" "$w/$lang.out"; then
        echo "   $lang: WRONG OUTPUT (compare $w/$lang.out with $ref)" >&2
        record "$tname" "$prog" "$lang" wrong_output "output differs from the reference" "$bms" "$size_kb"; continue
      fi
      if [ "$SIZE" = small ]; then echo "   $lang: ok" >&2; record "$tname" "$prog" "$lang" ok "" "$bms" "$size_kb"; continue; fi
      cmdline="$(grep "^$lang " "$w/cmds.txt" | tail -n 1 | cut -d' ' -f2-)"
      r=1
      while [ $r -le "$RUNS" ]; do
        $pin_cmd "$out/timeit" "$w/stats.tmp" $cmdline > /dev/null 2>&1
        read -r wall user sys rss code < "$w/stats.tmp"
        printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$tname" "$prog" "$lang" "$r" "$wall" "$user" "$sys" "$rss" "$code" >> "$tsv"
        r=$((r + 1))
      done
      echo "   $lang: timed" >&2
      record "$tname" "$prog" "$lang" timed "" "$bms" "$size_kb"
    done
  done
done < "$here/tracks.txt"

# ---- summary --------------------------------------------------------------------------------------------------
med() { sort -n | awk '{a[NR]=$1} END { if (NR==0) {print "0"; exit} m=(NR%2)?a[(NR+1)/2]:(a[NR/2]+a[NR/2+1])/2; printf "%.1f", m }'; }
min() { sort -n | head -n 1; }
max() { sort -n | tail -n 1; }
{
  echo "# Cross-language benchmark ($SIZE input, $RUNS timed runs)"
  echo
  echo "- machine: $(uname -srm), $cpu, $ncpu CPUs$( [ -n "$PIN" ] && echo ", every timed process pinned to CPU $PIN" )"
  for l in c cpp rust cs go zig odin ubel; do
    t="$(tool_of $l)"; have "$t" && echo "- $l: $(version_of $l)"
  done
  echo "- flags: C/C++ -O2 -ffp-contract=off; Rust opt-level=3, codegen-units=1; C# Release (tiered PGO); Go default; Zig ReleaseFast; Odin -o:speed; Ubel -O 2"
  echo
} > "$summary"
if [ "$SIZE" = small ]; then
  { echo "Small inputs: only correctness was checked."; echo; echo "| track | program | language | result |"; echo "|---|---|---|---|"
    awk -F'\t' '{ r=$4; if ($5 != "") r = r ": " $5; printf "| %s | %s | %s | %s |\n", $1, $2, $3, r }' "$status"; } >> "$summary"
  cat "$summary"; exit 0
fi
ratios="$out/ratios.tsv"; : > "$ratios"
while IFS='|' read -r tname tprogs tlangs tdesc; do
  tname="$(echo "$tname" | tr -d ' ')"
  case "$tname" in ''|'#'*) continue ;; esac
  grep -q "^$tname	" "$status" || continue
  { echo "## Track $tname:$tdesc"; echo; } >> "$summary"
  for prog in $tprogs; do
    grep -q "^$tname	$prog	" "$status" || continue
    best_of_all="$(awk -F'\t' -v t="$tname" -v p="$prog" '$1==t && $2==p {print $5}' "$tsv" | min)"
    { echo "### $prog"; echo
      echo "| language | best wall ms | median wall ms | median CPU ms | peak MB | binary KB | build s | vs fastest (best) |"
      echo "|---|---:|---:|---:|---:|---:|---:|---:|"; } >> "$summary"
    for lang in $tlangs; do
      line="$(awk -F'\t' -v t="$tname" -v p="$prog" -v l="$lang" '$1==t && $2==p && $3==l' "$status" | tail -n 1)"
      [ -z "$line" ] && continue
      st="$(echo "$line" | cut -f4)"; note="$(echo "$line" | cut -f5)"; bms="$(echo "$line" | cut -f6)"; kb="$(echo "$line" | cut -f7)"
      if [ "$st" != timed ]; then
        echo "| $lang | n/a | | | | | | $st: $note |" >> "$summary"; continue
      fi
      wmin="$(awk -F'\t' -v t="$tname" -v p="$prog" -v l="$lang" '$1==t && $2==p && $3==l {print $5}' "$tsv" | min)"
      wmed="$(awk -F'\t' -v t="$tname" -v p="$prog" -v l="$lang" '$1==t && $2==p && $3==l {print $5}' "$tsv" | med)"
      cpu_ms="$(awk -F'\t' -v t="$tname" -v p="$prog" -v l="$lang" '$1==t && $2==p && $3==l {print $6 + $7}' "$tsv" | med)"
      rss="$(awk -F'\t' -v t="$tname" -v p="$prog" -v l="$lang" '$1==t && $2==p && $3==l {print $8}' "$tsv" | max)"
      ratio="$(awk -v a="$wmin" -v b="$best_of_all" 'BEGIN { printf "%.2f", a / b }')"
      printf '%s\t%s\t%s\t%s\n' "$tname" "$lang" "$prog" "$ratio" >> "$ratios"
      awk -v l="$lang" -v wn="$wmin" -v wm="$wmed" -v c="$cpu_ms" -v r="$rss" -v k="$kb" -v b="$bms" -v x="$ratio" \
        'BEGIN { printf "| %s | %s | %s | %s | %.1f | %s | %.1f | %sx |\n", l, wn, wm, c, r / 1024, k, b / 1000, x }' >> "$summary"
    done
    echo >> "$summary"
  done
  nprog="$(echo $tprogs | wc -w | tr -d ' ')"
  if [ "$nprog" -gt 1 ]; then
    { echo "Geometric mean of the ratio to the fastest, over the programs of this track:"; echo
      echo "| language | geometric mean | programs |"; echo "|---|---:|---:|"
      for lang in $tlangs; do
        awk -F'\t' -v t="$tname" -v l="$lang" '$1==t && $2==l {s += log($4); n++} END { if (n > 0) printf "| %s | %.2fx | %d |\n", l, exp(s / n), n }' "$ratios"
      done; echo; } >> "$summary"
  fi
done < "$here/tracks.txt"
cat >> "$summary" <<'EOT'
Best of N is the headline on a noisy machine (noise only ever adds time); the median shows how noisy the runs were. A row
that says no_program, build_failed or skipped is a language/tier combination that cannot run yet; the text after it says why.
EOT
cat "$summary"
echo "raw numbers: $tsv  status: $status" >&2
