# Ownership errors (BORROW, MOVE, LIFETIME)

LOW-tier memory safety: borrow conflicts, use after move, and declared lifetimes. Each family is its own check; they are listed together because they serve the same goal.

Every example below was compiled with the real `ubel` binary, and the output shown is exactly what it printed. Entries marked *no verified example yet* come from the compiler's diagnostics registry (`docs/DIAGNOSTICS_RULES.md`) but could not be reproduced from ordinary source when this page was written; the registry text is given as is.

## Borrow checking

### BORROW-001

**ConflictingAccessWhileBorrowed** — `` cannot use `place` while it is mutably borrowed ``

A `&mut` loan is still live (it will be read again later) at the point another statement reads or re-borrows the same place. The check is non-lexical: a conflicting use after the borrow's last use is fine. Only mutable loans bound to a local are checked today.

<!-- ubel-test: error BORROW-001 -->
```ubel
@tier(low)
fn conflict_while_live() int {
    let mut n = 10
    let p = &mut n
    let y = n
    return *p + y
}

fn main() void {
    let result = conflict_while_live()
}
```

```text
>>> error[BORROW-001]: cannot use `n` while it is mutably borrowed
  --> 5:5
    |
  5 |     let y = n
    |     ^^^^^^^^^
  ~> note: mutable borrow occurs here
    --> 4:13
      |
    4 |     let p = &mut n
      |             ^^^^^^
  <~
  ~> help: move this use before the borrow's last use, or restructure so the borrow doesn't need to outlive it
  <~
<<<
```

**Fix:** Move the conflicting use before the borrow's last use, or restructure so the borrow does not need to live that long.

## Move checking

### MOVE-001

**UseAfterMove** — `` use of `place` after it was already moved ``

A `Unique<T>` local is used again after an earlier bare use consumed it, with no reassignment in between. The analysis is may-analysis: if the value might already be moved on some path, it is rejected. A value consumed on every loop iteration without being reassigned is caught too, in which case both spans point at the same line. Only `let`-bound locals are tracked, and a method call on the value counts as a move of it.

<!-- ubel-test: error MOVE-001 -->
```ubel
@tier(low)
fn main() void {
    let a = Unique.new(42)
    let b = a
    let c = a
}
```

```text
>>> error[MOVE-001]: use of `a` after it was already moved
  --> 5:13
    |
  5 |     let c = a
    |             ^
  ~> note: value moved here
    --> 4:13
      |
    4 |     let b = a
      |             ^
  <~
  ~> help: `a` is `Unique<T>` -- move-only, not `Copy`. Borrow it instead (`&a`/`&mut a`) if the earlier use didn't need to consume it, or reassign a fresh value to `a` before this point
  <~
<<<
```

**Fix:** Borrow instead of moving (`&a` / `&mut a`), or assign a fresh value before the second use.

## Lifetimes

### LIFETIME-001

**UndeclaredLifetime** — `` undeclared lifetime `name` ``

A lifetime name is used in a `where X outlives Y` clause, or written as `&name T` in a signature or `edge struct` field, that the declaration's own `[lifetime ...]` list does not declare.

<!-- ubel-test: error LIFETIME-001 -->
```ubel
fn foo [lifetime L where L outlives M] () void {}

fn main() void {
}
```

```text
>>> error[LIFETIME-001]: undeclared lifetime `M`
  --> 1:26
    |
  1 | fn foo [lifetime L where L outlives M] () void {}
    |                          ^^^^^^^^^^^^
  ~> help: add `lifetime M` to this declaration's `[...]` list, or use a name it already declares
  <~
<<<
```

**Fix:** Add `lifetime name` to the declaration, or use a name it already declares.

### LIFETIME-002

**DuplicateLifetimeParam** — `` lifetime `name` declared more than once ``

The same lifetime name appears twice in one `[lifetime ...]` list. A note points at the first.

*No verified example yet.*

**Fix:** Remove one, or rename it.

### LIFETIME-003

**OutlivesCycle** — `` lifetime `name` cannot outlive itself / outlives constraints form a cycle among ... ``

Declared `outlives` constraints must form a strict ordering: no lifetime may directly or indirectly outlive itself.

<!-- ubel-test: error LIFETIME-003 -->
```ubel
edge struct Broken [lifetime L where L outlives L] {
    value: &L int
}

edge struct Fine [lifetime L] {
    value: &L int
}

fn helper(n: int) int {
    return n + 1
}

fn main() void {
}
```

