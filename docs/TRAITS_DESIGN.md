# Traits: Design Options

> **Status: decisions made 2026-10-03; slice S1a built (section 0).**
> Sections 1 to 5 are the options as presented, kept as the record of what
> was weighed. Written for the dedicated traits design session that `docs/PARKED_IDEAS.md`
> ("Traits / interface system") called for. That section holds the
> reference-language survey and the evaluation of an outside synthesis; this
> document does not repeat them. It records what the compiler does with a
> trait today (read from source and confirmed by running programs), the
> constraints that shape the answer, one decision per section with options
> and a recommendation, and a delivery order. Once the decisions are made
> they move to "Decided, not yet built" in `PARKED_IDEAS.md`, one delivery
> at a time, each with its own fixtures.

---

## 0. Decisions and progress

### Decisions (2026-10-03)

| Decision | Chosen |
|---|---|
| **D1** kind of trait | **A**, nominal traits with explicit `impl Trait for Type`. |
| **D2** dispatch | **Both** static and dynamic. Static first (S1), `dyn Trait` in the last slice (S6). Whether `dyn` is `HIGH` only (B) or every tier (C) is settled when S6 starts. |
| **D3** coherence | The recommended v1 answers: one impl per trait and type, overlap is an error, an inherent method beats a trait method, two traits supplying one name is an error at an unqualified call and is resolved with `Trait.method(value)`. The orphan rule, blanket impls and C#-style explicit implementation are **deferred** until there is more context. |
| **D4** trait contents | **All four levels**: methods and defaults, generic traits and supertraits, associated types, required fields and associated constants. Built in stages (S1, S3, and a later slice for the last level, which needs its field-layout interaction with ECS and arenas checked first). |
| **D5** built-in protocols | The recommendation: lift the six derives into prelude traits (S2); `Display`/`Debug`, `Iterable`, operators one at a time, each its own decision (S5). |
| **D6** bounds | The recommendation: enforce at call sites and through generic bodies, and make a method call on an unbounded type parameter an error (S1b); `where` clauses and bounds with arguments later. |
| **D7** tiers | A **hybrid** of options a and b: a trait can set a default tier for all its methods, each method can carry its own tier, an implementer must match the tier of the method it implements, and a specific method can override to change the tier. Flagged as complex and to be handled flawlessly, so it gets its own specification before any code (S4, below). |
| **D8** keywords | The recommendation: `extend` inherent only, `impl Trait for Type` the trait form, plain `impl Type { }` still accepted. |

### Slice S1 is split in two

| Slice | Status | Contents |
|---|---|---|
| **S1a** | **Built** | Trait impls registered in sema and interpreter dispatch; conformance checks (missing method, method not in the trait, signature mismatch with `Self`); default methods inherited or overridden; `Self` in signatures and default bodies; overlap; inherent-first; ambiguity and the qualified call `Trait.method(value)`; calls made through a trait run the trait's method; every trait feature not built yet reported as `TYPE-129` instead of ignored. |
| **S1b** | **Built** | Bounds: declared bounds validated, bound obligations checked wherever a bounded parameter is instantiated, method calls on a type parameter resolved through its bounds, a method call on an unbounded type parameter an error (`TYPE-130`). |
| **S2a** | Designed, not built | Prelude traits for the six derives with real signatures, the `Ordering` enum, built-in impls, `@derive` as an impl generator, bounds and operator checks on type parameters, `Hash` and `Eq` on `Dictionary` keys. See "What S2 settled" below. |
| **S2b** | Designed, not built | Runtime dispatch: `==`, `!=`, ordering operators, list `contains`, dictionary keys, Linqerizer ordering and `clone` call hand-written impls. |
| S3 to S6 | Planned | As in section 4. |

### What S1a settled that the options did not

- **A call made through a trait runs the trait's method.** Inside a default
  method `self.area()` is resolved through the trait (inherent methods are not
  visible through a bound), so it must run the trait's `area` even when the
  concrete type also has an inherent `area`. Sema records each such call
  (`SemaContext::trait_call_sites`) and the interpreter dispatches those
  through its per-trait table, the same way it already takes
  `int_literal_types`. S1b reuses this for every call through a declared bound.
