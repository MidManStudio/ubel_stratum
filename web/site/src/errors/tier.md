# Tier and memory-region errors (TIER)

These enforce the [tier model](../tier-model.md): which tier may call which, where arenas, pools and the ownership wrappers may be used, and that references into a region never outlive it. `TIER-007` is retired (it belonged to LINQ query syntax, since removed in favour of `Linqerizer<T>`) and its number is not reused.

Every example below was compiled with the real `ubel` binary, and the output shown is exactly what it printed. Entries marked *no verified example yet* come from the compiler's diagnostics registry (`docs/DIAGNOSTICS_RULES.md`) but could not be reproduced from ordinary source when this page was written; the registry text is given as is.

## Tier rules

### TIER-001

**ArenaInWrongTier** — `` `with arena` is only valid in `@tier(mid)`; this function is `@tier(x)` ``

A `with arena(...)` block appears in a function that is not MID tier.

<!-- ubel-test: error TIER-001 -->
```ubel
fn main() void {
    with arena(64KB) {
        let scratch = [1, 2, 3]
    }
}
```

```text
>>> error[TIER-001]: `with arena` is only valid in `@tier(mid)`; this function is `@tier(high)`
  --> 2:5
    |
  2 |     with arena(64KB) {
    |     ^^^^^^^^^^^^^^^^^^
  ~> help: annotate this function with `@tier(mid)` or remove the arena block
  <~
<<<
```

**Fix:** Annotate the function `@tier(mid)`, or remove the arena block.

### TIER-002

**AwaitInWrongTier** — `` `await` is only valid in `@tier(high)`; this function is `@tier(x)` ``

`await` is used outside HIGH tier.

*No verified example yet.*

**Fix:** Annotate the function `@tier(high)`, or remove the `await`.

### TIER-003

**AsyncFunctionNotHigh** — `` async functions must be `@tier(high)`; this function is `@tier(x)` ``

