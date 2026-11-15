#!/usr/bin/env python3
"""scripts/build_docs_versions.py

Builds the mdBook docs site once per documentation version and lays the
results out for the deployed site:

    <out>/next/          built from the working tree (the in-development docs)
    <out>/vX.Y.Z/        built from the git tag vX.Y.Z, one per release
    <out>/latest/        the newest release, or the same as next while none exists
    <out>/versions.json  what exists; read by web/site/theme/ubel-nav.js
    <out>/index.html     redirects /docs/ to /docs/latest/

A release is a git tag named vMAJOR.MINOR.PATCH (a pre-release suffix such as
v0.2.0-alpha.1 is allowed). The docs of a release are whatever web/site looked
like at that tag, so cutting a version means tagging; nothing is copied or
frozen by hand. A tag that has no web/site/book.toml is skipped (it predates
the docs site).

Old tags carry their own copy of web/site/theme/. That folder is replaced with
the CURRENT one for every build, so every version gets the version picker, the
syntax highlighter and the reader-customisable colours, and a version is never
stranded on a stale theme. Only the chapters (src/) and book.toml come from
the tag.

A pre-release tag is built and listed but is never "latest".

Usage (from the repository root):
    python3 scripts/build_docs_versions.py --mdbook mdbook --out dist/docs

Needs a full clone (git fetch --tags, or actions/checkout with fetch-depth: 0).
"""
import argparse
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile

TAG_RE = re.compile(r"^v(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z.\-]+))?$")


def semver_key(tag):
    m = TAG_RE.match(tag)
    major, minor, patch, pre = int(m.group(1)), int(m.group(2)), int(m.group(3)), m.group(4)
    # A release sorts after its own pre-releases (1.0.0 > 1.0.0-rc.1).
    pre_key = (1,) if pre is None else (0, tuple(
        (0, int(p), "") if p.isdigit() else (1, 0, p) for p in pre.split(".")))
    return (major, minor, patch, pre_key)


def git(*args, cwd=None, check=True, text=True):
    return subprocess.run(["git"] + list(args), cwd=cwd, check=check,
                          capture_output=True, text=text)


def list_release_tags(repo):
    out = git("tag", "--list", "v*", cwd=repo).stdout.split()
    return sorted((t for t in out if TAG_RE.match(t)), key=semver_key, reverse=True)


def tag_has_docs(repo, tag, source):
    return git("cat-file", "-e", "%s:%s/book.toml" % (tag, source), cwd=repo, check=False).returncode == 0


def tag_date(repo, tag):
    # Committer date of the tagged commit, YYYY-MM-DD.
    return git("log", "-1", "--format=%cs", tag, cwd=repo).stdout.strip()


def run_mdbook(mdbook, book_dir, dest, site_url):
    env = dict(os.environ)
    # mdBook reads any config key from MDBOOK_* variables; this keeps the
    # generated 404 page and sitemap pointing at the right /docs/<version>/.
    env["MDBOOK_OUTPUT__HTML__SITE_URL"] = site_url
    if os.path.exists(dest):
        shutil.rmtree(dest)
    r = subprocess.run([mdbook, "build", book_dir, "--dest-dir", os.path.abspath(dest)],
                       env=env, capture_output=True, text=True)
    if r.returncode != 0:
        sys.stderr.write(r.stdout + r.stderr)
        raise SystemExit("mdbook build failed for %s" % book_dir)


def export_tag(repo, tag, source, into):
    """Extract <source>/ as of `tag` into `into`/<source>/."""
    archive = subprocess.run(["git", "archive", "--format=tar", tag, source],
                             cwd=repo, check=True, capture_output=True).stdout
    subprocess.run(["tar", "-x", "-C", into], input=archive, check=True)


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--mdbook", default="mdbook", help="path to the mdbook binary")
    ap.add_argument("--out", default="dist/docs", help="output directory (the deployed /docs/)")
    ap.add_argument("--source", default="web/site", help="book directory, relative to the repo root")
    ap.add_argument("--repo", default=".", help="repository root")
    args = ap.parse_args()

    repo = os.path.abspath(args.repo)
    out = os.path.abspath(args.out)
    os.makedirs(out, exist_ok=True)
    current_book = os.path.join(repo, args.source)
    if not os.path.exists(os.path.join(current_book, "book.toml")):
        raise SystemExit("no book.toml under %s" % current_book)

    # 1. next: the working tree.
    print("building next ...")
    run_mdbook(args.mdbook, current_book, os.path.join(out, "next"), "/docs/next/")

    # 2. one build per release tag.
    built = []
    for tag in list_release_tags(repo):
        if not tag_has_docs(repo, tag, args.source):
            print("skipping %s: no %s/book.toml at that tag" % (tag, args.source))
            continue
        print("building %s ..." % tag)
        with tempfile.TemporaryDirectory() as tmp:
            export_tag(repo, tag, args.source, tmp)
            book = os.path.join(tmp, args.source)
            theme = os.path.join(book, "theme")
            if os.path.exists(theme):
                shutil.rmtree(theme)
            shutil.copytree(os.path.join(current_book, "theme"), theme)
            run_mdbook(args.mdbook, book, os.path.join(out, tag), "/docs/%s/" % tag)
        built.append({
            "id": tag,
            "date": tag_date(repo, tag),
            "prerelease": TAG_RE.match(tag).group(4) is not None,
        })

    # 3. latest: newest non-pre-release, else a copy of next.
    stable = [v for v in built if not v["prerelease"]]
    latest = stable[0]["id"] if stable else None
    print("building latest (%s) ..." % (latest or "alias of next"))
    if latest:
        with tempfile.TemporaryDirectory() as tmp:
            export_tag(repo, latest, args.source, tmp)
            book = os.path.join(tmp, args.source)
            shutil.rmtree(os.path.join(book, "theme"), ignore_errors=True)
            shutil.copytree(os.path.join(current_book, "theme"), os.path.join(book, "theme"))
            run_mdbook(args.mdbook, book, os.path.join(out, "latest"), "/docs/latest/")
    else:
        run_mdbook(args.mdbook, current_book, os.path.join(out, "latest"), "/docs/latest/")

    # 4. versions.json and the /docs/ redirect.
    manifest = {"schema": 1, "latest": latest, "versions": built}
    with open(os.path.join(out, "versions.json"), "w", encoding="utf-8") as fh:
        json.dump(manifest, fh, indent=2)
        fh.write("\n")
    with open(os.path.join(out, "index.html"), "w", encoding="utf-8") as fh:
        fh.write(
            '<!DOCTYPE html>\n<html lang="en"><head><meta charset="utf-8">\n'
            '<title>Ubel Stratum documentation</title>\n'
            '<meta http-equiv="refresh" content="0; url=latest/">\n'
            '<link rel="canonical" href="latest/">\n</head>\n'
            '<body><p>Redirecting to the <a href="latest/">documentation</a>.</p></body></html>\n')

    print("docs versions: next%s%s" % (
        "".join(", " + v["id"] for v in built), " (latest = %s)" % latest if latest else " (no release yet)"))
    return 0


if __name__ == "__main__":
    sys.exit(main())
