# Ubel Stratum — Diagnostics Rules

> **Canonical reference for every error, warning, and suggestion the
> compiler ever prints.
> Every contributor reads this before touching `error_management/`.**

---

## 0. Why this document exists

A language lives or dies by how well it tells you what you did wrong.
Rust's error messages are good not by accident — they name the exact
problem, point at the exact span, and usually tell you the exact fix.
That's the bar for Ubel Stratum's diagnostics, and until this document
existed, the compiler wasn't close to it: three separate, drifting
renderers existed (`diagnostics.rs::DiagnosticFormatter`,
`logger.rs::Logger::formatted_error`, and an inline copy in
`error_manager.rs`), only one was ever actually wired up, none of them
drew more than a single `^` regardless of span length, and the one tool
a person actually runs (`diagnose.rs`, and by extension the CI pipeline
dashboard) printed raw Rust `{:?}` Debug dumps instead of any of them.

All of that is fixed as of this document. There is now exactly one
renderer (`error_management/render.rs`), every error type implements
one trait (`Diagnosable`) to feed it, and `diagnose.rs` calls it. This
document is the spec for that renderer and the house style for every
message that flows through it — read it before adding a new error
variant, not after.

---

## 1. The format, worked example

Every diagnostic renders as this shape. This is real, current output —
not a mockup — captured from `err_duplicate_def.ubl`:

```
>>> error[NAME-002]: `x` is already defined in this scope
  --> 7:5
    |
  7 |     let x = 2    // ERROR: `x` is already defined in this scope
    |     ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
  ~> note: first defined here
    --> 6:5
      |
    6 |     let x = 1
      |     ^^^^^^^^^
  <~
<<<
```

Line by line:

| Line | What it is |
|---|---|
| `>>> error[CODE]: message` | Always the first line. `CODE` is a stable, greppable identifier (§4). `message` is one sentence, lowercase after the colon, no trailing period (§2). `>>>` opens the whole-diagnostic fold region — see §5. |
| `  --> line:col` | The primary location. Two-space indent, always. |
| `    \| ` / `  N \| <source>` | The gutter and the exact source line the span is on. Gutter width tracks the line number's own digit count (`104 \|` is wider than `7 \|`) — this is the one place a hand-rolled fixed-width gutter breaks, so don't reintroduce one. |
| `    \| ^^^^^^^^` | The underline. **Full width of the span, not one character** — this is the whole reason `render.rs` exists instead of the three things it replaced. |
| `  ~> note: ...` | Opens a secondary-span block with its own nested `-->`/gutter/underline, indented one level deeper. Optional — only errors that reference another location use it (right now: `NameError::DuplicateDefinition`, `TypeError::TypeMismatch` when `because_of` is set). |
| `  <~` | Closes the `~>` block directly above it. Every `~>` has exactly one matching `<~`, even a one-line `help` with no nested span. |
| `  ~> help: ...` | Opens a plain-text suggestion block, no location. Optional. Also closed by its own `  <~`. |
| `<<<` | Closes the whole diagnostic. Pairs with the opening `>>>`. Always the last line. See §5. |

---

## 2. Message-writing rules

These apply to every `message()` and `suggestion()`/`~> help:` body in
`errors/`.

1. **Lowercase after the colon**, matching rustc: `error: unexpected
   token`, not `error: Unexpected token`. Three of the four error type
   files were inconsistent about this before this document — check
   your new message against a sibling in the *same* file, not just
   against memory, since the inconsistency was file-by-file, not
   scattered.
   - Exception: a token, type name, or identifier in backticks keeps
     its own casing (`` `List<int>` ``), and so does an acronym used as
     a proper noun (`LINQ query expressions...` is correct as-is —
     the L isn't an artificially-capitalized sentence-starter, it's an
     acronym that's always uppercase).
   - Exception: tier names (`HIGH`, `MID`, `LOW`) stay uppercase
     wherever they appear, matching how `docs/MEMORY_MODEL.md` refers
     to them everywhere else. Treat them as proper nouns for this
     language, not as English words that happen to need emphasis.
2. **No trailing period** on the `message()` line. `~> help:` and
   `~> note:` text also skip the trailing period — matches the rest of
   the format's terse, label-like tone.
3. **State the fix, not just the problem**, whenever there IS a fix
   that isn't obvious from the message alone. `TypeMismatch` doesn't
   need a `suggestion()` — "expected `int`, found `string`" already
   tells you everything. `ArenaRefEscapesBoundary` does — "you can't do
   that" is true but useless without "restructure with the callback
   pattern instead." If you can't write a suggestion that adds real
   information beyond the message, leave `suggestion()` returning
   `None` for that variant rather than padding with something generic
   like "fix this error."
4. **Use backticks around anything the person would type or see in
   their own source**: types, identifiers, keywords, operators. Never
   backtick a category name (`type mismatch`, not `` `type mismatch` ``).
5. **Never use `{:?}` (Debug) to format a user-facing value that has a
   real `Display` impl.** `TokenType` has one — `parse_error.rs` used
   to print `` unexpected `Ident("foo")` `` via `{:?}` instead of the
   readable `` unexpected `foo` `` via `{}`. This was a real bug, now
   fixed, and the kind of thing to check for in any new message that
   embeds a token or type.
6. **Never use emoji.** Not in a message, not in a suggestion, not in
   a caller's `println!`/`eprintln!` around a diagnostic. This isn't
   stylistic pickiness: `diagnose.rs`'s output is captured to a file
   and re-displayed inside an HTML `<pre>` block by the pipeline
   dashboard, and an emoji is exactly as disruptive there as an ANSI
   escape code is (see §6) — it's noise a machine has to work around,
   not information. `crates/parser/src/main.rs` had `❌`/`✅` in its
   log lines before this document; they're gone now. If you're
   tempted to reach for one to mark pass/fail, `status: OK` /
   `status: FAIL` (already the house convention in `diagnose.rs`) says
   the same thing and greps cleanly.

---

## 3. Secondary spans

Use `secondary_spans()` (default: empty) whenever an error is fully
explained only by pointing at TWO places, not one — "you did X here,
but Y already happened there." The pattern, from `NameError`:

```rust
fn secondary_spans(&self) -> Vec<(Span, String)> {
    match self {
        NameError::DuplicateDefinition { first_defined, .. } =>
            vec![(*first_defined, "first defined here".to_string())],
        _ => Vec::new(),
    }
}
```

Don't reach for this to explain *why* a rule exists (that's what
`suggestion()`/`~> help:` is for) — only for a genuine second location.
`TypeError::TypeMismatch`'s `because_of` field is the other real user
of this today: when a type was established somewhere other than the
mismatch site, `~> note: expected type was established here` points at
it instead of the old text-only `"Type was established at line N"` —
show the actual line, don't just cite a line number.

