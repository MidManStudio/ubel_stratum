#!/usr/bin/env python3
"""
build_dashboard_report.py

Turns raw output from two sources into the dashboard's report files:

  1. Per-fixture diagnose output (crates/rd_parser/examples/diagnose.rs),
     one .txt file per .ubl fixture, section-delimited with "=== NAME ===".
  2. Criterion's own estimates.json files under target/criterion/**, one
     per benchmark.

Output layout (both under the directory that holds --out):

    report.json                 index: commit, stage counts, benchmarks,
                                and one summary per fixture. Small.
    fixtures/<name>.json        the heavy text of one fixture: source,
                                tokens, parse, sema, interpret. The
                                dashboard fetches one of these when a
                                fixture is opened.

Why the split: Cloudflare Pages rejects any single file over 25 MiB and
the whole deploy fails ("Pages only supports files up to 25 MiB in
size"). With every fixture's dumps embedded, report.json reached 43.3 MiB.
Per-fixture files stay a fraction of that, and the dashboard no longer
downloads every dump to show a list.

Two guards keep a future growth spurt from breaking the deploy step:

  * a section longer than SECTION_CAP_CHARS is cut at a line boundary and
    ends with a marker saying how much was omitted;
  * any written file larger than FILE_BUDGET_BYTES makes this script exit
    non-zero, so the failure appears in the build job instead of during
    the Cloudflare upload.

Deliberately separate from both the Rust tool that produces (1) and the
HTML page that renders the result: this script only transforms data, it
does not generate or display it.

Usage:
    python3 scripts/build_dashboard_report.py \\
        --results-dir results/ \\
        --criterion-dir target/criterion \\
        --out web/dashboard/report.json \\
        --commit-sha <sha> --run-url <url>
"""
import argparse
import json
import re
import sys
from pathlib import Path

SECTION_RE = re.compile(r"^=== (.+?) ===\s*$")
SUMMARY_LINE_RE = re.compile(r"^(\w+):\s+(.+)$")

# The heavy text sections of one fixture, in the order the dashboard shows
# them. These are the keys of a fixtures/<name>.json file.
DETAIL_SECTIONS = ("source", "tokens", "parse", "sema", "interpret")

# Name of the directory, next to the --out file, that receives one JSON
# file per fixture. The dashboard fetches "<this>/<name>.json" relative to
# report.json, so the two must stay in sync with web/dashboard/index.html.
DETAIL_DIRNAME = "fixtures"

# Cloudflare Pages limit for a single file.
PAGES_FILE_LIMIT_BYTES = 25 * 1024 * 1024

# Hard stop for any file this script writes. Kept under the Pages limit so
# the failure is reported here, with the file name, rather than by wrangler.
FILE_BUDGET_BYTES = 20 * 1024 * 1024

# A single section longer than this is truncated. A fixture's largest
# section is well under 1 MB today; five capped sections plus JSON escaping
# stay under FILE_BUDGET_BYTES even in the worst case.
SECTION_CAP_CHARS = 2_000_000


def parse_diagnose_output(text: str) -> dict:
    """Split one diagnose.rs report into its named sections."""
    sections: dict[str, list[str]] = {}
    current = "PREAMBLE"
    sections[current] = []
    for line in text.splitlines():
        m = SECTION_RE.match(line)
        if m:
            current = m.group(1).strip()
            # "FILE: <path>" is its own section header with content baked in
            if current.startswith("FILE:"):
                sections["FILE"] = [current[len("FILE:"):].strip()]
                current = "PREAMBLE"
            sections.setdefault(current, [])
            continue
        sections[current].append(line)

    summary = {}
    for line in sections.get("SUMMARY", []):
        m = SUMMARY_LINE_RE.match(line.strip())
        if m:
            summary[m.group(1)] = m.group(2).strip()

    return {
        "source": "\n".join(sections.get("SOURCE", [])).strip("\n"),
        "tokens": "\n".join(sections.get("TOKENS", [])).strip("\n"),
        "parse": "\n".join(sections.get("PARSE", [])).strip("\n"),
        "sema": "\n".join(sections.get("SEMA", [])).strip("\n"),
        "interpret": "\n".join(sections.get("INTERPRET", [])).strip("\n"),
        "summary": summary,
    }


def collect_fixture_reports(results_dir: Path) -> dict:
    fixtures = {}
    for txt_path in sorted(results_dir.glob("*.txt")):
        name = txt_path.stem
        try:
            fixtures[name] = parse_diagnose_output(txt_path.read_text())
        except Exception as e:  # noqa: BLE001 — report, don't crash the build
            fixtures[name] = {
                "source": "", "tokens": "", "parse": "", "sema": "", "interpret": "",
                "summary": {"error": f"failed to parse diagnose output: {e}"},
            }
    return fixtures


def collect_benchmarks(criterion_dir: Path) -> list[dict]:
    """
    Criterion writes target/criterion/<group>/<id>/new/estimates.json (or
    base/estimates.json on a run with no comparison baseline). Walk the
    whole tree rather than assuming one exact layout, since that's varied
    slightly across criterion versions.
    """
    benches = []
    if not criterion_dir.exists():
        return benches

    for estimates_path in criterion_dir.rglob("estimates.json"):
        # Only take the "new" (latest run) copy; skip "base" (comparison
        # baseline) and "change" (diff) if a "new" one exists alongside it.
        if estimates_path.parent.name not in ("new", "base"):
            continue
        if estimates_path.parent.name == "base":
            sibling = estimates_path.parent.parent / "new" / "estimates.json"
            if sibling.exists():
                continue  # prefer the "new" copy, skip this "base" one

        try:
            data = json.loads(estimates_path.read_text())
        except Exception:
            continue

        # Path relative to target/criterion, minus the trailing
        # new|base/estimates.json, minus the report/ dir if present.
        rel_parts = estimates_path.relative_to(criterion_dir).parts[:-2]
        if not rel_parts:
            continue
        bench_name = "/".join(rel_parts)

        mean = data.get("mean", {}).get("point_estimate")
        std_dev = data.get("std_dev", {}).get("point_estimate")
        if mean is None:
            continue

        benches.append({
            "name": bench_name,
            "mean_ns": mean,
            "std_dev_ns": std_dev,
        })

    benches.sort(key=lambda b: b["name"])
    return benches


