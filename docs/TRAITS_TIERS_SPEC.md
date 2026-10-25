# Tiers on Trait Methods: S4 Specification

> **Status:** Proposed. The choices in section 5 are marked for selection. No
> code is written until they are agreed.
>
> **Verified against:** `MidManStudio/ubel_stratum` master at `43fc418`, using
> `ubel check --json` and its `stage` field. Every probe in section 3 is paired
> with a control that must fail, so a clean probe means a real gap and not a
> bad program.
>
> **Companion:** `docs/TRAITS_DESIGN.md` (decision D7, slice S4, and the
> section "D7: what has to be specified before S4").

---

## 1. Scope

S4 makes the tier of a method a real, checked property.

In scope:

- A tier written on a trait, on a trait method signature and on a trait
  default method.
- The rule an impl method has to satisfy against the trait's tier.
- Tier checking for every method call on a receiver, which is not checked at
  all today (section 3).
- The places where a tier is already written but never reaches a method
  (section 3, probes P3 to P6).

Out of scope, and why:

| Item | Where it goes |
|---|---|
| `dyn Trait` | S6. Calls through `dyn` use the declared tier, the same rule as a call through a bound. |
| Operators, `for-in`, `Display` | S5. Each is an implicit method call; S5 reuses the lookup that S4 adds. |
| Associated types | Deferred until S5 `Iterable` needs them. |
| Impls for `Pool` and `Handle` | Reported as `TYPE-129` until a written `Pool<T>` or `Handle<T>` is a real type. |
| Tier-polymorphic traits, per-instantiation checking | Not planned. Sema is erased over generics and the interpreter has no instantiation. |

---

## 2. What is already decided

Decision D7 (recorded 2026-10-03) is a hybrid:

1. A trait can set a default tier for all of its methods.
2. Each trait method can carry its own tier, which replaces that default.
3. An implementer must match the tier of the method it implements.

The decision also says "a specific method can override to change the tier".
That clause is read here as rule 2: one method overriding the trait default.
If it was meant to let an implementer override the tier, that is option B or C
of choice Q1 below.

The cross-tier rule is unchanged: a callee must be at the caller's tier or
lower. `HIGH` may call `HIGH`, `MID` and `LOW`; `MID` may call `MID` and `LOW`;
`LOW` may call only `LOW`.

---

## 3. Verified starting point

Each row is a program checked with `ubel check --json`. "Clean" means no
diagnostic. A control for the same shape written as a plain function (or with
the tier on the method itself) fails with `TIER-005` in every case.

| Probe | Program shape | Result | Correct result |
|---|---|---|---|
| P1 | `@tier(low) fn` calls `s.hi()`, where `hi` comes from `extend S` and is `HIGH` | clean | `TIER-005` |
| P1b | the same call, where `hi` comes from `impl T for S` | clean | `TIER-005` |
| P1c | `@tier(mid) fn run<T: Tick>(t: T)` calls `t.label()`, where the trait declares `label` `HIGH` | clean | `TIER-005` |
| P2 | `@tier(low) fn f(self) { h() }` as a default method inside a trait, `h` being `HIGH` | clean | `TIER-005` |
| P3 | `@tier(low) impl S { @inline fn f(self) { h() } }` | clean | `TIER-005` |
| P4 | `@tier(low) { impl S { fn f(self) { h() } } }` | clean | `TIER-005` |
| P5 | `@tier(low) extend S { fn f(self) { h() } }` | clean | `TIER-005` |
| P6a | `@tier(low) struct S { fn f(self) { h() } }` | clean | `TIER-005` |
| P6b | `@tier(low) { struct S { fn f(self) { h() } } }` | clean | `TIER-005` |

Controls: a plain `@tier(low) fn` calling `h()`; `@tier(low)` on an `extend`
method; `@tier(low) impl S { fn f(self) { h() } }` with no other attribute; a
function inside `@tier(low) { }`; `@tier(low)` on a struct method. All five
report `TIER-005`.

These nine results fall into three groups.

**Method calls on a receiver are never tier-checked (P1, P1b, P1c).** The
cross-tier rule runs in `tier_check.rs` only for a call whose callee has an
entry in the resolution map, which a plain function name has and
`recv.method()` does not. Built-in instance methods have their own rule
(`TIER-008`), whose registries are empty today. Giving a trait method a tier
is meaningless while a call can bypass it.

**A tier written on a trait item is dropped (P2).** `MethodSig` has no tier and
no attributes, so the parser reads `@tier(...)` on a required signature and
discards it. A default method is built with a hard-coded
`TierAnnotation::High`, so its own `@tier(...)` is read into `attributes` and
ignored. `TraitDecl` has no tier field.