---

## 4. Error Code Registry

Format: `<PHASE>-<NNN>`. Stable once assigned — never renumber an
existing code, even if the variant's wording changes. If a variant is
removed, retire its code; don't reassign the number to something else.

`TypeError` used to carry two number ranges in one enum (`TYPE-1xx`
ordinary type checking, `TYPE-2xx` tier & arena enforcement) as
groundwork for a later physical split — see §9. That split is done:
`TYPE-2xx` moved out entirely and now lives as its own `TIER-0xx`
family in `TierError`.

### LEX-0xx — `errors/lexical/mod.rs`

| Code | Variant |
|---|---|
| LEX-001 | UnexpectedChar |
| LEX-002 | UnterminatedString |
| LEX-003 | UnterminatedBlockComment |
| LEX-004 | InvalidNumber |
| LEX-005 | InvalidEscape |
| LEX-006 | InvalidInterpolation |
| LEX-007 | InvalidCharLiteral |

### PARSE-0xx — `errors/parse/mod.rs`

| Code | Variant |
|---|---|
| PARSE-001 | UnexpectedToken |
| PARSE-002 | UnexpectedEof |
| PARSE-003 | UnclosedDelimiter |
| PARSE-004 | IllegalInContext |
| PARSE-005 | Raw |

### NAME-0xx — `errors/naming/mod.rs`

| Code | Variant |
|---|---|
| NAME-001 | UndefinedName |
| NAME-002 | DuplicateDefinition |
| NAME-003 | UnresolvedImport |
| NAME-004 | UnresolvedPathSegment |
| NAME-005 | SelfOutsideMethod |
| NAME-006 | UnresolvedTypeParam |

### TYPE-1xx — ordinary type checking, `errors/types/mod.rs`

| Code | Variant |
|---|---|
| TYPE-101 | TypeMismatch |
| TYPE-102 | ArgumentCountMismatch |
| TYPE-103 | NoSuchField |
| TYPE-104 | NoSuchMethod |
| TYPE-105 | TryOnNonFallible |
| TYPE-106 | AwaitOnNonTask |
| TYPE-107 | CannotInferType |
| TYPE-108 | GenericArgCountMismatch |
| TYPE-109 | UnknownVariant |
| TYPE-110 | VariantArityMismatch |
| TYPE-111 | NonExhaustiveMatch |
| TYPE-112 | MixedDiscriminantAndPayload |
| TYPE-113 | InlineListCapacityNotLiteral |
| TYPE-114 | DerefOnNonReference |
| TYPE-115 | InvalidFormatSpec |
| TYPE-116 | UnknownDeriveTrait |

### TIER-0xx — tier & arena enforcement, `errors/tier/mod.rs`

| Code | Variant |
|---|---|
| TIER-001 | ArenaInWrongTier |
| TIER-002 | AwaitInWrongTier |
| TIER-003 | AsyncFunctionNotHigh |
| TIER-004 | ArenaRefEscapesBoundary |
| TIER-005 | IllegalTierCall |
| TIER-006 | MidReturnContainsArenaRef |
| TIER-007 | *(retired — was `LinqInWrongTier`; LINQ query syntax was removed in favor of `Linqerizer<T>`, a value/type rather than dedicated grammar — see the detailed entry below)* |
| TIER-008 | MethodInWrongTier |
| TIER-009 | CollectionConstructionInLowTier |
| TIER-010 | PoolInWrongTier |
| TIER-011 | PoolRefEscapesBoundary |
| TIER-012 | MidReturnContainsPoolRef |
| TIER-013 | PoolConstructedOutsideBlock |
| TIER-014 | OwnershipWrapperOutsideLowTier |

Physically split out of `TypeError` — see §9. Variant names and
meaning are unchanged from their `TYPE-2xx` days; only the enum and
the code prefix moved. The numeric tail was *not* kept stable across
the move (`TYPE-201` did not become `TIER-201`) — the whole range got
a fresh prefix and renumbered from 1, since these seven no longer
share a namespace with `TYPE-1xx`. See §9 for why that's still
consistent with "never renumber an existing code" above.

### BORROW-0xx — LOW-tier borrow checking, `errors/borrow/mod.rs`

| Code | Variant |
|---|---|
| BORROW-001 | ConflictingAccessWhileBorrowed |

New family, Phase D of the borrow checker (`sema/borrow_check.rs`) — not
a split-out of an existing enum the way `TIER-0xx` was. Only one variant
today; see `borrow_check.rs`'s own module doc for the checker's current
scope (mutable loans only, traceable-carrier loans only, no
intra-statement checking yet) — each of those boundaries is a real,
separate, documented follow-up, not silently dropped.

### MOVE-0xx — LOW-tier move checking, `errors/move_check/mod.rs`

| Code | Variant |
|---|---|
| MOVE-001 | UseAfterMove |

