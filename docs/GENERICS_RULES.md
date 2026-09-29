# Generics — Design & Implementation Notes

**Status:** Implemented for structs, enums, and free functions. Not yet
implemented: a struct/enum method's *own* extra generic params, trait
bounds enforcement, `impl`/`extend` blocks on generic structs.

---

## 0. What was actually true before this

Before this round, generics didn't work anywhere in the compiler — not
just for enums. `T` inside any generic struct/enum/fn's own signature
resolved straight to `SemaType::Unknown`, and `unify()` absorbs `Unknown`
silently on either side. `ok_generics.ubl`'s `Box<T>`/`identity<T>`
fixture passed with "zero errors expected" for that reason alone, not
because anything was actually checked.

Two other, previously-invisible gaps turned out to be prerequisites:

- **`self` was unconditionally `Unknown`** inside every method body
  (`current_struct_type` was declared as a TODO, never threaded in) — a
  generic struct's methods can't be checked at all without this, since
  every method body starts with `self`.
- **General struct field/method resolution didn't exist at all**, generic
  or not. `foo.field` was a literal `// TODO: field table` returning
  `Unknown` unconditionally; `TypeName.method(..)` and `value.method(..)`
  weren't dispatched to anything struct-aware. Making `Box<T>` real
  meant building the non-generic foundation underneath it first, since
  there wasn't one to substitute on top of.

This doc covers all three together, since none of them were separable in
practice — the actual implementation is a single connected pass.

---

## 1. The core mechanism

### `SemaType::Param(usize)`

A positional placeholder for "the enclosing struct/enum/fn's own Nth
generic param." Only ever appears in the *raw*, stored signature of a
generic declaration — `struct_fields`, `struct_methods`, `enum_variants`,
or a `SemaType::Function`'s `params`/`return_type`. `Param` is interned
by index (see `Internable::Param` in `type_table.rs`), so every
reference to "generic param 0 of the declaration currently being
processed" — whether built while collecting the signature, or rebuilt
independently later while checking the body — is the *identical*
`TypeId`, not merely an equal one. This matters: `unify`'s fast path is
raw `TypeId` equality, and two structurally-identical but numerically
different placeholders would otherwise fail to unify against each other
(a real bug caught while building this — `self.value` inside a generic
method's own body failed against its own declared return type until
`Param` was made internable).

### `current_generic_params: HashMap<String, TypeId>`

Active only while collecting the signature or checking the body of the
struct/enum/fn currently being processed. `push_generic_scope`/
`pop_generic_scope` build and restore it. `ast_type_to_sema`'s
`TypeKind::Named` arm checks this *before* falling back to
`top_level_def`, so a bare `T` that names one of the enclosing decl's own
generic params resolves to its `Param` placeholder instead of silently
becoming `Unknown`. Also validates argument count against the target
type's declared arity (`GenericArgCountMismatch`, TYPE-108 — this
diagnostic already existed, unused, before this round).

### `substitute(ty, args) -> TypeId`

Recursively replaces every `Param(i)` inside `ty` with `args[i]`, walking
every composite `SemaType` variant (List/Dictionary/Tuple/Array/
ArenaRef/PoolRef/OwnedRef/Function/Named/…). A type containing no `Param`
is reconstructed unchanged — correctness over cheapness, matching this
file's existing style elsewhere. This is the one function every generic
use site funnels through.

### `instantiate(def_id) -> TypeId`

Builds a *fresh* instantiation — one new `Var` per the def's own declared
generic params — for a bare construction site with no already-known
concrete args (`Option.None`, `Option.Some(5)`, `Box.new(42)`,
`Rectangle { width = w, height = h }`). The fresh `Var`s bind through
ordinary unification against however the constructed value is actually
used, the same way an untyped `[]` literal's element type already got
resolved from context before any of this round's work. For a
non-generic def (`generic_arity == 0`) this is just the cheap,
already-computed `def_type` — no allocation.