- **`Self`** is the implementing type inside an `impl`, `extend` or struct,
  and an abstract parameter inside a trait. The abstract `Self` is the
  reserved `Param` index equal to the trait's generic arity, so one
  `substitute` call replaces the trait's own arguments and `Self` together
  once generic traits exist.
- **Not yet checkable:** an impl for a built-in type, a generic trait and an
  associated type are each reported as `TYPE-129` (`UnsupportedTraitFeature`),
  so none is silently ignored. Impls for built-in types arrive with S2, generic
  traits with S3.

### What S1b settled that the options did not

- **Bound names.** A bound must name a trait declared in the program
  (`TYPE-122` otherwise, once per declaration, at the parameter). The six
  derive names (`PartialEq`, `Eq`, `Hash`, `Ord`, `PartialOrd`, `Clone`) are
  not declared traits until S2, so a bound written with one is `TYPE-129`
  rather than being accepted and ignored. A user trait that happens to use
  one of those names is a real bound. A bound on a type alias parameter or on
  a method's own generic parameter is `TYPE-129` too, because neither scope
  exists in sema yet.
- **Where a bound is enforced.** Each place a bounded parameter is
  instantiated records an obligation that the argument type implements the
  trait: a call to a generic function (including a call through a variable
  that holds it), a struct literal, an enum variant construction, an
  associated function called on a generic struct, and a type annotation that
  names a bounded struct or enum with arguments. A call is reported at the
  first argument whose declared type is the bounded parameter, the others at
  the expression or annotation.
- **When it is decided.** Obligations are settled at the end of the body,
  after integer literal types are settled (so `total(5)` reports `int`, not
  `{integer}`), and reported in source order. An argument that is still an
  unresolved inference variable is not an error. Obligations raised by
  signatures wait until every impl is registered.
- **Method calls on a type parameter.** The receiver's type parameter is
  looked up in the bounds in force for the body: a generic function, a method
  of a generic struct, and an `extend` or `impl` block on one all see the
  declaration's bounds. With bounds, the method must come from one of them
  (`TYPE-104` when none supplies it, `TYPE-127` when two do). With none, the
  call is `TYPE-130`. Calls resolved through a bound are recorded in
  `trait_call_sites`, so the trait's method runs even when the concrete type
  has an inherent method of the same name.
- **Reference and ownership wrappers are ignored.** A value reached through
  `ArenaRef`, `GcRef`, `OwnedRef`, `PoolRef`, `Unique`, `Shared`, `SyncShared`
  or a borrow satisfies the same bounds as the bare value. This can only miss
  a violation, never reject a program that is fine.
- **Still open:** `where` clauses and bounds with arguments (later); bounds
  naming built-in traits and impls for built-in types need S2.

### What S2 settled (decided 2026-10-07)

Decisions made for S2, in addition to D5 B (lift the six derives into prelude
traits):

- **Real methods, with dispatch to hand-written impls.** The six traits carry
  methods, and `==`, `<`, hashing and `clone` call a hand-written impl when one
  exists. This is the larger runtime change, chosen over marker traits.
- **Floats.** `float` and `double` implement `PartialEq`, `PartialOrd` and
  `Clone` only, not `Eq`, `Hash` or `Ord`, because of NaN.
- **Enforcement on existing code.** `Dictionary` keys must implement `Hash` and
  `Eq`, and `==`, `!=` and the ordering operators on a value of type-parameter
  type require `PartialEq` or `PartialOrd` in its bounds.
- **Method surface.**

| Trait | Methods | Notes |
|---|---|---|
| `PartialEq` | `eq(self, other: Self) bool` | Default `ne`. |
| `Eq` | none | Supertrait `PartialEq`. |
| `PartialOrd` | `partial_cmp(self, other: Self) Ordering?` | Defaults `lt`, `le`, `gt`, `ge`. Supertrait `PartialEq`. |
| `Ord` | `cmp(self, other: Self) Ordering` | Supertraits `PartialOrd` and `Eq`. |
| `Hash` | `hash(self, state: Hasher)` | Rust style: the hasher is a parameter. |
| `Clone` | `clone(self) Self` | |

  `Ordering` is a prelude enum `{ Less, Equal, Greater }`. `Hasher` is a
  built-in type. Its surface is proposed as `Hasher.new()` and
  `state.finish() u64`, with each primitive's `hash` writing into the state;
  it is confirmed when S2a starts.
