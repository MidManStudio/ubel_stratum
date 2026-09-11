# Ubel Stratum — Memory Model & Tier Rules

> **Canonical reference for how HIGH / MID / LOW tiers govern data structures,
> allocation, and references.**
> Spans `crates/core/src/sema/tier_check.rs`, `crates/core/src/sema/type_infer.rs`,
> `crates/core/src/sema/type_table.rs`, and any interpreter code that touches
> collections or allocators. Every contributor reads this before touching
> tier-related code.
>
> **Status legend used throughout this document:**
> - ✅ **IMPLEMENTED** — true in the codebase today, verified by direct inspection.
> - ⚠️ **GAP** — designed/expected but not enforced yet; a known hole.
> - 🔭 **PROPOSED** — a rule we are adopting going forward; no code exists for it yet.
>
> Reflects repo state as of commit `6d65dcb` (fresh clone) **plus this
> session's Gap 2 changes to `type_infer.rs`/`tier_check.rs`/`tests.rs`,
> not yet pushed as of this writing.** Re-verify against `git log` before
> relying on any ✅ claim in this doc after significant sema changes.

---

## 1. The Three Tiers, and What Actually Backs Them

| Tier | Sema reference kind | Backing model | Status |
|---|---|---|---|
| **HIGH** | `GcRef(TypeId)` | Shared, `Rc<RefCell<...>>`-style GC aliasing, no lifetime | ✅ IMPLEMENTED |
| **MID** | `ArenaRef { arena: ArenaId, mutable: bool, inner: TypeId }` | Bump-style allocation, scoped to a specific `with arena(...)` block | ⚠️ PARTIAL — type-level only, see §5–§6 |
| **LOW** | `OwnedRef { mutable: bool, inner: TypeId }` | Move semantics + borrow checker | ❌ NOT STARTED — codebase's own comment marks this "Phase 4" |

These three variants already exist in `type_table.rs` and are the correct shape —
this document is about closing the gaps around `ArenaRef`, not redesigning the
reference kinds themselves.

---

## 2. Rule: Tier Lives in the Reference, Never in the Type Name

`List<T>`, `Dictionary<K,V>`, `Queue<T>`, `Stack<T>`, and any future `Pool<T>`
are **tier-agnostic shapes**. A given instance's tier is determined entirely by
which reference kind wraps it (`GcRef<List<T>>` / `ArenaRef<List<T>>` /
`OwnedRef<List<T>>`), which is in turn determined by *where the value was
constructed* — not by anything written in the type annotation itself.

```
let a = List.new()                    // GcRef<List<T>> — default, HIGH
with arena(1MB) {
    let b = List.new()                // ArenaRef<List<T>> — MID, tagged to this arena
}
```

### Why this shape, and not Rust's

Rust bakes the allocator into the container's type parameter —
`Vec<T, A: Allocator = Global>`, backed by `RawVec<T, A>` — so `Vec<T, Global>`
and `Vec<T, MyArena>` are distinct monomorphized types. That buys zero-cost
static dispatch at the price of type proliferation (any function generic over
"a Vec" must also be generic over `A`).

Zig instead keeps one concrete container shape (`ArrayList(T)`) and passes the
allocator as an explicit runtime value — closer to what we're doing. The one
important refinement Zig's design surfaces that ours is still missing:
**Zig's managed `ArrayList` stores its allocator internally**, while the
`Unmanaged` variant requires passing it to every call. See §7 — our current
`ArenaRef` is sema-only and erased at runtime, which is the "Unmanaged"
problem without even the escape hatch of passing the arena in by hand.

**Decision:** stick with tagging the reference, not parameterizing the
container type. This matches the existing `List<T>`-reads-the-same-everywhere
goal. Documented here so it's a deliberate choice, not an unexamined default.

---

## 3. `ArenaId` Scoping — Nested Arenas Are Not Interchangeable

✅ IMPLEMENTED: each `with arena(...)` block gets its own `ArenaId` via
`push_arena()` / `pop_arena()` on an `arena_stack`. Two nested arena blocks
produce two distinct ids, even though both are "MID tier."

**Rule:** any check involving arena membership must compare the *specific*
`ArenaId`, never just "is/isn't tagged `ArenaRef`." A value tagged with arena
`#1` assigned somewhere only arena `#2` is valid in must still be rejected —
both are MID, neither is interchangeable with the other.

```
with arena(1MB) {                  // ArenaId(1)
    let outer_list = List.new()
    with arena(1MB) {              // ArenaId(2) — nested, distinct
        outer_list = List.new()    // WRONG TIER: ArenaId(2) value assigned
                                    // into an ArenaId(1)-scoped binding
    }
}
```

---

## 4. `contains_arena_ref` — How Tier-ness Propagates Through Generics

✅ IMPLEMENTED in `type_table.rs`. Recursively walks `List`, `Set`, `Queue`,
`Stack`, `Dictionary`, `Tuple`, `Slice`, `Fallible`, `Task`, `Optional`,
`Named` (user generics), and `Function` types looking for a buried
`ArenaRef`. This is the actual "arena coloring" mechanism — it's how
`List<SomeStructContainingAnArenaRef>` is correctly detected as
arena-tainted even though the taint is nested two levels deep.

No changes proposed here — this part is solid. Referenced by name in later
sections because §6 and §8 depend on it.

---

## 5. Construction-Site Tagging — GAP #1

✅ **IMPLEMENTED.** `maybe_arena_ref` — the function that wraps a
freshly-constructed value's type in `ArenaRef` when inference is currently
inside a `with arena(...)` block — was originally only called from four
places in `type_infer.rs`: array-literal, tuple-literal, dict-literal, and
struct-literal inference. It was never called from the `Call`-expression
inference path, so `List.new()` and friends type-checked as `Unknown` and
silently skipped arena tagging entirely (see history below for the original
writeup).

**Fixed via `InferCtx::builtin_constructor_type`**, a small helper matching
on the namespace name (`"List"`, `"Dictionary"`, `"Queue"`, `"Stack"` —
mirrors `constructors.rs` / `BUILTIN_NAMESPACES` exactly) that produces the
correct fresh `SemaType` for each constructor. The `ExprKind::Call` arm now
recognizes the `Field { target: Ident(ns), field: "new" }` shape, calls this
helper, and — if it matches — returns `maybe_arena_ref(ctor_ty)` instead of
falling through to the generic (still-`Unknown`-producing) call path.
General field/method resolution is untouched; this is narrowly scoped to
recognized constructor calls only.

```
with arena(1MB) {
    let a = [1, 2, 3]      // ✅ tagged ArenaRef — literal path (already worked)
    let b = List.new()     // ✅ tagged ArenaRef — call path (this fix)
}
let c = List.new()          // ✅ stays a plain List — no arena in scope
```

**Verified**, not just claimed: `cargo check --lib` clean (zero new warnings),
full `cargo test --lib -p ubel_stratum` — 54/54 passing, including 5 new
tests added to `sema/tests.rs` (`test_sema_list_new_in_arena_is_arena_ref`
and siblings for `Dictionary`/`Queue`/`Stack`, plus a negative case
confirming `List.new()` stays untagged outside any arena). This was also the
first test coverage arena coloring had at all — previously zero tests
touched `ArenaRef`, `maybe_arena_ref`, or any constructor call.

**Still open, deliberately out of scope for this fix:** the `.with_capacity()`
/ `.growable()` chain convention from `BUILTINS_RULES.md` §2 doesn't exist
yet as actual instance methods — those will hit the *same* "no field table"
problem this fix worked around for `.new()` specifically, but for arbitrary
instance method calls on an already-typed receiver. That's a separate,
larger piece of work (general method resolution), not folded into this fix.

---

## 6. Escape Boundary Enforcement — GAP #2

