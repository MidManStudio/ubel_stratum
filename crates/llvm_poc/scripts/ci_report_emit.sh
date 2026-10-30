#!/usr/bin/env bash
# crates/llvm_poc/scripts/ci_report_emit.sh OUT_DIR LOG [ARCH]
#
# Builds the job summary of the Stage 1B workflow (markdown on stdout) from what
# run_equiv.sh left in OUT_DIR: results.tsv, metrics.tsv, and the raw LOG. It is a script, not
# yml, so the report can grow without touching the workflow. Uses only POSIX tools and awk
# (no bash 4 features), so it also runs on a laptop.
set -u

out="${1:?usage: ci_report_emit.sh OUT_DIR LOG [ARCH]}"
log="${2:-}"
arch="${3:-$(uname -m)}"
here="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
res="$out/results.tsv"
met="$out/metrics.tsv"

echo "## Ubel LLVM PoC, Stage 1B: emitter against the interpreter ($arch)"
echo
bash "$here/scripts/ci_info.sh" ${CI_INPUTS:-} 2>/dev/null
echo

if [ ! -s "$res" ]; then
  echo "**No results were produced.** The run stopped before the first check; the log is below."
  echo
else
  # One line per example: the first sentence of its leading comment, used as the "what it covers" text.
  desc="$out/desc.tsv"
  : > "$desc"
  for f in "$here"/examples/*.ubl; do
    n="$(basename "$f" .ubl)"
    # The leading comment block, joined: its first sentence, or the first two when the first
    # is short, cut at a word boundary after 140 characters.
    d="$(awk '
      /^\/\// { sub(/^\/\/ ?/, ""); t = t $0 " "; next }
      { exit }
      END {
        n = split(t, p, /\. /)
        s = p[1]
        if (length(s) < 45 && n > 1) s = s ". " p[2]
        sub(/[ .]+$/, "", s)
        if (length(s) > 140) { s = substr(s, 1, 140); sub(/ [^ ]*$/, "", s); s = s "..." } else s = s "."
        print s
      }' "$f")"
    printf '%s\t%s\n' "$n" "$d" >> "$desc"
  done

  pass="$(awk -F'\t' '$1=="PASS"{n++} END{print n+0}' "$res")"
  fail="$(awk -F'\t' '$1=="FAIL"{n++} END{print n+0}' "$res")"
  cnt() { awk -F'\t' -v s="$1" -v k="$2" '$1==s && $3==k {n++} END{print n+0}' "$res"; }
  have_san="$(awk -F'\t' '$1=="META" && $2=="have_san"{print $4}' "$res")"
  flags="$(awk -F'\t' '$1=="META" && $2=="extra_flags"{print $4}' "$res")"

  if [ "$fail" -eq 0 ]; then verdict="All checks passed"; else verdict="**$fail check(s) FAILED**"; fi
  echo "### Result"
  echo
  echo "$verdict: $pass passed, $fail failed."
  echo
  echo "| Kind of check | Passed | Failed |"
  echo "|---|---:|---:|"
  echo "| Programs compiled and compared with the interpreter | $(cnt PASS runnable) | $(cnt FAIL runnable) |"
  echo "| Programs that must fail the same way | $(cnt PASS failing) | $(cnt FAIL failing) |"
  echo "| Programs the emitter must refuse | $(cnt PASS refusal) | $(cnt FAIL refusal) |"
  echo "| Target program (interpreter reference) | $(cnt PASS target) | $(cnt FAIL target) |"
  echo "| Collector runtime tests (C: large objects, size classes, arrays) | $(cnt PASS runtime) | $(cnt FAIL runtime) |"
  echo
  if [ "${have_san:-0}" = "1" ]; then san="ASan+UBSan on"; else san="sanitizers NOT available on this runner"; fi
  echo "Each compiled program is built at -O0 and -O2 and under a sanitizer build ($san), run with an arena-leak check, and its output must equal the interpreter's byte for byte.${flags:+ Extra flags: \`$flags\`.}"
  echo

  if [ "$fail" -gt 0 ]; then
    echo "### Failures"
    echo
    awk -F'\t' '$1=="FAIL" { printf "- **%s** (%s): %s\n", $2, $3, $4 }' "$res"
    echo
  fi

  echo "### What the emitter handles today"
  echo
  echo "Each row is proven by the listed examples; the status follows their results, so a regression turns a row red."
  echo
  echo "| Feature | Status | Proven by |"
  echo "|---|---|---|"
  awk -F'\t' '
    FILENAME == ARGV[1] { st[$2] = $1; next }
    {
      feat = $1; names = $2; want = $3; n = split(names, a, " "); ok = 1
      for (i = 1; i <= n; i++) if (st[a[i]] != "PASS") ok = 0
      if (want == "compiles") s = ok ? "compiles" : "**BROKEN**"
      else s = ok ? "refused (not supported yet)" : "**UNEXPECTED**"
      printf "| %s | %s | %s |\n", feat, s, names
    }' "$res" - <<'FEATURES'
int, bool and void functions, arithmetic, control flow	01_arith 02_loops 03_recursion 04_logic 05_scopes 06_if_expr	compiles
println and print with interpolation	07_interpolation	compiles
Tier annotations and cross-tier calls	08_tier_calls	compiles
with arena, destroyed on every exit path	09_arena_exits 12_struct_arena 13_edge_arena 17_enum_arena	compiles
Plain structs (HIGH on the GC heap, MID in an arena)	10_struct_basic 11_struct_gc 12_struct_arena	compiles
Edge structs and & references	13_edge_arena 14_edge_gc	compiles
Enums with tuple payloads and match	15_enum_basic 16_enum_gc 17_enum_arena	compiles
Division by zero traps like the interpreter	fail_div_zero	compiles
Strings as values	u_string_value	refused
Closures and function values	u_closure	refused
Collections	u_array	refused
Sized integers	u_sized_int	refused
Struct methods	u_struct	refused
Struct literals in LOW, or in MID outside an arena	u_struct_low u_struct_mid	refused
&mut and explicit dereference	u_edge_mut_ref u_edge_deref	refused
Enums: string payloads, struct-style variants, guards, nested patterns	u_enum_string u_enum_struct_variant u_enum_guard u_enum_nested_pattern	refused
Printing or comparing struct and enum values	u_struct_print u_enum_print u_enum_eq	refused
FEATURES
  echo

  echo "### Compiled programs"
  echo
  echo "| Program | What it covers | Result | IR lines | Functions | GC sites | Bump sites | Arena blocks | Interpreter | Native -O2 |"
  echo "|---|---|---|---:|---:|---:|---:|---:|---:|---:|"
  awk -F'\t' '
    FILENAME == ARGV[1] { d[$1] = $2; next }
    FILENAME == ARGV[2] { m[$1] = $0; next }
    $3 == "runnable" || $3 == "failing" {
      name = $2; txt = d[name]; gsub(/\|/, "\\|", txt)
      if (name in m) {
        split(m[name], f, "\t")
        printf "| `%s` | %s | %s | %s | %s | %s | %s | %s | %s ms | %s ms |\n", name, txt, $1, f[4], f[5], f[6], f[7], f[8], f[2], f[3]
      } else {
        printf "| `%s` | %s | %s | | | | | | | |\n", name, txt, $1
      }
    }' "$desc" "$met" "$res"
  echo
  echo "Times are one wall-clock run each on a shared runner, native time includes process start. They are for orientation only; the benchmark workflow measures properly. Sites are static call sites in the emitted IR."
  echo

  echo "### Collector activity (programs that allocate on the GC heap)"
  echo
  echo "| Program | Collections | Allocations | Allocated | Peak heap | Live at last GC | Longest pause | Mark | Sweep |"
  echo "|---|---:|---:|---:|---:|---:|---:|---:|---:|"
  awk -F'\t' '
    $10 > 0 {
      printf "| `%s` | %s | %s | %.1f KiB | %.0f KiB | %.1f KiB | %.1f us | %.1f us | %.1f us |\n", $1, $9, $10, $11 / 1024, $15 / 1024, $16 / 1024, $14 / 1000, $12 / 1000, $13 / 1000
    }' "$met"
  echo
  echo "Stop-the-world collector, so the longest pause is a whole collection. Mark and sweep are totals over all collections of that run."
  echo

  echo "### Refusals (what the emitter says no to, and why)"
  echo
  echo "| Program | Result | Refused because |"
  echo "|---|---|---|"
  awk -F'\t' '
    $3 == "refusal" {
      r = $4; i = index(r, "not supported yet: ")
      if (i > 0) r = substr(r, i + 19)
      gsub(/\|/, "\\|", r)
      printf "| `%s` | %s | %s |\n", $2, $1, r
    }' "$res"
  echo

  echo "### Target program: \`examples/arena_demo/ast_builder.ubl\`"
  echo
  echo "$(awk -F'\t' '$3=="target" && ($1=="PASS" || $1=="FAIL") { printf "Interpreter reference output: **%s**. ", $1 }' "$res")$(awk -F'\t' '$1=="NOTE" && $2=="target_emit" { if ($4 == "compiles") print "The emitter **compiles it now**: move it into the equivalence suite."; else print "The emitter does not compile it yet. First thing it refuses: " $4 }' "$res")"
  echo
  echo "The roadmap to this program is in \`docs/LLVM_ROADMAP.md\`."
  echo
fi

echo "<details><summary>Full log</summary>"
echo
echo '```'
if [ -n "$log" ] && [ -r "$log" ]; then cat "$log"; else echo "No log was produced."; fi
echo '```'
echo
echo "</details>"