- **Two deliveries.** S2a is the static side and S2b the runtime dispatch. Until
  S2b lands, a hand-written `impl PartialEq for X` (and the other five) is
  `TYPE-129`, so an impl that `==` would ignore is never accepted silently.
- **Defaults taken without objection.** `@derive(X)` registers the impl and the
  derive prerequisite chain becomes the supertraits; lists, tuples and
  optionals satisfy a trait when their elements do, by a built-in rule rather
  than impl blocks; enums keep automatic `PartialEq`, `Eq` and `Hash` and gain
  no `Ord` or `Clone` in S2; a user trait named like a prelude trait shadows it;
  `==` on a concrete struct without `PartialEq` stays reference identity.

What reading the code established before this was decided:

- `Value::equals` and `Value::partial_cmp` take no interpreter and are called
  from more than a dozen places (the binary operators, list, queue, stack and
  inline-list `contains`, the dictionary methods, indexing, the Linqerizer
  grouping and ordering). S2b has to give each of them a route back into the
  interpreter.
- `Dict` is `Vec<(Value, Value)>` searched by `equals`; hashing has no consumer
  yet. A struct key without `PartialEq` is found by reference identity, so a
  lookup with an equal but distinct key returns `null`.
- Sema's maps are keyed by `Span` (four `usize` fields, `Hash` and `Eq`), so
  prelude declarations cannot be parsed from a separate text at ordinary spans.
  The two ways to bring the prelude traits in are a prelude written in Ubel whose
  token spans are shifted into a reserved range, and programmatic registration
  with natively implemented methods; diagnostics are rendered by indexing the
  source's lines, so a label that points into the prelude would need handling in
  the renderer. The mechanism is chosen when S2a is built and recorded here.

### D7: what has to be specified before S4

Two facts found while building S1a bear on the tier design.

1. **A method call on a receiver is not tier-checked at all today.** The
   cross-tier call rule runs only for calls whose callee resolves to a
   definition (a plain function name). `recv.method()` has no such
   resolution, so a `MID` function can call a `HIGH` method on a value with
   no error. Putting a tier on trait methods is meaningless until method
   calls are checked, so S4 starts there.
2. **What "override to change the tier" lets an implementer do needs an exact
   rule.** Under the existing rule a callee must be at the caller's tier or
   lower. A call through a bound is checked against the tier the TRAIT
   declares, because the impl is not known. An impl method at a tier **at or
   below** the declared one can never make such a call unsound. An impl
   method **above** the declared tier can, unless every instantiation is
   checked separately. The S4 specification chooses between: exact match only;
   match or lower (sound with the existing rule); or arbitrary override with
   per-instantiation checking. It is written and agreed before S4 is built.

---

## 1. Where things stand (at the start of the design session)

Verified by reading the source and by running small programs, not taken from
the older notes.

| Stage | What happens to a trait today |
|---|---|
| Parser | Parses `trait` (method signatures, default methods, associated type *names*), `impl Trait for Type`, and `T: A + B` bounds. Does **not** parse supertraits (`trait B: A`), `where` clauses, bounds with arguments (`T: Into<U>`), a tier on a signature, or associated type bindings in an impl. The generic arguments of a trait path in an impl (`impl From<int> for X`) are parsed and then discarded (`let _ = self.try_parse_generic_args()`). |
| Name resolution | Declares the trait (`DefKind::Trait`) and resolves default-method bodies. `AssociatedType` is a `TODO`. |
| Sema | Collects trait method signatures and infers default-method bodies. Trait `impl`s are **excluded from dispatch** (`register_extend_impl_methods` skips any impl with a `trait_path`), so `impl Shape for Sq { fn area .. }` followed by `q.area()` is `NoSuchMethod`: **a trait impl cannot be used at all.** Bounds are stored and never checked. A method call on an unbounded type parameter is accepted as `Unknown`. |
| Interpreter | Dynamically typed, generics erased. Dispatch is `method_table[type_name][method]`; trait impls are skipped there too. |
| Derive | The one trait-like feature that works: six hard-coded names (`PartialEq`, `Eq`, `Hash`, `Ord`, `PartialOrd`, `Clone`), structs only, with prerequisite chains (`Eq` needs `PartialEq`, `Ord` needs `PartialOrd` and `Eq`, `Hash` needs `Eq`). They set flags on `Value::Struct` that change `==`, ordering, hashing and cloning at runtime. |

