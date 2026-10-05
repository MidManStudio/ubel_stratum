# Ubel Stratum — Testing & Verification Rules

> Canonical reference for how new features get verified in this project,
> going forward. Written after the `Unique<T>`/`Shared<T>`/`SyncShared<T>`
> construction-syntax session surfaced a real, previously-invisible test
> hazard — see §3.

---

## 1. Every feature gets a real `.ubl` fixture — not just a hand-built AST test

`sema/tests.rs`'s hand-built-AST style (construct `Program`/`Item`/
`FunctionDecl` directly via the arena, no parser involved) is fast and
good for isolated, targeted checks — "does this exact match arm fire."
It is not a substitute for a real fixture, for two reasons:

1. It never exercises the lexer or parser at all, so it can't catch
   anything wrong in either.
2. It has a real, sneaky hazard of its own — §3 below.

**Rule:** every new feature lands with at least one `.ubl` fixture under
`tests/fixtures/`, run through the real pipeline (`pipeline.rs`/
`diagnose.rs`), in addition to (not instead of) any hand-built unit
tests. Hand-built tests are for fast iteration while building; fixtures
are the real proof. A fixture only records accepted or rejected, so a
bug that changes how many diagnostics a program produces (one typo
reported twice, say) is covered by
`crates/rd_parser/tests/diagnostic_counts.rs` instead, which runs source
text through lex, parse and sema and asserts exact counts.
`crates/rd_parser/tests/int_literal_typing.rs` does the same for integer
literal typing and also runs programs through the interpreter, since
that feature spans sema and runtime. `crates/rd_parser/tests/statics.rs`
does it for `static` items, where the runtime half (one value shared by
every function) is the property most likely to regress silently.
`traits.rs` covers nominal traits (codes, messages, dispatch order and the
call-through-a-trait runtime contract). After `cargo test -p <crate>`, rebuild
with `cargo build --workspace --all-targets` before using the `pipeline`
example: a `-p` test run does not rebuild examples, and a stale binary once
made a correct fix look broken.
`type_aliases.rs`, `member_calls.rs` and `diagnostic_spans.rs` cover the
alias, enum-method, function-field and nested-span fixes; the last asserts
a diagnostic's LINE because a count cannot show that it pointed at the top
of the file.

## 2. Mix features together, not just in isolation

A fixture that only exercises one feature at a time can't catch a bug
that only shows up when two features interact. Once a feature has its
own isolated `ok_*`/`err_*` fixture pair, add at least one fixture that
combines it with something already landed — e.g. a `Unique<T>` field
inside a struct that also uses `Reference`, or a `Pool<T>`-tier function
that also constructs a `Shared<T>`. This is exactly the kind of coverage
gap that single-feature fixtures are structurally unable to catch, no
matter how many of them exist.

This doesn't mean every combination needs its own fixture — pick the
combinations that are actually plausible in real code, the same
judgment call already applied to which `ok_*`/`err_*` fixtures exist at
all.

## 3. The `Z`-span hazard (why rule #1 matters)

`name_resolution`'s `resolutions: HashMap<Span, DefId>` and
`type_infer`'s `expr_types: HashMap<Span, TypeId>` are both keyed
**purely by `Span`**. Real parsed source always gives every token a
distinct span, so this is invisible in anything that goes through the
real lexer/parser.

Hand-built AST tests conventionally use a single shared `Z` constant
(`Span { start: 0, end: 0, line: 0, column: 0 }`) for every node, for
convenience. That's harmless for a test with only one meaningfully-
resolved identifier reference. It breaks silently the moment a single
test has **more than one** distinct identifier reference sharing that
span (e.g. a function name and a builtin namespace name in the same
body) — the later one resolved overwrites the earlier one's slot in
both maps, and a completely unrelated lookup gets corrupted. This
surfaced first in `test_sema_unique_new_inner_type_matches_argument`
(`sema/tests.rs`) — the error message (`NoSuchMethod` on a type that
made no sense for that call site) gave no hint the actual cause was a
span collision; only printing the real resolved types made it obvious.

**Rule:** any hand-built test with more than one distinct identifier
reference must give the extra ones distinct spans — `span_n(n)`
(`sema/tests.rs`) exists for exactly this. `Z` is only safe for a test's
*first* (or only) identifier reference.

## 4. When a test fails unexpectedly, print the real value — don't guess from the assertion message

`assert!(sema::analyse(...).is_ok(), "sema should succeed but returned
an error")` tells you *that* something failed, never *what*. When a
failure doesn't match intuition, the fastest real answer is a temporary
debug test that prints the actual errors:

```rust
match sema::analyse(&prog, &arena, String::new()) {
    Ok(_) => println!("OK - no error"),
    Err(mut errors) => {
        println!("NAME: {:?}", errors.take_name_errors());
        println!("TYPE: {:?}", errors.take_type_errors());
        println!("TIER: {:?}", errors.take_tier_errors());
        println!("BORROW: {:?}", errors.take_borrow_errors());
    }
}
```
```bash
cargo test --workspace --lib <test_name> -- --nocapture
```

Delete the debug test once the real cause is found — it's a diagnostic
tool, not a permanent fixture. This is exactly how the §3 hazard was
actually found: the first guess (arg-order, tier logic) was wrong, and
printing the real error immediately showed a type (`Unique<int>`) that
made no sense for that call site, which is what pointed at span
collision rather than product logic.

## 5. Prefer fixtures that print, so results are visible without re-deriving them

Where a fixture can reasonably demonstrate a feature by printing an
actual value (`print`/`println`) rather than only type-checking a
declaration, do that. A fixture that only proves "this compiles" is
weaker evidence than one that also shows the real interpreted result —
and it means a reviewer (or a future session) can see what happened
directly in `pipeline.rs`/`diagnose.rs` output instead of re-deriving
expected values by reading the implementation.

This has a real, current limit worth naming honestly: Ubel Stratum's
`print`/`println`/`log` and string interpolation (`$"...{expr}..."`)
all go through one recursive `Value: Display` formatter — no format
specifiers (width, precision, padding, base), no `Debug`-vs-`Display`
split, no per-type custom formatting hook. It's more capable than it
might sound (compound types already render close to Rust's `{:?}`
derive shape — `TypeName { field: value, ... }`, nested collections
recurse correctly), but nowhere near `format!`/`println!`'s format-spec
mini-language. Flagged as a real gap worth its own dedicated pass, not
solved here — see the project's session notes for the fuller comparison.

---

## Summary

| Situation | Do this |
|---|---|
| Landing a new feature | Real `.ubl` fixture(s) (ok + err), *and* hand-built unit tests for fast iteration |
| Feature interacts with existing ones | At least one fixture combining them, not just isolated coverage |
| Hand-built test needs 2+ identifier references | `span_n(n)` for all but the first — never share `Z` across distinct identifiers |
| A test fails and the cause isn't obvious | Temporary `--nocapture` debug test printing real errors/values, delete once resolved |
| A fixture can show a real value, not just type-check | Print it |