✅ **IMPLEMENTED.** `ArenaRefEscapesBoundary` was a fully defined error
variant — `Display` impl, span handling, everything — but was never
constructed anywhere in the codebase. Only one escape path was caught at
all, and only by accident:

- Return position was rejected, but via a generic `TypeMismatch` (declared
  `List<int>` vs. actual `&arena List<int>`) — the *dedicated*
  `MidReturnContainsArenaRef` rule checks the function's **declared**
  return type for an `ArenaRef` marker, and there is no surface syntax for
  writing `ArenaRef` in a type annotation at all, so that rule can
  structurally never fire. `TypeMismatch` was doing the real work, for the
  wrong stated reason.

Three more escape paths caught nothing at all:

- Assignment to an outer-scope binding
- Storage into a struct field on a struct that isn't itself arena-scoped
- Capture by a closure that can outlive the block

**Fixed entirely in `type_infer.rs`'s Pass 2** — not in `tier_check.rs`,
even though that file's own header comment used to claim the job. Pass 2
already tracks live arena scope via `arena_stack`/`maybe_arena_ref`, so
that's where the answer is cheapest to compute, right as each site is
inferred:

- **`InferCtx::check_assign_arena_escape`**, called from the `Assign`
  expression arm, computes each side's **specific** `ArenaId` (never just
  "tagged or not" — see §3) and compares them:
  - `Ident` target — the arena (if any) baked into the binding's
    *original* declared type. Reassignment never mutates `def_types`, so
    re-reading it always reflects the declaration site.
  - `Field { target: receiver, .. }` target — the receiver's own
    top-level `ArenaId`, if the receiver itself is arena-tagged (storing a
    same-arena value into one of its own fields is safe — both die
    together). No struct field can be *declared* `ArenaRef` (no surface
    syntax exists for it), so an untagged receiver's home arena is always
    `None`, making any arena-tagged value assigned into one of its fields
    unconditionally an escape.
  - `Index { target: container, .. }` target — same idea, using the
    indexed container's own top-level `ArenaId`.
- **`structurally_compatible`** gained a same-arena-required
  `ArenaRef`↔`ArenaRef` arm. This was also a real, previously-undetected
  bug in its own right: with no arm for the pair at all, two `ArenaRef`s
  with the **identical** `ArenaId` were never recognized as compatible
  either, so ordinary, safe same-arena reassignment
  (`with arena(1MB) { let mut x = List.new(); x = List.new() }`) would
  have spuriously failed with a generic `TypeMismatch` the moment anyone
  wrote it.
- **`unify`** now prefers `ArenaRefEscapesBoundary` over the generic
  `TypeMismatch` fallback whenever either side of a failed unification
  carries the tag — this is what upgrades the return-position diagnostic
  "for free," with no changes needed to `tier_check.rs` at all.
- **Closures** — every lambda built inside a `with arena(...)` block is
  now conservatively `maybe_arena_ref`-tagged too, the same as any other
  constructed value. Lexical scoping guarantees a lambda can only
  reference an arena-scoped local from inside that local's own block, so
  this over-approximates (a closure that captures nothing arena-related
  still gets tagged) but never under-approximates — once tagged, the
  lambda value rides the exact same assignment/field/return checks as any
  other arena-scoped value. No separate free-variable capture analysis
  was needed.

```
let mut outer = List.new()          // ❌ was uncaught, now rejected
with arena(1MB) {
    outer = List.new()              // ArenaRefEscapesBoundary
}

with arena(1MB) {                   // ArenaId(1)
    let mut a = List.new()
    with arena(1MB) {               // ArenaId(2) — distinct arena
        a = List.new()              // ❌ was uncaught, now rejected
    }                                // (compared by specific id, per §3 —
}                                    //  both are "tagged", but not the same one)

with arena(1MB) {
    let mut a = List.new()
    a = List.new()                  // ✅ same arena both times — legal,
}                                    //    and no longer spuriously rejected
                                     //    by the structurally_compatible fix

struct Holder { data: List<int> }
let holder = Holder { data = List.new() }
with arena(1MB) {
    holder.data = List.new()        // ❌ was uncaught, now rejected
}
```

**Verified**, not just claimed: `cargo check --lib` clean, full
`cargo test --lib -p ubel_stratum` — 56/56 passing (54 prior + 2 new:
`test_sema_arena_escape_outer_binding_rejected` and
`test_sema_arena_same_arena_reassign_ok`, the latter a regression guard
for the `structurally_compatible` fix specifically). Four new end-to-end
fixtures added and confirmed failing sema exactly as intended
(`err_arena_escapes_outer_binding.ubl`, `err_arena_escapes_struct_field.ubl`,
`err_arena_escapes_nested_mismatch.ubl`, `err_arena_escapes_closure_capture.ubl`),
plus one new fixture confirming the legitimate same-arena case still
passes end to end (`ok_arena_same_arena_reassign.ubl`). All 19
pre-existing fixtures re-verified unaffected.
`err_mid_fn_returns_arena_value.ubl`'s header comment updated — it now
documents (and the fixture confirms) that it fails via the dedicated
`ArenaRefEscapesBoundary`, not the old accidental `TypeMismatch`.

**Still open, deliberately out of scope for this fix:** the escape check
is deliberately conservative rather than precise — it has no liveness
analysis, so it can flag a pattern that's arguably safe if the escaped
value is provably never read again (see the code comment on
`check_assign_arena_escape` for the specific untyped-parameter case this
affects). Over-flagging a borderline-safe pattern is the right default
for a memory-safety check; a real borrow checker makes the same tradeoff.
Passing an arena-tagged value as an ordinary function **argument** is
still unenforced — not one of the three escape paths this gap targeted,
and there's a separate, pre-existing gap that call arguments aren't
type-checked against declared parameter types at all yet (see §12/open
gaps list in the handover notes), so there's currently no mechanism by
which an argument's tag could propagate into the callee's own reasoning
regardless.

---

## 7. Runtime Representation — What's Real Now, What's Deferred

**🔭 PROPOSED, explicit decision:** sema (§2–§6, §8) fully enforces the tier
*contract*. The tree-walking interpreter is permitted to keep sharing one
Rust-heap-backed `Value` representation across all three tiers for now — no
`Value::ArenaList` yet. Physical memory-layout divergence (real bump
allocation for MID, moved/owned buffers for LOW) is deferred to LLVM
lowering, a phase that doesn't exist yet. Building three parallel runtime
collection engines by hand in the interpreter, before there's an LLVM backend
to benefit from the performance difference, would be wasted motion.

**One forward-looking requirement carried over from the Zig research (§2):**
whenever a MID-tier runtime representation *does* get built, it must carry a
live handle back to its originating arena — not just a type-level tag that
disappears after sema. This is required for §8's "allocate the result into
the same arena as the receiver" rule to be implementable at all. Flagging it
now so it isn't designed away later.

---

## 8. Method Availability Per Tier ("Method Tier Inheritance")

✅ **Existing precedent:** `await` (`ExprKind::Await`) is already
hard-gated to `@tier(high)` functions only, at the expression level.
(LINQ query-comprehension syntax used to be the second example here —
removed in favor of `Linqerizer<T>`, a HIGH-tier-only *type* rather than
special expression-level grammar; see `docs/PARSER_RULES.md` §6.)

✅ **Implemented.** Getting here required more than the two rules below —
builtin instance methods (`myList.push(x)`, `"s".to_upper()`, ...) had *no*
type checking at all beforehand. `ExprKind::Field`'s generic arm had a literal
`// TODO: field table` and always inferred `Unknown`, and
`instance::method_names(ReceiverKind)` — meant to be sema's source of truth
for valid method names — said so itself: "used by sema (once wired in)." It
wasn't. So this section's actual scope ended up being: build real
method-call type checking (`instance::resolve_receiver` / `signature` /
`is_high_only`, wired into `type_infer.rs`'s `ExprKind::Call` handler —
`NoSuchMethod` and `ArgumentCountMismatch` now fire for real, not just at
runtime), *then* the two rules below on top of it.

