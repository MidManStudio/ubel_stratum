# Ubel Stratum — Module System Ideas (`summon`)

> **Status:** Parked design exploration. Nothing here is built — no
> lexer token, no parser rule, no AST node. This document exists so the
> good parts of a long brainstorm don't have to be re-derived later, and
> so the confused parts don't get carried forward by accident.
>
> Source: a separate conversation's brainstorm on Ubel's future module
> system. Filtered here — kept what's genuinely good and consistent
> with the rest of the language, corrected one real mistake, dropped
> what was just branding chat with no technical content.

---

## 1. The core idea, kept

`summon` as the import keyword is a good, on-theme fit (Ubel is already
named after a Frieren character; "casting a spell" to pull in code reads
naturally, the way Zig's `@import` or Nim's `import` read naturally in
their own languages). The syntax kernel that came out of the brainstorm
is clean and worth keeping as the starting point for a real design pass:

```ubel
// Whole module
summon std.fs

// Aliased
summon compiler.parser as json_parser

// Destructured — [] deliberately mirrors array/slice syntax, not a new
// bracket convention invented just for this
summon [ Token, TokenKind, tokenize ] from selfhosted.lexer
summon [ Result, Option as Opt ] from std.core
```

No string-literal paths (`import "std/fs"`) — dot-separated identifier
paths instead, consistent with how the rest of the language already
names things (`std.fs`, not `"std/fs"`). This also sidesteps the
file-path-vs-logical-namespace tension the brainstorm raised: a path is
a name, not a filesystem location, so renaming a file doesn't
necessarily break every importer the way Go's literal-path imports do.

**Every `.ubl` file is automatically a module** — no `mod.rs`/`mod.ubl`
boilerplate tax the way Rust requires. Private by default; only items
marked `pub` are visible to a `summon`er. This was clearly aimed at
Rust's own biggest ergonomics complaint (explicitly named in the
brainstorm) and is a reasonable, on-target answer to it.

## 2. Scope-local `summon`

Most languages force imports to file scope. The brainstorm's proposal —
let `summon` appear inside a function, an `if` block, or any other
block, with the imported name going out of scope at the block's end —
is worth keeping as a real option, not just flavor text:

```ubel
fn process_payload(raw_data: Str, use_json: bool) void {
    if (use_json) {
        summon std.json          // only visible inside this block
        let doc = json.parse(raw_data)
    }
    // `json` is not a name here
}
```

D and Scala both do real versions of this. It's a genuine, checkable
language feature (not just sugar), and it fits a codebase that already
treats scoping seriously (`with arena(...)`, `[lifetime L]` boundaries).

## 3. Tier-aware `summon`

This is the one idea in the whole brainstorm that's genuinely *novel* —
not lifted from an existing language, and specific to what Ubel already
has that most languages don't (the three-tier memory model). The
proposal: let a `summon` declare a tier ceiling on what it's allowed to
pull in.

```ubel
// Statically guarantees this entire module contains zero @tier(high)
// (managed-heap) allocations
summon std.slice_utils @tier(low)
```

Worth taking seriously precisely because it's not just import ergonomics
— it would let a caller get a *compile-time* guarantee about a whole
dependency subtree's tier behavior, which nothing else in the language
currently offers (tier checking today is per-function, via `@tier(...)`
on the declaration itself, not per-dependency-graph). Real enforcement
would need every exported item in the target module (transitively) to
be checked against the ceiling — a genuinely new kind of check, not a
small addition to `tier_check.rs`. Flagged as interesting and worth
designing properly, not as something to bolt on casually.

## 4. Feature-gated and parameterized `summon`

Two related ideas, both worth keeping as *options* rather than
committing to either yet:

```ubel
// Feature flags, conditional on the target's own manifest
summon strat_json with [ simd_accelerated, zero_copy ]

// Parameterized ("functor"-style, from OCaml/Agda) — pass
// configuration at the import site itself
summon std.allocator with (capacity = 64KB, policy = "arena") as local_arena
```

The brainstorm reused the same `with` keyword for both feature flags
(a list) and functor-style parameters (named args), which is a real
ambiguity worth resolving before either gets built — `with [ ... ]`
(list) vs. `with ( ... )` (function-call-shaped) is one way to
disambiguate syntactically, but it's worth deciding on purpose, not by
accident of which example got typed first.

## 5. Package concept: `Grimoire`