New family, not folded into `BORROW-0xx` even though both live under
LOW-tier's umbrella — aliasing (loans) and consuming (moves) are
genuinely different questions about a value, same call `borrow`'s own
family made rather than being a split-out of an existing enum. Fires
from `sema/move_check.rs`'s reachability fixed point, seeded by
`sema/move_facts.rs`'s move-candidate collection — see both modules'
doc comments for exactly how a `Unique<T>` local gets identified as
move-tracked (syntactic: an explicit `Unique<...>` annotation, or an
initializer that's directly a `Unique.new(...)` call — not real type
inference) and what counts as consuming it (any bare, non-`&`/`&mut`
use). May-analysis, same philosophy `BORROW-001` already uses: a value
that *might* already be moved on some incoming path is rejected
unconditionally. One design point worth naming since it isn't obvious
from the diagnostic alone: the reachability check allows a move point
to reach *itself*, which is what catches a value consumed on every loop
iteration without ever being reinitialized in between — in that case
`moved_span` and `used_span` end up pointing at the same line, which is
correct, not a rendering bug (see `err_move_in_loop_combined.ubl`).
Scope, today: only `let`-bound locals are tracked, not `Unique<T>`-typed
*parameters* — real, separate follow-up, not silently assumed safe; a
method-call receiver (`a.method()`) is conservatively treated as a move
of `a`, the safe direction while method dispatch through a `Unique`
wrapper is itself still an open question elsewhere (`MEMORY_MODEL.md`
§9).

### LIFETIME-0xx: lifetime declaration well-formedness, `errors/lifetime/mod.rs`

| Code | Variant |
|---|---|
| LIFETIME-001 | UndeclaredLifetime |
| LIFETIME-002 | DuplicateLifetimeParam |
| LIFETIME-003 | OutlivesCycle |

New family, not folded into `BORROW-0xx`/`MOVE-0xx` even though all
three eventually serve the same LOW-tier memory-safety story
(`MEMORY_MODEL.md` §9/§12): this one is a purely structural check over
declared names and constraints in `[lifetime L]`/`[lifetime L where L
outlives M]` on functions and `edge struct`s, no CFG or liveness fixed
point involved, so it doesn't share `borrow_check.rs`/`move_check.rs`'s
machinery or scope. Fires from `sema/lifetime_check.rs`, a new pass
that runs right after name resolution, before type inference, since it
needs neither. Checks a declaration's own lifetime names are declared
once each, that every name a `where` clause references is one of them,
that the declared `outlives` constraints don't form a cycle (including
the trivial `L outlives L` case), and that every `&name`/`ref name`
written directly in that same declaration's own signature or fields
(however deeply nested inside other types) names one of them. Does
*not* check that real usage in a function body actually respects a
declared bound — that's the actual outlives/subset fixed point, still
the borrow checker's job, and still unbuilt.

