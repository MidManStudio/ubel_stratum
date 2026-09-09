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