### Where concrete args come from at a *use* site (not a construction)

No fresh `Var`s needed — the args are already known from whatever was
constructed earlier: an instance method call (`boxed.unwrap()`) or field
access (`p.first`) substitutes using the *receiver's own* `Named { args,
.. }`, inferred back when the receiver itself was built.

---

## 2. Where it's wired in

| Site | Before | Now |
|---|---|---|
| `collect_struct_sig` / `collect_enum_sig` | `Named { args: vec![] }` unconditionally | Pushes the decl's own generic scope; stores raw (`Param`-containing) field/variant/method shapes; records arity |
| `collect_fn_sig` | No generic scope | Same, plus `SemaType::Function` gained a `generic_arity: usize` field |
| `enum_shapes_of` | Discarded the scrutinee's own `args` entirely | Substitutes before returning — every existing enum consumer (`check_pattern`, construction) got real generics for free, no changes of their own needed |
| `self` (`ExprKind::SelfExpr`) | `Unknown` | `current_struct_type`, pushed per struct in `infer_struct_bodies` |
| `foo.field` (`ExprKind::Field`, general arm) | `Unknown`, no field table | Real lookup via `struct_fields`, substituted through the receiver's `args`; `NoSuchField` on a genuine typo |
| `Box.new(42)` (static/associated call) | Fell through to `Unknown` | Dispatches via `struct_methods` (method with no `self` param); fresh `Var`s per the struct's own arity |
| `boxed.unwrap()` (instance call) | Fell through to `Unknown` | Dispatches via `struct_methods` (method *with* `self`); substitutes using the receiver's own `args` |
| `Rectangle { width = w, .. }` (plain struct literal) | Fields inferred but never checked against declared types | Real per-field unification, `NoSuchField` on typos |
| `call_return_type` (plain function calls) | Returned the declared return type; args inferred individually but **never unified against params at all** — true for *every* function call, generic or not | Real per-param unification; `generic_arity` fresh-`Var`s make this uniform for generic and non-generic calls alike |

### Two `structurally_compatible` gaps that were prerequisites, not generics-specific

- **`Function`-vs-`Function` had no case at all** — two function-typed
  values could only ever unify via the exact-`TypeId` fast path. Surfaced
  immediately once `call_return_type` started really checking args: an
  untyped lambda passed where a `fn(int) int` param was declared
  (`ok_lambda_scope.ubl`) has no way to bind its own inferred `?T` param
  type against the declared `int` without recursing into both sides.
  Fixed with pairwise param unification + return-type unification.
- **`Named`-vs-`Named` compared only the `def`, ignoring `args` entirely**
  — meaning two independently-built instantiations of the same generic
  type were "compatible" by def-equality alone, with the actual type
  arguments never reconciled. This is load-bearing for the single most
  ordinary generic pattern there is — `let x: Option<int> =
  Option.None` — since the annotation and the initializer each build
  their own separate `Named { Option, args: [...] }`, and without
  pairwise `args` unification the initializer's fresh `Var` never binds
  to `int` at all. Fixed the same way as `Function`.

---

## 3. Known gaps (real, not hidden)

- **A method's own extra generic params** (beyond whatever the enclosing
  struct/enum already declares) aren't substituted at call sites. No
  current fixture needs this; `collect_method_sig` records the method's
  own `generic_arity` but nothing currently allocates fresh `Var`s for it
  at a call site the way free functions and struct associated-calls do.
- **`impl`/`extend` blocks on a generic struct** aren't wired to that
  struct's own generic scope — `current_generic_params` is only pushed
  around a struct's *inline* members. No current fixture uses `impl`/
  `extend` at all (checked: zero hits in `tests/fixtures/*.ubl`).
- **Trait bounds** (`GenericParam.bounds`, parsed, stored, never
  validated) are still completely unenforced — same as before this round.
- **A diagnostic-duplication wart, now fixed**: a genuinely unknown
  method name called on an instance (`rect.shrink()` where `shrink`
  doesn't exist) used to fire both `NoSuchField` (from the callee's own
  standalone pre-inference, which the `Call` arm always runs first) and
  `NoSuchMethod` (from the dedicated call-dispatch check) for the same
  typo. The `Call` arm now sets `callee_field_pending` just before
  pre-inferring a `Field` callee and the `Field` arm takes it before
  inferring anything, so it applies to that one node only; a typo in
  the callee's own target (`o.innr.n()`) is still reported. Only
  `NoSuchMethod` fires for the call itself. Not covered by this fix:
  an unknown method on an `enum` receiver, which sema never reported.
- **Generic enums have no prelude** — `Option`/`Result` are ordinary
  user-declarable generic enums, not builtins. A fixture that wants them
  declares them locally (matching how `ok_enum_payloads.ubl` already
  declared a concrete, non-generic `Result` before this round). No
  prelude/auto-injection mechanism exists.
- **An unconstrained fresh `Var`** (e.g. `Option.None` used somewhere its
  `T` never gets pinned down by anything) silently resolves however an
  unresolved `Var` already resolved before this round — this file's own
  "no occurs-check in unification" rough edge, not a new one.

---

## 4. New fixtures

- `ok_generic_enums.ubl` — `Option<T>`/`Result<T,E>` construction,
  pattern matching, two different instantiations of the same enum in one
  program, and `let b: Option<int> = Option.None` (annotation-driven
  inference for a fieldless variant with no payload to infer `T` from).
- `ok_generic_struct_pair.ubl` — a two-type-param struct (`Pair<A, B>`)
  whose `swap()` method returns `Pair<B, A>` — genuine substitution with
  the params reordered, not just single-`T` identity passthrough.
- `err_generic_arg_count.ubl` — `Option<int, string>` against a
  one-param enum (`GenericArgCountMismatch`).
- `err_generic_enum_type_mismatch.ubl` — passing `Option<string>` where
  `Option<int>` is declared (proves the two instantiations are genuinely
  distinct, checked types now, not both silently `Unknown`-compatible).
- `err_struct_no_such_field.ubl` / `err_struct_no_such_method.ubl` — the
  general (not generics-specific) struct field/method error paths, which
  had zero fixture coverage before this round since the underlying
  checking didn't exist.
- `err_fn_call_arg_count.ubl` — a plain, non-generic function called with
  the wrong argument count (`call_return_type` never checked this at all
  before, generic or not).

---

## 5. A parser bug found along the way (not a sema issue)

`let b: Option<int> = Option.None` failed to *parse* — `unexpected `<``
— before any of the above sema work mattered. Root cause: `rd_parser`'s
`try_parse_generic_args` (used by `parse_type_base`'s `Named` arm, and by
the `impl Foo<T> for Bar` trait-impl check in `parse_decl.rs` — both
unambiguous type-grammar positions) speculatively backed out and left the
`<` unconsumed whenever the token right after the closing `>` was `=` or
`==`, on the theory that it might really have been a chained comparison
(`a < b >= c`). That heuristic only makes sense in *expression* position
— and nothing in `parse_expr.rs`'s Pratt loop actually calls this
function at all (checked). In its two real call sites, both type-grammar
positions, it only ever misfired — `Option<int>` immediately followed by
`=` is the single most ordinary shape a generic type annotation takes.
Removed the guard entirely; see the comment left in `try_parse_generic_args`.

---

## 6. Naming note

`Box<T>` (in `ok_generics.ubl`) and `Rectangle`/`Pair<A,B>` (new fixtures)
are arbitrary test-fixture names for exercising the generic-struct
mechanism — none of them are, or were ever meant to be, the language's
real stdlib memory-wrapper type. That naming decision (`Unique<T>` vs
`Heap<T>` vs other options, plus `Shared<T>`/`SyncShared<T>`/`Span<T>` vs
`&[T]`) is tracked separately and still open.
