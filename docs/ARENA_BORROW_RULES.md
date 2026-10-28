# Borrowing arena values across calls (roadmap D1)

Status: **provisional**. Written 2026-10-28 (LLVM PoC, chat 3). Implements the owner's decision D1
in `docs/LLVM_ROADMAP.md`: a callee may take a reference into the caller's arena and read it, but
may not store it or return it; its lifetime is the caller's arena. This file records the rules, the
experiments behind them, what is tested, and what is still open. The sema change is the patch to
`crates/core/src/sema/type_infer.rs` delivered with it.

Tags: **checked** (run in this pass), **reasoned** (follows from what was read, not run),
**not covered** (known gap).

---

## 1. Where the rejection lives

Values built inside `with arena(...)` are typed `ArenaRef` (`docs/MEMORY_MODEL.md` section 6). For
`&x` the tag sits one layer inside the reference. `call_return_type` in `type_infer.rs` unifies each
argument with the declared parameter type, and `unify` reports `ArenaRefEscapesBoundary` whenever
one side carries the tag and the other does not. So today **every** arena argument is rejected:
by value, as `&x`, to a `[lifetime L]` parameter, to a closure (checked, probes `p1` to `p7`).

The same blunt rule already has one deliberate exception: `unify_struct_field` strips the tag for an
edge struct's `&L` field and leaves enforcement to `outlives_check`. D1 needs the same kind of
exception at call arguments, with the enforcement made explicit, because `outlives_check` does not
know about arena scopes (it orders loans by where they are issued in the caller).

## 2. Is the rejection conservative, or does it guard something?

Both. Experiments in a scratch clone (checked):

- **Naive stripping is unsound.** Strip the tag whenever the parameter is `&L T` and this passes
  sema: `fn pick [lifetime L] (a: &L Vec2) &L Vec2 { return a }`, then `keep = pick(&v)` inside the
  arena with `keep` declared outside it. The declared result type carries no tag, so the borrow
  leaves the arena unnoticed. The interpreter prints the right number only because its memory is
  collected; compiled code would read a recycled arena chunk.
- **A `where L outlives M` clause is another way out.** It lets a callee tie the borrow to a
  longer-lived parameter, and the caller's loan ordering cannot see that an arena ends earlier.
- **Two channels are unchecked for ordinary references too** (the unmodified build accepts both):
  a closure that captures a reference parameter and is returned (probe `e8b`), and a callee that
  stores a reference parameter into a struct passed alongside, `out.r = v` (probe `e9`). They are
  not tracked even when no arena is involved. They are a pre-existing gap, not caused by D1, but
  D1 would have routed arena memory through them, so the rules below close them for arena values.

## 3. The rules (v1)

A call may pass `&x` or `&mut x`, where `x` lives in an arena, when all of these hold:

- **R1, what is lent and to whom.** The argument is a reference to an arena value; the callee is a
  plain identifier naming a non-async function; the parameter is a reference. A struct
  passed by value, a method call, a closure and a function-typed variable are not lending targets
  and stay rejected.
- **R2, the parameter's lifetime goes nowhere.** It is not named in a `where` clause, and if it is
  named it appears exactly once in the signature (this parameter). An unnamed reference qualifies.
- **R3, the result cannot carry a reference.** Nothing in the return type can hold a reference, a
  closure, a task or an unresolved generic. Struct fields and enum payloads are followed.
- **R4, no other parameter is a place to store it.** Looking through reference layers to the
  pointee, no other parameter's type can hold a reference, closure or task. A `Vec2` of two ints
  cannot; a holder with a `&M` field can.

What a callee may then do: read through the reference, read its fields, pass it to other
functions, compute and return plain values. What it cannot do, because its signature gives it no
way: return it, store it, or capture it in something that outlives the call. So the borrow's
lifetime is the call, which is inside the caller's arena. The result is not re-tagged, because
by R3 it cannot contain the borrow.

`&mut` is allowed under the same rules; a callee may change the fields of an arena value it
borrows. Lifetime names are erased by the emitter, so R2 is the only thing they do here, and the
unnamed form is the simplest to write.

## 4. What changed

`type_infer.rs`, seven anchored edits: a map from function to its `where` lifetimes (filled when
signatures are collected), a function that finds the callee's definition, `count_ref_lifetime`
(exhaustive over `SemaType` on purpose, so a new variant must say whether it can hold a
reference), `can_carry_ref`, `param_is_sink_free`, `param_is_borrow_only`, `strip_arena_borrow`,
and the argument loop in `call_return_type`, which now also receives the callee expression. The
emitter needed no change: a reference is the same pointer as the struct, and the lifetime is
erased. The diagnostic is still the generic one; naming which of R1 to R4 failed would need a new
error variant and is left out of this change.

## 5. Tests

| what | result |
|---|---|
| 5 `ok_arena_borrow_*` and 10 `err_arena_borrow_*` fixtures (this delivery) | all behave as stated in their headers (checked) |
| `crates/llvm_poc/examples/18_arena_borrow.ubl` | identical to the interpreter at -O0, -O2 and under ASan+UBSan, no arena left alive (checked) |
| every existing fixture and PoC example (312 files) through `check`, before and after | no difference (checked) |
| `run_equiv.sh` over all PoC examples | 37 passed, 0 failed (checked) |
| mutation: drop R3 | `err_arena_borrow_closure_capture` and `_elided_result_ref` go red |
| mutation: drop R4 | the three `_stored_` fixtures go red |
| mutation: drop the `where` check, or the single-use check | `_where_clause`, or `_shared_lifetime`, goes red |

`err_arena_borrow_returned` is guarded twice (R2 and R3), so no single mutation turns it red;
`_by_value` and `_closure_callee` rest on the pre-existing rule and on the two gates in R1.
Not run: the `cargo test` suites and the `pipeline` example (the sandbox has cargo 1.75 and the
root lockfile needs 1.78 or newer); CI runs them.

## 6. Not covered

- **Closure captures and field stores are still unchecked for ordinary references** (section 2).
  The proper fix is to check a reference stored into a field against the holder's lifetime, and to
  track what a closure captures. That is also what D2 and A7 need.
- **Methods and trait methods** go through a separate argument loop and still reject arena
  arguments.
- **Closures as lending targets** wait for D2 (does a function type carry a tier) and A7.
- **Region-polymorphic results** (`pick` above) are refused. Allowing them means instantiating `L`
  with the caller's arena and tagging the result, and `SemaType::Named` carries no lifetime
  arguments, so a returned edge struct could not be tagged. A possible v2.
- **Generic callees** never qualify: any type parameter counts as able to hold a reference.

## 7. For the owner and the traits chat

- D1's wording says the callee may not *return or store* the borrow. v1 enforces that through the
  signature: any signature that could return or store it is refused. A callee that returns `&L`
  is the v2 case above; say if it is wanted.
- `type_infer.rs` is shared with the traits chat. The patch uses anchors, so a moved anchor is
  reported and skipped, never silently misapplied. But a whole-file replacement of `type_infer.rs`
  built from a copy older than this patch would drop it: refresh from master before the next one.
- When this lands, `docs/LLVM_ROADMAP.md` should mark D1 implemented and link here; that edit is
  left out because the same file is being rewritten by other deliveries.
