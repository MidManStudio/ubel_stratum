#!/usr/bin/env bash
# Cross-language benchmark runner: the same programs in Ubel, C, Rust and C#, built with each language's usual
# optimizing settings, checked against the same expected output, then timed.
#
#   bash run.sh [--size small|bench] [--runs N] [--only PROGRAM] [--langs "ubel c rust cs"]
#
# --size small   a tiny input: checks every language prints the expected output (fast, no timing noise to read)
# --size bench   the timed input (default); each language's first run is a correctness run and is not timed
# Environment: UBEL (default ubel), CLANG (default clang), RUSTC (default rustc), DOTNET (default dotnet),
#              OUT_DIR (default ${TMPDIR:-/tmp}/ubel_langbench). A language whose tool is missing is skipped.
# Portable: bash 3.2 (macOS), BSD awk, no GNU-only options. The timer is timeit.c, built here with CLANG.
# See LANGBENCH.md for what is measured and how to read it. Numbers from a shared or busy machine are indicative only.

here="$(cd "$(dirname "$0")" && pwd)"
SIZE=bench
RUNS=5
ONLY=""
LANGS="ubel c rust cs"
while [ $# -gt 0 ]; do
  case "$1" in
    --size)  SIZE="$2"; shift 2 ;;
    --runs)  RUNS="$2"; shift 2 ;;
    --only)  ONLY="$2"; shift 2 ;;
    --langs) LANGS="$2"; shift 2 ;;
    -h|--help) sed -n '2,13p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "unknown option: $1 (try --help)" >&2; exit 2 ;;
  esac
done

UBEL="${UBEL:-ubel}"
CLANG="${CLANG:-clang}"
RUSTC="${RUSTC:-rustc}"
DOTNET="${DOTNET:-dotnet}"
out="${OUT_DIR:-${TMPDIR:-/tmp}/ubel_langbench}"
mkdir -p "$out"
tsv="$out/results.tsv"
summary="$out/summary.md"
: > "$tsv"

have() { command -v "$1" >/dev/null 2>&1; }
have "$CLANG" || { echo "error: C compiler '$CLANG' not found (needed for the timer and for Ubel's native backend)" >&2; exit 1; }
"$CLANG" -O2 -o "$out/timeit" "$here/timeit.c" || { echo "error: could not build timeit.c" >&2; exit 1; }

# which languages can run here
avail=""
for lang in $LANGS; do
  case "$lang" in
    ubel) have "$UBEL"  && avail="$avail ubel" || echo "skip ubel: '$UBEL' not found" >&2 ;;
    c)    avail="$avail c" ;;
    rust) have "$RUSTC" && avail="$avail rust" || echo "skip rust: '$RUSTC' not found" >&2 ;;
    cs)   have "$DOTNET" && avail="$avail cs" || echo "skip cs: '$DOTNET' not found" >&2 ;;
    *)    echo "unknown language '$lang' (ubel c rust cs)" >&2; exit 2 ;;
  esac
done

cpu="$( (grep -m1 'model name' /proc/cpuinfo 2>/dev/null | sed 's/.*: //') || true)"
[ -z "$cpu" ] && cpu="$(sysctl -n machdep.cpu.brand_string 2>/dev/null || echo unknown)"
{
  echo "# Cross-language benchmark ($SIZE input, $RUNS timed runs)"
  echo
  echo "- machine: $(uname -srm), $cpu"
  echo "- C: $("$CLANG" --version | head -n 1), -O2 -ffp-contract=off"
  have "$RUSTC"  && echo "- Rust: $("$RUSTC" --version), -C opt-level=3 -C codegen-units=1"
  have "$DOTNET" && echo "- C#: .NET $("$DOTNET" --version 2>/dev/null), Release, tiered PGO on (the default)"
  have "$UBEL"   && echo "- Ubel: $UBEL build -O 2 (LLVM backend through $CLANG)"
  echo
} > "$summary"

