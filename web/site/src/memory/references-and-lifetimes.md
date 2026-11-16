# References, borrowing and lifetimes

A **reference** lets code use a value without owning it. References are
a LOW-tier feature: they are what makes it possible to read and change a
value without moving it, which matters because
[`Unique<T>`](./ownership.md) can only have one owner.

## Reference types and expressions

| You write | Means |
|-----------|-------|
| `&T`, `ref T` | a reference to a `T` that may be read |
| `&mut T`, `ref mut T` | a reference to a `T` that may be changed |
| `&x`, `ref x` | take a reference to `x` |
| `&mut x`, `ref mut x` | take a mutable reference to `x` |
| `*r`, `deref r` | read through the reference `r` |

Each form has a symbol spelling and a keyword spelling, and they mean exactly
the same. Use whichever reads better; the examples here use symbols.

<!-- ubel-test: run -->
```ubel
@tier(low)
fn peek(n: &int) int {
    return *n
}

@tier(low)
fn demo() int {
    let x = 41
    return peek(&x) + 1
}

fn main() {
    println(demo())
}
```

```text
42
```

A `&mut` reference can be written to with `*r = value`. That is accepted by
the type checker, but **the interpreter does not run it yet**: a program that
assigns through a reference stops at run time with *assignment through a
dereferenced reference isn't implemented yet*. Reading through a reference
works everywhere.

> **A line that starts with `*` continues the line before it.** Ubel has no
> statement terminator, so after `let p = &mut n` a new line beginning `*p = 5`
> is read as `&mut n * p = 5`, and the compiler reports `p` as undefined:

<!-- ubel-test: error NAME-001 -->
```ubel
@tier(low)
fn demo() {
    let mut n = 1
    let p = &mut n
    *p = 5
}

fn main() {
}
```

> Begin the statement with something else, or wrap the previous line, if you
> hit this.

## Borrow checking

While a mutable borrow is alive, the value it borrows cannot be used any other
way, because the reference could change it underneath you:

<!-- ubel-test: error BORROW-001 -->
```ubel
@tier(low)
fn demo() int {
    let mut n = 1
    let p = &mut n
    let y = n               // error: `n` is borrowed mutably by `p`...
    return *p + y           // ...and `p` is used again here
}

fn main() {
}
```

The check is **non-lexical**: a borrow lasts only until its *last use*, not
until the end of the block. Move that conflicting use after the last use of
`p` and it is fine:

<!-- ubel-test: run -->
```ubel
@tier(low)
fn demo() int {
    let mut n = 10
    let p = &mut n
    let doubled = *p        // last use of `p`
    let y = n               // `n` is free again
    return doubled + y
}

fn main() {
    println(demo())
}
```

```text
20
```

The diagnostic is [BORROW-001](../errors/ownership.md#borrow-001). Only
mutable loans bound to a local are checked today.

## Lifetimes

A reference can only be used while the thing it points at is alive. When a
function takes or returns references, **lifetime parameters** name the
regions involved, so the compiler can check the relationships between them.
They are declared in square brackets after the function name, and written on
the reference type after the `&`:

<!-- ubel-test: run -->
```ubel
fn first [lifetime L, lifetime M where L outlives M] (a: &L int, b: &M int) &L int {
    return a
}

fn main() {
    let x = 10
    let y = 20
    let result = first(&x, &y)
    println(*result)
}
```

```text
10
```

Reading it:

- `[lifetime L, lifetime M ...]` declares two lifetime names.
- `where L outlives M` says region `L` lasts at least as long as region `M`.
- `&L int` is a reference to an `int` that lives in region `L`.

Lifetime parameters are **optional**: a function such as `peek(n: &int)` above
declares none and gets no lifetime checking. When you do want a relationship
checked, you write the lifetimes out; there are no elision rules, a deliberate
simplification compared with Rust.

### What the compiler checks

Three layers, each with its own diagnostics:

1. **The declaration makes sense.** A lifetime used but not declared is
   [LIFETIME-001](../errors/ownership.md#lifetime-001), a name declared twice
   is [LIFETIME-002](../errors/ownership.md#lifetime-002), and `outlives`
   constraints that loop are [LIFETIME-003](../errors/ownership.md#lifetime-003).

<!-- ubel-test: error LIFETIME-001 -->
```ubel
fn foo [lifetime L where L outlives M] () void {}

fn main() void {
}
```

2. **A call keeps its promises.** When you pass a reference to a function with
   a lifetime parameter, the borrowed value must still be borrowed at the call.
   If the loan was already cancelled (here by assigning to `n`), the call is
   rejected ([LIFETIME-004](../errors/ownership.md#lifetime-004)):

<!-- ubel-test: error LIFETIME-004 -->
```ubel
@tier(low)
fn take [lifetime L] (v: &L int) &L int {
    return v
}

@tier(low)
fn call_after_kill() int {
    let mut n = 42
    let p = &n
    n = 7                   // ends the loan that `p` holds
    let result = take(p)
    return *result
}

fn main() void {
    let value = call_after_kill()
}
```

3. **A declared `outlives` relation holds at the call site.** If a function
   says `where L outlives M`, the arguments' real regions must have that
   order, or it is
   [LIFETIME-005](../errors/ownership.md#lifetime-005). When the checker cannot
   trace an argument back to a local (it came out of another call), it
   rejects conservatively with
   [LIFETIME-006](../errors/ownership.md#lifetime-006); bind the value to a
   local first.

The same checks apply when a reference is stored in a struct field; see
[Edge structs and cross-tier patterns](./edge-structs.md).

### What is not covered yet

Lifetime checking is deliberately narrow. As documented in
`docs/OUTLIVES_RULES.md`:

- references nested in generics (`List<&L T>`) are not tracked;
- closures that capture references are not tracked;
- LOW-tier methods (`impl`/`extend`) are not borrow-checked, and lifetime
  checking inherits that boundary;
- everything is checked inside one function body plus the two boundaries
  above, a call and a struct construction.

Where it cannot prove something safe, it rejects rather than allowing it: it
is meant to be conservative and never unsound, even if that sometimes refuses
a program that would have been fine.
