# Ubel Stratum — Outlives / Subset Enforcement (Design, Not Yet Built)

> **Scoping document for the one piece LOW tier's reference story doesn't
> have yet. Nothing in this document is implemented. Read this before
> touching `lifetime_check.rs`, `borrow_check.rs`, `cfg.rs`, or `facts.rs`
> for this work — it explains what's being extended, what's being reused
> verbatim, and where the real risk is.**

---

## 0. What already exists (checked directly against source, not assumed)

LOW tier's reference story is much further along than "outlives checking:
not started" suggests. Four of five pieces are real and landed:

| Piece | File | Status |
|---|---|---|
| Reference types & syntax | `ast/types.rs` `TypeKind::Reference`, `sema/type_table.rs` `SemaType::Reference` | Landed |
| Lifetime *declaration* well-formedness | `sema/lifetime_check.rs` | Landed — duplicate names, undeclared names, `outlives` cycles |
| Loan-based borrow checking (real NLL-lite) | `sema/cfg.rs`, `sema/facts.rs`, `sema/borrow_check.rs` | Landed — genuinely non-lexical, CFG-based, unit-tested |
| Move checking | `sema/move_facts.rs`, `sema/move_check.rs` | Landed |
| **Outlives/subset *enforcement*** | — | **This document** |

The fifth piece is narrower than "build a borrow checker" — that part's
done. It's specifically: **when a reference crosses a function-call or
struct-field boundary, verify the declared `outlives` relationship
actually holds**, reusing the CFG/liveness machinery that already
answers "how long is this specific borrow actually alive" for the
intra-function case.

Concretely, right now: `edge struct View [lifetime L] { point: &L Point }`
parses, and `lifetime_check.rs` confirms `L` is declared once and used
correctly — but nothing checks that a `View` doesn't outlive the `Point`
it borrows. `is_edge` and `lifetime_params` are, in the type table's own
words, decorative everywhere in sema. This document is the plan for
making them not decorative.

---

## 1. The exact existing machinery this reuses

Grounding in real names, not paraphrase, because the whole point of this
plan is that the new pass mostly *assembles* pieces that already exist
rather than duplicating them:

- **`facts::Point { block: BlockId, stmt_index: usize }`** — one
  statement, CFG-block-granular. A region, in this design, is a
  `HashSet<Point>` — nothing new needed for the representation itself.
- **`facts::Place<'ast>`** — `Local(&str)` or `Unknown`. Already the unit
  everything is tracked by.
- **`facts::Loan { id, place, bound_place, mutable, issued_at, span }`**
  — one borrow instance within one function body.
- **`borrow_check::compute_reaches_before(cfg, loan, facts)`** — forward
  reachability of a loan's issue point. Already computed today, just
  never materialized as a standalone value — `check()` computes it,
  intersects with liveness, and discards it.
- **`borrow_check::compute_live_after(cfg, place)`** — backward liveness
  of a place. Same story: computed, used for one intersection, discarded.
- **`borrow_check::succ_points(cfg, p)`** — point-level CFG successor,
  already shared with `move_check` for its own forward propagation.

**The load-bearing realization:** `reaches ∩ live_after` for a given loan
*is already, today, that loan's natural region* — the exact set of
points where the borrow is both reachable and needed. `check()` just
never keeps this set around; it computes the intersection inline against
a specific candidate set of invalidation points and throws the rest
away. Phase E1 below is almost entirely "keep the value that already
gets computed" rather than new analysis.

---

## 2. What a region is, here (deliberately not Polonius's exact formulation)

Two different things need a region, and they're not the same kind of
thing:

1. **A loan's region** — concrete, per-borrow-instance, computed exactly
   as above (`reaches ∩ live_after`, materialized instead of discarded).
   This exists per call to a function, effectively — it's about one
   specific `&p` in one specific body.
2. **A named lifetime parameter's region** — an abstraction over the
   signature (`[lifetime L]` on a function or `edge struct`), which has
   to work for *every* call site / construction site, not one specific
   borrow. This one doesn't have a single fixed region; it has a
   **requirement**: "whatever region gets bound to `L` at a given call
   site must be a superset of what that call site's actual usage needs."

