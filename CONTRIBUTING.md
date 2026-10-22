# Contributing to Ubel Stratum

Thanks for taking a look at this. Ubel Stratum is early — Phase 3 of 5
(tree-walking interpreter, with only a proof of concept of the LLVM backend so far (measured tiers and a small emitter), see docs/LLVM_POC.md and
`README.md`) — which means there's real design still being worked out
in the open, not just bug fixes on a settled language. Read before you
write; this project has strong, specific, *documented* conventions, and
"that's not how the rest of the codebase does it" is a valid review
comment.

---

## Before you start

Read what's relevant to what you're touching. Don't skim — these
documents exist because the decisions in them were genuinely argued
through, not defaults nobody thought about:

| Doc | Read it if you're touching |
|---|---|
| `docs/MEMORY_MODEL.md` | tiers, `GcRef`/`ArenaRef`/`OwnedRef`, `Unique`/`Shared`/`SyncShared`, borrow checking |
| `PARSER_RULES.md` | anything in `crates/rd_parser` |
| `docs/GENERICS_RULES.md` | generic structs/enums, type parameters |
| `docs/ENUM_RULES.md` | enum declarations, variant payloads |
| `docs/BUILTINS_RULES.md` | builtin namespaces/methods (`List`, `Math`, instance methods) |
| `docs/DATASTRUCTURES.md` | `Pool<T>`, `InlineList<T>`, `Linqerizer<T>`, and open data-structure design questions |
| `docs/DIAGNOSTICS_RULES.md` | any new error variant — **error codes are a registry, not a free-for-all** |
| `docs/NAMING_CONVENTIONS.md` | anything with a name |
| `docs/TESTING_RULES.md` | any test or fixture |
| `docs/ubel.ebnf` | the formal grammar |

If a decision in one of these docs seems wrong, open an issue and argue
it there before writing code against the alternative. Several of these
documents record decisions that were *deliberately* made one way after
considering the alternative — re-litigating in a PR diff, after the
fact, wastes everyone's time.

---

## Project structure

```
crates/core/        ubel_stratum      — lexer, AST, sema, interpreter, builtins
crates/rd_parser/    ubel_stratum_rd   — recursive-descent + Pratt parser
crates/parser/                        — legacy LALRPOP parser, reference only,
                                         NOT a default workspace member (too
                                         heavy for CI / modest hardware — see
                                         PARSER_RULES.md §1)
tests/fixtures/*.ubl                  — real source files run through the
                                         full pipeline; see TESTING_RULES.md
docs/                                  — the rules documents above
```

## Getting set up

```bash
git clone https://github.com/MidManStudio/ubel_stratum.git
cd ubel_stratum
cargo build --workspace
cargo test --workspace --lib
cargo run -p ubel_stratum_rd --example pipeline -- tests/fixtures
```

CI (`ci-check.yml`, `pipeline-dashboard.yml`) builds against `stable`
Rust — no pinned MSRV currently. If a change needs a newer Rust feature,
say so in the PR; that's a real conversation, not an automatic no.

