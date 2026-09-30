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
- **Plain (non-trait) `extend`/`impl` dispatch is fixed** — was the
  entry directly below this one before that session's own follow-up
  fixed it, so keeping the original finding on record rather than
  deleting it: `resolve_impl` (`name_resolution.rs`) said so in its own
  comment (*"impl blocks don't introduce a name... we record method
  definitions without a specific parent DefId"*), confirmed directly for
  both `extend Foo { }` and plain inherent `impl Foo { }` (no trait, no
  `for` clause), both instance (`self`) and static/associated (no
  `self`) methods — none of the four combinations linked their methods
  to the target struct's `struct_methods` (sema, `type_infer.rs`) or
  `method_table` (interpreter, `eval/mod.rs`) entry at all; the actual
  `resolve_receiver` comment cited below was accurate about why, just
  not the only place the linkage was missing. Fixed by
  `register_extend_impl_methods`/`register_extend_or_impl_methods` (one
  new pass each, sema and interpreter, run once after every struct's own
  entry already exists so `extend`/`impl` blocks work regardless of
  which side of the `struct` they appear on in the file) plus
  `infer_extend_impl_bodies`, the `current_struct_type` fix below.
  **Trait `impl`s (`impl X for Y`) are deliberately still excluded from
  both dispatch tables** — registering a trait impl's methods into
  ordinary dispatch would let `v.method()` resolve to one whose trait
  requirement was never actually checked, since traits themselves are
  still exactly as unbuilt as the rest of this section describes.
  4 new fixtures (`ok_extend_impl_dispatch_isolated`/`_combined`,
  `err_extend_self_type_mismatch`, `err_extend_unknown_method`).
- **`self`'s type inside an `extend`/`impl` method body is also fixed** —
  previously always `Unknown` (`current_struct_type` was only ever set
  for methods declared directly inside a `struct { }` block,
  `infer_struct_bodies`; nothing set it for `extend`/`impl`, and
  `seed_param` explicitly does nothing for `self` params, see that
  function's own former `TODO`), so `self.x + "not a number"` inside an
  `extend` method passed sema with zero diagnostic before this. Applies
  to *every* `impl`, trait or inherent — a trait impl method body still
  deserves correct `self` typing even though it isn't dispatchable yet.
  1 new fixture (`err_extend_self_type_mismatch`).
- The `TYPE-115` "self-derived format spec" bug immediately below
  **turned out to be the exact same root cause as the `self`-typing gap
  just above, confirmed by retesting rather than assumed** — fixed as a
  side effect of `current_struct_type` now being set correctly, not a
  separate change. 1 new fixture
  (`ok_extend_self_format_spec_isolated`) pins the retest down.
- Method dispatch through `Unique<T>`/`Shared<T>`/`SyncShared<T>` for
  **user-defined** struct methods: **fixed**. `resolve_receiver`
  (`builtins/instance/`) is still narrowly scoped to the builtin kinds
  and returns `None` for a user struct, so the struct-instance-method
  branch in `type_infer.rs` now peels one ownership wrapper itself
  before its `SemaType::Named` check (sema only; `eval_method_call`
  already peeled all three for every receiver). 4 new fixtures
  (`ok_struct_ownership_dispatch_isolated`/`_combined`,
  `err_struct_ownership_dispatch_unknown_method`/`_arg_mismatch`).
- Found while fixing the above, since **fixed**: `move_facts.rs`'s
  by-name exemption (a builtin instance method call does not count as
  a move of its receiver) now also covers every user-declared instance
  method name in the program (`user_method_names`, threaded in through
  `collect_with` and `move_check::check_function_with`). Still by name,
  not by type, so a same-named consuming method on an unrelated type
  under-reports; it never rejects a safe program. 4 fixtures
  (`ok_unique_user_method_calls_isolated`/`_combined`,
  `err_unique_user_method_after_move`/`_after_move_into_call`).
- A related, likely-same-root-cause bug, found alongside this — now
  fixed, kept here for the record: a value
  that traces back to `self` (directly, or via `let x = self.field`)
  failed `TYPE-115 InvalidFormatSpec` on `on_type: "<unknown>"` for
  *any* format spec (`{self.id:03}`) *inside an impl-block method
  body*, even though `self.field` resolved fine for ordinary use
  (`self.id * 2` type-checked and ran correctly) — so `self`'s own type
  wasn't actually unresolved in general, only the interpolation-hole
  walk's own, separate type lookup failed for it. Extracting to a local
  first did **not** fix it either (confirmed by testing the exact "fix,"
  not assumed from the pattern that worked for a plain, non-`self`-derived
  local) — `let node_id = self.id; println($"{node_id:03}")` failed
  identically, while the same extraction on a struct field accessed
  *outside* an impl block (`n.id` in `main()`, no `self` involved)
  worked fine.
- `GENERICS_RULES.md` confirms trait bounds on generics (`T: Comparable`)
  are parsed, stored, never enforced.
- Trait dispatch itself (`impl X for Y`, `dyn Trait`, bound enforcement)
  has zero fixtures and is still exactly as undesigned as this section
  originally found — the two fixes above close the *mechanism* gap that
  would otherwise have blocked trait dispatch too, not trait dispatch
  itself.
- One thing that *does* work, confirmed fresh (a separate conversation
  proposed the test but the transcript never showed whether it passed):
  generic enum payload matching — `enum Option<T> { Some(T), None }` /
  `enum Result<T, E> { Ok(T), Err(E) }`, constructed via
  `Result.Ok(val)`/`Option.Some(idx)` and destructured via
  `match r { Result.Ok(val) => ..., Result.Err(msg) => ... }` — runs
  correctly end to end, full pipeline. Unrelated to the impl-block gap
  above (enum variant construction/matching, not `impl`-block method
  dispatch), and not something to re-verify again later as if it were
  still open.

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

## External test findings — verified against source, not taken on faith

A separate conversation ran its own test scripts against (it claimed) this
compiler and reported several findings. Re-ran the testable ones directly
against real source before recording anything — about half held up exactly
as described, and about half were wrong or actively introduced APIs that
don't exist here:

**Held up, confirmed real:**
- **Sized integer literals never coerce — half of this is now shipped, half
  is still open.** `u8`/`u16`/`u32`/`u64` (and the signed/`isize`/`usize`
  equivalents) are real `TypeKind` variants, not something the other
  conversation invented, and now have real runtime backing to match:
  `interpreter::value::Value` gained dedicated `I8`/`I16`/`I32`/`U8`/`U16`/
  `U32`/`UInt(u64)` variants (only `Uint`/`Ulong` — 64-bit-and-below-vs-
  above-`i64::MAX` — actually need a variant of their own; `Int`/`Long`/
  `I64`/`Isize` all share the existing `Value::Int(i64)`, being the same
  64-bit-signed width already), `eval_cast` truncates/wraps correctly for
  every one of them instead of the no-op pass-through it used to be, and
  `eval_binop` got real wrapping arithmetic (`wrapping_add`/`_sub`/`_mul`/
  `_div`/`_rem`, not the `f64`-promotion path the plain `Int`/`Float`/
  `Double` numeric ops still use, which cannot represent `u64`'s full
  range at all). This also closed a real, separate, pre-existing crash:
  negating `i64::MIN` (reachable via a suffixed literal, see below) used
  plain `-n` and panicked on overflow in a debug build; now `wrapping_neg`
  throughout, matching what `docs/PRINT_FORMAT_RULES.md` §4 already
  documented `Int` as (two's complement) but `-n` didn't actually give.
  One of the two disambiguation options this entry originally proposed —
  **a literal suffix (`10u32`)** — is what got built (`docs/ubel.ebnf`'s
  new `IntSuffix` production; `TYPE-120` rejects a suffixed literal whose
  value doesn't fit, matching Rust's own "literal out of range" compile
  error rather than silently wrapping; an unsuffixed literal past
  `i64::MAX` auto-promotes to an implicit `u64` suffix, since there's no
  other way such a value could exist as a literal at all — this is also
  what raised the lexer's overflow ceiling from `i64::MAX` to the full
  `u64::MAX`). What's still genuinely open, unchanged from before: a
  **bare, unsuffixed** literal assigned directly to an already-sized-typed
  field or `let` (`active_streams: u32 = 10`, no suffix) is still a real
  `TYPE-101` — `infer_literal` still hard-codes an unsuffixed literal to
  plain `SemaType::Int` with no contextual/deferred typing, so the other
  disambiguation option this entry originally raised (Rust-style
  context-driven literal inference) remains fully undecided and unbuilt.
  Also newly found while building the suffix work, not part of the
  original external-conversation claims: `byte` (C#) currently maps to
  *signed* `I8`, not unsigned `U8` — backwards from C#'s own convention —
  and `sbyte` isn't recognized at all (confirmed by grep, zero references
  anywhere in the codebase); left as-is on explicit request this session,
  revisit if C#-parity on this specific keyword ever actually matters.
  Range *patterns* (`0u8..200u8 => ...`) over a sized-int value also
  aren't wired up yet — `PatternKind::Range`'s match arm is still gated to
  `Value::Int`/`Value::Char` only — single-value literal patterns
  (`5u8 => ...`) work correctly via `match_literal`, only ranges don't.
- **Nested generic closing (`List<List<int>>`) fails to parse.** Confirmed
  directly: `RightShift` (`>>`) is lexed as one token, and the type-expr
  parser wants a lone `Greater` to close a generic, so
  `UnexpectedToken { found: RightShift, expected: ["Greater"] }` fires on
  the second-level close. `List<List<int> >` (space between the two `>`)
  parses and runs fine — confirmed too, not just repeated from the other
  conversation's claim. `PARSER_RULES.md` §5.1 already covers the `<`
  open-vs-less-than ambiguity with speculative parsing; this is the
  matching close-side gap, not yet handled the same way.

**Also confirmed, not new, but worth having in one place:** the
"impl-block methods don't dispatch" gap from the Traits section above
(now fixed — see that section) was the same root cause a separate part
of this batch hit too (framed there as two different, unrelated-sounding
errors — `TYPE-103`/`TYPE-104` on one test, then a distinct "isn't wired
up" claim on another). Struct-shaped
enum variants (`enum E { V { a: int, b: int } }`, constructed
`E.V { a = 1, b = 2 }`, destructured `E.V { a, b } => ...` in a `match`)
work correctly end to end when the field types actually match — confirmed
directly rather than assumed from the parts of that batch that *did* use
matching types.

**Did not hold up — don't carry these forward:**
- `printf` doesn't exist anywhere in this codebase. Only `println`.
- `String` doesn't exist as a type name here. Only `Str` — confirmed by
  grepping the actual `TypeKind` enum and `DATASTRUCTURES.md`, not assumed.
- The other conversation's own diagnosis of the `u32` mismatch above (that
  it was a struct-literal `=`-vs-`:` parsing issue) was wrong — the actual
  errors were plain `TYPE-101` type mismatches, not parse errors at all,
  and the script already used `=` correctly throughout.
- `GcRef.new(...)`, `Unique.new(...)`, `.into_shared()`, and
  `frame_arena.alloc(...)` as explicit constructor/conversion calls were
  never checked against real source in that conversation and don't appear
  anywhere in this codebase either — treat as unconfirmed, not as an
  established API, until someone actually checks `MEMORY_MODEL.md`'s own
  account of how `GcRef`/`Unique`/`Shared`/`ArenaRef` actually get
  constructed.



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

## Findings from a second external test session (verified by execution)

A pasted transcript of another assistant's exploratory testing was run
against the real compiler, scenario by scenario, rather than taken on
faith. Most of it matched what was already known (generics, nested
patterns, tier checks, precedence, short-circuiting all passed). What
it actually turned up:

**Fixed in the delivery that recorded this section:**

- Global `const` items type-checked but were never evaluated by the
  interpreter, so reading one panicked with `undefined name`.
  `run_program` now evaluates them before `main`, retrying constants
  whose initializer refers to one declared later in the file, and
  reports a constant cycle at startup. Assigning to one is now
  `NAME-007` (`AssignToConst`); a local that shadows a constant's name
  stays assignable.
- `List<List<int>>` did not parse: `>>` is one lexer token. The cursor
  now splits it (`eat_generic_close`), see `PARSER_RULES.md` §5.9.
- `Dictionary<K, V>` was missing from `structurally_compatible`, so any
  annotated dictionary failed with a `TypeMismatch` against
  `Dictionary<?T, ?T>`. Same recurring bug class as `Set`/`Queue`/
  `Stack` before it.
- A lambda return type annotation (`fn(x: int) string { ... }`) parsed
  `string` as the whole body and reported two unrelated `NAME-001`
  errors. Lambdas have no return type syntax by design; the parser now
  reports one `PARSE-004` on the annotation itself.
- An unknown method on a struct reported both `TYPE-103` and
  `TYPE-104`. The `Call` arm now tells the callee's `Field` node it is
  a callee (`callee_field_pending`), so only `TYPE-104` fires. The
  earlier note in `GENERICS_RULES.md` calling this a known wart is
  updated accordingly.

**Still open, confirmed by execution:**

- A bare unsuffixed literal into a sized-integer field or `let`
  (`P { n = 10 }` with `n: u32`) is still `TYPE-101`. Needs a design
  decision (context-driven literal typing versus suffix-only).
- `type` aliases are not transparent: with `type Score = int`,
  `let a: Score = 5` is `TYPE-101`, and `Score` and `int` do not unify
  in either direction. `type NodeId = u64` only worked in the
  transcript because the value went through `as NodeId`.
- An unknown method called on an `enum` value passes sema and panics at
  runtime (`no method 'shade' on enum`); `NoSuchMethod` is only
  reported for struct receivers.
- Calling a function-typed struct field (`c.cb(4)` where `cb: fn(int)
  int`) is reported as `NoSuchMethod`, because the `Call` arm only
  consults `struct_methods` for a `Field` callee.
- `pub` and `@tier(...)` written on a `const` or `type` item parse and
  are silently dropped: `ConstDecl` and `TypeAlias` carry no
  visibility or tier, and name resolution declares both `Private`.
- A `TypeMismatch` raised while unifying the arguments of two generic
  types carries `Span::at(0)`, so the diagnostic points at line 0.
- Struct field default values (`n: u32 = 5u32`) do not parse, and there
  is no mutable global item; a top-level `let` is a parse error.

## Decided, not yet built

Design questions that were presented as options and answered. Each gets
its own delivery with fixtures; the choice is recorded here so it is not
re-opened by accident.

**Unsuffixed integer literals: full context-driven typing (Rust style).**
An unsuffixed integer literal takes its type from where it is used, so
`P { n = 10 }` with `n: u32`, `let x: u8 = 5`, `f(3)` where the parameter
is `i16`, and `u32_var + 1` all type-check, with the existing `TYPE-120`
range check. A literal with nothing to constrain it stays plain `int`. The
interpreter has no static types, so the runtime side needs coercion of a
plain integer value at the same boundaries (annotated `let`, struct field,
call argument, return, assignment, and a sized-integer binary operation
with a plain integer operand). Rejected: suffix-only (status quo) and
direct-site-only coercion.

**Mutable globals: `static`, HIGH tier only.** A `static` item is a
mutable global living in the HIGH tier (GC-managed), private by default,
`pub` to export once the module system lands. Only `@tier(high)` code may
read or write one. A `const` stays an immutable, tier-agnostic value; a
tier annotation on a `const` becomes an error instead of being silently
dropped. Rejected: tier-specific mutable globals, since an arena has no
lifetime that can hold a global and a LOW-tier global needs unsafe rules.

**Still open:** struct field default values (`n: u32 = 5u32`), which
depends on the literal decision above.