**A tier written around a method does not reach it (P3 to P6).**

- In an `impl` block, a method with any attribute at all (here `@inline`) is
  given the tier of its own attributes, which is `HIGH` when none is a
  `@tier`, instead of the block's tier (P3).
- `ImplBlock.tier` is set when a tier block wraps an impl, but the methods keep
  their own parsed tier, and sema never reads `ImplBlock.tier` (P4).
- `ExtendDecl` has no tier field, and a `@tier` on an `extend` is dropped (P5).
- A struct has no tier of its own, so neither `@tier(low) struct` nor a tier
  block around a struct reaches its methods (P6a, P6b). The comment on
  `MethodDecl.tier` ("inherits the enclosing struct/impl tier") describes
  behaviour that exists only for a method's own `@tier`.

Enums carry no methods, so only structs matter for P6.

Every one of these is the recurring bug class: sema accepts a program whose
text states one tier while the checker applies another.

---

## 4. Proposed rules

### 4.1 Tier of a trait method

The tier of a trait method is, in order:

1. the `@tier(...)` written on the method (a required signature or a default
   method);
2. otherwise the `@tier(...)` written on the trait, or on a tier block around
   it;
3. otherwise `HIGH`.

```ubel
@tier(mid) trait Tick {
    fn tick(self) void                  // MID, from the trait
    @tier(high) fn label(self) string   // HIGH, written on the method
}
```

A trait with no tier written anywhere behaves exactly as today: every method
is `HIGH`.

### 4.2 Tier of an impl method in a trait impl

The tier of an impl method is checked against the trait method it implements
(choice Q1 fixes what "checked" means). A tier the impl does not write is taken
from the trait method (choice Q2). A tier the impl does write, on the method or
on the block, has to satisfy the rule of Q1.

A default method that an impl does not override keeps the tier the trait gives
it. An impl that overrides a default method follows the same rule as any other
impl method.

### 4.3 Tier of a method in an inherent block

For a struct, an `impl Type { }` and an `extend Type { }`, the tier of a method
is, in order:

1. its own `@tier(...)`;
2. otherwise the `@tier(...)` on the enclosing `impl`, `extend` or struct, or
   on a tier block around it;
3. otherwise `HIGH`.

An unrelated attribute on a method (`@inline`, `@doc(...)`) never changes the
result, the same rule `apply_block_attrs` already follows for functions. The
tier of a struct is only the default for its methods. It does not constrain
where a value of the struct lives; that stays in the reference wrapper.

### 4.4 Tier of a method call

Every call that resolves to a method is checked with the existing cross-tier
rule, using the current function's tier as the caller and the tier below as
the callee.

| Call shape | Example | Callee tier |
|---|---|---|
| Inherent method, concrete receiver | `s.hi()` after `extend S { fn hi(self) }` | the method's own tier |
| Trait method, concrete receiver | `s.f()` where `impl T for S` supplies `f` | the impl method's tier |
| Default method, not overridden | `s.g()` where `g` is a trait default | the tier the trait gives `g` |
| Through a bound | `t.f()` with `t: X` and `X: T` | the tier the trait declares for `f` |
| Qualified call | `T.f(s)` | the tier the trait declares for `f` |
| Associated function | `Box.wrap(v)` | the function's own tier |
| Built-in method | `list.push(x)` | unchanged, governed by `TIER-008` |

Two properties follow.

- A call through a bound is checked against the trait because the impl is not
  known. That is sound only while every impl method is at or below the trait's
  tier. A caller allowed by the trait's tier is then always allowed by the impl
  it reaches.
- A receiver whose type is still unresolved is never an error, the same
  stance every other sema check takes for an unresolved type.

The check lives in `type_infer.rs`, beside the existing method resolution, and
not in `tier_check.rs`. The tier checker has no resolved receiver types, which
is the reason `TIER-008` is placed in `type_infer.rs` too. It reuses the
resolution that already finds the method, so a method registered through any
route is covered: user structs, `extend` on built-ins, and the generic impls of
S3e.

### 4.5 What does not change

- Tiers have no run-time effect. The interpreter is untouched, so S4 cannot
  add a sema-accepts, run-time-panics case.
- Methods with no source declaration (the native implementation of a prelude
  trait for a primitive, a derived implementation, a built-in method) carry no
  tier and are never rejected by the new check.
- `a == b`, `a < b`, `for x in v` and the other implicit calls are not method
  calls in the syntax. They stay unchecked until S5.

---

## 5. Choices

### Q1. What an impl method's tier has to be

Declared tier means the tier the trait gives the method.