"Grimoire" (recommended over "Tome" or "Scroll" in the brainstorm) is a
good, on-theme name for a distributable unit of Ubel code, and pairs
naturally with `summon`. No real objection to it.

## 6. The manifest — one real correction

The brainstorm's manifest design is sound in shape (separate the
**package manifest** — metadata, dependencies, features, default tier —
from the **module entry point** — what a directory re-exports when
`summon`ed), directly mirroring `Cargo.toml` vs. `lib.rs`/`mod.rs`. That
split is worth keeping. But partway through, the brainstorm proposed
naming the manifest `Grim.mdix` and writing it in **DixScript's `.mdix`
format** — this is a mistake, not a design decision, and shouldn't be
carried forward:

- `.mdix`/DixScript (`Mid-D-Man/DixScript-Rust`) is a **separate
  project** — a general-purpose data-interchange format for game/app
  configs, with its own compiler, its own runtime, its own versioning.
  It has no relationship to Ubel Stratum beyond sharing an author.
- Giving Ubel's own package manifest a file extension and grammar that
  belongs to an unrelated project creates a real dependency (Ubel's
  package manager would need to embed or shell out to a DixScript
  parser) for zero benefit — nothing about package manifests needs
  DixScript's specific features (compile-time `QuickFuncs`, `@DLM`
  encryption/compression, `@RAW` payloads). It also means anyone reading
  Ubel's own manifest format has to go learn a second, unrelated
  language's syntax first.
- The brainstorm's own **earlier** proposal — before it drifted into
  the `.mdix` idea — was better: a manifest in **native Ubel syntax**,
  using Ubel's own attribute style (`@grimoire { ... }`). That's the one
  worth keeping.

Recommended naming, using the full/short word split to keep the two
files' roles distinct at a glance (rather than both being called some
form of "Grim," which is what made the brainstorm's own final proposal
confusing — `Grim.mdix` for the manifest and `grim.ubl` for the entry
point read as near-identical names for two different jobs):

- **`grimoire.ubl`** — the package manifest (Cargo.toml equivalent), one
  per package root, native Ubel attribute syntax:

  ```ubel
  @grimoire {
      name = "strat_json",
      version = "0.1.0",
      authors = ["Ubel Core Team"],
  }

  @features {
      simd_accelerated = [ std.simd ],
      zero_copy = [],
  }

  @defaults {
      tier = "mid",
  }
  ```

- **`grim.ubl`** — a module entry point (`mod.rs`/`lib.rs` equivalent),
  one per directory that wants to control what it re-exports:

  ```ubel
  // src/parser/grim.ubl
  summon [ AstNode, make_ast_node ] from self.ast
  summon [ parse_stream ] from self.engine

  pub fn configure_parser(max_depth: int) void { ... }
  ```

Everything else about the manifest idea (per-feature dependency lists,
a `@defaults` block for tier, directories without a `grim.ubl` falling
back to plain file-name auto-discovery) carries over unchanged — only
the file format/extension for the manifest needed correcting.

## 7. Explicitly not carried forward as a commitment

- **Content-addressed caching (AST hashing, Unison-style).** Genuinely
  interesting — skip re-parsing/re-typechecking a symbol whose AST
  shape hasn't changed since the last build — but it's a whole build-
  caching subsystem, not part of the import *syntax* design, and
  shouldn't block or complicate the syntax work above. Worth a look
  once there's a real compiler pipeline slow enough to need it, not
  before.
- **Nim's `except { ... }` exclusion syntax** and **C#-style explicit-
  interface-implementation-flavored destructuring** were both mentioned
  in passing as inspiration but never turned into a concrete Ubel
  proposal — nothing to lock in yet, just noted as prior art if the
  destructuring syntax above ever needs an exclusion form.

## 8. Genuinely open, not resolved by this filtering pass

- `with [ ... ]` vs. `with ( ... )` disambiguation between feature
  flags and functor-style parameters (§4).
- What "tier-checked transitively" (§3) actually requires from
  `tier_check.rs` — likely a real, separate scoping pass of its own
  before any of this gets built.
- Whether `@tier(...)` on a `summon` caps the *module's own* declared
  tiers or *coerces* them (i.e. is it a static assertion that fails
  loudly, or does it silently treat everything as capped at that tier)
  — the brainstorm's phrasing ("statically guarantees...zero
  @tier(high) allocations") suggests assertion-and-fail, but this
  wasn't pinned down precisely.
