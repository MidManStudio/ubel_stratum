# Arena strings (D3b)

Status: PROVISIONAL. The sema half is the patch to `type_infer.rs` (re-issued with A6b: the closure patch was committed over the first copy before it ran, so check that `peel_arena_str` is in `type_infer.rs`). The emitter half is done: A6 (strings as values, built by site) and A6b (`+`, ordering).

## Decision

D3b: a string built at run time is allocated **where it is written**, exactly like a struct literal:
inside `with arena(...)` it lives in the arena, anywhere else it lives in the GC heap. Sema tracks
this so an arena string cannot outlive its block. (The alternative, D3a, allocated every built string
in the GC heap and needed no sema change. It was offered and not chosen.)

## Why sema had to change

Before this change an interpolated string was a plain `string` everywhere. A function could write
`with arena(1KB) { return $"n={n}" }`, or assign the string to a variable declared outside the block,
and sema accepted both. With by-site allocation in native code those would return a pointer into an
arena that has ended. Struct literals never had the problem (they are typed `&arena T`); strings had
no tag.

## Rules

| Rule | Statement |
|------|-----------|
| S1 | An interpolated string (`$"..."`) written inside `with arena` has type `&arena string`. Outside any arena it is a plain `string`. |
| S2 | String literals are plain `string` (static data) and can flow anywhere. |
| S3 | `==`, `!=`, `<`, `<=`, `>`, `>=` read their operands; an arena string is accepted on either side and the result is a `bool`. |
| S4 | `+` reads its operands and its result is a plain `string`, always allocated in the GC heap (it copies bytes, so it holds no borrow). **Exception to by-site**, kept so `log = log + "..."` inside an arena block stays legal: `ok_integration_game_events.ubl` does exactly that. |
| S5 | An arena string follows every escape rule an arena struct does: it cannot be returned out of the block, assigned to an outer variable, stored in a field of an outer struct, or passed to a by-value `string` parameter. |

Everything is in `crates/core/src/sema/type_infer.rs`: one helper (`peel_arena_str`), one line in the
interpolation literal arm, one line at the top of `binop_result`.

## Stricter than necessary (known, accepted for now)

- `let s = if c { $"yes{n}" } else { "no" }` inside an arena is rejected: the branches are `&arena string`
  and `string`, and sema has no "a plain string may stand where an arena string is expected" coercion.
  Workaround: build both branches with `$"..."`, or compute the branch outside the arena.
- `log = $"{log}x"` into an outer variable is rejected (S5). Use `+` (S4).
- An arena string cannot be lent to a function through a by-value `string` parameter. A `&string`
  parameter goes through the D1 rules; not tested here.

## What the emitter does (A6, A6b: done)

- A literal is a static `{i64 length, bytes}` constant. `mark_ptr` ignores pointers outside the heap, so
  literals need no rooting.
- Interpolation outside an arena: a GC object of `8 + length` bytes with mask 0. Inside: bump `8 + length`
  in the current arena (a string holds no pointers), but only when sema tags the literal `&arena string`
  (this patch); without it the emitter refuses.
- `+` on strings: always a GC object, wherever it is written; refused in LOW and in MID outside `with arena`.
- Ordering: byte-wise, `ubel_str_cmp`.
- A string field in a struct or enum allocated in an arena is allowed: such an arena is created
  scannable (docs/LLVM_POC.md, "Scannable arenas"), so a GC string stored there is kept alive.
- Refused, for now: building a string (interpolation or `+`) in a LOW function.

## Evidence

Probed on a fresh build of master, then on the patched build. 328 existing fixtures and examples gave
identical `check` output before and after (one fixture, `ok_integration_game_events`, differed while `+`
was still tagged by site; that is why S4 exists).

| Probe | Before | After |
|-------|--------|-------|
| return an interpolated string out of the block | accepted | rejected |
| assign it to an outer variable | accepted | rejected |
| store it in a field of an outer struct | accepted | rejected |
| `log = $"{log}x"` into an outer variable | accepted | rejected |
| pass it to a by-value `string` parameter | rejected (tier) | rejected (escape) |
| `println`, `let`, nested interpolation, `==`, `!=`, `<`, `+`, `.len()` inside the block | accepted | accepted |
| `log = log + tag` into an outer variable | accepted | accepted |

Tests (`tests/fixtures`): `ok_arena_string_basic`, `ok_arena_string_concat_outer`,
`err_arena_string_returned`, `err_arena_string_assigned_outer`, `err_arena_string_stored_field`,
`err_arena_string_accumulate`, `err_arena_string_by_value_param`. All five err fixtures are accepted by
the unpatched build, so they fail without the change.

Not covered: method-call arguments of methods other than the built-in string ones (a separate loop in
`type_infer`), closures capturing an arena string, format specs on an arena string hole, `&string`
parameters.

Checked 2026-11-06 (A9): the built-in string methods that read a string (`len`, `is_empty`, `contains`,
`starts_with`, `ends_with`) are accepted on an arena string as receiver and as argument, in `@tier(mid)`
inside `with arena`; sema checks only their argument count, and they allocate nothing, so no borrow is
created. The methods that build a new string (A9c) are done as rule S4, a GC copy: sema types the result of such a
method on an arena receiver with the receiver's arena tag (probed: `a.trim().to_upper()` cannot be returned out
of the block), so the native result, a GC string, is stricter than its type needs, never looser. The
alternative, allocating in the arena when the receiver is an arena string, is open and needs the owner's call.

Found 2026-11-07 (A9b): `match` on an arena string is rejected by sema. In
`with arena(1KB) { let a = $"k{n}"; return match a { "k1" => 10, _ => 0 } }` each literal pattern is typed
`&arena string` and reported as "scoped to a `with arena(...)` block and cannot outlive it". Comparing a string
with a literal pattern reads it and creates no borrow, so peeling the arena tag as `peel_arena_str` does for
`==` would be sound here too; that is a change in `type_infer`, which the owner edits, and it is not made. Until
then, a string built in an arena can be matched by first copying it with `+` (a GC string, rule S4) or compared
with `==`.
