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

## What is enforced today

The cross-tier call matrix above, arena-escape checking for the patterns
shown, and lifetime well-formedness (declared lifetime names must exist,
`where` clauses can only reference declared names, no outlives cycles)
are real, running checks in the semantic analysis pass, independent of
which backend eventually executes the program.

Two things are still in progress rather than complete:

**Outlives / subset enforcement across a boundary.** LOW-tier borrow
checking itself is real: a control-flow graph is built per function,
and loan and liveness checking on top of it genuinely rejects a
conflicting borrow with NLL-style precision — a reference's *last
actual use* determines when it stops conflicting, not the block it was
declared in — with move checking (use-after-move, loop-carried moves,
reinitialization) alongside it. What that checking does *not* yet cover
is a reference crossing a function-call or `edge struct` boundary:
declared lifetime parameters (`[lifetime L]`) are checked for internal
well-formedness, but nothing yet verifies a caller's argument or a
struct's field actually satisfies the declared relationship once it
leaves the function body that created it. That piece is scoped (see the
repository's `docs/OUTLIVES_RULES.md`) and landing in phases.

**The interpreter's memory model.** The tree-walking interpreter runs
every tier on the same reference-counted representation. `with arena`
blocks are recognized and validated by the tier checker but do not yet
allocate or free real memory in the interpreter; genuine bump-allocation
arrives with the LLVM backend. The static rules above are enforced
regardless, since tier checking happens independently of execution, but
running a MID-tier function today does not yet exercise real arena
memory pressure or reclamation timing.
