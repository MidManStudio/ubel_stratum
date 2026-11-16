# Edge structs and cross-tier patterns

## Edge structs

A normal struct holds values. An **`edge struct`** may also hold *references*,
and declares the lifetimes those references live in. The keyword goes before
`struct`, after `pub` if there is one.

<!-- ubel-test: run -->
```ubel
edge struct Cache [lifetime L] {
    item: &L int
}

@tier(low)
fn build_while_live() int {
    let n = 42
    let p = &n
    let c = Cache { item = p }
    return *c.item
}

fn main() {
    println(build_while_live())
}
```

```text
42
```

Building a `Cache` is a **boundary**: the reference going into the field must
still be valid at that point. The same check as for a function call applies,
with the same diagnostics ([LIFETIME-004](../errors/ownership.md#lifetime-004),
[-005](../errors/ownership.md#lifetime-005) and
[-006](../errors/ownership.md#lifetime-006)), so this is rejected, because
assigning to `n` ended the loan that `p` carried:

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

Reading the field through the struct, `c.item`, then `*c.item`, works as with
any other reference.

### Edge structs in an arena

`edge struct` was designed for the case where a value in an arena needs to
refer to other data. A holder built inside a `with arena` block can point at an
ordinary value from outside it:

<!-- ubel-test: run -->
```ubel
struct Vec2 {
    x: int
    y: int
}

edge struct Holder [lifetime M] {
    r: &M Vec2
}

@tier(mid)
fn run(g: Vec2) int {
    let first = Vec2 { x = 9, y = 9 }
    with arena(256) {
        let h = Holder { r = &first }
        return h.r.x + g.y
    }
}

fn main() {
    println(run(Vec2 { x = 5, y = 6 }))
}
```

```text
15
```

Two honest caveats, both tracked in [Project Status](../project-status.md):

- Marking a struct `edge` does not yet change how the *arena escape* check
  treats it: a plain struct that stores an arena value in a field is rejected
  with [TIER-004](../errors/tier.md#tier-004) in exactly the same way.
- Lifetime checking covers a function call and a struct construction, not
  every use; see
  [What is not covered yet](./references-and-lifetimes.md#what-is-not-covered-yet).

## Crossing tiers

The tiers only help if values can move between them safely. The rules, in
brief (the full table is on [The Tier Model](../tier-model.md)):

- HIGH may call MID and LOW; MID may call LOW; nothing may call upward
  ([TIER-005](../errors/tier.md#tier-005)).
- A reference into a region never leaves it
  ([TIER-004](../errors/tier.md#tier-004), [TIER-011](../errors/tier.md#tier-011)).
- A result from MID to HIGH must be **copied out**, or handed over through
  one of three patterns below.

### The callback pattern

HIGH keeps control; MID calls a function HIGH gave it, once for each item,
while its arena is still alive. Nothing from the arena ever crosses the
boundary, only the call does:

<!-- ubel-test: run -->
```ubel
@tier(mid)
fn process(n: int, visit: fn(int)) {
    with arena(1KB) {
        let scratch = [n, n + 1, n + 2]
        for x in scratch {
            visit(x)            // HIGH's closure runs while `scratch` is alive
        }
    }
}

fn main() {
    process(10, fn(x: int) println(x))
}
```

```text
10
11
12
```

Note the direction: a closure made by HIGH is handed *to* MID. The opposite,
a closure made *inside* the arena and passed out, is
[TIER-004](../errors/tier.md#tier-004) (see
[Arenas and pools](./arenas-and-pools.md#what-may-not-leave-the-block)).

### Copy out

The simplest pattern: compute inside the region, return a plain value.

<!-- ubel-test: run -->
```ubel
@tier(mid)
fn longest_run(n: int) int {
    let mut best = 0
    with arena(1KB) {
        let items = List.new()
        for i in 0..n {
            items.push(i * i)
        }
        for v in items {
            if v > best {
                best = v
            }
        }
    }
    return best
}

fn main() {
    println(longest_run(6))
}
```

```text
25
```

### Iterator and view

The other two patterns, an iterator that yields items one at a time and a
read-only *view* (sugar over the callback pattern), are described with
illustrative code on [The Tier Model](../tier-model.md). That code is a sketch
of the intent; the compiler's test suite does not yet have working examples of
them, so this page does not either.
