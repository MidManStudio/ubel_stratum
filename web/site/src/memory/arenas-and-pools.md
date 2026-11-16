# Arenas and pools

MID tier is **region-based** memory. Instead of freeing values one by one, you
open a region, allocate freely inside it, and the whole region disappears when
the block ends. There are two kinds:

| Region | Syntax | Holds | Frees |
|--------|--------|-------|-------|
| Arena | `with arena(size) { ... }` | anything, of any type, in any quantity | everything at once, at the closing brace |
| Pool | `with pool<T>(count) { ... }` | up to `count` values of one type `T`, each reachable through a handle | each slot when you `release` it, the whole pool at the closing brace |

Both are **MID-only**: `with arena` and `with pool` are accepted only in a
function marked `@tier(mid)`
([TIER-001](../errors/tier.md#tier-001), [TIER-010](../errors/tier.md#tier-010)).
The point of the tier is the boundary, so read [The Tier Model](../tier-model.md)
first if you have not.

## Arenas

<!-- ubel-test: run -->
```ubel
@tier(mid)
fn sum_to(n: int) int {
    let mut total = 0
    with arena(1KB) {
        let scratch = List.new()        // allocated in the arena
        for i in 0..n {
            scratch.push(i)
        }
        for v in scratch {
            total += v
        }
    }                                    // scratch is gone here
    return total
}

fn main() {
    println(sum_to(5))
}
```

```text
10
```

The size can be a plain byte count (`256`) or use `KB`, `MB` or `GB`
(`64KB`, `1MB`). It is part of the contract a native build honours; the
interpreter checks every rule on this page but does not allocate real arena
memory yet (see [what is enforced today](../tier-model.md#what-is-enforced-today)).

Everything created inside the block belongs to it: collections, and strings
built with interpolation. Such a value has an **arena lifetime**:
the compiler tracks it as `&arena T`, and it may not outlive the block.

### Strings in arenas

An interpolated or concatenated string created inside an arena lives in it.
You can build one there and copy its contents into a value that was made
outside, because `+` produces a new outer string from the pieces:

<!-- ubel-test: run -->
```ubel
@tier(mid)
fn greet(name: string) string {
    let mut out = "hello"
    with arena(256) {
        let scratch = $"{name}!"          // arena string
        out = out + " " + scratch         // an outer string, built from copies
    }
    return out
}

fn main() {
    println(greet("ada"))
}
```

```text
hello ada!
```

Returning the arena string itself is the mistake:

<!-- ubel-test: error TIER-004 -->
```ubel
@tier(mid)
fn label(n: int) string {
    with arena(256) {
        let s = $"n={n}"
        return s
    }
}

fn main() {
    println(label(5))
}
```

### What may not leave the block

A reference into an arena is invalid the moment the block closes, so the
compiler rejects every way of letting one escape. All of them are
[TIER-004](../errors/tier.md#tier-004):

<!-- ubel-test: error TIER-004 -->
```ubel
@tier(mid)
fn make_list() List<int> {
    with arena(64KB) {
        return [1, 2, 3]            // returned
    }
}

fn main() {
    let x = make_list()
}
```

<!-- ubel-test: error TIER-004 -->
```ubel
@tier(mid)
fn leak() {
    let mut outer = List.new()
    with arena(1KB) {
        let scratch = List.new()
        scratch.push(1)
        outer = scratch             // stored in something that outlives the block
    }
}

fn main() {
}
```

The same goes for a closure created inside the block, because it may capture
arena values. A closure can be created and called inside the block, and it can
capture variables there:

<!-- ubel-test: run -->
```ubel
@tier(mid)
fn run(base: int) int {
    with arena(256) {
        let k = base + 1
        let add = fn(x: int) x + k      // captures `k`
        return add(1)                   // called inside the block: fine
    }
}

fn main() {
    println(run(2))
}
```

```text
4
```

but it may not be returned, assigned to something outside, or passed to
another function:

<!-- ubel-test: error TIER-004 -->
```ubel
@tier(mid)
fn apply(f: fn(int) int, x: int) int {
    return f(x)
}

@tier(mid)
fn run(base: int) int {
    with arena(256) {
        let k = base + 1
        let add = fn(x: int) x + k
        return apply(add, 1)        // passed out
    }
}

fn main() {
    println(run(2))
}
```

### Getting a result out

Copy it. Compute inside the block and return a plain value (a number, or a
string built with `+` as above), as `sum_to` did. For results that are
themselves collections, the [Tier Model](../tier-model.md) describes three
patterns for crossing from MID to HIGH: the callback, the iterator and the
view. The callback pattern is shown on
[Edge structs and cross-tier patterns](./edge-structs.md).

## Pools

An arena frees everything at once. A **pool** is for the opposite shape of
work: many short-lived objects of one type that come and go while the block is
open (particles, connections, events). It has a fixed number of slots and gives
out **handles**.

<!-- ubel-test: run -->
```ubel
@tier(mid)
fn demo() {
    with pool<int>(2) {
        let p = Pool.new()              // takes its type and size from the block
        let a = p.acquire(10)           // a handle to a slot holding 10
        let b = p.acquire(20)
        let full = p.acquire(30)        // no free slot left
        println(full == null)           // true: a full pool answers null
        println(p.get(a))               // 10
        p.release(a)                    // give the slot back
        println(p.get(a) == null)       // true: `a` is stale now
        let c = p.acquire(30)           // reuses the freed slot
        println(p.get(c))               // 30
        println(p.get(a) == null)       // still true: the old handle stays dead
        p.release(b)
        p.release(c)
    }
}

fn main() {
    demo()
}
```

```text
true
10
true
30
true
```

The pool methods:

| Method | Does |
|--------|------|
| `acquire(value)` | stores `value` in a free slot and returns its handle, or `null` if none is free |
| `get(handle)` | the value, or `null` if the handle is stale (its slot was released or reused) |
| `release(handle)` | frees the slot |
| `fifo()` | reuse the *longest*-free slot first; the default reuses the most recently freed one |
| `growable()` | when the pool is full, add another block of slots instead of answering `null` |

A `for` loop over a pool visits each live value and skips released slots.

A handle is not a pointer: it records which *generation* of the slot it was
issued for, so a handle kept past its `release` can never read someone else's
data. That is the point of the design.

### Pool rules

`Pool.new()` takes its element type and capacity from the surrounding
`with pool<T>(count)` block, so it only makes sense inside one
([TIER-013](../errors/tier.md#tier-013)):

<!-- ubel-test: error TIER-013 -->
```ubel
@tier(mid)
fn bad() {
    let p = Pool.new()
}

fn main() {
    bad()
}
```

And like an arena value, a pool, or anything acquired from it, cannot outlive
its block ([TIER-011](../errors/tier.md#tier-011)):

<!-- ubel-test: error TIER-011 -->
```ubel
@tier(mid)
fn nested() {
    with pool<int>(4) {
        let mut outer_pool = Pool.new()
        with pool<int>(4) {
            outer_pool = Pool.new()
        }
    }
}

fn main() {
    nested()
}
```

## Choosing

- Many allocations, one lifetime, everything dropped together: an **arena**.
- Objects that are created and destroyed independently inside the region, and
  you want a stale reference to be harmless: a **pool**.
- Nothing allocates, or ownership is tracked per value: that is LOW tier, see
  [Ownership](./ownership.md).
