#!/usr/bin/env python3
"""scripts/sync_grammar_page.py

Keeps web/site/src/reference/grammar.md in step with docs/ubel.ebnf, the
grammar's one source of truth. The page is the grammar text between two
markers plus a fixed introduction; everything outside the markers is
hand-written and left alone.

    python3 scripts/sync_grammar_page.py            # rewrite the page
    python3 scripts/sync_grammar_page.py --check    # exit 1 if it is out of date

scripts/check_doc_examples.py runs the same check, so the Docs Examples
workflow fails when the grammar changes and the page does not follow.
"""
import argparse
import os
import sys

BEGIN = "<!-- grammar:begin (generated from docs/ubel.ebnf by scripts/sync_grammar_page.py; do not edit by hand) -->"
END = "<!-- grammar:end -->"


def render_block(ebnf_path):
    with open(ebnf_path, encoding="utf-8") as fh:
        text = fh.read().rstrip("\n")
    if "```" in text:
        raise SystemExit("docs/ubel.ebnf contains a code fence; the page cannot embed it")
    return "%s\n\n```text\n%s\n```\n\n%s" % (BEGIN, text, END)


def sync(page_path, ebnf_path, check):
    with open(page_path, encoding="utf-8") as fh:
        page = fh.read()
    if BEGIN not in page or END not in page:
        raise SystemExit("%s has no grammar markers" % page_path)
    head, rest = page.split(BEGIN, 1)
    _old, tail = rest.split(END, 1)
    new = head + render_block(ebnf_path) + tail
    if new == page:
        return True
    if check:
        return False
    with open(page_path, "w", encoding="utf-8") as fh:
        fh.write(new)
    return True


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--page", default="web/site/src/reference/grammar.md")
    ap.add_argument("--ebnf", default="docs/ubel.ebnf")
    ap.add_argument("--check", action="store_true")
    args = ap.parse_args()
    if not os.path.exists(args.ebnf) or not os.path.exists(args.page):
        print("grammar page check skipped: %s or %s not found" % (args.page, args.ebnf))
        return 0
    ok = sync(args.page, args.ebnf, args.check)
    if args.check:
        print("grammar page: %s" % ("in sync with docs/ubel.ebnf" if ok else "OUT OF DATE, run scripts/sync_grammar_page.py"))
        return 0 if ok else 1
    print("grammar page written")
    return 0


if __name__ == "__main__":
    sys.exit(main())
