# Ubel Stratum: Parked Ideas

Ideas and external references noted during a session for possible
future use. Nothing here is scheduled or committed; each entry needs
its own scoping and design discussion before any implementation
starts, same as every other design-bearing item this project has
handled.

## Jai language guide

A community-written guide to Jonathan Blow's Jai language was reviewed
for relevance. The guide itself carries a disclaimer that Blow has said
it doesn't describe Jai or his intentions well; treat specifics in it
as plausible direction, not spec.

The biggest philosophical divergence: Jai has no references and no
borrow checking at all. "References" is explicitly on its own "Not
Planned" list, replaced by raw pointers plus programmer discipline.
That is the one area where Ubel Stratum is deliberately doing something
Jai's own philosophy argues against, so it is not a template for the
LOW-tier reference/borrow-checking work.

Specific ideas that looked worth keeping in mind, each a separate,
later feature area, not folded into current work:
- `!`-marked owned struct fields, auto-cascade-delete when the owner
  is deleted. A different mechanism from `Unique<T>` (field marker vs
  wrapper type) but conceptually adjacent; possibly relevant to the
  edge-struct arena-lifetime story, since it is the same "does this
  field's lifetime ride along with its owner" question.
- A `#no_abc`-style bounds-check escape hatch (disable bounds checking
  for a block or statement). A concrete, proven pattern for LOW-tier
  performance control.
- `#run`, arbitrary compile-time code execution, baking computed data
  directly into the binary. Large, separate feature area.
- Automatic SoA/AoS struct-of-arrays transformation. Also large and
  separate; a real data-oriented-design feature with no current Ubel
  Stratum equivalent.

## mid-arena (Mid-D-Man/mid-engine, crates/mid-arena)

A real, benchmarked, well-tested arena/slot-allocator crate the person
built in the mid-engine repo, not for mid-engine's own use, explicitly
meant as a candidate for Ubel Stratum.

Current state, checked directly before writing any of this down:
`interpreter/mod.rs`'s own module doc says MID-tier `with arena(...)`
blocks are marker scopes only today; values still use `Rc` in the
tree-walker. Real bump-allocation is documented as landing with the
LLVM backend, not before. So there is no real arena backend running
anywhere yet for either idea below to plug into.

mid-arena has four types, which map onto two already-separate Ubel
Stratum constructs rather than being four flavors of one thing:
- `BumpArena<T>`: single-typed, chunk-linked, classic bump allocation.
  Matches `with arena(N) { ... }`'s own documented semantics: bump
  style, scoped to the block, freed together.
- `SlotArena<T>` / `CompactSlotArena<T>` / `UncheckedSlotArena<T>`:
  generational (or, for `Unchecked`, raw-index, no ABA protection)
  individually insertable/removable slot storage. Matches `Pool<T>`,
  which already exists as its own separate Ubel Stratum construct.

Real snag to solve before "which type": `BumpArena<T>`'s own public API
(`alloc(&self, value: T) -> &mut T`) is single-typed, one `T` per
instance, confirmed by reading the source directly. A real `with
arena(N) { ... }` block allocates a mix of types in practice (lists,
dicts, structs), so one `BumpArena<T>` cannot back a whole block as is;
needs either multiple typed arenas composed per block, or something
closer to `bumpalo`'s own mixed-type `Bump`, before this is a drop-in
backend.

If a type-selection parameter is ever added, it more plausibly belongs
on `Pool<T>` (picking which of the three slot-arena types backs it,
genuinely interchangeable at that call site) than on `with arena`
(which already has its own answer, `BumpArena`, and where "type" would
collide with the with-arena/`Pool<T>` split that already exists).

mid-arena is unpublished (version 0.1.0, no external dependencies in
its default build). Its own dev-dependencies (criterion) need a newer
toolchain than this project's rustc 1.75 floor for mid-arena's own
tests/benches; that is about building mid-arena's own test suite, not
about using it as a plain dependency.

## Traits / interface system

Raised as "did we ever actually discuss traits?" — checked the real
answer against the source rather than guess. Short version: **no, not
as a design decision.** What exists is parser/AST/name-resolution
scaffolding that was clearly built alongside some other declaration work
(struct/enum/generics, most likely — `TraitDecl`/`ImplBlock` sit right
next to those in `parse_decl.rs`), never the subject of its own design
session, never fixture-tested, and not wired to anything functional:

- `trait`/`impl X for Y` parse — `TraitDecl`, default methods,
  required-signature methods, all real AST nodes.
- Name resolution and type inference walk into trait bodies and collect
  method signatures.