def cap_section(text: str) -> str:
    """Cut one section at SECTION_CAP_CHARS, on a line boundary, with a marker."""
    if len(text) <= SECTION_CAP_CHARS:
        return text
    kept = text[:SECTION_CAP_CHARS].rsplit("\n", 1)[0]
    omitted = len(text) - len(kept)
    return (
        f"{kept}\n\n"
        f"[truncated: {omitted:,} more characters omitted to keep this file under "
        f"the Cloudflare Pages 25 MiB limit; the full text is in the "
        f"dashboard-results artifact of the workflow run]"
    )


def write_json(path: Path, obj: dict, indent=None) -> int:
    """Write obj as JSON and return the file size in bytes."""
    if indent is None:
        text = json.dumps(obj, ensure_ascii=False, separators=(",", ":"))
    else:
        text = json.dumps(obj, ensure_ascii=False, indent=indent)
    data = text.encode("utf-8")
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(data)
    return len(data)


def write_detail_files(fixtures: dict, detail_dir: Path) -> tuple[dict, list[tuple[int, str]]]:
    """
    Write fixtures/<name>.json for every fixture.

    Returns (index_entries, sizes): index_entries maps a fixture name to the
    small dict that goes into report.json, sizes is a list of
    (bytes, file name) for every file written.
    """
    detail_dir.mkdir(parents=True, exist_ok=True)
    # Drop JSON left over from an earlier local run so a removed fixture
    # does not keep a stale detail file. Only *.json is touched.
    for old in detail_dir.glob("*.json"):
        old.unlink()

    index_entries = {}
    sizes = []
    for name, parsed in fixtures.items():
        detail = {key: cap_section(parsed.get(key, "")) for key in DETAIL_SECTIONS}
        file_name = f"{name}.json"
        size = write_json(detail_dir / file_name, detail)
        sizes.append((size, f"{DETAIL_DIRNAME}/{file_name}"))
        index_entries[name] = {
            "summary": parsed["summary"],
            "detail": f"{DETAIL_DIRNAME}/{file_name}",
        }
    return index_entries, sizes


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--results-dir", required=True, type=Path)
    ap.add_argument("--criterion-dir", type=Path, default=None)
    ap.add_argument("--out", required=True, type=Path)
    ap.add_argument("--commit-sha", default="")
    ap.add_argument("--run-url", default="")
    ap.add_argument("--generated-at", default="")
    args = ap.parse_args()

    if not args.results_dir.exists():
        print(f"error: results dir not found: {args.results_dir}", file=sys.stderr)
        sys.exit(1)

    fixtures = collect_fixture_reports(args.results_dir)
    benchmarks = collect_benchmarks(args.criterion_dir) if args.criterion_dir else []

    stage_counts = {"lex": {"ok": 0, "fail": 0}, "parse": {"ok": 0, "fail": 0, "skipped": 0},
                    "sema": {"ok": 0, "fail": 0, "skipped": 0},
                    "interpret": {"ok": 0, "fail": 0, "skipped": 0}}
    for f in fixtures.values():
        for stage in ("lex", "parse", "sema", "interpret"):
            status = f["summary"].get(stage, "skipped").lower()
            bucket = "fail" if "fail" in status else ("skipped" if status == "skipped" else "ok")
            stage_counts[stage][bucket] = stage_counts[stage].get(bucket, 0) + 1

    detail_dir = args.out.parent / DETAIL_DIRNAME
    index_entries, sizes = write_detail_files(fixtures, detail_dir)

    report = {
        "commit_sha": args.commit_sha,
        "run_url": args.run_url,
        "generated_at": args.generated_at,
        "stage_counts": stage_counts,
        "fixtures": index_entries,
        "benchmarks": benchmarks,
    }
    report_size = write_json(args.out, report, indent=2)
    sizes.append((report_size, args.out.name))

    sizes.sort(reverse=True)
    detail_total = sum(size for size, _ in sizes) - report_size
    print(f"wrote {args.out} ({len(fixtures)} fixtures, {len(benchmarks)} benchmarks)")
    print(f"  {args.out.name}: {report_size / 1024:.1f} KiB")
    print(f"  {len(fixtures)} detail files in {detail_dir}: {detail_total / 1048576:.1f} MiB total")
    print("  largest files:")
    for size, file_name in sizes[:3]:
        print(f"    {file_name}: {size / 1024:.1f} KiB")

    too_big = [(size, file_name) for size, file_name in sizes if size > FILE_BUDGET_BYTES]
    if too_big:
        for size, file_name in too_big:
            print(
                f"error: {file_name} is {size / 1048576:.1f} MiB, over the "
                f"{FILE_BUDGET_BYTES // 1048576} MiB budget "
                f"(Cloudflare Pages rejects files over "
                f"{PAGES_FILE_LIMIT_BYTES // 1048576} MiB)",
                file=sys.stderr,
            )
        sys.exit(1)


if __name__ == "__main__":
    main()
