# The Tier Model

Every function in Ubel Stratum declares which memory strategy it uses.
The compiler enforces the rules statically, at every tier, regardless of
whether the program runs through the interpreter or, eventually, compiles
to native code.

| Tier | Annotation | Memory | `await` | Typical use |
|------|-----------|--------|---------|--------------|
| HIGH | `@tier(high)` (default) | Garbage collected | Allowed | Business logic, I/O |
| MID  | `@tier(mid)`            | Arena allocated    | Not allowed | Parsers, hot paths |
| LOW  | `@tier(low)`            | Manual, borrow-checked | Not allowed | Systems code, FFI, packet handling |

A function with no `@tier` annotation is HIGH. Lower tiers are opted into
for performance, never opted out of by default.

## The core constraint

A MID-tier function allocates data in an arena. Once that arena is freed,
every pointer into it becomes invalid, so HIGH-tier code must never hold a
live pointer into a freed arena. The type system enforces this at compile
time: a value living in arena `A` carries a type parameterized by the
arena's lifetime (`&'a T`), and the compiler rejects any program where an
`&'a T` appears in a type that outlives arena `A`.

Three patterns cross the MID to HIGH boundary safely. These are
illustrative of the pattern the tier checker enforces, not a claim that
these exact function names ship in a standard library yet; there is no
standard library today.

### Pattern 1: callback

MID parses into an arena, calls a HIGH-tier closure with a borrow into
the arena, the closure produces a GC-owned result, then the arena frees.
The closure's borrow never outlives the arena.

<!-- ubel-test: skip -->
```ubel
@tier(mid)
fn parse_json_with<F, R>(input: string, callback: F) R
    where F: fn(&JsonView) R   // R must contain no arena references
{
    with arena(1MB) {
        let view = build_json_view(input)
        return callback(&view)
    }
}

@tier(high)
fn handle_request(req: Request) Response {
    parse_json_with(req.body, fn(json) {
        let user_id = json.get("user_id").as_int()
        return fetch_user(user_id)
    })
}
```

### Pattern 2: iterator

For processing large datasets without materializing the whole result at
once. MID drives the iteration; HIGH only ever sees GC-owned values, one
at a time.

<!-- ubel-test: skip -->
```ubel
@tier(mid)
fn transform_each<R>(items: &[Item], f: fn(&TransformedItem) R) List<R> {
    with arena(10MB) {
        let mut results = List.new()
        for item in items {
            let transformed = expensive_transform(item)
            results.push(f(&transformed))
        }
        return results
    }
}
```

### Pattern 3: view

Syntactic sugar over the callback pattern for read-only access, following
the same rules.

<!-- ubel-test: skip -->
```ubel
@tier(high)
fn parse_config(path: string) Config {
    let config_view = read_toml_view(path)
    using let v = config_view {
        let host = v.get("host").to_owned()
        let port = v.get("port").as_int()
        return Config { host, port }
    }
}
```

### Rejected at compile time

<!-- ubel-test: skip -->
```ubel
// storing an arena reference in a GC-managed struct
@tier(high)
struct BadCache {
    data: &JsonView   // error: contains an arena lifetime
}

// HIGH tier constructing an arena directly
@tier(high)
fn bad() {
    with arena(1MB) { }   // error: 'with arena' is MID-tier only
}

// a cross-tier function returning an arena reference
@tier(mid)
fn bad_leak(input: string) &JsonView {
    with arena(1MB) {
        return &parse(input)   // error: return type carries an arena lifetime
    }
}
```

## The cross-tier call matrix

Not simply "LOW cannot call HIGH": every direction is a separate rule.

| Caller | Callee | Allowed |
|--------|--------|---------|
| HIGH | MID | Yes (callback/view patterns encouraged, not required) |
| HIGH | LOW | Yes |
| MID  | HIGH | No, an arena lifetime could escape |
| MID  | LOW | Yes |
| LOW  | HIGH | No |
| LOW  | MID | No |

## See it enforced

The call matrix is checked at every call. Here a LOW function calls a HIGH one
(the default tier), which the matrix forbids:

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
```

Arenas belong to MID tier. A `with arena` block in a HIGH function is
rejected:

<!-- ubel-test: error TIER-001 -->
```ubel
fn bad() {
    with arena(1MB) {
    }
}

