#!/usr/bin/env bash
# Installs what the benchmark needs on an Ubuntu machine (a GitHub Actions ubuntu-latest runner, or any apt-based Linux).
# The hosted runner already has clang, g++, Rust, Go and .NET; this fills in whatever is missing and adds Zig (pinned,
# from PyPI) and Odin (the latest release from GitHub). Nothing is built here; run.sh skips a language whose tool is absent.
#
#   bash install_tools.sh            # then:  export PATH="$HOME/.langbench/bin:$PATH"   (done for you on GitHub Actions)
# Environment: ZIG_VERSION (default 0.13.0: the Zig programs use that version's standard library), ODIN_TAG (default: latest release).
set -eu
ZIG_VERSION="${ZIG_VERSION:-0.13.0}"
home="${HOME:-/tmp}/.langbench"
bin="$home/bin"
mkdir -p "$bin"
SUDO=""; [ "$(id -u)" != 0 ] && command -v sudo >/dev/null 2>&1 && SUDO="sudo"
have() { command -v "$1" >/dev/null 2>&1; }
apt_install() { # apt_install package...  (best effort: a failure is reported, not fatal)
  $SUDO apt-get install -y "$@" >/dev/null 2>&1 || echo "warning: could not install: $*" >&2
}

if have apt-get; then
  $SUDO apt-get update -y >/dev/null 2>&1 || true
  have clang   || apt_install clang
  have clang++ || apt_install clang
  have go      || apt_install golang-go
  have dotnet  || apt_install dotnet-sdk-8.0
  have python3 || apt_install python3
  python3 -c 'import venv' >/dev/null 2>&1 || apt_install python3-venv
fi
# Odin links with `clang`: make sure that name exists (some images only have clang-NN)
if ! have clang; then
  for v in 19 18 17 16 15 14; do
    if have "clang-$v"; then ln -sf "$(command -v "clang-$v")" "$bin/clang"; break; fi
  done
fi
if ! have clang++; then
  for v in 19 18 17 16 15 14; do
    if have "clang++-$v"; then ln -sf "$(command -v "clang++-$v")" "$bin/clang++"; break; fi
  done
fi

# Zig: the ziglang wheel on PyPI carries the compiler; a virtual environment keeps it off the system Python
if ! have zig; then
  if python3 -m venv "$home/venv" >/dev/null 2>&1 && "$home/venv/bin/pip" install --quiet "ziglang==$ZIG_VERSION" >/dev/null 2>&1; then
    zbin="$(ls "$home"/venv/lib/python*/site-packages/ziglang/zig 2>/dev/null | head -n 1)"
    [ -n "$zbin" ] && ln -sf "$zbin" "$bin/zig"
  else
    echo "warning: could not install Zig $ZIG_VERSION; the arena track will skip it" >&2
  fi
fi

# Odin: the release tarball for Linux x86-64 (or arm64), from the GitHub releases API
if ! have odin; then
  arch="$(uname -m)"; case "$arch" in x86_64|amd64) oa=amd64 ;; aarch64|arm64) oa=arm64 ;; *) oa="" ;; esac
  if [ -n "$oa" ] && have curl; then
    if [ -n "${ODIN_TAG:-}" ]; then api="https://api.github.com/repos/odin-lang/Odin/releases/tags/$ODIN_TAG"
    else api="https://api.github.com/repos/odin-lang/Odin/releases/latest"; fi
    url="$(curl -fsSL "$api" | python3 -c "
import json, sys
for a in json.load(sys.stdin).get('assets', []):
    if a['name'].startswith('odin-linux-$oa') and a['name'].endswith('.tar.gz'):
        print(a['browser_download_url']); break
" 2>/dev/null || true)"
    if [ -n "$url" ]; then
      rm -rf "$home/odin"; mkdir -p "$home/odin"
      if curl -fsSL "$url" | tar xz -C "$home/odin"; then
        obin="$(ls -d "$home"/odin/*/odin 2>/dev/null | head -n 1)"
        [ -n "$obin" ] && ln -sf "$obin" "$bin/odin"
      else echo "warning: could not unpack Odin" >&2; fi
    else echo "warning: no Odin release found for linux-$oa" >&2; fi
  else echo "warning: cannot install Odin here (needs curl and x86-64 or arm64 Linux)" >&2; fi
fi

# make the tools visible to the rest of the job
export PATH="$bin:$PATH"
if [ -n "${GITHUB_PATH:-}" ]; then echo "$bin" >> "$GITHUB_PATH"; fi
echo "toolchains:"
for t in clang clang++ rustc go dotnet zig odin; do
  if have "$t"; then
    case "$t" in dotnet) v="$(dotnet --version 2>/dev/null)" ;; *) v="$("$t" version 2>/dev/null | head -n 1)"; [ -z "$v" ] && v="$("$t" --version 2>/dev/null | head -n 1)" ;; esac
    echo "  $t: $v"
  else echo "  $t: NOT FOUND (its language will be skipped)"; fi
done