```text
>>> error[LIFETIME-003]: lifetime `L` cannot outlive itself
  --> 1:38
    |
  1 | edge struct Broken [lifetime L where L outlives L] {
    |                                      ^^^^^^^^^^^^
  ~> help: outlives relationships must form a strict ordering, with no lifetime (directly or indirectly) outliving itself
  <~
<<<
```

**Fix:** Remove a constraint from the cycle.

### LIFETIME-004

**BoundaryTooShort** — `` value doesn't live long enough for declared lifetime `L` ``

At a call to a function with a `[lifetime L]` parameter, or a struct literal for a struct with one, the borrowed value was already invalidated (for example by reassigning the borrowed-from place) before reaching that point. A note points at the loan.

<!-- ubel-test: error LIFETIME-004 -->
```ubel
edge struct Cache [lifetime L] {
    item: &L int
}

@tier(low)
fn build_after_kill() int {
    let mut n = 42
    let p = &n
    n = 7
    let c = Cache { item = p }
    return *c.item
}

fn main() void {
    let value = build_after_kill()
}
```

```text
>>> error[LIFETIME-004]: value doesn't live long enough for declared lifetime `L`
  --> 10:28
     |
  10 |     let c = Cache { item = p }
     |                            ^
  ~> note: borrow occurs here
    --> 8:13
      |
    8 |     let p = &n
      |             ^^
  <~
  ~> help: move this call/construction before the loan's last use, or restructure so the loan lives at least as long as this site
  <~
<<<
```

**Fix:** Move the call or construction before the loan's last use, or restructure so the loan lives long enough.

### LIFETIME-005

**OutlivesConstraintViolated** — `` `L` doesn't actually outlive `M` at this site ``

A function declares `where L outlives M`, each argument passes its own check, but the arguments' real regions do not have the promised ordering.

<!-- ubel-test: error LIFETIME-005 -->
```ubel
@tier(low)
fn pair [lifetime L, lifetime M where L outlives M] (a: &L int, b: &M int) int {
    return *a + *b
}

@tier(low)
fn combine() int {
    let n = 5
    let shorter_ref = &n
    let longer_ref = &n
    return pair(longer_ref, shorter_ref)
}

fn main() void {
    let value = combine()
}
```

```text
>>> error[LIFETIME-005]: `L` doesn't actually outlive `M` at this site
  --> 2:39
    |
  2 | fn pair [lifetime L, lifetime M where L outlives M] (a: &L int, b: &M int) int {
    |                                       ^^^^^^^^^^^^
  ~> note: `L` bound here
    --> 11:17
       |
    11 |     return pair(longer_ref, shorter_ref)
       |                 ^^^^^^^^^^
  <~
  ~> note: `M` bound here
    --> 11:29
       |
    11 |     return pair(longer_ref, shorter_ref)
       |                             ^^^^^^^^^^^
  <~
  ~> help: whatever's bound to `L` at this site must live at least as long as whatever's bound to `M` — pick arguments where that actually holds, or drop the `L outlives M` requirement if it isn't needed
  <~
<<<
```

**Fix:** Pass arguments where the ordering holds, or drop the constraint if it isn't needed.

### LIFETIME-006

**NonLocalBoundaryValue** — `` can't verify this value satisfies declared lifetime `L` ``

The conservative case: the value is a local that is neither one of the caller's own parameters nor bound to a known loan (typically it came from another call's return value), so the checker cannot prove it is fine and rejects it, even in cases that are actually safe.

<!-- ubel-test: error LIFETIME-006 -->
```ubel
@tier(low)
fn take [lifetime L] (v: &L int) &L int {
    return v
}

@tier(low)
fn relay [lifetime L] (original: &L int) int {
    let p = original
    let result = take(p)
    return *result
}

fn main() void {
    let n = 9
    let value = relay(&n)
}
```

```text
>>> error[LIFETIME-006]: can't verify this value satisfies declared lifetime `L`
  --> 9:23
    |
  9 |     let result = take(p)
    |                       ^
  ~> help: bind this to a local first (e.g. `let tmp = ...; g(tmp)`) so its lifetime can be traced, or pass/store a fresh borrow directly
  <~
<<<
```

**Fix:** Bind the value to a local first so its lifetime can be traced, or pass a fresh borrow directly.