This is exactly the vocabulary the person's own framing used — `L`'s
bound region must be a **subset** of `M`'s if the signature says `M
outlives L`, and a use site's requirement must be a subset of whatever
gets bound to the parameter it flows into. Nothing here needs Polonius's
actual Datalog formulation or a dependency to get this vocabulary; it's
the same fixed-point-over-relations idea `borrow_check.rs` already uses,
one level up.

---

## 3. The four phases

### Phase E1 — Materialize loan regions (refactor, not new logic)

Change `borrow_check::check`'s inline `reaches`/`live_after`
intersection into a real, returned `HashMap<LoanId, HashSet<Point>>` —
each loan's own region, available to Phase E2 rather than thrown away
after one use. Existing conflict-checking behavior is unchanged; this
is purely "keep a value that already exists."

### Phase E2 — Boundary constraint generation

At each of the two boundary shapes that exist in the language today
(confirmed — nothing else currently lets a reference cross a function or
struct boundary):

- **A call to a function with `[lifetime ...]` params**, where an
  argument's type is `&L T` for one of those declared lifetimes.
- **A struct literal for an `edge struct` with `[lifetime ...]`**, where
  a field's declared type is `&L T`.

At each site, match the actual argument/field-value expression back to
a `Place` (reusing `facts::expr_as_place`, already exported) and, if
it resolves to a **local** whose loan region is known from Phase E1,
generate a constraint: *this call/construction site requires `L`'s
bound region to be a superset of the loan's actual region.* If the
expression doesn't resolve to a traceable local (a call result, an
already-`Unknown` place), that's a real scope boundary for v1 — see §4.

### Phase E3 — Propagate declared `outlives` constraints

For every `LifetimeConstraint { longer, shorter }` already parsed by
`lifetime_check.rs`, add the fixed-point rule: region(`longer`) ⊇
region(`shorter`). Plain worklist propagation over the (typically tiny —
one function's declared lifetimes) constraint graph, same shape as
`borrow_check`'s own fixed points, just over named lifetimes instead of
loans.

### Phase E4 — Check and report

For each Phase E2 requirement, once Phase E3's propagation has settled,
confirm the bound region actually satisfies what was required. Violation
→ new `LIFETIME-0xx` code (see §6), pointing at both the construction
site and the place whose lifetime is too short — mirroring
`borrow_check::Violation`'s existing `loan_span`/`conflict_span` pair.

---

## 4. Scope for v1 — what's deliberately not covered, and why that's fine for now

Checked before writing this down, not assumed:

- **References nested inside generics** (`List<&L T>`) — zero fixtures
  anywhere use this shape, zero mentions of variance anywhere in the
  codebase, and `structurally_compatible` doesn't compare lifetimes at
  all today. There is no existing code this would break by deferring
  it, and no test would exercise it if implemented. Explicit v1
  boundary, not an oversight.
- **Closures capturing references** — no closure-plus-reference fixture
  exists either. Same reasoning.
- **Methods** (`Item::Impl`/`Item::Extend`) — `borrow_check.rs`'s own
  header comment already excludes these from the *existing* loan
  checker (`@tier(low)` methods aren't checked yet, full stop, for
  reasons unrelated to this work). Outlives enforcement inherits that
  same boundary rather than getting ahead of the checker it depends on.
  Fixing method coverage is real work, but it's borrow-checking's
  prerequisite gap, not an outlives-specific one — tracked as its own
  item below, not silently bundled in.
- **Lifetime elision** — doesn't need to be built at all, and this is a
  genuine simplification versus Rust rather than a gap: every example
  in the fixtures and `NAMING_CONVENTIONS.md` writes `[lifetime L]`
  explicitly. Nothing here needs elision rules.
- **Non-local sources at a boundary** (an argument that's itself a call
  result, not a traceable local) — Phase E2 requires a resolvable
  `Place::Local`. A boundary argument that isn't one is a real
  limitation, not silently accepted: v1's answer is to conservatively
  reject (can't prove it's fine → don't allow it), same "conservative,
  never unsound" stance `cfg.rs`/`facts.rs` already documented making
  elsewhere in this exact checker.

---

## 5. What doesn't need to change

Worth stating plainly so it isn't assumed otherwise once implementation
starts: **this is entirely a sema-time check, erased before anything
downstream.** Same as `borrow_check`/`move_check` today —

- No interpreter (`interpreter/eval/*.rs`) changes. Zero runtime
  representation of a region; nothing to evaluate.
- No AST changes found to be necessary — `Type::Reference`'s existing
  `lifetime: Option<&str>` already carries what's needed everywhere a
  `Type` appears, struct fields included. If Phase E2's implementation
  turns up a spot where that's not quite true, that's a real finding to
  flag when it happens, not assumed now.
- No new `DixValue`/binary-format concerns — this is a sema, not a
  `.mdix`-adjacent concept at all (different project, unrelated).

---

## 6. New error codes

`LIFETIME-001`/`002`/`003` (`UndeclaredLifetime`/`DuplicateLifetimeParam`/
`OutlivesCycle`) already exist, well-formedness only. New, enforcement:

| Code | Meaning |
|---|---|
| `LIFETIME-004` | A call/construction site's actual argument doesn't live long enough for the declared lifetime parameter it's bound to |
| `LIFETIME-005` | A declared `outlives` constraint between two named lifetimes doesn't hold given how they're actually used |
| `LIFETIME-006` | A boundary argument isn't a traceable local (§4's conservative-reject case) — kept distinct from `004` since the fix is different (bind it to a local first vs. genuinely shorten a lifetime) |

---

## 7. Fixtures — this doesn't fit the standard 4

The usual rule (2 `err_` + 2 `ok_`, one isolated one combined) fits an
atomic feature; this has three independently-testable phases plus a
real cross-cutting story (`edge struct` + arena escape, §8). Proposed
instead, one pair per phase plus the integration case:

- `ok_/err_outlives_call_boundary_*` — Phase E2/E4, a plain function
  with `[lifetime L]` params.
- `ok_/err_outlives_edge_struct_field_*` — Phase E2/E4, the `edge
  struct` construction case (the `View`/`Point` shape from
  `MEMORY_MODEL.md` §9, made to actually dangle).
- `ok_/err_outlives_constraint_propagation_*` — Phase E3 specifically,
  two lifetime params with a declared `outlives` between them.
- One combined, real-world fixture tying all three together — closest
  in spirit to the diagonal-grid/particle-pool combined fixtures
  already in the suite.

## 8. Free win: this also closes a separately-known gap

`MEMORY_MODEL.md` §9 already documents that `is_edge` doesn't connect
to the arena-escape checker at all — an `edge struct` gets rejected (or
not) by the exact same check as a non-edge one. Phase E2's
struct-construction-site handling *is* that connection: once a
construction site can check "does this field's actual borrow satisfy
its declared lifetime," the arena-escape checker's existing rule can
defer to it for `edge` structs specifically instead of applying its
current one-size-fits-all boundary rule. Flagged here so it's landed as
one coherent piece of work rather than rediscovered as a separate gap
later.

---

## 9. Order of landing (not one PR)

1. ✅ **Landed.** Phase E1 — `compute_loan_regions` in `borrow_check.rs`.
   Turned out not to be quite the "pure refactor" this originally said:
   intersecting `reaches` with the existing `live_after` map produced an
   empty region for every loan, caught by the new tests actually failing
   rather than by re-reading the diff. `live_after[p]` means "read
   strictly after `p`" (correct for `check`'s own conflict question),
   not "read at or after `p`" (what a loan's own region needs, counting
   its own final use). Fixed by widening the liveness function —
   renamed `compute_live_after` to `compute_liveness`, now returning
   `(live_before, live_after)` — `check` takes `.1` (unchanged
   behavior, all existing tests pass as-is), `compute_loan_regions`
   takes `.0`. 4 new unit tests, including one that would fail if this
   function were ever "simplified" into reusing `check`'s own
   mutable-only loan loop (shared loans need regions too — most `&L T`
   parameters in real code are shared, not `&mut`). No `.ubl` fixtures
   for this step: nothing about which programs are accepted or rejected
   changed, so there is nothing at the language level for a fixture to
   exercise yet — that starts at step 2.
2. Phase E2 + E4 for the plain-function-call-boundary case only, no
   `outlives`-between-multiple-lifetimes yet (single declared lifetime
   per signature). Smallest real slice that produces a working
   `LIFETIME-004`.
3. Phase E2 for the `edge struct` construction case + §8's connection to
   the arena-escape checker.
4. Phase E3 (multi-lifetime `outlives` propagation) + `LIFETIME-005`.
5. `LIFETIME-006` (non-local boundary argument rejection) — can land
   whenever; genuinely independent of 2-4.

## 10. Open questions this scoping pass did *not* resolve

Real, left for whoever picks this up (probably still you, but flagged
as decisions rather than silently defaulted):

- Exact diagnostic wording/UX for `LIFETIME-004`/`005` — `borrow_check`'s
  own violations point at two spans; worth deciding if a third
  ("declared here") pointing at the `[lifetime L]` clause itself adds
  clarity or clutter.
- Whether method coverage (the `borrow_check` prerequisite gap, §4)
  should get scheduled *before* step 3 above, since `edge struct`
  methods are a very plausible near-term use case once this lands and
  they'd hit the same "not checked" wall immediately.
- Whether `LIFETIME-006`'s conservative rejection should have an escape
  hatch (binding the call result to a local first is already a
  workaround that exists without new syntax) or whether that's
  sufficient and no hatch is needed.
