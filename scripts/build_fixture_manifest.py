#!/usr/bin/env python3
"""Bundle .ubl fixtures for the playground's fixture loader.

Copies every fixture from --fixtures-dir into --out-dir and writes a
manifest.json listing their filenames. web/playground/index.html fetches
that manifest at runtime to populate the "Load a fixture..." picker, then
fetches individual files by name on selection. See
.github/workflows/pipeline-dashboard.yml for where this runs in the
deploy pipeline, and PARKED_IDEAS.md / the fixture-rule docs for the
ok_/err_ naming convention the picker groups by.
"""
import argparse
import json
import shutil
from pathlib import Path


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--fixtures-dir", required=True, help="Source directory of .ubl fixtures")
    parser.add_argument("--out-dir", required=True, help="Destination directory (created if missing)")
    args = parser.parse_args()

    fixtures_dir = Path(args.fixtures_dir)
    out_dir = Path(args.out_dir)
    out_dir.mkdir(parents=True, exist_ok=True)

    names = sorted(p.name for p in fixtures_dir.glob("*.ubl"))
    if not names:
        raise SystemExit(f"no .ubl fixtures found under {fixtures_dir}")

    for name in names:
        shutil.copy2(fixtures_dir / name, out_dir / name)

    manifest_path = out_dir / "manifest.json"
    manifest_path.write_text(json.dumps(names, indent=2) + "\n")

    print(f"{len(names)} fixtures bundled into {out_dir}")


if __name__ == "__main__":
    main()