### Probes

| Program | Result |
|---|---|
| `impl Shape for Sq { fn area(self) .. }` then `q.area()` | sema `NoSuchMethod` |
| `fn total<T>(x: T) int { return x.area() }`, called with a struct that has `area` and with `5` | sema accepts both; the `5` call panics at runtime (`no method 'area' on int`) |
| `for x in b` where `b` is a user struct | sema accepts; runtime panic (`cannot iterate over value of type 'struct'`) |
| `a + b` on two user structs | sema accepts; runtime panic (`arithmetic not supported on struct`) |
| `a == b` on two equal structs with no derive | `false` (identity, not structure) |
| `println($"{p}")` on a struct declared `x, y` | prints `P {x: 1, y: 2}` in some runs and `P {y: 2, x: 1}` in others (6 of 8 runs vs 2 of 8): field order comes from a `HashMap`, not from the declaration |
| `trait B: A { .. }` | parse error |

Three of these are the same hole the project has closed several times: sema
accepts, the interpreter panics. A trait system is the natural home for the
protocols behind them (iteration, operators, formatting, equality).

---

## 2. Constraints

- **C1. Monomorphization is the stated direction.** `MEMORY_MODEL.md` assumes
  monomorphized types and an eventual LLVM backend; the tree-walking
  interpreter is a stand-in. Whatever is chosen must lower to static dispatch
  by default.
- **C2. The interpreter erases generics.** There are no runtime type
  parameters. Every option below must be implementable as sema checks plus a
  method table.
- **C3. Tiers.** The cross-tier call rule is: a callee must be at the caller's
  tier or lower. `HIGH` may call `HIGH`, `MID` and `LOW`; `MID` may call `MID`
  and `LOW`; `LOW` may call only `LOW`. `MethodSig` has no tier today, and the
  tier lives in the reference wrapper (`GcRef`, `ArenaRef`, `OwnedRef`), not in
  the type name.
- **C4. No module system.** `summon` is unscheduled, so an orphan rule cannot
  be written or tested yet.
- **C5. Taste.** The project avoids heavy machinery until a concrete need
  proves it worth the weight (LALRPOP, SMT solvers). "Should this be a
  pattern instead of a feature" is a legitimate first question.

---

## 3. Decisions

Each decision lists its options, what each costs, and a recommendation. The
recommendations are consistent with each other; changing one may change
another, noted where it does.

### D1. Is there a `trait`, and what kind?

