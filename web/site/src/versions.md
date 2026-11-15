# Versions and releases

The documentation is **versioned**: each release of Ubel Stratum gets a
frozen copy of these pages as they were at that release, so a program written
for one version can always be checked against the matching docs.

## What you are reading

At the top of every page there is a version picker, and sometimes a banner.

| URL | What it is |
|-----|-----------|
| `/docs/next/` | **Development docs**, built from the `master` branch. They describe the language as it is right now, including unreleased changes. |
| `/docs/vX.Y.Z/` | The docs of release `vX.Y.Z`, built from that release's git tag. They never change afterwards. |
| `/docs/latest/` | An alias for the newest release. Until the first release exists it is the same as `next`, so links to `/docs/latest/` always work. |

**No stable release has been published yet.** Until then `latest` and `next`
are the same pages, and the banner says so. The version machinery is in
place so the first release needs no change to the site.

## Releases

A release is a git tag named `vMAJOR.MINOR.PATCH`. A pre-release suffix such
as `v0.2.0-alpha.1` is allowed; a pre-release is listed in the picker but is
never what `latest` points at.

The site build (`scripts/build_docs_versions.py`, run by the deploy
workflow) does this on every deploy:

1. builds `next` from the checked-out `master`;
2. builds one copy of the docs for every release tag, from the
   `web/site/` folder as it was at that tag;
3. points `latest` at the newest non-pre-release tag, or at a copy of `next`
   if there is none;
4. writes `/docs/versions.json`, which the picker reads.

Every version is built with the *current* site theme (the `theme/` folder),
so every version has the picker, the syntax highlighting and the
reader-customisable colours. Only the chapters and `book.toml` come from the
tag.

## How the project cuts one

1. Update the docs so `next` describes what the release will be.
2. Tag the commit: `git tag v0.1.0 && git push origin v0.1.0`.
3. Run the **Pipeline Dashboard** workflow from `master` (Actions, then
   *Run workflow*). It rebuilds the site with the new version in the picker.

The tag, not the workflow run, is what makes a version: re-running the
workflow later rebuilds the same frozen docs.

## Writing docs that survive versioning

- Describe the language as it is *at that version*. When a feature lands,
  its chapter changes in `next` only; released versions keep their copy.
- Put "not built yet" statements on the page they concern, in plain words, so
  an old version's page is still true for that version.
- Every `ubel` example is checked against the compiler by CI when it is
  written (`docs/SITE_DOCS.md` explains the annotations), so when a version
  is tagged its examples were verified against that version's compiler.