- But `resolve_impl` (`name_resolution.rs`) says so in its own comment:
  *"impl blocks don't introduce a name... we record method definitions
  without a specific parent DefId."* Methods inside `impl Foo for Bar`
  are never linked to `Bar`. Not dispatchable.
- `GENERICS_RULES.md` confirms trait bounds on generics (`T: Comparable`)
  are parsed, stored, never enforced.
- Zero fixtures exercise any of it.

### Reference-language survey

Asked which of the languages already in the mix (Rust, Go, C#, plus Zig
and Odin as the two that get cited for the "hand-rolled, low-machinery"
side of Ubel Stratum's own taste) have something trait-like, in case
there's real prior art worth taking from rather than defaulting to
Rust's own trait design:

- **Rust** — the obvious starting point, and largely what the existing
  scaffolding already gestures at. Nominal (must be declared, not
  inferred structurally), static dispatch by default via monomorphization
  (`impl Trait` / generic bounds), dynamic dispatch opt-in via `dyn
  Trait` (a real vtable, needs a pointer wrapper — `Box`/`&`/`Rc`).
  Associated types, supertraits, blanket impls, an orphan rule (can't
  impl a foreign trait for a foreign type). Rust's own biggest ergonomics
  complaint, and the one a second conversation explored ways to route
  around: `impl<T: TraitA> TraitB for T` blanket-impl boilerplate to give
  a trait a default based on another trait.
- **Go** — structural, not nominal. A type satisfies an interface just by
  having the right methods, no `impl` declaration anywhere, checked at
  the call site not the declaration site. Interfaces can embed other
  interfaces. `any` (`interface{}`) is the universal empty interface.
  Genuinely different philosophy from Rust's explicit-opt-in model, not
  just sugar over the same thing — worth naming as a real fork in the
  road, not just "the other mainstream option."
- **C#** — nominal like Rust (`class Foo : IBar`), but with default
  interface methods (C# 8+, closer to Swift's protocol extensions than
  Rust's blanket impls) and *explicit interface implementation* — `void
  IRenderable.Draw()` — for resolving a name collision when a type
  implements two interfaces that both declare `Draw()`, disambiguated
  right at the implementation, not at every call site. A second
  conversation independently proposed the same mechanic for Ubel
  (`fn Renderable.draw(self)` / `fn UIElement.draw(self)`) without
  knowing it's a direct lift from real C# syntax — it is, and it is a
  genuinely clean answer to that specific collision, not an invented one.
- **Zig** — deliberately has *no* trait/interface keyword. Static
  duck-typing via `anytype` params resolved at comptime (`fn
  update(entity: anytype) { entity.tick(); }`, fails at the specific
  call site inside the generic instantiation if the method's missing,
  not up front). Dynamic dispatch, when genuinely needed, is hand-rolled:
  a type-erased data pointer (`*anyopaque`) plus a struct of function
  pointers — `std.mem.Allocator` is exactly this pattern, not a language
  feature.
- **Odin** — also no interface keyword. Leans on an *implicit context*
  parameter instead (every function gets a hidden `context` struct
  carrying the active allocator/logger; swap behavior by reassigning
  `context.allocator` in a block, not by threading a trait object down
  every call), plus explicit union types + type switches, plus
  parametric (generic) procedures where duck-typing fits.

Zig and Odin's shared answer — "no dedicated feature, hand-roll a vtable
struct or thread it through context when you actually need one" — is
philosophically the closest fit to how this project already treats
LALRPOP, SMT solvers, and other heavy machinery: avoid the feature until
a concrete need proves it's worth the weight. Worth naming as a real
option, not dismissing it just because Rust/C# are the more familiar
starting points — "should Ubel have `trait` at all, or should the
answer be a documented duck-typing + hand-rolled-vtable *pattern*
instead, the way `Pool<T>`/`with arena` are patterns rather than
compiler magic" is a legitimate first question for whatever design
session this becomes, not a foregone conclusion.

### A second conversation's synthesis — evaluated, not adopted wholesale

A separate chat (outside this one, no direct codebase access) explored
a synthesis worth recording, since parts of it hold up under checking
against the real source and parts don't:

- **Required fields in a trait** (Scala's `trait Spatial { val pos:
  Vec2 }` — no getter/setter boilerplate, the field itself is the
  contract). Plausible on its face; not checked against how Ubel
  Stratum's own struct field storage/layout actually works, so still
  genuinely open, not verified either way.
- **C#-style explicit interface implementation** for name collisions —
  see above, this one's a real, proven mechanic, not just plausible.
- **Tier-gated trait methods** (`@tier(mid) fn tick(mut self)` inside a
  trait, enforced on every implementor) — checked this one directly:
  `tier_check.rs`'s `check_expr` already does real cross-tier call
  validation (`check_callee_tier`, the same machinery behind the
  `await`-only-in-HIGH-tier rule) as its core job today. Extending that
  to validate a trait method's body against its own declared tier is a
  natural extension of a pattern that already exists, not a stretch —
  the most concretely buildable piece of the whole proposal.
- **`Shared<dyn Trait>` for HIGH-tier dynamic dispatch, `FfiSpan<dyn
  Trait>` for MID/LOW** — `Shared<T>` is real. `FfiSpan` is also a real,
  named concept, but checked directly against `DATASTRUCTURES.md`:
  *`FfiSpan`'s own architecture is still listed as genuinely open* —
  own type vs. validated construction, not yet decided. Building
  dyn-trait dispatch semantics on a construct that isn't itself settled
  is premature; this part of the proposal is speculative, not
  ready-to-build, however clean it sounds.
- **C++20-concept-style lightweight bound predicates** (`where T:
  Moveable and not HighTierOnly`) — not checked against anything, pure
  syntax suggestion, no current Ubel Stratum equivalent to compare
  against either way.

None of this is a design decision — it's material for whatever session
actually scopes traits, flagged so the good parts (explicit
implementation, tier-gating) don't have to be re-derived from scratch,
and the shakier parts (required fields, `FfiSpan<dyn Trait>`) don't get
assumed settled just because they were written down confidently
somewhere.

## Vale generational references — pre-checking as a future `Pool<T>` optimization

Read directly (not secondhand) against `Pool<T>`/`Handle<T>`'s actual
implementation (`pool_methods.rs`, `MEMORY_MODEL.md` §10):
[Vale's Memory Safety Strategy: Generational References and Regions](https://verdagon.dev/blog/generational-references).
Full comparison now lives in `MEMORY_MODEL.md` §10 itself (the
generation-tables structural match, and the `wrapping_add` overflow
decision) since it's a finding about already-shipped code, not a parked
idea. One piece of the article is genuinely a *future* idea, not a
finding about today's code, so it's parked here instead:

Vale's "pre-checking" optimization — when the compiler can prove data
won't change for a scope (their `pure`/regions), it validates a
generational reference once up front instead of on every access within
that scope, turning N runtime checks into 1. `Handle<T>.get()` has no
equivalent today; it checks on every single call, always. This is a
real, concrete optimization angle for a hot loop that calls `.get()` on
the same handle repeatedly — the entity-allocator use case
`MEMORY_MODEL.md` §12 Open Decision #4 already flags as the reason to
get `Pool<T>` right for Mid Engine. Not worth building until profiling
of an actual entity-allocator workload says it's worth it; noted here so
it isn't rediscovered from scratch later.

## Loop power-ups

Also raised during the same exploratory testing that found the
condition/struct-literal ambiguity (§5.8 in `PARSER_RULES.md`). Checked
each one directly against the source rather than assume from the
feature name alone — two are real gaps, two are partially there
already:

- **Labeled loops** (`break 'outer` / `continue 'outer`) — genuinely not
  present. No lexer token, no AST field, nothing. A real gap for
  anything doing a broad-phase/grid/archetype search with early exit
  from a nested loop, which is exactly the kind of code this project's
  own fixtures already lean toward (see the diagonal-grid scan above).
- **Range-based `for`** (`for i in 0..100`, no backing list allocated) —
  half true. `0..100` already parses fine as its own expression
  (`BinOp::Range`/`RangeIncl`, real binding-power table entries) — but
  it is not wired up as something a `for`-loop knows how to iterate;
  no fixture does this, and nothing in the interpreter's iteration
  logic mentions `Range`. Probably the cheapest of the four to close,
  since the expression-level piece already exists; what's missing is
  purely the iterator-protocol side.
- **Loop expressions** (`let x = loop { ... break 42 }`, the loop
  itself evaluating to the `break` value) — not present.
  `StmtKind::Loop` exists (bare `loop { }` already parses) and `break
  <value>` itself parses and is even type-checked (`type_infer.rs`
  infers the break value's expression type) — but `StmtKind::Loop` is a
  *statement*, there is no `ExprKind::Loop`, and the break statement's
  own type is hardcoded to `void` regardless of its value's type. The
  value is computed and immediately discarded; nothing plumbs it back
  out as the loop's result.
- **Completion clauses** (Python/Zig-style `while cond { } else { }`,
  runs only if the loop finished without a `break`) — not present, no
  trace of it anywhere in the grammar or AST.

Same status as traits: recorded so it doesn't need re-deriving, not
scoped or prioritized. Range-based `for` is the standout "probably
worth doing first" candidate purely because the hard part (the
expression itself) is already done; the other three are each their own
real design-and-build effort.