| Option | Meaning | Cost |
|---|---|---|
| **A. Nominal traits** | A type implements a trait only by declaring `impl Trait for Type` (Rust, C#). | Matches the grammar, the AST and `T: Bound` that already exist. Needs the dispatch and bound checking that do not. |
| **B. Structural interfaces** | A type satisfies an interface by having the right methods; no `impl .. for` (Go). | Makes the parsed `impl .. for` dead syntax. A method name carries no tier and no intent, which sits badly with tier rules and with the derive prerequisite chains. |
| **C. No keyword** | Duck typing plus a hand-rolled vtable: a struct of function-typed fields (Zig, Odin). | Cheapest. It is what the interpreter already does, minus any checking, and the cost shows: the unbounded-`T` probe above compiles and then panics at runtime. Function-typed fields are callable now, so the vtable pattern works today. |

**Recommended: A.** The six derives already behave like named capability sets
with supertraits, and the parser already commits to nominal syntax. Option C
stays available as the *pattern* for dynamic dispatch (D2) even with A chosen.

### D2. Static dispatch, dynamic, or both?

| Option | Meaning |
|---|---|
| **A. Static only** | Bounds are checked at declaration; calls resolve per instantiation. No `dyn Trait`. |
| **B. Static plus `dyn` in `HIGH` only** | `dyn Trait` as a type behind a GC reference, giving heterogeneous collections such as `List<dyn Shape>`. |
| **C. Static plus `dyn` in every tier** | `dyn` behind the tier wrappers (`GcRef<dyn T>`, `ArenaRef<dyn T>`, `OwnedRef<dyn T>`). |

Facts that bear on it: a vtable needs a pointer wrapper; the architecture of
`FfiSpan` is still listed as open in `DATASTRUCTURES.md`; and `dyn` adds a
type form, an object-safety question (methods returning `Self`, generic
methods) and a per-tier representation. None of that is needed for checked
bounds. The interpreter would run all three identically, so the choice is
about what sema checks and what the future backend must generate.

**Recommended: A first, B as a later slice, C deferred.** Heterogeneous
collections have two workable answers in the meantime: an `enum` of the
variants, or the function-field vtable pattern (D1 option C).

### D3. Coherence: which impl applies, and what is allowed?

Questions and the proposed v1 answer to each:

| Question | Recommended v1 answer |
|---|---|
| Two impls of one trait for one type | Error (`OverlappingImpl`). No specialization. |
| Impl for a built-in type (`impl Shape for int`) | Allowed. Needed so that `T: Ord` can be satisfied by `int` and `string` at all (D5). |
| Orphan rule | Deferred until modules exist. The intended rule, to avoid painting into a corner: an impl is allowed only in the package that declares the trait or the type. |
| Blanket impls (`impl<T: A> B for T`) | Not in v1. Revisit once bounds and supertraits are solid. |
| Inherent method and trait method with the same name | The inherent method wins. |
| Two traits supplying the same method name to one type | Error at an unqualified call (`AmbiguousTraitMethod`); resolved with a qualified call, `Shape.area(q)`. C#-style explicit implementation (`fn Renderable.draw(self)`) is a proven mechanic and is deferred, not rejected. |

### D4. What can a trait contain?

The AST already has method signatures, default methods and associated type
names. Options, cumulative:

| Level | Adds |
|---|---|
| **i** | Methods and default methods, with `Self` (already parses in signatures; nothing resolves it). |
| **ii** | Generic traits (`trait Convert<T>`) and supertraits (`trait Ord: Eq`). Needs the impl's trait path to keep its arguments, and the parser to read `: Bound` after a trait name. |
| **iii** | Associated types (`type Item`). Needs bounds on the declaration and bindings in the impl. |
| **iv** | Required fields (Scala style) and associated constants. |

**Recommended: i in the first slice, ii next, and prefer generic traits over
associated types for iteration (`Iterable<T>`)**, since the language already
has generics with inference and it avoids a second mechanism. Associated types
are justified only if one-impl-per-type semantics turn out to matter. Required
fields are not recommended now: they interact with field layout (ECS `@core`
and `@tag`, arena layout), and that interaction is unverified.

### D5. How do the built-in protocols relate to traits?

| Option | Meaning |
|---|---|
| **A. Leave them magic** | Traits are for user abstractions only; derives, formatting, `for` and operators stay compiler features. |
| **B. Lift the six derives into prelude traits** | `PartialEq`, `Eq`, `Hash`, `Ord`, `PartialOrd`, `Clone` become real traits with the supertrait chains they already have; `@derive` becomes "generate this impl". Primitives and the built-in collections get built-in impls. Bounds such as `K: Hash + Eq` and `T: Ord` become enforceable. |
| **C. B plus language-item traits** | Adds `Display`/`Debug` (formatting), `Iterable<T>` (`for x in value`) and operator traits (`Add`, `Sub`, `Index`, ..). Each closes one of the sema-accepts, runtime-panics probes above, and `Display`/`Debug` would define field order by declaration, fixing the print nondeterminism. |

**Recommended: B in the second slice; the items of C one at a time, each its
own decision.** Operator overloading in particular is a language-taste choice
(vector math for a game language argues for it) and deserves its own
yes or no, not a ride-along.

### D6. Bounds: syntax and enforcement

| Option | Meaning |
|---|---|
| **a. Call sites only** | Inferred type arguments must satisfy the bounds. |
| **b. (a) plus bodies** | Inside a generic body, a method call on `T` resolves only through `T`'s bounds, and a method call on an unbounded `T` is an error. |
| **c. (b) plus richer syntax** | `where` clauses and bounds with arguments (`T: Into<U>`). Needs `GenericParam.bounds` to become structured instead of `&[&str]`. |

**Recommended: b in the first slice, c later.** Option b is a deliberate
strictness change: the unbounded-`T` probe compiles today and panics at
runtime for `total(5)`. Whether any existing fixture relies on calling a
method on an unbounded parameter has not been checked; the fixture sweep
decides it, and any that does is a real bug the change would surface.

### D7. Tiers and traits

| Option | Meaning |
|---|---|
| **a. Tier on the signature** | A trait method signature carries an optional tier (default `high`, like every function). An impl method's tier must be at most the declared tier. A call through a bound is checked against the declared tier with the existing cross-tier rule. |
| **b. Tier-polymorphic traits** | Each impl declares its own tier and callers adapt. Sound only if call checks cannot be fooled; considerably more machinery. |
| **c. `HIGH` only** | Trait methods and bounded generic code usable only from `HIGH`; `MID` and `LOW` use concrete types. |

**Recommended: a, staged.** With no tier on signatures every trait method is
`high` by default, which is exactly option c for free through the existing
rule, so the first slice needs no new tier code. Adding `@tier(mid) fn
tick(self)` to a signature later only relaxes it. This is the most
concretely buildable piece of the outside synthesis noted in
`PARKED_IDEAS.md`, because `check_callee_tier` already exists.

### D8. Keywords: `impl` and `extend`

For inherent methods the two are equivalent today. `ImplBlock` carries a
block tier and an optional trait path; `ExtendDecl` carries neither.
**Recommended:** `extend` stays inherent only, `impl Trait for Type` is the
trait form, and a plain `impl Type { }` stays accepted so nothing breaks.

---

## 4. Proposed delivery order

Each slice is one delivery with an isolated and a combined fixture per phase,
exact-count tests, and the docs updated in the same bundle.

| Slice | Contents | Decisions it needs |
|---|---|---|
| **S1** | Nominal traits with static checking: register trait impl methods in sema and interpreter dispatch; conformance checks (missing method, method not in the trait, signature mismatch with `Self`); default methods inherited; bounds enforced at call sites and through bodies; inherent-first resolution and the qualified call. | D1 A, D2 A, D3, D4 i, D6 b, D7 a, D8 |
| **S2** | Prelude traits for the six derives; built-in impls for primitives; bounds on built-in collections; `@derive` becomes an impl generator. | D5 B |
| **S3** | Supertraits and generic traits; the impl's trait path keeps its arguments. | D4 ii |
| **S4** | Tier on trait method signatures. | D7 a |
| **S5** | Language-item traits, one decision and one delivery each: `Display`/`Debug`, `Iterable<T>`, operators. | D5 C |
| **S6** | `dyn Trait` in `HIGH`. | D2 B |

Indicative new diagnostics for S1, codes assigned when built:
`TraitMethodMissing`, `UnknownTraitMethod`, `TraitMethodSignatureMismatch`,
`UnsatisfiedBound`, `AmbiguousTraitMethod`, `OverlappingImpl`,
`MethodOnUnboundedParam`.

Explicitly deferred: blanket impls, the orphan rule, explicit-implementation
syntax, associated types, required fields, `FfiSpan<dyn Trait>`,
specialization.

---

## 5. Found along the way

Recorded here because they surfaced while reading and probing. None is part
of the trait work, and none has been changed.

- **Struct printing is nondeterministic.** The default struct display walks a
  `HashMap`, so field order varies between runs. `Value::Struct` already
  carries `field_order` (used for ordering and hashing); formatting does not
  use it. A small, separate fix.
- **`for` over a user type and arithmetic on user types are accepted by sema
  and panic at runtime.** Two more sema-accepts, runtime-panics holes, folded
  into D5 option C as the protocols that would give them a type error.
- **Method calls on an unbounded type parameter are accepted.** Folded into
  D6.
- **Supertrait syntax does not parse.** Folded into D4 level ii.
