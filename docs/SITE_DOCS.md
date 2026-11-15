# The website and its documentation

How `web/` is put together, how the versioned docs are built, how to write a
documentation page whose examples cannot go stale, and how the syntax colours
work. Written for people changing the site; readers of the site want
[the docs](https://github.com/MidManStudio/ubel_stratum/tree/master/web/site/src)
themselves, not this file.

## Layout

```
web/
  home/          the landing page (static HTML)
  site/          the docs: an mdBook (book.toml, src/, theme/)
  playground/    the in-browser compiler (static HTML + the wasm build)
  dashboard/     the CI results page
  shared/        files used by more than one of the above
    ubel-theme.css          design tokens + syntax-colour variables (--ubl-*)
    ubel-highlight.js       the Ubel highlighter (docs + CodeMirror mode)
    ubel-syntax-theme.js    the "Syntax colours" panel readers use
    favicon.png, ubel-logo.png
```

The deploy workflow (`.github/workflows/pipeline-dashboard.yml`) assembles
`dist/` from these and publishes it to Cloudflare Pages:

```
dist/
  index.html, ...        from web/home
  shared/                from web/shared
  docs/                  built by scripts/build_docs_versions.py (below)
  playground/            web/playground + the wasm build
  results/               the dashboard
```

`web/shared/` is referenced by absolute URL (`/shared/...`) from the docs
theme, because mdBook only copies files from inside the book's own folder.

## The docs: pages, structure, drafts

Pages are Markdown under `web/site/src/`, listed in `SUMMARY.md`. The
sidebar is exactly `SUMMARY.md`. An entry with an empty link,
`- [Async]()`, is a *draft chapter*: it shows greyed out in the sidebar and
promises a page that does not exist yet. Use drafts to keep the planned
structure visible; replace the `()` with a path when you write the page.

| Folder | Holds |
|--------|-------|
| `guide/` | the language, one topic per page |
| `tooling/` | `ubel`, `ubel-lsp`, the playground, the native backend |
| `reference/` | lookup material (built-ins, later the grammar) |
| `errors/` | the error reference, one page per family |
| top level | introduction, getting started, tour, tier model, status, versions |

Rules of thumb: describe what the compiler does *today* and say plainly what
is not built; link to the matching error codes from the place a mistake is
likely; keep one idea per page.

## Code examples are tested

Every example in a page is a fenced `ubel` block, preceded by an HTML
comment that says what the compiler must do with it. The comment is
invisible when rendered.

```text
<!-- ubel-test: ok -->               checks clean (`ubel check`)
<!-- ubel-test: run -->              checks clean, then `ubel run` exits 0
<!-- ubel-test: error TYPE-101 -->   fails with exactly this code
<!-- ubel-test: error A,B -->        fails with exactly this set of codes
<!-- ubel-test: skip -->             illustrative fragment, not compiled
```

`scripts/check_doc_examples.py` compiles each annotated block with the real
`ubel` binary and fails if it does something else:

```text
cargo build -p ubel_stratum_cli
python3 scripts/check_doc_examples.py                 # defaults: target/debug/ubel, web/site/src
python3 scripts/check_doc_examples.py --strict        # also fail on unannotated blocks
```

CI runs it as the **Docs Examples** workflow (on changes to the docs or the
compiler, and by hand). It is separate from the deploy on purpose, so a docs
typo never blocks publishing. A scaffold apply commits with `[skip ci]`, so
run the workflow by hand after one.

Use a `text` block for compiler output, program output and shell sessions.
Only `ubel` blocks are highlighted and tested.
Prefer showing the *exact* output the compiler printed: run it and paste it.

## The error reference

`web/site/src/errors/` has one page per family (`lex`, `parse`, `name`,
`type`, `tier`, `ownership`) and an `index.md` with the table of every code.
Each entry is a `### CODE` heading (the heading text becomes the anchor, so
`#type-101` works, and the playground links to it), the variant name and
message, what triggers it, an example with its exact output, and the fix.

When you add or change a diagnostic:

1. Update the registry in `docs/DIAGNOSTICS_RULES.md` (as before).
2. Add or update the entry in the matching `errors/*.md` page, with an
   example annotated `<!-- ubel-test: error CODE -->`.
3. Add the code to the table in `errors/index.md`.

An entry that cannot be reproduced from ordinary source is marked
*No verified example yet*. Do not invent an example: leave the marker until a
real one exists.

The playground turns `error[CODE]` into a link to
`/docs/latest/errors/<page>.html#<code>`. The code-to-page map is
`ERROR_PAGES` in `web/playground/index.html`; add a line there if you add a
family.

## Versioned docs

See [Versions and releases](../web/site/src/versions.md) for the reader-facing
description. Mechanics:

- `scripts/build_docs_versions.py` builds `next` (the working tree), one
  folder per release tag `vX.Y.Z` (from `web/site/` *as of that tag*, with the
  current `theme/` copied over it), `latest`, `versions.json`, and the
  `/docs/` redirect.
- The deploy checkout uses `fetch-depth: 0` so every tag is available.
- `web/site/theme/ubel-nav.js` reads `/docs/versions.json` and draws the
  picker and the "development" / "old version" banner. With no release the
  manifest says `"latest": null` and the banner says nothing has shipped.
- To release: tag, push the tag, run the **Pipeline Dashboard** workflow from
  `master`. A tag push alone does not deploy, because a deploy from a tag ref
  would land on a preview branch, not production.
- Local preview of the whole site, including the picker:

```text
python3 scripts/build_docs_versions.py --mdbook mdbook --out dist/docs
mkdir -p dist && cp -r web/shared dist/shared
python3 -m http.server -d dist          # then open /docs/
```

  (`mdbook` is version 0.5.x; CI installs 0.5.4.)

## Syntax highlighting and reader-chosen colours

One tokenizer in `web/shared/ubel-highlight.js` serves the docs (it rewrites
every `code.language-ubel`) and the playground editor (it registers a
CodeMirror 5 mode named `ubel`). Its keyword tables mirror
`crates/core/src/lexer/keywords.rs`: **when a keyword is added to the lexer,
add it to `KEYWORDS`, `TIER_WORDS` or `TYPES` there too.**

Token classes: keyword, type, tier, string, number, comment, attribute
(`@name`), function (an identifier before `(` or after `fn`), constant
(`ALL_CAPS`). Each maps to a CSS variable (`--ubl-keyword` and so on) with its
default in `web/shared/ubel-theme.css`. Nothing may hard-code a token colour.

`web/shared/ubel-syntax-theme.js` is the "Syntax colours" panel. It writes the
reader's choices onto `<html>` as inline custom properties and keeps them in
`localStorage` under `ubel.syntax.v1`, so the docs, the playground and other
tabs agree. It offers presets, per-token colour pickers (plus a background and
a plain-text colour), a live preview, a contrast warning, reset, and
export/import as JSON. To add a token class: add it to `TOKENS` there, to the
tokenizer, and to the `.ubl-*` / `.cm-s-ubel .cm-ubl-*` rules in the theme CSS.

The preview sample in the panel is a real Ubel program; if you change it, run
it through `ubel check`.

## Checklist for a docs change

- [ ] New page added to `SUMMARY.md` (or a draft entry filled in).
- [ ] Every `ubel` block annotated; `check_doc_examples.py` passes.
- [ ] Error codes linked where a mistake is likely.
- [ ] Anything not built yet is said so, on that page.
- [ ] If a diagnostic changed: registry, error page and index updated.