**Adding a new variant:** append at the end of its class's range
(don't renumber to keep things "tidy" — see above), add the `code()`
match arm, add a row to the relevant table above, and add an entry to
the full reference below — all in the same change. A variant without a
code doesn't compile (`Diagnosable::code` has no default) — the
registry can't silently drift out of sync with the enum the way the
old inline suggestion text used to drift from what `report_section`
actually printed. What *can* still drift silently: a brand-new error
*class* not being drained by every place that walks `ErrorManager`'s
output — see §9's case study, this reorg found exactly that gap.

### Full reference — message, suggestion, severity

Compact stand-in for what a per-code `rustc --explain`-style page
would say. Appended here in the same change that adds the variant,
same discipline as the tables above. Severity is always *Error*
today — `Diagnosable` has no warning tier yet, so this column is a
placeholder for if/when one gets added, not a claim that warnings
currently exist. This is deliberately lighter-weight than the ~33 full
worked-example pages proposed in §9 — those are still a separate,
larger effort, not started.

**LEX-0xx**
- `LEX-001` `UnexpectedChar` — *Error*. "Unexpected character '{ch}'". Suggestion only when the lexer has a specific guess for what was meant; otherwise none.
- `LEX-002` `UnterminatedString` — *Error*. "Unterminated {kind} string literal" (Normal/Interpolated/Verbatim/InterpolatedVerbatim). Suggestion: add the missing closing quote.
- `LEX-003` `UnterminatedBlockComment` — *Error*. "Unterminated block comment (nesting level: N)". Suggestion: add closing `*/`.
- `LEX-004` `InvalidNumber` — *Error*. "Invalid number literal 'text': reason". Suggestion is heuristic: extra `..` → remove it; bare `0x`/`0b` → add digits; otherwise none.
- `LEX-005` `InvalidEscape` — *Error*. "Invalid escape sequence 'seq'". Suggestion lists the valid escape sequences.
- `LEX-006` `InvalidInterpolation` — *Error*. Message and suggestion both passed through verbatim from the interpolation parser.
- `LEX-007` `InvalidCharLiteral` — *Error*. "Invalid character literal 'content': reason". Suggestion: a char literal must contain exactly one character.

**PARSE-0xx**
- `PARSE-001` `UnexpectedToken` — *Error*. "unexpected `found` while parsing context, expected: ...". Suggestion only when exactly one token was expected ("try replacing `found` with expected").
- `PARSE-002` `UnexpectedEof` — *Error*. "unexpected end of file while parsing context, expected: ...". No suggestion.
- `PARSE-003` `UnclosedDelimiter` — *Error*. "unclosed `delim` — found `x` instead" or "...reached end of file" if nothing closed it. Suggestion: add the matching closing delimiter.
- `PARSE-004` `IllegalInContext` — *Error*. "`what` is not allowed here: reason". Suggestion passed through verbatim when the caller supplies one.
- `PARSE-005` `Raw` — *Error*. Message passed through verbatim, no suggestion. Escape hatch for parser errors that don't fit the other four shapes.

**NAME-0xx**
- `NAME-001` `UndefinedName` — *Error*. "undefined name `x`". Suggestion: "did you mean `y`?" when a close match was found, otherwise none.
- `NAME-002` `DuplicateDefinition` — *Error*. "`x` is already defined in this scope". No suggestion.
- `NAME-003` `UnresolvedImport` — *Error*. "cannot resolve import path `x`". No suggestion.
- `NAME-004` `UnresolvedPathSegment` — *Error*. "no member `seg` in `resolved-so-far` (while resolving `full-path`)". No suggestion.
- `NAME-005` `SelfOutsideMethod` — *Error*. "`self` can only be used inside a method body". Suggestion: move the code into a method that takes `self`.
- `NAME-006` `UnresolvedTypeParam` — *Error*. "unknown type parameter `x`". No suggestion.

**TYPE-1xx**
- `TYPE-101` `TypeMismatch` — *Error*. "type mismatch: expected `x`, found `y`". No suggestion; carries a secondary span at where the expected type was established, when known.
- `TYPE-102` `ArgumentCountMismatch` — *Error*. "expected N argument(s), found M". No suggestion.
- `TYPE-103` `NoSuchField` — *Error*. "type `T` has no field `f`". No suggestion.
- `TYPE-104` `NoSuchMethod` — *Error*. "type `T` has no method `m`". No suggestion.
- `TYPE-105` `TryOnNonFallible` — *Error*. "`?` requires a fallible type (`T!`), found `x`". No suggestion.
- `TYPE-106` `AwaitOnNonTask` — *Error*. "`await` requires `Task<T>`, found `x`". No suggestion.
- `TYPE-107` `CannotInferType` — *Error*. "cannot infer type — add an explicit type annotation". Suggestion passed through verbatim when the caller supplies one.
- `TYPE-108` `GenericArgCountMismatch` — *Error*. "`T` expects N type argument(s), found M". No suggestion.
- `TYPE-109` `UnknownVariant` — *Error*. "enum `E` has no variant `V`" — ENUM_RULES.md §5. Fires for both expression position (`Direction.Northeast`) and pattern position (a bad variant name, including the `{field}` form used when a struct-payload pattern names a field that isn't declared). No suggestion — the enum's real variant names are already visible at the declaration site.
- `TYPE-110` `VariantArityMismatch` — *Error*. "`E.V` expects N value(s), found M" — ENUM_RULES.md §5. Fires for tuple-payload arity mismatches at both construction (`Result.Ok(1, 2)`) and pattern (`Ok(a, b) => ...`) sites, and for a payload-shape mismatch entirely (a bare `Ok => ...` pattern against a tuple-payload variant, reported as expected-N-found-0). No suggestion — the fix is always "match the declared shape," visible at the enum declaration.
- `TYPE-111` `NonExhaustiveMatch` — *Error*. "match is not exhaustive — missing variant(s): ..." — ENUM_RULES.md §5. Pragmatic top-level-only coverage check (no nested-pattern usefulness analysis, unlike Rust's full decision-tree algorithm) — real and useful, not a stub. A guarded arm never counts toward coverage, since the guard can fail. Suggestion: add arms for the missing variant(s), or a wildcard `_ => ...` arm.
- `TYPE-112` `MixedDiscriminantAndPayload` — *Error*. "enum `E` mixes an explicit discriminant variant with a payload-carrying variant" — ENUM_RULES.md §3.2/§5. Checked once per enum in `collect_enum_sig`; matches Rust's own restriction rather than defining a combined runtime representation for a shape nobody asked for. Suggestion: use either explicit discriminants on every fieldless variant, or payload variants — not both in the same enum.
- `TYPE-113` `InlineListCapacityNotLiteral` — *Error*. "`InlineList.new(...)` requires a literal integer capacity" — DATASTRUCTURES.md §5. `InlineList<T>` is stack-checked and bounded, so its capacity has to be known at compile time — a variable or computed expression can't be used. Backfilled into this registry; existed in code before this entry did. Suggestion: write the capacity as a plain integer literal, e.g. `InlineList.new(64)`.
- `TYPE-114` `DerefOnNonReference` — *Error*. "`*`/`deref` requires a reference type (`&T`/`ref T`), found `{found}`" — MEMORY_MODEL.md §9. Fires when `*`/`deref` (the dual-spelling deref operators) are applied to a non-`Reference`-typed value. Backfilled into this registry; existed in code before this entry did.
- `TYPE-115` `InvalidFormatSpec` — *Error*. "`{spec_part}` in a format spec doesn't apply to type `{on_type}`" — docs/PRINT_FORMAT_RULES.md. Currently the only checked case is `.precision` outside Float/Double/Str (width/align/`?` apply to any type, since padding/truncation work on the rendered string regardless of what produced it). Fires from the same `infer_literal` walk that made interpolation holes get real type-checking at all for the first time — see that doc's "what this fixed along the way" section. Suggestion: drop `.precision`, or format a value of one of the three types it applies to.
- `TYPE-116` `UnknownDeriveTrait` — *Error*. "unknown derive trait `{trait_name}`", or "`@derive({trait_name})` is unnecessary — this is already automatic" for the three names that ARE recognized but never legitimately reach this error — docs/PRINT_FORMAT_RULES.md §6. Fires on `struct`/`enum` declarations from `check_derive_attrs` in `type_infer.rs`, called from `collect_struct_sig`/`collect_enum_sig`. One variant, three real cases: a genuinely unknown name; `Debug`/`Display` anywhere (both automatic for every struct/enum already — no `@derive` needed, ever); `PartialEq` specifically on an `enum` (an enum's `==` is already structural — `Value::equals`'s Enum arm predates this delivery). `PartialEq` on a `struct` is the one case that's actually accepted and does something (opts that struct's `==` into structural comparison, replacing the `Rc::ptr_eq` tier-consistent default) — see `Value::Struct::derives_partial_eq`. `trait_name` is a readable description of whatever was actually written, not necessarily a real identifier — a non-`Ident` arg (`@derive("PartialEq")`, `@derive(x = 1)`) reaches here too, deliberately not silently dropped. Suggestion: for the redundant three, drop the attribute; otherwise, "supported derive traits: `PartialEq`".

**TIER-0xx**
- `TIER-001` `ArenaInWrongTier` — *Error*. "`with arena` is only valid in `@tier(mid)`; this function is `@tier(x)`". Suggestion: annotate with `@tier(mid)` or remove the arena block.
- `TIER-002` `AwaitInWrongTier` — *Error*. "`await` is only valid in `@tier(high)`; this function is `@tier(x)`". Suggestion: annotate with `@tier(high)` or remove the `await`.
- `TIER-003` `AsyncFunctionNotHigh` — *Error*. "async functions must be `@tier(high)`; this function is `@tier(x)`". Suggestion: add `@tier(high)` or remove `async`.
- `TIER-004` `ArenaRefEscapesBoundary` — *Error*. "value of type `T` is scoped to a `with arena(...)` block and cannot outlive it". Suggestion: copy the data out before the block ends, or restructure with the callback pattern instead of returning/storing the arena-scoped value directly.
- `TIER-005` `IllegalTierCall` — *Error*. "`@tier(caller)` code cannot call `@tier(callee)` function `name`". Suggestion (HIGH-caller case only): wrap the LOW-tier logic in a MID-tier function.
- `TIER-006` `MidReturnContainsArenaRef` — *Error*. "return type `T` contains an arena-lifetime reference; this makes the function uncallable from `@tier(high)`". No suggestion.
- `TIER-007` — *Retired.* Was `LinqInWrongTier`: "LINQ query expressions are only valid in `@tier(high)`; this function is `@tier(x)`". LINQ query-comprehension syntax (`from x in ... where ... select ...`) was fully wired end-to-end — parser, name resolution, type inference, tier checking, interpreter — but eager, hardcoded to `List` only, `groupby` was an unimplemented no-op, and it had zero fixture coverage. Removed outright rather than kept alongside `Linqerizer<T>` (a real value/type with chainable, lazy query methods) to avoid two divergent implementations of the same idea. Per this section's own rule above, `TIER-007` is retired, not reassigned.
- `TIER-008` `MethodInWrongTier` — *Error*. "`.method()` is only valid in `@tier(high)`; this function is `@tier(x)`" — MEMORY_MODEL.md §8, same shape as `AwaitInWrongTier` but keyed by method name via `instance::is_high_only` rather than a dedicated keyword. Suggestion: move the call to a `@tier(high)` function, or avoid the method in `@tier(mid)`/`@tier(low)` code. Every `HIGH_ONLY` registry is empty today — real, consulted infrastructure, not currently flagging anything.
- `TIER-009` `CollectionConstructionInLowTier` — *Error*. "`Collection.new()` is not yet supported in `@tier(low)` — LOW tier's memory model (move semantics + borrow checker) hasn't been built yet" — MEMORY_MODEL.md §9. Fires for `List`/`Dictionary`/`Queue`/`Stack` construction inside a `@tier(low)` function, at the same site `type_infer.rs` already resolves `builtin_constructor_type`. Emit-and-continue, same non-fatal pattern as `ArenaInWrongTier`/`MethodInWrongTier` — chosen deliberately over returning `Unknown`, so one bad constructor call doesn't cascade spurious errors through the rest of the function. Suggestion: construct the collection in a `@tier(mid)` or `@tier(high)` function instead, or restructure to receive it as a parameter.
- `TIER-010` `PoolInWrongTier` — *Error*. "`with pool<T>(count)` is only valid in `@tier(mid)`; this function is `@tier(x)`" — MEMORY_MODEL.md §10, exact same rule and call site as `ArenaInWrongTier`. Suggestion: move the pool block to a `@tier(mid)` function.
- `TIER-011` `PoolRefEscapesBoundary` — *Error*. "value of type `&pool T` is scoped to a `with pool<T>(count) { }` block and cannot outlive it" — MEMORY_MODEL.md §10, same escape-boundary mechanism as `ArenaRefEscapesBoundary` (`check_assign_arena_escape`/`scope_mismatch_side`, generalized to check both scope kinds off one shared comparison, branching only at the report call so the wording stays accurate). Applies to `Pool<T>` itself and to anything acquired from it (`Handle<T>` results are re-wrapped in the receiver's own `PoolRef`, same as every other allocating instance method). Suggestion: copy the data out before the block ends, or restructure with the callback pattern.
- `TIER-012` `MidReturnContainsPoolRef` — *Error*. "MID-tier function's return type contains a pool-lifetime reference" — MEMORY_MODEL.md §10, generalized from `MidReturnContainsArenaRef` alongside `scope_ref_kind`. Like its arena counterpart, real consulted infrastructure that can't currently be *triggered* by any writable fixture — there's no surface syntax yet to write `Pool<T>` as an explicit return-type annotation (§10's "Known gap"), so the reachable path for this exact mistake is the escape-boundary check via assignment instead.
- `TIER-013` `PoolConstructedOutsideBlock` — *Error*. "`Pool.new()` requires an enclosing `with pool<T>(count) { }` block" — MEMORY_MODEL.md §10. Unlike every other builtin constructor, `Pool.new()` has no generic argument of its own to infer element type or capacity from; it reads both from `current_pool()`, which is `None` outside any pool block. Suggestion: call `Pool.new()` inside a `with pool<T>(count) { }` block.
- `TIER-014` `OwnershipWrapperOutsideLowTier` — *Error*. "`{Unique|Shared|SyncShared}.new()` is only valid in `@tier(low)`; this function is `@tier({actual})`" — MEMORY_MODEL.md §9. The deliberate *inverse* of `TIER-009`: `List`/`Dictionary`/`Queue`/`Stack` are banned *inside* LOW tier because LOW has no memory model of its own; `Unique`/`Shared`/`SyncShared` now *are* that memory model, so construction is banned everywhere *except* LOW tier. Fires at the same `Namespace.new(value)` call site `type_infer.rs` special-cases these three at, alongside the `TYPE-102` arg-count check. A HIGH/MID-tier function may still *receive* a `Unique<T>` value as a parameter — only construction is restricted. Suggestion: annotate the function `@tier(low)`, or receive the value as a parameter from LOW-tier code instead.

**BORROW-0xx**
- `BORROW-001` `ConflictingAccessWhileBorrowed` — *Error*. "cannot use `place` while it is mutably borrowed" — MEMORY_MODEL.md §9, `sema/borrow_check.rs` (Phase D). Fires when a `&mut` loan's `bound_place` (the local it's assigned to, e.g. `p` in `let p = &mut n`) is still *live* — will be read again later, per backward liveness over the CFG — at the point some other statement conflictingly reads or re-borrows the loan's place. Liveness-gated deliberately: a conflicting read after the loan's carrier has already had its last use is NOT flagged (see `ok_borrow_dead_after_last_use.ubl`) — that's the actual non-lexical-scope behavior this checker is built around, not a naive "any candidate is an error" rule. Secondary span points at the loan's own `&mut` site ("mutable borrow occurs here"). Suggestion: move the conflicting use before the borrow's last use, or restructure so the borrow doesn't need to outlive it. Scope, today: only mutable loans are checked (two shared loans never conflict with each other, and this checker doesn't yet distinguish "plain read" from "new borrow" among conflicting accesses precisely enough to safely check the shared-then-mutable-elsewhere direction — real, separate follow-up); only loans bound to a traceable local are checked (a borrow consumed inline, e.g. a bare call argument, has no carrier that could still be "live" later); intra-statement conflicts (two loans issued at the very same point, e.g. `f(&n, &mut n)`) are excluded upstream by `facts::collect` itself and never reach this check at all — a distinct, separate, not-yet-built piece of work.

**MOVE-0xx**
- `MOVE-001` `UseAfterMove` — *Error*. "use of `place` after it was already moved" — MEMORY_MODEL.md §9, `sema/move_check.rs`. Fires when a `Unique<T>`-typed local's bare (non-`&`/`&mut`) use is forward-reachable, over the same point-level CFG walk `BORROW-001` uses, from an *earlier* bare use of the same local, with no reinitialization (`facts::place_defined_at`) in between. May-analysis, same direction `BORROW-001` already takes: reachable on *some* path is enough, not every path. Reachability deliberately allows a move point to reach itself — the mechanism that catches a value consumed on every loop iteration without ever being reinitialized (see `err_move_in_loop_combined.ubl`); when that's what fired, `moved_span` and `used_span` point at the same line, correctly. Secondary span points at the earlier consuming use ("value moved here"). Suggestion: borrow instead of moving if the earlier use didn't need to consume the value, or reassign a fresh value before this point. Scope, today: only `let`-bound locals are tracked — a move-tracked local is identified *syntactically* (an explicit `Unique<...>` annotation, or an initializer that's directly a `Unique.new(...)` call), not via real type inference, so a `Unique<T>` value arriving more indirectly (returned from another function, round-tripped through a field) isn't tracked at all yet; a `Unique<T>`-typed function *parameter* isn't tracked either, only locals bound via `let`; a method-call receiver (`a.method()`) is conservatively treated as a move of `a`, matching the fact that `resolve_receiver` doesn't strip a `Unique` wrapper for dispatch yet either (`MEMORY_MODEL.md` §9's own open question) — each of these is real, separate, documented follow-up, not silently dropped.

**LIFETIME-0xx**
- `LIFETIME-001` `UndeclaredLifetime`: *Error*. "undeclared lifetime `name`", from `sema/lifetime_check.rs`. Fires for a name used in a `where X outlives Y` clause, or written as `&name T`/`ref name T` anywhere inside a function's own param/return types or an `edge struct`'s own field types, that isn't one of the names that same declaration's `[lifetime ...]` list actually declares. Suggestion: add `lifetime name` to the declaration, or use a name it already declares.
- `LIFETIME-002` `DuplicateLifetimeParam`: *Error*. "lifetime `name` declared more than once". Secondary span points at the first declaration. Suggestion: remove one of the two entries, or rename one of them.
- `LIFETIME-003` `OutlivesCycle`: *Error*. "lifetime `name` cannot outlive itself" for the trivial one-element case (`L outlives L`); "outlives constraints form a cycle among `L`, `M`, ..." for a longer cycle found via a plain DFS over the declaration's own (always tiny) constraint graph. Suggestion: outlives relationships must form a strict ordering, with no lifetime directly or indirectly outliving itself.

---

## 5. Fold markers

Every fold region has an explicit, symmetric open AND close marker —
nothing is inferred from an implicit "this line's shape means the
region started here." A dumb line-scanner — a regex, an editor
extension, `grep` — can then carve up a rendered diagnostic at *any*
nesting depth by matching open/close pairs alone, no
indentation-sniffing, no real parser needed. This matters today
because every consumer (the pipeline dashboard, this very document's
worked example) is working from captured plain text, and it matters
later because it's the same shape an LSP client will eventually want
to fold in an editor:

- **`>>>` / `<<<`** — wraps the whole diagnostic. `>>>` opens the
  `error[CODE]: message` line itself; `<<<` is the diagnostic's last
  line, alone, closing it.
- **`~>` / `<~`** — wraps each individual *supplementary* block: a
  `note` (points at a secondary span) or a `help` (plain suggestion,
  no span). Every `~>` has exactly one matching `<~`, even a one-line
  `help` with no nested span block of its own — no "sometimes
  symmetric" exception to remember or special-case in a parser.

The one thing deliberately **not** wrapped in its own marker pair is
the primary span block (`-->`/`^^^^`, right after the `>>> error[...]`
line) — that's the one fact a fold-aware reader should never be able
to collapse away. Only the `~>`/`<~` blocks are meant to fold
independently of it and of each other.

No warning severity exists yet (every current error type is, well, an
error), so `warning[...]` is aspirational — but the marker scheme
already accounts for it: `>>> warning[...]` / `<<<` works identically,
nothing about `~>`/`<~` needs to change when warnings are added.

When real LSP support lands, `Diagnostic` (the struct `render()`
consumes) maps close to 1:1 onto LSP's own `Diagnostic` type — `code`,
`message`, `range` from `primary_span`, `relatedInformation` from
`secondary`. These text markers are the interim, file-based version of
the same split, not a dead end that gets thrown away once an LSP
exists.

---

## 6. Plain text by design — no ANSI, no color, ever, in `render()`

`render()` never emits an escape code. Two reasons, both load-bearing:

1. `diagnose.rs`'s output is always captured to a file
   (`results/<fixture>.txt`) and re-displayed inside an HTML `<pre>`
   block by `scripts/build_dashboard_report.py` — the same dashboard
   the pipeline results screenshot in this project's history came
   from. An embedded escape code shows up there as literal `\x1b[31m`
   garbage, not color.
2. One renderer, one behavior, everywhere it's called from beats a
   renderer with a color flag that's right in one caller and wrong in
   another. `error_manager.rs::report_all` — the legacy `crates/parser`
   CLI's error path, the one place a human might plausibly be looking
   at a real terminal — prints this exact same plain output for that
   reason, not because color wouldn't be nice there too.

If a genuinely interactive terminal consumer shows up later (a REPL),
it colorizes by wrapping `render()`'s plain output line-by-line, not by
teaching `render()` a second mode. Keep the one renderer honest.

---

## 7. Known limitation: multi-line spans

`Span` (`crates/core/src/lexer/token.rs`) tracks `start`/`end` as byte
offsets plus a single `line`/`column` pair for the **start** position
only — there is no `end_line`/`end_column`. For the overwhelming
majority of spans (an identifier, an operator, most expressions) this
is irrelevant since they don't cross a line boundary. For a span that
genuinely does — or, as happened in the worked example in §1, a span
that's simply wider than intended and runs past the end of its line —
`render_span_block` clamps the underline to however many characters
are actually left on that one source line, rather than either
panicking on an out-of-bounds slice or drawing a nonsensically long
underline that wraps visual garbage onto the next line.

You can see this clamping in the §1 example directly: `x`'s
`DuplicateDefinition` span runs a little past the end of `let x = 2`
(onto the trailing comment / towards the next statement), and the
underline visibly stops at the end of line 7 instead of continuing
past it. That's the clamp working as intended, not a rendering bug —
but it's also a sign the span itself is wider than it needs to be.
Tightening spans to their true minimal extent is a separate, lower-
priority cleanup from anything in this document; this section exists
so nobody re-discovers the clamping from scratch and "fixes" it into a
crash.

**If `Span` ever grows `end_line`/`end_column`:** `render_span_block`
is the one function that needs to learn to print multiple source lines
instead of clamping to one. Nothing else in `render.rs` assumes
single-line spans.

---

## 8. A bug this document's own worked examples caught

While building the renderer described in this document and actually
looking at real rendered output instead of trusting the underlying
data, a real, pre-existing bug turned up: `logos_lexer.rs` used
`#[logos(skip r"[ \t]+")]` to discard whitespace between tokens. A
`skip` match in `logos` is consumed with **no callback at all** — so
`update_position` (the function that advances `self.column`) never ran
for it. Every token's reported column silently undercounted by however
much whitespace had been skipped since the last real token, compounding
across a line. A token 8 spaces into an indented line was reported at
column 1, not column 9; a token in `    let mut outer` was reported at
column 7, not 13.

This meant that before this fix, **every column in every diagnostic the
compiler had ever produced was potentially wrong** — the "arrow points
at the exact wrong place" failure mode this whole document exists to
prevent, hiding underneath a rendering layer that would have drawn a
perfectly formatted, perfectly confident underline in the wrong spot.

Fixed by making whitespace an explicit, position-tracked token
(`#[regex(r"[ \t]+")] Whitespace`) discarded in `handle_logos_token`
right alongside `Newline`/`LineComment` — same discard behavior, but
`update_position` runs first. If you ever add another `#[logos(skip
...)]` directive to this lexer, ask whether it can *ever* match
something with nonzero width before adding it — if it can, it needs
this same treatment, not `skip`.

---

A second instance of the same lesson turned up independently, this
time in `type_table.rs`'s `SemaType::display()` — the function every
error variant in the registry above calls to render a type into its
`found` / `expected` / `on_type` / `escaped_type` fields. It explicitly
handled a dozen or so variants and fell through a `_ => "<type>".into()`
catch-all for everything else: `Dictionary`, `Queue`, `Stack`, `Set`,
`Tuple`, `Array`, `Slice`, every fixed-width numeric alias, `GcRef`,
`OwnedRef`, `Named` (**every user struct and enum**), and `Function`
(**every closure**). None of it errored or panicked — it silently
rendered the literal four-character string `<type>` in place of the
real type, in any diagnostic that happened to involve one of those.

Caught the same way as the lexer bug above: someone actually reading a
rendered diagnostic instead of trusting that the underlying data must
be right. `err_arena_escapes_closure_capture.ubl`'s
`ArenaRefEscapesBoundary` message read `&arena <type>` where it should
have read `&arena fn() int` — unhelpful, and indistinguishable at a
glance from a genuine "unknown type" versus a renderer gap. Every other
existing fixture up to that point only ever escaped, mismatched, or
called a missing method on a `List<T>`, so the gap had never been
exercised.

Fixed by writing out every `SemaType` variant explicitly — no catch-all
left in the match, so adding a new `SemaType` variant without also
handling its display is now a hard compile error, not a silent gap.
`Named` needed `SymbolTable` access to resolve a `DefId` back to its
declared name, so `display()`'s signature grew a `symbols: &SymbolTable`
parameter; both call sites (`type_infer.rs`'s `display_type` helper,
`tier_check.rs`'s `MidReturnContainsArenaRef` check) were updated to
pass it through. Composite and collection types render using this
language's actual surface syntax, pulled from `ast/types.rs`'s own doc
comments rather than guessed: tuples as `(int, string)`, arrays as
`[4]int`, slices as `[]int`, function types as `fn(int, int) bool`.

If you ever add a new `SemaType` variant, the compiler will now force
you to give it a `display()` arm in the same change — but double-check
the string you write actually matches this language's surface syntax
(`ast/types.rs`) rather than defaulting to a Rust-looking guess.

---

## 9. Physical folder reorganization

**Status: implemented.** What follows was originally written as a
proposal for review before anyone spent an afternoon executing it.
It's kept here as the record of what was decided and why — the
"before" state stays for context, and three corrections against the
original proposal are called out inline, same spirit as §8.

### What existed before this reorg

```
error_management/
├── error_manager.rs      (ErrorManager — accumulator, delegates to render.rs)
├── logger.rs             (Logger — plain leveled logging, unrelated to diagnostics)
├── render.rs             (THE renderer — Diagnostic, Diagnosable, render())
└── error_types/
    ├── lexical_error.rs  (LEX-0xx)
    ├── parse_error.rs    (PARSE-0xx)
    ├── name_error.rs     (NAME-0xx)
    └── type_error.rs     (TYPE-1xx ordinary + TYPE-2xx tier/arena, one enum)
```

This was already organized by *phase*, which is why the Error Code
Registry (§4) numbers by phase too. What it wasn't organized by is
*class within a phase* — "errors about dynamic naming" and "errors
about imports" were both just cases inside one flat `NameError` enum,
for instance. Per-class reference docs (see point 3 below) would need
that finer split to have a natural home; they still don't exist, so
the finer split hasn't been exercised yet either.

### The actual shape

```
error_management/
├── error_manager.rs      (same path; gained a tier_errors bucket)
├── logger.rs             (untouched)
├── render.rs             (untouched)
└── errors/
    ├── mod.rs             (aggregator: pub mod + re-exports, all five)
    ├── lexical/mod.rs      (enum LexicalError — unchanged variants/codes)
    ├── parse/mod.rs        (enum ParseError — unchanged variants/codes)
    ├── naming/mod.rs         <- renamed from name_error.rs
    │                          (enum NameError — unchanged variants/codes)
    ├── types/mod.rs         (TYPE-1xx variants only)
    └── tier/mod.rs           <- physically split out of TypeError
                              (TYPE-2xx variants, renumbered TIER-0xx)
```

Corrections against the original proposal, found during execution:

1. **Variant names were kept exactly as they were, not shortened.**
   The proposal suggested `TierError::RefEscapesBoundary` (dropping
   the redundant "Arena"). What actually shipped is
   `TierError::ArenaRefEscapesBoundary`, unchanged from its
   `TypeError` days — every other variant in `TierError` already
   names its condition without an "Arena" prefix problem
   (`AwaitInWrongTier`, `IllegalTierCall`, ...), so shortening only
   this one variant for a redundant prefix its siblings don't share
   would have been inconsistent for no real gain. `tier/mod.rs`'s own
   module doc records this.
2. **The numeric tail was not "kept stable" the way the original
   wording implied.** `TYPE-201..207` became `TIER-001..007`
   (renumbered from 1), not `TYPE-201` → `TIER-201`. §4's "never
   renumber an existing code" rule is about not renumbering *after* a
   code is genuinely settled and grepped-for externally — nothing had
   shipped with a `TYPE-2xx` code outside this repo, so starting the
   new `TIER-0xx` family at 001 was the cleaner outcome, and it's
   what's actually in `tier/mod.rs` today.
3. **`diagnose.rs` and `pipeline.rs` silently dropped every tier
   diagnostic** until this was caught mid-session and fixed.
   `ErrorManager` correctly grew a `take_tier_errors()` bucket
   alongside `take_type_errors()`, and `tests.rs` correctly drained
   it — but both example binaries that walk error output only called
   `take_name_errors()` / `take_type_errors()`, so a real sema failure
   (correctly reported as `status: SEMA ERROR`) rendered *no*
   diagnostic text underneath it at all. Nothing failed to compile —
   `take_type_errors()` is a perfectly valid call on its own, so the
   type checker had no way to flag the missing `take_tier_errors()`
   call. Same shape as §8's bug: correct-looking, confidently-printed
   output hiding a real gap underneath. Fix was one line in each file;
   the lesson is now point 4 of "adding a new error class" below.

Per-class reference docs (`numbers.rs`, `undefined.rs`, etc. from the
original proposal) were **not** written as part of this pass — see
"Full reference" in §4 for the lighter stand-in that was built
instead, and the note below on why the full version stays deferred.

### Adding a new error class

The checklist in §4 ("Adding a new variant") is for extending an
*existing* class. Adding a whole new one — a `RuntimeError` for the
interpreter, say — is a different, slightly bigger list. Not needed
in practice yet, but worth writing down now that there's a real
second example (`TierError`) to generalize from:

1. New file: `errors/<name>/mod.rs`. Own enum, own `impl` with
   `span()`/`message()`/`suggestion()`, own `Display`, own
   `impl Diagnosable` with a fresh code prefix (pick the next unused
   short phase tag — `LEX`/`PARSE`/`NAME`/`TYPE`/`TIER` are taken).
2. `errors/mod.rs`: add `pub mod <name>;` and re-export the enum.
3. `ErrorManager` (`error_manager.rs`): add the `Vec<NewError>` field,
   `add_<name>_error`, `<name>_error_count`, `take_<name>_errors`, and
   wire it into `has_errors`, `total_errors`, and `report_all`'s
   section list — `TierError`'s addition is the template to copy.
4. **Grep every existing `take_<class>_errors()` call site**
   (`diagnose.rs`, `pipeline.rs`, `tests.rs`, and anywhere else that
   walks an error result) and add the new one. This is the step point
   3 above proves gets missed if it isn't a deliberate checklist item
   — nothing forces it at compile time.
5. §4: add the code table row and a "Full reference" entry in the
   same change.

Whether/when to build the deferred full per-class reference pages
(rustc `--explain`-style, one full worked example per code) is still
an open call — the folder-layout blocker that deferred them is gone
now, but writing ~33 of them is real, separate effort from anything
in this pass.