fn main() {
}
```

Both programs, and the working MID/LOW pair below, behave exactly as shown
because the project's CI compiles them.

<!-- ubel-test: run -->
```ubel
@tier(mid)
fn sum_squares(n: int) int {
    let mut total = 0
    with arena(64KB) {
        for i in 1..=n {
            total += i * i
        }
    }
    return total
}

@tier(low)
fn read_it(x: &int) int {
    return *x
}

fn main() {
    println(sum_squares(10))     // HIGH calls MID: allowed
    let n = 21
    println(read_it(&n))         // HIGH calls LOW: allowed
}
```

```text
385
21
```

## Errors you will meet

Every rule on this page has its own diagnostic, documented with an example in
[the tier error reference](./errors/tier.md) and
[the ownership error reference](./errors/ownership.md). The most common:

| Mistake | Code |
|---------|------|
| calling up the matrix (LOW calling HIGH or MID, MID calling HIGH) | [TIER-005](./errors/tier.md#tier-005) |
| `with arena` outside a MID function | [TIER-001](./errors/tier.md#tier-001) |
| an arena or pool value leaving its block | [TIER-004](./errors/tier.md#tier-004), [TIER-011](./errors/tier.md#tier-011) |
| building a collection inside LOW | [TIER-009](./errors/tier.md#tier-009) |
| `Unique.new` and friends outside LOW | [TIER-014](./errors/tier.md#tier-014) |
| touching a `static` outside HIGH | [TIER-015](./errors/tier.md#tier-015) |
| using a value after it was moved | [MOVE-001](./errors/ownership.md#move-001) |
| using a place while it is mutably borrowed | [BORROW-001](./errors/ownership.md#borrow-001) |

## What is enforced today

The cross-tier call matrix above, arena-escape checking for the patterns
shown, lifetime well-formedness (declared lifetime names must exist,
`where` clauses can only reference declared names, no outlives cycles),
and outlives enforcement across a boundary are real, running checks in
the semantic analysis pass, independent of which backend eventually
executes the program.

**Calls on a value.** `value.method()`, `Type.make()`, a call through a trait
bound and a qualified call (`Trait.f(v)`) obey the same call matrix as a call by
name, using the tier written for the method. A method is `HIGH` unless it, the
`impl`, `extend`, struct or trait around it, or a `@tier(...) { }` block around
them says otherwise, and an unrelated attribute such as `@inline` never changes
that.

**Trait methods.** A trait method has a tier: its own `@tier`, otherwise the
trait's, otherwise `HIGH`. A call through a bound is checked against it, because
the impl is not known there, so every impl method has to be at exactly that tier
(an impl method that writes no tier takes it). The prelude traits (`PartialEq`,
`Eq`, `PartialOrd`, `Ord`, `Hash`, `Clone`) are `@tier(low)`: code at any tier can
call them through a bound, and a hand-written impl of one is `LOW`, so its body
cannot read a `static`, build a collection or call a `HIGH` function.

**Outlives / subset enforcement across a boundary.** LOW-tier borrow
checking is real: a control-flow graph is built per function, and loan
and liveness checking on top of it rejects a conflicting borrow with
NLL-style precision, where a reference's *last actual use* determines
when it stops conflicting, not the block it was declared in, with move
checking (use-after-move, loop-carried moves, reinitialization)
alongside it. Outlives enforcement extends that across two boundaries:
a call to a function with declared `[lifetime ...]` parameters, and the
construction of an `edge struct` whose fields carry declared lifetimes.
Declared `outlives` relationships propagate across multiple lifetimes,
and a boundary argument that cannot be traced to a local is rejected
conservatively rather than accepted. Three shapes are outside the
current scope, documented in the repository's `docs/OUTLIVES_RULES.md`:
methods (which the loan checker does not cover yet either), references
nested inside generic arguments, and closures that capture references.

One thing is still in progress rather than complete:

**The interpreter's memory model.** The tree-walking interpreter runs
every tier on the same reference-counted representation. `with arena`
blocks are recognized and validated by the tier checker but do not yet
allocate or free real memory in the interpreter; genuine bump-allocation
exists only in the preview [native backend](./tooling/native-backend.md). The static rules above are enforced
regardless, since tier checking happens independently of execution, but
running a MID-tier function today does not yet exercise real arena
memory pressure or reclamation timing.