`crates/parser` (the LALRPOP crate) is excluded from default builds on
purpose — don't add it back to workspace members. Build it explicitly
if you're specifically working on it: `cargo build --manifest-path
crates/parser/Cargo.toml`.

---

## Making a change

1. **Fork, branch.** Branch names aren't enforced, but `feature/…`,
   `fix/…`, `docs/…` prefixes are appreciated.
2. **Small, coherent commits.** A commit should be one real change, not
   a checkpoint. "wip" / "fix" / "更新" commit messages get squashed on
   merge regardless, but a reviewable history makes review faster.
3. **Test it — really test it**, per `docs/TESTING_RULES.md`:
   - New behavior gets a real `.ubl` fixture under `tests/fixtures/`
     (an `ok_*`/`err_*` pair where applicable), run through the actual
     pipeline — not only a hand-built AST unit test in `sema/tests.rs`.
   - If your feature interacts with something already landed, add at
     least one fixture that exercises both together, not just each in
     isolation.
   - `cargo test --workspace --lib` passes, and the fixture sweep
     (`cargo run -p ubel_stratum_rd --example pipeline -- tests/fixtures`)
     shows the counts you expect — paste the actual output in the PR,
     don't just assert it passed.
4. **New diagnostics go through the registry.** A new `TypeError`/
   `TierError`/`NameError`/`BorrowError` variant needs: a `Diagnosable`
   impl (code, message, span, suggestion — see any existing variant for
   the shape), an entry in `docs/DIAGNOSTICS_RULES.md`'s code table
   *and* its detailed-entry section, and a fixture that actually
   triggers it and shows the rendered diagnostic (`cargo run -p
   ubel_stratum_rd --example diagnose -- <fixture>`), not just the
   error enum's `Debug` output.
5. **Update the docs you read in step 0** if your change moves a
   documented decision from "open" to "resolved," or resolves an item
   in a doc's own open-questions section. A landed feature whose design
   doc still says "not yet implemented" is a bug in the docs.
6. **Open the PR.** Describe what changed, why, and paste real
   `cargo test`/fixture-sweep output. "Should work" is not evidence;
   the actual command output is.

### Performance-sensitive code (`crates/rd_parser`)

`PARSER_RULES.md` §3 is not a suggestion — hot-path functions need
`#[inline(always)]`, `TokenType` dispatch is always `match` (never a
hash map), `FxHashMap` not `std::collections::HashMap`, arena
allocation not `Box::new`, pre-sized collections for list parsing. The
pre-submit checklist at the bottom of that document is the actual bar
for anything in that crate.

---

## Filing issues

A good bug report includes: the `.ubl` source that triggers it (a
minimal repro if you can manage one), the exact command you ran, and
the actual output — including which stage failed
(`lex`/`parse`/`sema`/`interpret`), since `diagnose.rs`'s
`=== SUMMARY ===` block tells you that directly. "It doesn't work"
without a repro will just get a request for one.

For feature requests touching an area with an existing design doc
(memory model, generics, diagnostics, etc.), read that doc's open
questions first — you may be proposing to resolve something already
being tracked, which is useful to know either way.

---

## AI-assisted contributions

This project is built with substantial AI assistance, so a blanket
"no AI" policy would be dishonest — this isn't that. What isn't
accepted is **unverified** output: a diff nobody actually built, ran,
or understood, submitted because a tool produced text that looked
plausible.

If you use AI assistance — an LLM, a coding agent, autocomplete,
whatever — to help produce a contribution, that's fine, on these terms:

- **You ran it.** `cargo build`, `cargo test --workspace --lib`, and
  the fixture sweep all pass, on your machine, against the actual diff
  you're submitting — not a description of it, not "it should work."
- **You understand it.** You can explain what changed and why, in your
  own words, and answer follow-up questions in review. "The AI wrote
  it that way" is not an answer to "why does this work this way."
- **You verified it against a clean checkout**, not only the session
  that produced it — the same reason this project's own deliveries get
  re-confirmed against a fresh clone before being called done (see
  `docs/TESTING_RULES.md`). A diff that only works in the environment
  that generated it isn't finished.
- **New behavior ships with a real fixture**, per `docs/TESTING_RULES.md`
  — not only a hand-built assertion.
- **You disclose it if asked.** No need to caveat every PR description,
  but don't misrepresent how something was produced if a maintainer
  asks.

What gets rejected on sight: a PR that doesn't build, tests that were
never actually run, a diff the author can't walk through, or generated
code with no fixture coverage behind it. That bar is identical whether
a human or an AI wrote the first draft — AI assistance just makes it
easier to produce a lot of plausible-looking, unverified text quickly,
which is exactly the failure mode this rules out.

---

## Code of conduct

Be direct about the work, not about the person. Disagree with a
decision or a line of code as much as you want, with reasons; don't
make it personal. Maintainers can and will close issues/PRs that
devolve into that regardless of who's "right" about the code.

---

## License

Dual-licensed under MIT OR Apache-2.0 (see `LICENSE`). By contributing,
you agree your contribution is licensed under the same terms.
