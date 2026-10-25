#!/usr/bin/env bash
# crates/llvm_poc/scripts/ci_info.sh [key=value ...]
#
# Prints a markdown "Run info" table: which commit, which run, which machine, which tools.
# Used by the job summaries of both workflows; works on a laptop too (fields that only exist
# in CI say "local run"). Extra arguments become rows, e.g. `ci_info.sh clang=clang-18`.
set -u

row() { printf '| %s | %s |\n' "$1" "$2"; }

sha="${GITHUB_SHA:-$(git rev-parse HEAD 2>/dev/null || echo unknown)}"
msg="$(git log -1 --format=%s "$sha" 2>/dev/null | cut -c1-100)"
if [ -n "${GITHUB_RUN_ID:-}" ]; then
  run="[#${GITHUB_RUN_NUMBER:-?}](${GITHUB_SERVER_URL:-https://github.com}/${GITHUB_REPOSITORY:-}/actions/runs/${GITHUB_RUN_ID})"
  who="${GITHUB_ACTOR:-?} (${GITHUB_EVENT_NAME:-?})"
else
  run="local run"
  who="$(id -un 2>/dev/null || echo unknown)"
fi

os="${RUNNER_OS:-$(uname -s)}"
if [ -r /etc/os-release ]; then
  os="$(. /etc/os-release; echo "${PRETTY_NAME:-$os}")"
fi
image=""
[ -n "${ImageOS:-}" ] && image="${ImageOS}${ImageVersion:+ $ImageVersion}"

cpu="$(lscpu 2>/dev/null | awk -F': *' '/^Model name/ { print $2; exit }')"
[ -z "$cpu" ] && cpu="$(awk -F': *' '/^model name/ { print $2; exit }' /proc/cpuinfo 2>/dev/null)"
[ -z "$cpu" ] && cpu="$(sysctl -n machdep.cpu.brand_string 2>/dev/null)"
[ -z "$cpu" ] && cpu="unknown"
cores="$(nproc 2>/dev/null || sysctl -n hw.ncpu 2>/dev/null || echo '?')"
mem="$(awk '/^MemTotal/ { printf "%.1f GiB", $2 / 1048576; exit }' /proc/meminfo 2>/dev/null)"
[ -z "$mem" ] && mem="$(sysctl -n hw.memsize 2>/dev/null | awk '{ printf "%.1f GiB", $1 / 1073741824 }')"
[ -z "$mem" ] && mem="unknown"

clang_v="$("${CLANG:-clang}" --version 2>/dev/null | head -n 1)"
cargo_v="$(cargo --version 2>/dev/null)"
rustc_v="$(rustc --version 2>/dev/null)"

echo "| | |"
echo "|---|---|"
row "Commit" "\`$(printf '%s' "$sha" | cut -c1-7)\` ${msg:+$msg}"
[ -n "${GITHUB_REF_NAME:-}" ] && row "Branch" "\`${GITHUB_REF_NAME}\`"
row "Workflow run" "$run"
row "Started by" "$who"
row "Date" "$(date -u +'%Y-%m-%d %H:%M UTC')"
row "Machine" "$(uname -m), $cores cores, $mem"
row "CPU" "$cpu"
row "OS" "$os, kernel $(uname -r)${image:+, runner image $image}"
row "Compiler" "${clang_v:-not found}"
row "Rust" "${cargo_v:-cargo not found}${rustc_v:+, $rustc_v}"
for kv in "$@"; do
  row "Input \`${kv%%=*}\`" "\`${kv#*=}\`"
done