build() { # build LANG PROGRAM N  -> sets CMD
  lang="$1"; name="$2"; n="$3"; dir="$here/programs/$name"; w="$out/$name"
  mkdir -p "$w"
  case "$lang" in
    ubel) sed "s/@N@/$n/" "$dir/$name.ubl" > "$w/$name.ubl"
          "$UBEL" build "$w/$name.ubl" -o "$w/ubel.bin" -O 2 --cc "$CLANG" > "$w/ubel.build.log" 2>&1 || return 1
          CMD=("$w/ubel.bin") ;;
    c)    sed "s/@N@/$n/" "$dir/$name.c" > "$w/$name.c"
          "$CLANG" -O2 -ffp-contract=off -o "$w/c.bin" "$w/$name.c" -lm > "$w/c.build.log" 2>&1 || return 1
          CMD=("$w/c.bin") ;;
    rust) sed "s/@N@/$n/" "$dir/$name.rs" > "$w/$name.rs"
          "$RUSTC" -C opt-level=3 -C codegen-units=1 -o "$w/rust.bin" "$w/$name.rs" > "$w/rust.build.log" 2>&1 || return 1
          CMD=("$w/rust.bin") ;;
    cs)   mkdir -p "$w/cs"
          sed "s/@N@/$n/" "$dir/$name.cs" > "$w/cs/Program.cs"
          cp "$here/prog.csproj" "$w/cs/prog.csproj"
          "$DOTNET" build "$w/cs/prog.csproj" -c Release -o "$w/cs/out" -nologo -v q > "$w/cs.build.log" 2>&1 || return 1
          CMD=("$DOTNET" "$w/cs/out/prog.dll") ;;
  esac
}