An async function declared outside HIGH tier. In the compiler as it stands, `async fn` under `@tier(mid)` is already reported by the parser as [PARSE-004](./parse.md#parse-004), so that is what you will see for that case.

*No verified example yet.*

**Fix:** Add `@tier(high)`, or remove `async`.

### TIER-005

**IllegalTierCall** — `` `@tier(caller)` code cannot call `@tier(callee)` function `name` ``

A call crosses tiers in a direction the call matrix forbids. HIGH may call MID and LOW; MID may call LOW; LOW may call only LOW. The same rule applies to a call by name, a method call `value.method()`, `Type.assoc()`, a call through a trait bound, and a qualified call `Trait.f(v)`.

<!-- ubel-test: error TIER-005 -->
```ubel
fn helper() int {
    return 1
}

@tier(low)
fn tight() int {
    return helper()
}

fn main() {
}
```

```text
>>> error[TIER-005]: `@tier(low)` code cannot call `@tier(high)` function `helper`
  --> 7:12
    |
  7 |     return helper()
    |            ^^^^^^^^
<<<
```

**Fix:** For a HIGH caller of a LOW function, wrap the logic in a MID function. Otherwise move the call to a function of an allowed tier.

### TIER-008

**MethodInWrongTier** — `` `.method()` is only valid in `@tier(high)`; this function is `@tier(x)` ``

A method that is only available in HIGH tier is called from MID or LOW. The query methods that produce a `Linqerizer` are one.

<!-- ubel-test: error TIER-008 -->
```ubel
@tier(mid)
fn summarize() void {
    let nums = List.new()
    nums.push(1)
    nums.push(2)
    let pipeline = nums.query()
}
```

```text
>>> error[TIER-008]: `.query()` is only valid in `@tier(high)`; this function is `@tier(mid)`
  --> 6:20
    |
  6 |     let pipeline = nums.query()
    |                    ^^^^^^^^^^^^
  ~> help: move this call to a `@tier(high)` function, or avoid `.query()` in `@tier(mid)`/`@tier(low)` code
  <~
<<<
```

**Fix:** Move the call to a `@tier(high)` function.

### TIER-009

**CollectionConstructionInLowTier** — `` `Collection.new()` is not yet supported in `@tier(low)` ``

`List`, `Dictionary`, `Queue` or `Stack` is constructed inside a LOW function. LOW tier has no allocation of its own; its memory model is move semantics and the borrow checker.

<!-- ubel-test: error TIER-009 -->
```ubel
@tier(low)
fn build() void {
    let items = List.new()
}

fn main() void {
    build()
}
```

```text
>>> error[TIER-009]: `List.new()` is not yet supported in `@tier(low)` — LOW tier's memory model (move semantics + borrow checker) hasn't been built yet
  --> 3:17
    |
  3 |     let items = List.new()
    |                 ^^^^^^^^^^
  ~> help: construct the `List` in a `@tier(mid)` or `@tier(high)` function instead, or restructure this function to receive it as a parameter
  <~
<<<
```

**Fix:** Construct the collection in a MID or HIGH function, or receive it as a parameter.

### TIER-015

**StaticAccessOutsideHigh** — `` the static `x` can only be used from `@tier(high)` code; this function is `@tier(actual)` ``

A `static` is a garbage-collected mutable global shared by every function. MID and LOW code has no sound way to hold a reference into it, so any read or write from there is rejected.

<!-- ubel-test: error TIER-015 -->
```ubel
static TOTAL: int = 0

@tier(low)
fn low_reader() int {
    return TOTAL
}

fn main() void {
    println($"{low_reader()}")
}
```

```text
>>> error[TIER-015]: the static `TOTAL` can only be used from `@tier(high)` code; this function is `@tier(low)`
  --> 5:12
    |
  5 |     return TOTAL
    |            ^^^^^
  ~> help: read the static in a `@tier(high)` function and pass the value in as a parameter, or return a result for a `@tier(high)` caller to store
  <~
<<<
```

**Fix:** Read the static in a HIGH function and pass the value in, or return a result for a HIGH caller to store.

### TIER-016

**TraitMethodTierMismatch** — `` method `m` in `impl Trait for Type` is `@tier(a)`, but trait `Trait` declares it `@tier(b)` ``

A trait method has a tier: its own `@tier`, otherwise the trait's, otherwise HIGH. A call through a bound is checked against that tier, so every impl method must have exactly the same one. An impl method that writes no tier takes the trait's. The prelude traits are LOW.

<!-- ubel-test: error TIER-016 -->
```ubel
trait Tr {
    fn f(self) int
}

struct S {
    n: int
}

impl Tr for S {
    @tier(low)
    fn f(self) int { return 1 }
}

fn main() {
}
```

```text
>>> error[TIER-016]: method `f` in `impl Tr for S` is `@tier(low)`, but trait `Tr` declares it `@tier(high)`
  --> 11:5
     |
  11 |     fn f(self) int { return 1 }
     |     ^^^^^^^^^^^^^^^^^^^^^^^^^^^
  ~> help: remove the tier written for this method (on it, on its `impl`, or on a tier block around it) to take the trait's tier, or write `@tier(high)`
  <~
<<<
```

**Fix:** Remove the tier on the impl method to take the trait's, or write the trait's tier.

### TIER-017

**LowCallsUnknownClosure** — `` `@tier(low)` code cannot call the closure `f`: reason ``

LOW code may call a closure only when it is an immutable `let` bound straight to a lambda in that same function. A closure has the tier of the function that created it and a function type carries no tier, so a parameter, a `mut` binding or a closure returned by a call could belong to a higher tier. MID and HIGH code may call any closure.

<!-- ubel-test: error TIER-017 -->
```ubel
@tier(low)
fn run(f: fn(int) int, x: int) int {
    return f(x)
}

fn main() void {
    println(run(fn(v: int) v + 1, 2))
}
```

```text
>>> error[TIER-017]: `@tier(low)` code cannot call the closure `f`: it is a parameter, and a function type carries no tier, so it could be a closure from a higher tier
  --> 3:12
    |
  3 |     return f(x)
    |            ^^^^
  ~> help: bind the closure in this function with `let f = fn(...) ...` (not `mut`), or call it from a `@tier(mid)` or `@tier(high)` function
  <~
<<<
```

**Fix:** Bind the closure in this function with `let f = fn(...) ...` (not `mut`), or call it from a MID or HIGH function.

## Arenas and pools

### TIER-004

**ArenaRefEscapesBoundary** — `` value of type `T` is scoped to a `with arena(...)` block and cannot outlive it ``

A reference into an arena is returned, stored in a longer-lived place, or otherwise outlives the `with arena` block that owns the memory.

<!-- ubel-test: error TIER-004 -->
```ubel
@tier(mid)
fn make_list() List<int> {
    with arena(64KB) {
        return [1, 2, 3]
    }
}

fn main() void {
    let x = make_list()
}
```

```text
>>> error[TIER-004]: value of type `&arena List<?T7>` is scoped to a `with arena(...)` block and cannot outlive it — check the binding, struct field, or closure it's flowing into
  --> 4:9
    |
  4 |         return [1, 2, 3]
    |         ^^^^^^^^^^^^^^^^
  ~> help: copy the data out into a plain (non-arena) value before it leaves the `with arena(...)` block, or restructure with the callback pattern: pass a closure in and return a GC-owned value from inside it, instead of returning or storing the arena-scoped value directly
  <~
<<<
```

**Fix:** Copy the data out before the block ends, or use the callback pattern instead of returning the arena value.

### TIER-006

**MidReturnContainsArenaRef** — `` return type `T` contains an arena-lifetime reference; this makes the function uncallable from `@tier(high)` ``

A MID function's return type carries an arena lifetime.

*No verified example yet.*

**Fix:** Return an owned value instead.

### TIER-010

**PoolInWrongTier** — `` `with pool<T>(count)` is only valid in `@tier(mid)`; this function is `@tier(x)` ``

A `with pool` block in a function that is not MID tier. Same rule as [TIER-001](#tier-001).

<!-- ubel-test: error TIER-010 -->
```ubel
fn leaky() void {
    with pool<int>(4) {
        let p = Pool.new()
        let h = p.acquire(1)
    }
}

fn main() void {
    leaky()
}
```

```text
>>> error[TIER-010]: `with pool` is only valid in `@tier(mid)`; this function is `@tier(high)`
  --> 2:5
    |
  2 |     with pool<int>(4) {
    |     ^^^^^^^^^^^^^^^^^^^
  ~> help: annotate this function with `@tier(mid)` or remove the pool block
  <~
<<<
```

**Fix:** Move the pool block to a `@tier(mid)` function.

### TIER-011

**PoolRefEscapesBoundary** — `` value of type `&pool T` is scoped to a `with pool<T>(count) { }` block and cannot outlive it ``

The pool counterpart of [TIER-004](#tier-004), for the `Pool<T>` itself and anything acquired from it.

<!-- ubel-test: error TIER-011 -->
```ubel
@tier(mid)
fn nested() void {
    with pool<int>(4) {
        let mut outer_pool = Pool.new()
        with pool<int>(4) {
            outer_pool = Pool.new()
        }
    }
}

fn main() void {
    nested()
}
```

```text
>>> error[TIER-011]: value of type `&pool Pool<int>` is scoped to a `with pool<...>(...)` block and cannot outlive it — check the binding, struct field, or closure it's flowing into
  --> 6:13
    |
  6 |             outer_pool = Pool.new()
    |             ^^^^^^^^^^^^^^^^^^^^^^^
  ~> help: copy the data out into a plain (non-pool) value before it leaves the `with pool<...>(...)` block, or restructure with the callback pattern: pass a closure in and return a GC-owned value from inside it, instead of returning or storing the pool-scoped value directly
  <~
<<<
```

**Fix:** Copy the data out before the block ends, or use the callback pattern.

### TIER-012

**MidReturnContainsPoolRef** — `MID-tier function's return type contains a pool-lifetime reference`

The pool counterpart of [TIER-006](#tier-006). There is no surface syntax yet to write `Pool<T>` as an explicit return type, so today the same mistake is caught by [TIER-011](#tier-011).

*No verified example yet.*

**Fix:** Return an owned value.

### TIER-013

**PoolConstructedOutsideBlock** — `` `Pool.new()` requires an enclosing `with pool<T>(count) { }` block ``

`Pool.new()` reads its element type and capacity from the surrounding `with pool` block, so it has no meaning outside one.

<!-- ubel-test: error TIER-013 -->
```ubel
@tier(mid)
fn bad() void {
    let p = Pool.new()
}

fn main() void {
    bad()
}
```

```text
>>> error[TIER-013]: `Pool.new()` requires an enclosing `with pool<T>(count) { }` block to supply its element type and capacity
  --> 3:13
    |
  3 |     let p = Pool.new()
    |             ^^^^^^^^^^
  ~> help: wrap this call in a `with pool<T>(count) { }` block
  <~
<<<
```

**Fix:** Call `Pool.new()` inside a `with pool<T>(count) { }` block.

## Ownership wrappers

### TIER-014

**OwnershipWrapperOutsideLowTier** — `` `{Unique|Shared|SyncShared}.new()` is only valid in `@tier(low)`; this function is `@tier(x)` ``

The inverse of [TIER-009](#tier-009): collections are banned inside LOW because LOW has no memory model of its own, and the ownership wrappers are that memory model, so constructing them is banned everywhere except LOW. A HIGH or MID function may still receive one as a parameter.

<!-- ubel-test: error TIER-014 -->
```ubel
fn build_unique_wrong_tier() int {
    let u = Unique.new(42)
    return 1
}

fn main() void {
    let result = build_unique_wrong_tier()
}
```

```text
>>> error[TIER-014]: `Unique.new()` is only valid in `@tier(low)`; this function is `@tier(high)`
  --> 2:13
    |
  2 |     let u = Unique.new(42)
    |             ^^^^^^^^^^^^^^
  ~> help: annotate this function with `@tier(low)`, or receive the `Unique` as a parameter from LOW-tier code instead of constructing it here
  <~
<<<
```

**Fix:** Annotate the function `@tier(low)`, or receive the value as a parameter from LOW code.
