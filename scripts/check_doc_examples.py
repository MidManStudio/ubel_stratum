#!/usr/bin/env python3
"""scripts/check_doc_examples.py

Verifies the code examples in the docs site against the real `ubel` binary,
so a documented example can never drift away from what the compiler does.

An example is a fenced ```ubel block in web/site/src/**/*.md that is
immediately preceded (blank lines allowed) by an HTML comment naming what
the compiler is expected to do with it. The comment is invisible in the
rendered page:

    <!-- ubel-test: ok -->                 `ubel check` finds no diagnostics
    <!-- ubel-test: run -->                checks clean, then `ubel run` exits 0
    <!-- ubel-test: error TYPE-101 -->     check fails with exactly this code
    <!-- ubel-test: error TYPE-101,TYPE-102 -->   exactly this set of codes
    <!-- ubel-test: skip -->               illustrative fragment, not compiled

A ```ubel block with no annotation is reported as "untested". Pass --strict
to make that a failure once a chapter is fully annotated.

Usage:
    cargo build -p ubel_stratum_cli
    python3 scripts/check_doc_examples.py                # default paths
    python3 scripts/check_doc_examples.py --ubel target/release/ubel --strict

Exit status: 0 all checked examples behaved as documented, 1 otherwise.
"""
import argparse
import json
import os
import re
import subprocess
import sys
import tempfile

ANNOTATION = re.compile(r"^\s*<!--\s*ubel-test:\s*(.*?)\s*-->\s*$")
FENCE_OPEN = re.compile(r"^```ubel\s*$")
FENCE_CLOSE = re.compile(r"^```\s*$")


def find_examples(path):
    """Yield (line_number, annotation_or_None, source) for each ```ubel block."""
    with open(path, encoding="utf-8") as fh:
        lines = fh.read().split("\n")
    pending = None
    i = 0
    while i < len(lines):
        line = lines[i]
        m = ANNOTATION.match(line)
        if m:
            pending = m.group(1)
        elif FENCE_OPEN.match(line):
            start = i + 1
            j = start
            while j < len(lines) and not FENCE_CLOSE.match(lines[j]):
                j += 1
            yield (start, pending, "\n".join(lines[start:j]) + "\n")
            pending = None
            i = j
        elif line.strip():
            pending = None
        i += 1


def run_ubel(ubel, args, source):
    with tempfile.TemporaryDirectory() as tmp:
        src = os.path.join(tmp, "example.ubl")
        with open(src, "w", encoding="utf-8") as fh:
            fh.write(source)
        return subprocess.run([ubel] + args + [src], capture_output=True, text=True, timeout=60)


def check_one(ubel, annotation, source):
    """Return None on success, or a failure message."""
    parts = annotation.split(None, 1)
    mode = parts[0]
    if mode == "ok":
        r = run_ubel(ubel, ["--json", "check"], source)
        j = json.loads(r.stdout)
        if not j["ok"]:
            return "expected a clean check, got: " + ", ".join(d["code"] for d in j["diagnostics"])
    elif mode == "run":
        r = run_ubel(ubel, ["--json", "check"], source)
        j = json.loads(r.stdout)
        if not j["ok"]:
            return "expected a clean check, got: " + ", ".join(d["code"] for d in j["diagnostics"])
        r = run_ubel(ubel, ["--quiet", "run"], source)
        if r.returncode != 0:
            return "`ubel run` exited %d: %s" % (r.returncode, (r.stderr or r.stdout).strip()[:300])
    elif mode == "error":
        want = sorted(c.strip() for c in parts[1].split(",")) if len(parts) > 1 else []
        r = run_ubel(ubel, ["--json", "check"], source)
        j = json.loads(r.stdout)
        got = sorted(set(d["code"] for d in j["diagnostics"]))
        if j["ok"]:
            return "expected %s, but the example checks clean" % ", ".join(want)
        if got != want:
            return "expected exactly %s, got %s" % (", ".join(want), ", ".join(got))
    else:
        return "unknown ubel-test mode %r" % mode
    return None


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--ubel", default=os.environ.get("UBEL_BIN", "target/debug/ubel"))
    ap.add_argument("--root", default="web/site/src")
    ap.add_argument("--strict", action="store_true", help="fail on unannotated ```ubel blocks")
    args = ap.parse_args()

    if not os.path.exists(args.ubel):
        print("error: ubel binary not found at %s (build it: cargo build -p ubel_stratum_cli)" % args.ubel, file=sys.stderr)
        return 2

    passed = failed = skipped = untested = 0
    failures = []
    for dirpath, _dirs, files in os.walk(args.root):
        for name in sorted(files):
            if not name.endswith(".md"):
                continue
            path = os.path.join(dirpath, name)
            for line_no, annotation, source in find_examples(path):
                where = "%s:%d" % (path, line_no)
                if annotation is None:
                    untested += 1
                    if args.strict:
                        failures.append((where, "ubel block has no <!-- ubel-test: ... --> annotation"))
                    continue
                if annotation.split(None, 1)[0] == "skip":
                    skipped += 1
                    continue
                msg = check_one(args.ubel, annotation, source)
                if msg is None:
                    passed += 1
                else:
                    failed += 1
                    failures.append((where, msg))

    for where, msg in failures:
        print("FAIL %s: %s" % (where, msg))
    print("doc examples: %d passed, %d failed, %d skipped, %d untested" % (passed, failed, skipped, untested))
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