for dir in "$here"/programs/*/; do
  name="$(basename "$dir")"
  [ -n "$ONLY" ] && [ "$ONLY" != "$name" ] && continue
  n="$(sed -n "s/^$SIZE=//p" "$dir/sizes")"
  [ -z "$n" ] && { echo "$name: no '$SIZE' size in sizes, skipped" >&2; continue; }
  w="$out/$name"; mkdir -p "$w"
  echo "== $name ($SIZE, N=$n)" >&2

  # build, and one correctness run per language (not timed)
  built=""
  for lang in $avail; do
    if ! build "$lang" "$name" "$n"; then echo "   $lang: BUILD FAILED (see $w/$lang.build.log)" >&2; continue; fi
    "${CMD[@]}" > "$w/$lang.out" 2> "$w/$lang.err"
    echo "$lang ${CMD[*]}" >> "$w/cmds.txt"
    built="$built $lang"
  done
  # the reference output: the expected file for the small input, otherwise C (or the first language that built)
  ref=""
  if [ "$SIZE" = small ] && [ -f "$dir/expected_small.txt" ]; then ref="$dir/expected_small.txt"
  elif [ -f "$w/c.out" ] && case "$built" in *" c"*) true ;; *) false ;; esac; then ref="$w/c.out"
  else for lang in $built; do ref="$w/$lang.out"; break; done; fi

  for lang in $built; do
    ok=1; cmp -s "$ref" "$w/$lang.out" || ok=0
    if [ $ok = 0 ]; then echo "   $lang: WRONG OUTPUT (compare $w/$lang.out with $ref)" >&2; fi
    if [ "$SIZE" = small ]; then
      [ $ok = 1 ] && echo "   $lang: ok" >&2
      continue
    fi
    [ $ok = 1 ] || continue
    # re-derive the command for this language (CMD is overwritten by later builds)
    cmdline="$(grep "^$lang " "$w/cmds.txt" | tail -n 1 | cut -d' ' -f2-)"
    r=1
    while [ $r -le "$RUNS" ]; do
      "$out/timeit" "$w/stats.tmp" $cmdline > /dev/null 2>&1
      read -r wall user sys rss code < "$w/stats.tmp"
      printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$name" "$lang" "$r" "$wall" "$user" "$sys" "$rss" "$code" >> "$tsv"
      r=$((r + 1))
    done
    echo "   $lang: timed" >&2
  done
done

[ "$SIZE" = small ] && { echo "small input done: only correctness was checked" >&2; exit 0; }

# summary: median and best wall time, median CPU time, peak memory, and the ratio to C (median wall)
med() { sort -n | awk '{a[NR]=$1} END { if (NR==0) {print "0"; exit} m=(NR%2)?a[(NR+1)/2]:(a[NR/2]+a[NR/2+1])/2; printf "%.1f", m }'; }
min() { sort -n | head -n 1; }
max() { sort -n | tail -n 1; }
ratios="$out/ratios.tsv"; : > "$ratios"
{
  for dir in "$here"/programs/*/; do
    name="$(basename "$dir")"
    grep -q "^$name	" "$tsv" || continue
    cmed="$(awk -F'\t' -v p="$name" '$1==p && $2=="c" {print $4}' "$tsv" | med)"
    cmin="$(awk -F'\t' -v p="$name" '$1==p && $2=="c" {print $4}' "$tsv" | min)"
    echo "## $name"
    echo
    echo "| language | best wall ms | median wall ms | median CPU ms | peak memory MB | vs C (best) | vs C (median) |"
    echo "|---|---:|---:|---:|---:|---:|---:|"
    for lang in ubel c rust cs; do
      awk -F'\t' -v p="$name" -v l="$lang" '$1==p && $2==l {f=1} END {exit !f}' "$tsv" || continue
      wmed="$(awk -F'\t' -v p="$name" -v l="$lang" '$1==p && $2==l {print $4}' "$tsv" | med)"
      wmin="$(awk -F'\t' -v p="$name" -v l="$lang" '$1==p && $2==l {print $4}' "$tsv" | min)"
      cpu="$(awk -F'\t' -v p="$name" -v l="$lang" '$1==p && $2==l {print $5 + $6}' "$tsv" | med)"
      rss="$(awk -F'\t' -v p="$name" -v l="$lang" '$1==p && $2==l {print $7}' "$tsv" | max)"
      ratio="n/a"; ratiob="n/a"
      if [ "$cmed" != "0" ]; then
        ratio="$(awk -v a="$wmed" -v b="$cmed" 'BEGIN { printf "%.2fx", a / b }')"
        ratiob="$(awk -v a="$wmin" -v b="$cmin" 'BEGIN { printf "%.2fx", a / b }')"
        awk -v a="$wmed" -v b="$cmed" -v c="$wmin" -v d="$cmin" -v l="$lang" 'BEGIN { printf "%s\t%.6f\t%.6f\n", l, c / d, a / b }' >> "$ratios"
      fi
      awk -v l="$lang" -v wm="$wmed" -v wn="$wmin" -v c="$cpu" -v r="$rss" -v x="$ratio" -v y="$ratiob" 'BEGIN { printf "| %s | %s | %s | %s | %.1f | %s | %s |\n", l, wn, wm, c, r / 1024, y, x }'
    done
    echo
  done
  echo "## Geometric mean of the ratio to C (over the programs above)"
  echo
  echo "| language | vs C (best) | vs C (median) | programs |"
  echo "|---|---:|---:|---:|"
  for lang in ubel c rust cs; do
    awk -F'\t' -v l="$lang" '$1==l {s += log($2); t += log($3); n++} END { if (n > 0) printf "| %s | %.2fx | %.2fx | %d |\n", l, exp(s / n), exp(t / n), n }' "$ratios"
  done
  echo
  echo "Best-of-N is the headline on a noisy machine (noise only ever adds time); the median shows how noisy the runs were."
  echo
} >> "$summary"
cat "$summary"
echo "raw numbers: $tsv" >&2