Lives in `type_infer.rs` (Pass 2), not `tier_check.rs` alongside
await/LINQ's otherwise-identical rule — `tier_check` (Pass 3) has no
`Unifier` of its own, and `expr_types` entries can still be raw unresolved
`Var`s at record time, so resolving a receiver's type correctly needs the
live `apply()` Pass 2 already has. `InferCtx` gained a `current_tier` field
(mirroring `tier_check`'s) for exactly this reason.

**Rule, HIGH-only methods:** a MID- or LOW-tier collection should reject any
HIGH-only method (LINQ-adjacent, anything requiring GC-only semantics) as a
sema error — not silently allow it, and not let it become a runtime
surprise. Mechanism is real and verified (`TierError::MethodInWrongTier`,
`TIER-008`) — confirmed firing correctly by temporarily flagging a real
method HIGH-only and running it from `@tier(mid)`. But every
`HIGH_ONLY` const across all six receiver kinds is empty today: nothing
currently implemented is actually LINQ-adjacent or GC-only (checked all ~43
methods across List/Str/Dict/Tuple/Queue/Stack). So the rule is real
infrastructure, not a stub, but inert until a method that needs it exists.

**Rule, allocation-producing methods:** any instance method that constructs
a *new* collection or value from an existing receiver (`to_upper`,
`to_lower`, `trim`/`trim_start`/`trim_end`, `split`, `replace`, `chars` on
`Str`; `keys`, `values` on `Dictionary`) must allocate that result into the
**same `ArenaId`** as the receiver, not a fresh, untracked allocation —
`instance::MethodReturn::allocates()` marks exactly these, and
`type_infer.rs`'s `method_return_type`/`apply_receiver_wrap` re-wrap the
result in the receiver's own `ReceiverWrap` (`Gc`/`Arena`/`Owned`) rather
than defaulting to untagged. Verified end-to-end: constructing a
`Dictionary` inside `with arena(...)`, calling `.keys()` on it, and
assigning the result to a binding declared outside the block correctly
raises `ArenaRefEscapesBoundary` — proving the tag survived the method
call. Regression-guarded by
`tests/fixtures/err_arena_escapes_via_method_result.ubl`. This is the
concrete tie-in to §7's "arena handle at runtime" requirement — resolved at
the sema/type level only, per §7's own scope; the interpreter still shares
one untyped `Value` representation across tiers.

One further correctness fix fell out of building this for real:
`pop`/`first`/`last`/`dequeue`/`peek` (List/Queue/Stack) and `at`
(Dictionary) all fall back to `Value::Null` on an empty/missing receiver at
runtime (`unwrap_or(Value::Null)` in every case) — so their sema return type
had to become `Optional(elem)`, not bare `elem`, or `x.pop() == null` (the
documented way to check for "empty") would fail a real `TypeMismatch` that
didn't exist before this section had any type checking at all.
`structurally_compatible` gained two matching rules: `Optional<T>` accepts
`Null` unconditionally, and compares directly against a bare `T` (not just
`Optional<T>`) — the latter needed for the many existing call sites like
`items.first() == 10` to keep working.

---

## 9. LOW Tier — Reference Syntax, Loan-Based Borrow Checking, the Ownership-Model Type Axis, and Move Checking All Landed; `outlives`/Lifetime Well-Formedness Landed, Real Enforcement Still Ahead

`OwnedRef` and full move/ownership checking are still marked "Phase 4" in
the codebase's own comments — that part's still true. The reference
*type*, its surface syntax, and loan-based borrow *checking* (the
`&`/`&mut` half of the story) are not; see below.

✅ **Implemented.** A `@tier(low)` function that constructs a builtin
collection (`List.new()`, `Dictionary.new()`, `Queue.new()`, `Stack.new()`)
now produces a clear, explicit sema error instead of silently falling
through to HIGH-tier (`GcRef`) behavior nobody has actually designed for LOW.

Lives in `type_infer.rs`, at the exact point `builtin_constructor_type`
already resolves — right before `maybe_arena_ref` would otherwise hand back
a bare, untagged type. Checks `self.current_tier == TierAnnotation::Low` and
emits `TierError::CollectionConstructionInLowTier` (`TIER-009`) if so. Same
emit-and-continue pattern as `ArenaInWrongTier`/`MethodInWrongTier` —
deliberately *not* returning `Unknown`, so one disallowed constructor call
doesn't cascade spurious follow-on errors through the rest of the function.
Verified via `tests/fixtures/err_collection_new_in_low_tier.ubl`.

Building this fixture's expected error message surfaced an unrelated,
pre-existing bug: `SemaType::display()` (the function every diagnostic uses
to render a type) fell through a silent `_ => "<type>".into()` catch-all for
most non-trivial types — `Dictionary`, `Queue`, `Stack`, `Named` (every
struct/enum), `Function` (every closure), and more. Every existing arena-
escape fixture happened to use `List`, so the gap had never been exercised.
Fixed alongside this section (every `SemaType` variant now has an explicit
arm, no catch-all) since §8's `ArenaRefEscapesBoundary` messages and this
section's own `TIER-009` message both depend on `display()` being honest.
Full case study, including why `Named` needed a new `symbols: &SymbolTable`
parameter threaded through, lives in `DIAGNOSTICS_RULES.md`.

✅ **Implemented — the reference type and its surface syntax.** `&T`/`ref T`
(shared) and `&mut T`/`ref mut T` (mutable), with an optional named
lifetime (`&L T`/`ref L T`), now produce a real `SemaType::Reference {
mutable, lifetime, inner }` — both as a type annotation and as the result
of a new expression-level `Borrow`/`Deref` operator pair (`&x`/`ref x`,
`*x`/`deref x`; see `PARSER_RULES.md` §5.6 for the dual-spelling mechanism).

This replaces a real, previously-silent stub: `TypeKind::Reference` used to
map straight to `SemaType::GcRef`, discarding `mutable` and `lifetime`
entirely — every user-written `&T` compiled as a plain HIGH-tier GC
reference, in any tier. Nothing had exercised the path before (no
expression-level operator existed to construct a value of the type), so it
went uncaught until this delivery added one. Fixed alongside; the new
`Reference` variant is wired into `structurally_compatible` (gated on
`mutable` — lifetime isn't compared yet, see below) and has its own
`unwrap_reference` helper in `type_infer.rs` (mirrors `unwrap_task`) backing
a new diagnostic, `TypeError::DerefOnNonReference` (`TYPE-114`).

`Reference` is deliberately orthogonal to `GcRef`/`ArenaRef`/`OwnedRef`: it
answers "is this binding an alias to a place," not "how is the pointee's
own memory managed." `inner` is just whatever `TypeId` the borrowed place
already had — tier wrapper and all — so this needed no reasoning about tier
interaction to add.

References are valid in **every** tier, not LOW-only — a HIGH/MID function
handing out a read-only view is fine on its own terms. What's still LOW-only
is *enforcement*, and both halves of it are now real:

✅ **Implemented: `outlives`/lifetime declaration well-formedness, the
first slice of the LOW-tier enforcement story's last missing piece.**
The parser has parsed `[lifetime L, lifetime M where M outlives L]` on
functions and `edge struct` for a while, see `PARSER_RULES.md` §5.1's
neighbor content, but nothing in sema consumed any of it until this
delivery. New pass, `sema/lifetime_check.rs` (`LIFETIME-0xx`), runs
right after name resolution, before type inference, since it's purely
structural (no CFG, no type information needed): every lifetime name
in a declaration is declared once each, every name a `where` clause
references is one of them, the declared constraints don't form a cycle
(including the trivial `L outlives L` case), and every `&name`/`ref
name` written directly in that same declaration's own signature or
fields (however deeply nested inside other types) names one of them.

Deliberately scoped to well-formedness only, not real enforcement: it
does not check that usage in a function body actually respects a
declared bound, and it does not look at method bodies or param/return
types at all (`MethodDecl` has no `lifetime_params` of its own, only
an enclosing struct can declare any, and checking a method's own
`&name` usage against its enclosing struct's declared names needs a
scope-inheritance story this pass doesn't build yet). The actual
outlives/subset fixed point remains the borrow checker's job, and
remains unbuilt. Both the loan half (`borrow_check.rs`) and the move
half (`move_check.rs`, described below) are landed and don't need this
to be useful on their own.

One thing worth being explicit about: this isn't only a LOW-tier
enforcement gap. `edge struct` exists specifically for arena-resident
types holding references into the same arena, and checked empirically
before choosing what to build here: marking a struct `edge` with a
matching `[lifetime L]` currently changes *nothing* about how the
existing arena-escape checker (§6) treats it, a plain, non-`edge`
struct storing an arena value into a field is rejected by
`ArenaRefEscapesBoundary` exactly the same way. `is_edge` and
`lifetime_params` were, before this delivery, decorative everywhere in
sema. This slice doesn't change that connection either (§6 still
doesn't consult `is_edge`), that remains real, separate follow-up, but
it's worth being clear that the well-formedness checking added here
doesn't yet make `edge struct` functional for the arena-reference-field
case it was designed for; it only guarantees the *declaration itself*
isn't nonsense.

✅ **Implemented — Phase A, the CFG builder** (`sema/cfg.rs`). Builds a
statement-granularity control-flow graph for one function body: real
branches for `if`/`match` used as statements, real header/body/back-edge/
exit shape for `while`/`for`/`loop`, `break`/`continue` correctly routed
via a loop-context stack, a conservative try/catch approximation, and
`with`/`using`/`unsafe` treated as scope-transparent (no new branching —
`unsafe` was a real gap found while building Phase C below: its body
wasn't being descended into at all, silently treated as one opaque
statement, exactly the kind of thing that matters most for LOW-tier
code specifically. Fixed the same way `with`/`using` already worked).
Explicit, documented scope limits: `if`/`match` used as *expressions*
(including a `then`-keyword single-expr branch) are one opaque unit for
now, not decomposed; no panics on malformed input (`break`/`continue`
outside a loop degrades to `Terminator::Unreachable` rather than
crashing — nothing upstream validates that yet, a real gap, tracked
separately). 6 unit tests in `sema/cfg.rs` (straight-line, if/else-both-
return, if-no-else convergence, while-loop shape, a nested-loop smoke
test, and the `unsafe`-transparency fix). Wired into `sema::analyse` by
Phase D, below — this module only ever built the graph itself.

✅ **Implemented — Phase C, fact collection** (`sema/facts.rs`). Walks
the CFG's statements producing the raw inputs Phase D's fixed point
consumes: every `Borrow` expression becomes a `Loan` (place +
mutability + issue point — found via a hand-written expression walker,
since `AstVisitor` doesn't exist yet, PARSER_RULES.md §10 still marks it
"proposed, future"), every reassignment of a loan's place becomes a
`loan_killed_at` fact, every other conflicting access becomes a
`loan_invalidated_at` candidate. `x = x + 1` is correctly classified as
a *conflict* (it reads the old `x` on its way to overwriting it), not a
clean kill — a real distinction the fact collector gets right, checked
directly by a unit test. Explicit, documented scope limits, same spirit
as `cfg.rs`'s own: **loans only, no move tracking** (`Unique<T>`
semantics need a settled ownership-type story that doesn't exist yet —
building move-checking against it now would mean building it twice);
**places are local-bindings only** — `&x.field`/`&arr[i]` are still
recorded as real loans (nothing vanishes) but as `Place::Unknown`, with
no conflict detection, an under-approximation Phase D must treat as
"insufficient information," never as "safe"; **`loan_invalidated_at` is
a pure syntactic scan, not CFG-reachability-aware** — over-approximate
on purpose, since Phase D needs full liveness for its own propagation
anyway and is the natural place for reachability precision to live. 12
unit tests (7 from the original delivery — single/mutable loans,
kill-via-reassignment, invalidation-via-read, no-false-positives,
multiple loans in one call's argument list proving the walker correctly
descends into `Call` args, and the `x = x + 1` conflict-not-kill
distinction — plus 5 added alongside Phase D for `bound_place`/
`place_defined_at`, below).

✅ **Implemented — Phase D, the liveness/loan fixed point**
(`sema/borrow_check.rs`). Wires `cfg::build` → `facts::collect` → the
actual dataflow into `sema::analyse` as its own pass, for every
`@tier(low)` free function. The real algorithm: a candidate conflict
from `loan_invalidated_at` is only a genuine error if the loan is still
*live* — **reaching** (forward propagation from `issued_at`, stopping at
`loan_killed_at` points and at any point where the loan's own carrier
local gets rebound to something else) **and** its carrier is **live**
(standard backward live-variable dataflow: will that reference-typed
local actually be read again?). This is the literal mechanism behind
real non-lexical-scope behavior: `let p = &mut n; let last = *p; n = 5;`
is accepted because `p`'s last use is strictly before `n = 5` — the loan
is already dead by the time `n` gets touched again. `facts.rs` gained
two small, additive facts to make this well-defined: `Loan::bound_place`
(which local, if any, a borrow is directly bound to — `Place::Unknown`
for a borrow consumed inline, e.g. a bare call argument, which can never
be "live" past its own issue point) and `Facts::place_defined_at`
(general def-site tracking, used both as ordinary liveness's "def" side
and to detect a loan's carrier being rebound). 6 unit tests in
`borrow_check.rs`, including one proving the two-level point graph
correctly crosses an `if`/`else` branch-and-converge boundary, not just
straight-line code — all correct on the first real run. Two new `.ubl`
fixtures exercise the wiring end-to-end:
`err_borrow_conflict_while_live.ubl` (a genuine liveness-gated
violation) and `ok_borrow_dead_after_last_use.ubl` (same shape, but the
carrier's last use comes *before* the conflicting access — proves
liveness-gating, not just candidate detection, is what's actually
running; a naive "any candidate is an error" checker would wrongly
reject it).

Scope, stated plainly, same spirit as `cfg.rs`/`facts.rs` above: **only
mutable loans are checked** — `facts::classify_access` doesn't
distinguish "plain read" from "new borrow" among conflicts, and two
*shared* loans of the same place never actually conflict with each
other, so checking every candidate regardless of mutability would
produce real false positives on valid shared-borrow code (a correctness
bug, not just imprecision, unlike the under-approximations elsewhere in
this checker). The real gap this leaves: a shared loan still live when a
*later* `&mut` of the same place appears elsewhere isn't caught yet —
documented, real, separate follow-up. **Only loans bound to a traceable
local are checked** — see `Loan::bound_place` above. **Intra-statement
conflicts are still out of scope**: `facts::collect` itself excludes a
loan's own `issued_at` point from its invalidation candidates
(`if point == loan.issued_at { continue; }`), so Phase D never even sees
two loans issued at the same point as a candidate — `mixed_signature` in
`tests/fixtures/ok_reference_dual_spelling.ubl` (two shared and two
mutable references to the same variable in one call) still passes today,
unchanged by this delivery, for exactly this reason. A same-point
multi-loan check is a distinct, separate, not-yet-built piece of work,
not something this delivery claims to fix. **Free functions only** —
`Item::Impl`/`Item::Extend` methods aren't walked yet; `cfg::build`/
`facts::collect` only accept `&FunctionDecl` today, and wiring methods
in needs broader plumbing than Phase D's building blocks currently give
it.

Assignment *through* a dereferenced reference (`*p = v`/`deref p = v`) is
also not implemented — it needs a real runtime place representation (a
`Value::Ref(Rc<RefCell<Value>>)`-shaped thing or equivalent) that plain
pass-through evaluation doesn't have. `write_lvalue` in
`interpreter/eval/expr.rs` gives a clear panic for it rather than silently
writing to the wrong place. Reading through a reference works today, and
for `Value::Struct`/`List`/`Dict` — already `Rc<RefCell<_>>`-backed —
aliasing already behaves correctly for free; it's specifically
mutation-through-a-reference for scalar-typed values that's the honest gap.

The actual borrow-checking algorithm — CFG construction, loan/liveness
fixed-point propagation — is landed for the loan half described above.

✅ **Implemented — move-fact collection** (`sema/move_facts.rs`), the
natural next slice once the ownership-model type axis and its
construction syntax landed (both described above) — the exact
prerequisite this section used to say was missing ("building
move-checking against it now would mean building it twice"). Mirrors
`facts.rs`'s own division of labor precisely: pure, cheap, per-function
AST scan, zero fixed-point computation, zero type-checker dependency —
**fact collection only**, same relationship `facts.rs` (Phase C) has to
`borrow_check.rs` (Phase D — and now `move_check.rs`, described just
below, is exactly that consuming phase for the move half).

A `let`-bound local is identified as move-tracked *syntactically* — an
explicit `Unique<...>` type annotation, or an initializer that's
directly a `Unique.new(...)` call, deliberately not by consulting real
type inference, same restraint `Place::Unknown` and `Loan::bound_place`
already use elsewhere in this checker family. A move is any bare
(non-`&`/`&mut`) use of a tracked local's name; the walker adds three
opacity rules on top of `facts.rs`'s own `Lambda`/`Block`/`If`/`Match`
boundary: `Borrow`'s contents are never descended into at all (borrowing
never consumes), an assignment's *target* is never treated as a use
(a definition site, not a read of the old value, matches
`facts::classify_access`'s existing "Reassign, not Conflict"
classification of that exact shape), and a call to a known builtin
instance method never treats its own receiver as a bare use either
(added alongside Open Decision #5's method-dispatch resolution just
above: once `resolve_receiver`/`eval_method_call` made
`Unique<List<int>>.push(5)` legal, the previous blanket "any method
call is a move" rule would have made the type nearly unusable, flagging
every call after the first as a use-after-move). That third rule is
name-based (`instance::is_builtin_instance_method_name`), not
type-based: a same-named user-declared method still falls through to
the general bare-use rule and still counts as a move, a known, narrow,
accepted gap, real follow-up once the language is more mature, not
solved here. Only `let`-bound locals are tracked, not
`Unique<T>`-typed *parameters*, real, separate follow-up. 9 unit tests
(both identification rules, move-vs-borrow, move-vs-reassignment-target,
multiple-candidates-on-one-place, and the `@tier(low)`-only program
walk).

✅ **Implemented — move enforcement** (`sema/move_check.rs`), landed the
same delivery as the fixed point itself, not a separate slice, a
`moved_at` candidate is a real violation (`MoveError::UseAfterMove`,
`MOVE-001`) if it's forward-reachable, same point-level worklist
`borrow_check::compute_reaches_before` already uses for loans, from
*another* candidate for the same place, stopping propagation at any
`facts::place_defined_at` point (a reinitialization). May-analysis, same
philosophy `BORROW-001` uses: reachable on *some* path is enough. The
one design point worth calling out by name: a candidate is allowed to
reach *itself* — that's what catches a value consumed on every loop
iteration without ever being reinitialized in between (one syntactic
move candidate sitting in a loop body, reachable from itself via the
back-edge), unifying the straight-line case and the loop-carried case
into a single rule instead of needing a separate one for loops. When
that's what fired, the diagnostic's `moved_span` and `used_span` point
at the same line — correct, not a bug (`err_move_in_loop_combined.ubl`).
Each violated place is reported once, even if multiple earlier
candidates would independently reach it. Wired into `sema::analyse` as
Pass 5, and into both example tools' error-collection (`pipeline.rs`,
`diagnose.rs`) *from this same delivery* — `BorrowError`'s own history
just above is the reason that wiring didn't get deferred and forgotten
a second time. 8 unit tests (straight-line use-after-move, a lone move
never flagged alone, a borrow between two real moves not interfering,
reinitialization clearing a would-be violation, the loop-carried case,
reinitialization *inside* the loop body clearing it, call-argument
moves, untracked locals never flagged). 4 fixtures — the standing
2-err/2-ok, isolated/combined split (`err_move_after_use.ubl`,
`err_move_in_loop_combined.ubl`, `ok_move_reinit_isolated.ubl`,
`ok_move_combined.ubl`) — these are the fixtures the fact-collection-only
delivery said would come back once there was a real diagnostic to assert
against; here they are.

Scope, today, honestly: only `let`-bound locals, same limit
`move_facts.rs` itself already has — a `Unique<T>`-typed *parameter*
isn't checked, and a value arriving indirectly (returned from another
function, round-tripped through a field) isn't tracked at all. `outlives`
checking (the unrelated lifetime half mentioned above) is still
completely untouched by any of this.

✅ **Implemented — the ownership-model type axis:** `Unique<T>`,
`Shared<T>`, `SyncShared<T>` (`DATASTRUCTURES.md` "Decisions locked in").
Landed as three new `SemaType` variants — `Unique(TypeId)`,
`Shared(TypeId)`, `SyncShared(TypeId)` — deliberately orthogonal to the
tier wrappers (`GcRef`/`ArenaRef`/`OwnedRef`/`PoolRef`), the exact
relationship `Reference` above already has to them: this axis answers
"who owns it and how," not "where does it live." A `Unique<T>` isn't a
tier tag or a place-alias — it composes freely as a struct field, a
return type, or nested inside any of the wrappers above it, same as any
other type.

No new `TypeKind` parser variant was needed. Bare `Identifier<Args>`
parsing already produces `TypeKind::Named { path: ["Unique"], args: [T] }`
for all three names, so `ast_type_to_sema` special-cases them directly —
checked before the `top_level_def` lookup, since these are real compiler
builtins with no user declaration to find (a user who separately declares
`struct Unique<T> { ... }` will find this shadows it — same class of
reserved-name collision as `get`/`set`/`where` being reserved tokens
elsewhere in the language; not diagnosed as a collision yet). Wired into
every exhaustive-match surface that needed it: `display()` (compiler-
forced, no catch-all), `scope_ref_kind` (delegates through `inner`, so a
`Unique<ArenaRef<T>>`-shaped escape is still caught), `substitute()`, and
`structurally_compatible` — the last of these is the actual enforcement
of "new orthogonal axis": each variant only matches its own kind, so
`Unique<Node>` and `Shared<Node>` stay incompatible even though `Node ==
Node`, confirmed via a real `TypeMismatch { expected: "Shared<int>",
found: "Unique<int>" }` diagnostic, not just by construction.

Fixing `substitute()` for these three surfaced an unrelated, pre-existing
gap in the same function: `InlineList`, `Linqerizer`, and `Reference`
were silently missing from it entirely, falling through to the
primitives-only catch-all instead of recursing into `inner` — meaning a
generic struct field of one of those three types would never have its
`Param` placeholder substituted at an instantiation site. Same bug class
`structurally_compatible`'s own Set/Queue/Stack/Pool/Handle fix already
documents (GAP note earlier in this file). Fixed alongside rather than
left sitting next to the fix that would have caught it.

4 unit tests in `sema/tests.rs` (round-trip param/return-type resolution,
`Shared`/`SyncShared` coverage, the cross-variant-mismatch enforcement
above, and arg-count validation reusing `TypeError::GenericArgCountMismatch`
— exactly one type argument, same as every other single-arg generic
wrapper). Two new `.ubl` fixtures:
`ok_unique_shared_syncshared_basic.ubl` and
`err_unique_shared_type_mismatch.ubl`, plus a third,
`err_unique_missing_type_argument.ubl`, whose own header comment
documents a real, separate, pre-existing sema characteristic it happened
to be the first fixture to surface: a function/method parameter's type
annotation is resolved twice, independently, from the raw AST — once in
`collect_fn_sig`/`collect_method_sig` (signature collection) and again in
`seed_param` (body-seeding) — so a param-level type error reports twice.
Harmless when the type is well-formed (every existing arity-mismatch
fixture before this one used a `let`-binding annotation instead of a
parameter, which only resolves once). Not fixed here — real, but out of
this delivery's scope; tracked in §12.

**What §9's type-axis delivery didn't cover, since resolved below:**
construction syntax (✅ now landed, see immediately below) and the tier
restriction question (✅ resolved — Open Decision #5). `resolve_receiver`
still does **not** strip a `Unique`/`Shared`/`SyncShared` wrapper to find
a receiver kind underneath (e.g. whether `Unique<List<int>>.push(5)`
should even be legal is still an open method-dispatch question) — falls
through to its existing `None` catch-all, same as any other unrecognized
receiver shape. Move *enforcement* — the actual borrow-checker pass that
would reject a used-after-moved `Unique<T>` — is still unbuilt; this
section only lands the type and, now, its construction — the eventual
move checker still needs to be built to actually read either.

✅ **Implemented — construction syntax:** `Unique.new(value)`,
`Shared.new(value)`, `SyncShared.new(value)` — the same `Namespace.new()`
idiom every other builtin wrapper in this language already uses
(`List.new()`, `Pool.new()`, etc.), so no parser changes were needed at
all; bare `Identifier<Args>`/`Namespace.method(args)` parsing already
produced the right shape. Unlike `List.new()`/`Dictionary.new()` (which
take no argument and get a fresh type variable resolved later by
unification), these three take exactly one *required* argument and its
own inferred type directly *is* `T` — no fresh var, no later unification
needed. Wrong argument count reuses the existing
`TypeError::ArgumentCountMismatch` (`TYPE-102`), not a bespoke error.

**Tier restriction (Open Decision #5, resolved):** construction is
banned everywhere *except* `@tier(low)` — a new
`TierError::OwnershipWrapperOutsideLowTier` (`TIER-014`). This is the
deliberate *inverse* of `CollectionConstructionInLowTier`
(`TIER-009`, §11): `List`/`Dictionary`/`Queue`/`Stack` are HIGH-tier's
(GC-default) story and are banned *inside* LOW tier because LOW had no
memory model of its own to fall back on; `Unique`/`Shared`/`SyncShared`
now *are* that memory model, so by the same "each tier's own allocation
strategy stays within that tier by default" logic, construction is
banned everywhere else. A HIGH/MID-tier function may still *receive* a
`Unique<T>` value as a parameter and read it (same as it may already
receive an `ArenaRef`-wrapped value) — only *constructing* a new one is
restricted.

**Runtime representation:** three new `Value` variants.
`Unique(Box<Value>)` — plain `Box`, not `Rc`: Rust's `Box` is itself
non-aliasable, so "cloning" a `Value::Unique` deep-clones the payload
rather than sharing it, the honest behavior for a strict single-owner
type even before move enforcement exists to make that matter.
`Shared(Rc<RefCell<Value>>)` — genuine aliasing, same backing
`List`/`Dict`/`Queue`/`Stack`/`Struct` already use.
`SyncShared(Rc<RefCell<Value>>)` — **not** actually `Arc<Mutex<_>>` yet;
`Value` overall isn't `Send` (`List`/`Dict`/etc. use `Rc` internally), so
an `Arc<Mutex<_>>` wrapper on just this one variant would be
structurally real but semantically hollow — nothing could actually send
one across a thread today regardless of this variant's own backing.
Kept as its own distinct `Value` variant anyway (not unified with
`Shared`) so diagnostics and any future real thread-safety story have
something distinct to point at. A genuine `Send`-safe story needs either
a `Value` representation that's actually `Send`, or waits for native
codegen, where this tier's semantics can be enforced by the type system
instead of the tree-walking interpreter.

Fixing `Value::equals()` for `Unique` surfaced the same "new variant
needs wiring everywhere" pattern one more time: it has a catch-all
(`_ => false`), so without an explicit arm every `Unique`/`Shared`/
`SyncShared` comparison — even against itself — would have silently
returned `false`. Fixed: `Unique` compares *by value* (`a.equals(b)`,
matching real Rust's own `Box<T>: PartialEq` — `Box::new(5) ==
Box::new(5)` is `true` — not the "heap types: pointer equality"
convention the rest of this match uses, since that convention exists
*because* those types are aliased/shared and `Unique` is deliberately
the one wrapper here that isn't). `Shared`/`SyncShared` use `Rc::ptr_eq`,
matching every other `Rc`-backed variant.

4 new unit tests, 3 new `.ubl` fixtures
(`ok_unique_shared_syncshared_construction.ubl`,
`err_unique_construction_outside_low_tier.ubl`,
`err_unique_new_wrong_arg_count.ubl`). One of the four unit tests
surfaced a real, general hazard of this test file's `Z`-span convention
(documented on `span_n`'s own doc comment in `sema/tests.rs`): both
`name_resolution`'s `resolutions` map and `type_infer`'s `expr_types`
map are keyed purely by `Span`, and every hand-built AST node sharing
the same zero `Span` constant is invisible right up until a single test
has more than one *meaningfully distinct* identifier reference — at
which point the later one silently overwrites the earlier one's slot in
both maps. Not a product bug; a test-authoring hazard, now documented
and given a `span_n(n)` escape hatch.

---

## 10. `Pool<T>` — Implemented

✅ **Implemented**, on top of the design principles below (kept as-is since
they're still the accurate constraints — this section now also records what
actually got built against them, and where it deliberately stops short).

1. **Tier-agnostic name, tier via wrapper** — built as designed. `Pool<T>` is
   `SemaType::Pool(elem_ty)`; `PoolRef { pool: PoolId, mutable, inner }` is
   the separate scope-tag wrapper, structurally identical to `ArenaRef` but
   kept as its own variant so diagnostics say "&pool"/"with pool<...>(...)"
   accurately instead of reusing arena's wording for a different block kind.
2. **Fixed capacity by default; `.growable()` — built.** `with pool<T>(count)`
   evaluates `count` once at block entry (must unify against `int`) and
   allocates exactly that many slots as the first *block* — `.growable()`
   opts into block-chained growth on exhaustion (a fresh `count`-sized
   block appended, existing blocks never reallocated/copied — the actual
   Hive-shaped property; `PoolData` absorbed this rather than a separate
   `Hive<T>` type, DATASTRUCTURES.md §1) instead of `.acquire()` returning
   `null`. `Handle`s stay valid across a grow (flat, cross-block indices;
   nothing moves).
3. **LIFO free list by default; `.fifo()` — built.** `PoolData.free_list` is
   a `VecDeque<usize>` (not a plain `Vec` any more — needed O(1) pops from
   either end); release always pushes to the back, LIFO (default) pops the
   back too (most-recently-freed reused first), `.fifo()` opts into popping
   the front instead (oldest-freed reused first). Reuse-order correctness
   is proved directly against `PoolData` in `pool_methods.rs`'s own Rust
   unit tests — `Handle` is deliberately opaque at the Ubel-language level
   (no accessor for its raw index), so the exact order isn't something a
   `.ubl` fixture can observe at all; the fixtures
   (`ok_pool_growable.ubl`/`ok_pool_iterate.ubl`/`ok_pool_fifo.ubl`) cover
   what a real program actually can observe instead (growth doesn't fail,
   holes get skipped during iteration, `.fifo()` doesn't break basic
   acquire/release/at).
4. **Generational handles, built — and not optional.** `Value::Handle {
   index, generation }`; every slot carries a `u64` generation counter,
   bumped on release. There is no raw-index opt-out — item 4's "opt-out"
   framing didn't get built either; generational is the only mode in v1
   (§12 below, item 2).
5. **Not `sync.Pool`-style — built as designed.** A slot persists exactly
   until an explicit `.release()` with a matching generation; nothing clears
   it behind the caller's back.
6. **Naming — resolved: unify.** `pool<T>(count)` is the allocator primitive;
   `Pool<T>` (via `Pool.new()`, called inside the `with pool<T>(count) { }`
   block it belongs to) is the value-level handle-issuing manager. Escape
   rule: **same as arena, not exempt** — a `Pool<T>` (and anything acquired
   from it) must not outlive its `with pool<T>(count) { }` block, enforced
   through the exact same mechanism as `ArenaRefEscapesBoundary`, just
   reported through its own `TierError` variant for accurate wording. This
   was a real, deliberate choice over the alternative (exempting handles from
   escape-checking and relying purely on generational safety) — real object
   pools are normally set up once and used for a program's whole lifetime,
   but scoping was chosen for v1 consistency with arena's already-trusted
   model; loosening it later is a smaller change than tightening it would be.

### Method surface

Three methods, on `Pool<T>` itself (not on `Handle<T>`, which is an opaque
`(index, generation)` value with no methods of its own):

- **`.acquire(value: T)`** → `Optional<Handle<T>>`. Stores `value` into a
  free slot; `null` if the pool is exhausted. Takes the value directly
  (rather than reserving an empty slot) since the language has no
  default-value/uninitialized-slot story to reserve *into*.
- **`.release(handle: Handle<T>)`** → `void`. Frees the slot and bumps its
  generation if the handle still matches; a stale or invalid handle is a
  silent no-op — fails safe, doesn't panic, matching `Optional`'s existing
  "checked failure, not memory corruption" philosophy elsewhere in the
  language.
- **`.get(handle: Handle<T>)`** → `Optional<T>`. Reads the slot if the
  generation matches, else `null`. Was named `at` — `get`/`set` were
  reserved lexer tokens for a property-accessor feature that turned out
  to have never actually been implemented (see §12 of
  `docs/NAMING_CONVENTIONS.md`); once that reservation was freed up and
  the accessor keywords renamed to `getter`/`setter`, `get`/`set` moved
  to the collections that actually wanted them all along.
- **`.growable()`** → `void`. Opts into block-chained growth on
  exhaustion — see item 2 above. No-arg, mutates the pool's own flag.
- **`.fifo()`** → `void`. Opts into oldest-freed-first reuse — see item 3
  above. No-arg, mutates the pool's own flag.

`for x in pool { }` also works directly — no `.iter()` method call needed,
matching every other collection here (none of `List`/`Queue`/`Stack` have
one either). Skipfield-style: walks every currently-occupied slot in
index order, holes skipped entirely, via `PoolData::iter_occupied`. Yields
bare `T` values, deliberately not `(Handle<T>, T)` pairs — pairing would
need `for (h, v) in pool { }` to actually type `h`/`v` separately, which
needs per-name destructure-binding typing `record_binding` doesn't do yet
(`BindingTarget::Destructure` records one type for the whole pattern's
span, not a type per bound name — a real, separate, pre-existing gap, not
solved here).

### Where it lives

- `type_table.rs`: `PoolId`, `ScopeKind`, `SemaType::{Pool, Handle, PoolRef}`,
  `display()` arms, `scope_ref_kind` (generalized from `contains_arena_ref`
  to recognize both scope kinds).
- `type_infer.rs`: **the real bug this section's fixture surfaced** —
  `StmtKind::With` used to ignore `allocator` entirely and push an arena
  scope for *every* `with` block, so `with pool<T>(count) { }` (and even
  `with gc { }`/`with heap { }`) silently inherited arena's escape semantics
  whether they wanted them or not. Now dispatches on the actual
  `AllocatorKind`. Also: `pool_stack`/`push_pool`/`pop_pool`/`current_pool`
  mirroring arena's equivalents; `check_assign_arena_escape` and `unify`'s
  mismatch-reporting (renamed `scope_mismatch_side`) generalized to check
  pool escapes using the same comparison algorithm, branching only at the
  final report call so the message stays accurate; a `Pool.new()` special
  case in the constructor call site pulling element type + capacity from
  `current_pool()` rather than inferring fresh like every other constructor.
- `tier_check.rs`: `with pool<T>(count)` requires `@tier(mid)`, same rule and
  same call site as arena's; `check_mid_return_type` generalized alongside
  `scope_ref_kind` to also catch a MID function returning a pool-containing
  type (`MidReturnContainsPoolRef`) — though, like its arena counterpart
  `MidReturnContainsArenaRef`, this is real consulted infrastructure that
  can't currently be *triggered* by any writable fixture, since there's no
  surface syntax yet to write `Pool<T>` as an explicit return-type
  annotation (see "Known gap" below) — the reachable path is the escape-
  boundary check via assignment/reassignment instead.
- `builtins/instance.rs` + new `builtins/instance/pool_methods.rs`:
  `Pool` as a 7th receiver kind (`ReceiverKind::Pool`, `ReceiverWrap::Pool`),
  `MethodReturn::AcquireHandle`, same dual sema-signature + interpreter-
  runtime file pattern every other collection already uses.
- `interpreter/value.rs`: `Value::Pool(Rc<RefCell<PoolData>>)`,
  `Value::Handle { index, generation }`, `PoolData { slots, generations,
  free_list }`.
- `interpreter/eval/{stmt,expr}.rs`: `with pool<T>(count)` evaluates `count`
  once and pushes it onto a small ambient `pool_capacity_stack` on
  `Interpreter` (needed because `Pool.new()` — unlike every other builtin
  constructor — has no generic argument of its own to size itself from, and
  the generic `BuiltinFn` dispatch table has no interpreter-state access at
  all); `Pool.new()` is special-cased in `eval_call_with_receiver` ahead of
  the generic namespace dispatch to read that ambient capacity.

### A real type-system gap this section's own fixture found

`AcquireHandle`'s result needs the escape-boundary wrap applied around the
*whole* `Optional<Handle<T>>` (so the handle itself can't escape) — unlike
every other `Elem`-shaped method (`pop`/`at`/`dequeue`/`peek`), which
deliberately leaves `Optional<T>` bare at the top level. That left `Optional`
one layer inside the `PoolRef` wrapper instead of at the top level, and the
existing `null`-compatibility rule in `structurally_compatible` only
recognized a bare `Optional<_>` — so `pool.acquire(x) == null` failed to
type-check, reported (incorrectly) as `PoolRefEscapesBoundary`. Fixed by
adding `PoolRef`/`ArenaRef`-peeling arms to the null-compatibility check
ahead of the existing bare-`Optional` one, resolving the wrapper's inner type
and checking *that* for `Optional`.

### Known gap — no surface type-annotation syntax yet

`Pool<T>`/`Handle<T>` are inference-only today. `let h = pool.acquire(x)`
works (type inferred); writing `let h: Handle<Entity>` explicitly, or a
function signature like `fn f() Pool<Entity> { ... }`, does not — `Pool`
and `Handle` aren't registered as type-annotation keywords in
`rd_parser`'s type grammar (unlike `List`/`Dictionary`/`Set`/`Queue`/`Stack`,
which each have a dedicated `TypeKind` variant, per PARSER_RULES.md). Adding
that is straightforward (new `TypeKind::Pool`/`TypeKind::Handle` variants +
grammar rules) but is a distinct, additive follow-up, not done here.

### Verified

`tests/fixtures/ok_pool_basic.ubl` — full pipeline (lex → parse → sema →
interpret), exercising acquire/release/get, LIFO reuse of a just-released
slot, exhaustion returning `null`, and — the actual point of generational
handles — a stale handle held past `.release()` provably failing `.get()`
instead of silently reading the slot's next occupant. Plus three sema-fail
fixtures, one per new `TierError` variant reachable today:
`err_pool_wrong_tier.ubl` (`PoolInWrongTier`), `err_pool_escapes_block.ubl`
(`PoolRefEscapesBoundary`, via nested-pool mismatch — the same shape
`err_arena_escapes_nested_mismatch.ubl` uses, and for the same reason: there's
no way to construct a plain untagged `Pool<T>` to reassign into, since
`Pool.new()` always requires an enclosing block, so the reachable escape
shape is two *different* pools rather than one pool vs. none), and
`err_pool_new_outside_block.ubl` (`PoolConstructedOutsideBlock`).
`cargo test --lib`: 58/58 passing, unchanged. Full fixture sweep: 34 total
(30 → 34, four new), 15 sema-ok (was 14) all reaching full interpret, 19
sema-fail all firing their specific intended variant.

---

## Design principles (as originally agreed, kept for reference)

1. **Tier-agnostic name, tier via wrapper** — same rule as §2. `Pool<T>` is
   one shape; `GcRef<Pool<T>>` / `ArenaRef<Pool<T>>` are what differ.
2. **Fixed capacity by default; `.growable()` opts in.** Matches the existing
   `pool<Type>(count)` allocator statement's "count" semantics — no tension
   between the low-level allocator primitive and the high-level collection.
3. **LIFO free list by default; `.fifo()` opts in.** LIFO is the
   least-surprising default (matches most general-purpose object pools:
   .NET's `ObjectPool<T>`, most C++ game pools) — cache-friendly, same slot
   stays hot. FIFO is the deliberate opt-in for delayed-reuse needs (entity-ID
   recycling, avoiding same-frame slot reuse while another system still holds
   a stale reference).
4. **Generational handles by default; raw-index available as an explicit
   opt-out.** A raw-index pool has the classic ABA problem — an old handle
   silently reading a different live object after its slot gets recycled.
   Generational handles (`(index, generation)`, `slotmap`/`generational-arena`
   style) make staleness detectable for the cost of one extra counter per
   slot. This is also the pattern Mid Engine's not-yet-built entity allocator
   will want, so getting it right here is directly reusable there.
5. **Must NOT have `sync.Pool`-style GC-eviction semantics.** Go's `sync.Pool`
   may silently clear entries between GC cycles (softened only by a
   "victim cache" giving roughly one extra cycle) — it exists for fungible,
   re-creatable scratch objects, never for anything with real identity. A
   checked-out or idle-but-reserved slot in Ubel's `Pool<T>` must persist
   deterministically until explicit release or pool teardown. Worth stating
   outright given the name overlap invites the wrong mental model.
6. **Naming** — resolved above: unify with `pool<T>(count)`.

---

## 11. Implementation Order

1. ✅ **Gap 1** (§5) — wire constructor calls through `maybe_arena_ref`. Done,
   verified via `cargo test --lib` (54/54 passing).
2. ✅ **Gap 2** (§6) — general `ArenaRefEscapesBoundary` emission
   (assignment / field / indexed storage / closure capture), compared by
   `ArenaId`. Done, verified via `cargo test --lib` (56/56 passing) plus
   5 new end-to-end fixtures.
3. ✅ **§8** — tier filter on instance method dispatch; arena-consistent
   allocation for result-producing methods. Done, verified via
   `cargo test --lib` (58/58 passing) plus 3 new end-to-end fixtures —
   ended up including building real method-call type checking from
   scratch, which turned out not to exist yet (see §8's own writeup).
4. ✅ **§9** — explicit LOW-tier rejection for collection construction.
   Done, verified via `cargo test --lib` (58/58 passing, unchanged) plus
   1 new end-to-end fixture — also surfaced and fixed an unrelated
   `SemaType::display()` catch-all bug affecting every diagnostic that
   renders a non-trivial type (see §9's own writeup).
5. ✅ **§10** — `Pool<T>` design and implementation. Done, verified via
   `cargo test --lib` (58/58 passing, unchanged) plus 4 new end-to-end
   fixtures — also surfaced and fixed a real bug in the `with`-statement
   dispatch (every allocator kind was silently getting arena's escape
   semantics) and a real gap in null-compatibility checking for wrapped
   `Optional` results (see §10's own writeup).

---

## 12. Open Decisions

| # | Question | Status |
|---|---|---|
| 1 | Unify `pool<T>(n)` / `Pool<T>` (§10.6), or rename the collection to avoid the collision entirely? | ✅ Resolved — unify |
| 2 | Generational handles as opt-out default, or opt-in? | ✅ Resolved — generational only in v1; no raw-index opt-out was built |
| 3 | Does a `@tier(mid)` function require an explicit `with arena(...)` for every collection, or should entering a `@tier(mid)` function implicitly open a function-scoped arena? | Open |
| 4 | Does `with pool<T>(n)` get the same `@tier(mid)`-only restriction `with arena(...)` currently has (`ArenaInWrongTier`), or should fixed-capacity pools be legal from HIGH-tier code too (an "unsafe-block"-style local optimization)? | ✅ Resolved — same `@tier(mid)`-only restriction as arena (`PoolInWrongTier`) |
| 5 | Is `Unique<T>`/`Shared<T>`/`SyncShared<T>` restricted to any particular tier, and does `resolve_receiver` strip these wrappers to dispatch methods on the inner type (e.g. is `Unique<List<int>>.push(5)` legal)? | ✅ Resolved. Tier: construction banned outside `@tier(low)` (`OwnershipWrapperOutsideLowTier`, TIER-014), inverse of `CollectionConstructionInLowTier`. Method dispatch: `resolve_receiver` and `eval_method_call` both strip one ownership wrapper before their existing tier/kind resolution, so `Unique<List<int>>.push(5)` is legal at both sema and runtime. `move_facts.rs`'s method-call-as-move over-approximation was refined alongside this (see its own module doc): a call to a known builtin instance method no longer counts as a move of its receiver. A same-named user-declared method still does, name-based, not type-based, and confirmed empirically to bite on genuinely common names (`get`, shared with `List`/`Dictionary`/`Pool`), not just a theoretical edge case; deliberately not resolved now, real follow-up once the language is more mature, same call the person made explicitly |
| 6 | Function/method parameter type annotations are resolved twice, independently, from raw AST (`collect_fn_sig`/`collect_method_sig` during signature collection, `seed_param` during body-seeding), so any error on a param's own type reports twice. Thread the already-computed signature type into `seed_param`, or deduplicate identical-span errors in the error manager? | ✅ Resolved, threaded. `collect_fn_sig`/`collect_method_sig` now record each param's resolved type into `SemaContext::binding_types` (new `seed_param_type` helper) as they compute it, under the correct generic scope; `seed_param` reads that back instead of calling `ast_type_to_sema` again. This turned out to matter for more than the duplicate diagnostic: `seed_param`'s old independent call never had the function's own generic scope pushed the way `collect_fn_sig` does, so a free function's own generic param (`fn identity<T>(x: T)`) silently resolved to an unconstrained type inside the body instead of the real `Param` placeholder, confirmed empirically (`let y: int = x` where `x: T` type-checked with zero errors before this fix). Inline struct methods were unaffected by that half of the bug (their generic scope is already pushed once, around both phases together, by `collect_struct_sig`/`infer_struct_bodies`), so the fix is a pure win there: same result, computed once instead of twice. `impl`/`extend`-block methods remain a separate, already-documented gap (§3 above / GENERICS_RULES.md "Known gaps") untouched by this fix either way, since neither phase pushes a struct's generic scope around them |