| Impl tier compared with declared | A. Exact match | B. At or below | C. Any, checked per instantiation |
|---|---|---|---|
| equal | accepted | accepted | accepted |
| lower | error | accepted | accepted |
| higher | error | error | accepted, with a check per instantiation |

- **A. Exact match.** One tier per trait method. A concrete call and a call
  through a bound always see the same tier, and `dyn Trait` in S6 needs no
  extra rule. Cost: a type that could do the work at `LOW` cannot offer it to
  `LOW` callers through a `MID` trait method.
- **B. At or below.** Sound with the existing rule, because a lower impl can
  never be reached by a caller the trait forbids. Cost: one method has two
  tiers in one program. `s.tick()` is allowed from `LOW` while a generic
  function that calls `t.tick()` is not, which is hard to explain.
- **C. Any, checked per instantiation.** Needs a check at every
  monomorphized use. Sema has no instantiation to check against, and the
  interpreter erases generics. Not buildable now.

**Recommended: A.** It is the literal reading of decision D7 ("an implementer
must match"). A can be relaxed to B later without breaking any program A
accepted, because accepting more is always compatible; the reverse is not.

### Q2. Where an unwritten tier on an impl method comes from

- **a. The trait.** An impl method with no tier of its own, in a block with no
  tier, takes the trait method's tier. A written tier is checked against it
  under Q1.
- **b. The block, then `HIGH`.** The rule an inherent impl uses (section 4.3).
  Every impl method of a `MID` trait must then carry `@tier(mid)`, or sit in a
  `@tier(mid)` block, and a missing tier is reported as a mismatch.

**Recommended: a.** A trait default tier that every implementer has to repeat
does nothing. Under (a) a tier written by an implementer still works, as an
assertion that Q1 checks.

A tier block that wraps both a trait and its impl (`@tier(low) { trait ...
impl ... }`) resolves to the same tier on both sides under either option.

### Q3. Whether the tier-reaching fixes (P3 to P6) are part of S4

Once calls on a receiver are checked, a method that is silently `HIGH` makes
every `MID` or `LOW` caller fail with `TIER-005`, naming a method that the
program text says is `MID` or `LOW`. The fixes are:

- an impl method with unrelated attributes keeps the block tier (P3);
- a tier block reaches the methods of an impl, an `extend` and a struct
  (P4, P5, P6b);
- `ExtendDecl` gets `tier: Option<TierAnnotation>`, the same shape as
  `ImplBlock` (P5);
- a struct's own `@tier` becomes the default for its methods (P6a).

Options: **include all four** (recommended); **include P3 and P4 only** and
reject `@tier` on a struct or an `extend` with a diagnostic, so that nothing
is silently ignored; or **leave them out** and keep the silent ignore.

**Recommended: include all four.** They make the documented inheritance true,
and the alternatives keep or replace a silent ignore. They also narrow what is
accepted (section 9), which is the reason to decide them explicitly.

### Q4. Tier of the prelude traits' methods

`PartialEq`, `Eq`, `PartialOrd`, `Ord`, `Hash` and `Clone` declare no tier, so
their methods (`eq`, `ne`, `partial_cmp`, `cmp`, `lt`, `le`, `gt`, `ge`, `hash`,
`clone`) are `HIGH` under rule 4.1. A call through a bound uses the declared
tier, so a `MID` or `LOW` function generic over `T: Clone` cannot write
`x.clone()`. The operators `==` and `<` are not method calls and stay
unchecked until S5.

- **A. `HIGH`, no special case.** The limit is narrow and visible, and S5
  decides the tier of the operator traits together with the operators.
- **B. A tier-agnostic marker for the prelude traits.** Calls through them are
  never checked and impls are free. Unsound by construction: a `LOW` caller
  can reach a `HIGH` impl.
- **C. `LOW`.** Under Q1 A every hand-written prelude impl must then be `LOW`,
  so any caller can use them. It restricts what a hand-written `eq`, `hash` or
  `clone` body may call, before any program needs it.

**Recommended: A.** B opens a hole on purpose and C imposes a restriction with
no current demand.

### Q5. Delivery shape

- **One delivery.** All of section 4 together.
- **Two deliveries** (recommended): S4a, then S4b, as in section 8.

**Recommended: two.** S4a is the one that narrows what existing programs are
accepted, so its fixture sweep can be read on its own. Bound calls cannot land
before trait tiers exist: with every trait method `HIGH`, checking a bound call
in S4a would reject every `MID` caller with no way to write the fix.

---

## 6. Worked examples

Results below assume the recommended options (Q1 A, Q2 a, Q3 all, Q4 A).

```ubel
@tier(mid) trait Tick {
    fn tick(self) void
    @tier(high) fn label(self) string
}

struct P { n: int }

impl Tick for P {
    fn tick(self) void { }                             // MID, taken from the trait
    @tier(high) fn label(self) string { return "p" }   // written, equals the trait
}
```

Accepted.

```ubel
trait Tick { @tier(mid) fn tick(self) void }
struct P { n: int }
impl Tick for P { @tier(high) fn tick(self) void { } }
```

Rejected: the impl is `HIGH` and the trait declares `MID`.

```ubel
trait Tick { @tier(high) fn label(self) string }
@tier(mid) fn run<T: Tick>(t: T) string { return t.label() }
```

Rejected with `TIER-005`: a `MID` function calls a method the trait declares
`HIGH`, through a bound.

```ubel
struct S { x: int }
extend S { fn hi(self) int { return self.x } }
@tier(low) fn l(s: S) int { return s.hi() }
```

Rejected with `TIER-005`: `hi` is `HIGH` by default and the caller is `LOW`.
Writing `@tier(low)` on `hi`, on the `extend`, or on a tier block around it
makes the program valid.

All four programs lex, parse and check clean on live today, which is what the
probes in section 3 rely on. The rejections above are the intended behaviour,
not the current one.

---

## 7. Diagnostics

| Situation | Diagnostic |
|---|---|
| A call whose callee tier is above the caller's | `TIER-005` `IllegalTierCall`, reused. The callee name is the method name. |
| An impl method whose tier breaks the rule of Q1 | New, in the `TIER-0xx` family (next free code `TIER-016`). Working name `TraitMethodTierMismatch`; the code is assigned when built. |

Draft wording for the new one: "method `tick` in `impl Tick for P` is
`@tier(high)`, but trait `Tick` declares it `@tier(mid)`", with the suggestion
"remove the `@tier` on the method to take the trait's tier, or write
`@tier(mid)`". The span is the impl method.

Nothing silently ignored remains: a `@tier` on a trait, a trait signature, a
struct, an `extend` and an impl method is either honoured or reported.

---

## 8. Delivery plan

Both deliveries follow the standing protocol: a fresh clone of live, a
replacements bundle applied with the real scaffold script, a byte comparison,
`cargo test --workspace --all-targets --no-fail-fast`, a fixture sweep by stage
before and after, and a mutation check of every new rule with each survivor
classified. The probes in section 3 become the isolated fixtures, and each
delivery adds a combined fixture that uses the rules together with generics
(S3e) and bounds (S1b).

**S4a: carry tiers and check concrete receivers.**

- Parser: P3, P4, P5 and P6 (section 4.3); `ExtendDecl.tier`.
- Sema: `tier` on `MethodShape`; the check in section 4.4 for the inherent,
  trait-on-concrete, default and associated-function rows.
- Tests and fixtures: `crates/rd_parser/tests/method_tiers.rs` with exact
  counts, an isolated and a combined `ok_` and `err_` fixture.
- Docs: `TRAITS_DESIGN.md` (D7 and the S4 rows), `DIAGNOSTICS_RULES.md`,
  `PARSER_RULES.md`, `GENERICS_RULES.md` if generic receivers change, the
  delivery log in `ubel_stratum.md` and `ubel_stratum_rd.md`, and
  `web/site/src/project-status.md`.

**S4b: tiers on traits.**

- Parser and AST: `TraitDecl.tier`, `MethodSig.tier`, the default-method tier
  from its own attribute (P2), and the trait tier from a surrounding block.
- Sema: the resolved tier in the trait's method shapes; the conformance check
  of Q1; the unwritten-tier rule of Q2; the bound, qualified and
  default-method rows of section 4.4.
- Tests, fixtures and docs as in S4a, plus `ubel.ebnf` if the grammar for
  trait items changes.

---

## 9. Impact on existing programs

Acceptance narrows in two ways.

1. A `MID` or `LOW` function that calls a method which is `HIGH` (by default or
   by writing) is now an error. Before, it passed.
2. A method that sat in a tier block, a tier-annotated struct, an `extend`, or
   an impl with unrelated attributes now has the tier written around it
   instead of `HIGH`. Its body is checked at that tier: calls to `HIGH`
   functions, `await`, and the `LOW` rules (borrow and move checks, no
   collection construction) now apply to it.

The number of existing fixtures that change stage is measured when S4a is
built and reported with the delivery. No count is claimed here. Any fixture
that changes is corrected, or its expected result is stated, and none changes
silently.

---

## 10. Deferred

`dyn Trait` (S6); operators, `for-in` and `Display` (S5, which reuse the
call-site check of section 4.4); associated types; impls for `Pool` and
`Handle`; tier-polymorphic traits; per-instantiation checking; a tier on a
`where` clause.
