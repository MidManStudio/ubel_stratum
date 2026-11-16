# Ownership: Unique, Shared, SyncShared

LOW tier has no collector and no arena. Memory is managed by **ownership**:
every value has an owner, and the compiler checks how ownership moves. Three
built-in wrapper types say *how* a value is owned.

| Type | Meaning |
|------|---------|
| `Unique<T>` | exactly one owner. Using it by value **moves** it; the old name can no longer be used. |
| `Shared<T>` | shared ownership: any number of names for the same value. |
| `SyncShared<T>` | shared ownership meant to be safe across threads. |

They answer a different question from the tiers. A tier says *where memory
lives* (collector, arena, ownership); these say *who owns it and how*. They
combine freely: a field, a return type, or nested inside another type.

All three are built with `.new(value)`, and **construction is allowed only in
LOW tier** ([TIER-014](../errors/tier.md#tier-014) otherwise). A HIGH or MID
function can still receive one as a parameter and use it.

<!-- ubel-test: ok -->
```ubel
@tier(low)
fn build_unique() int {
    let u = Unique.new(42)
    return 1
}

@tier(low)
fn build_shared() int {
    let s = Shared.new("hello")
    return 1
}

@tier(low)
fn build_sync_shared() int {
    let ss = SyncShared.new(true)
    return 1
}

fn main() {
    let a = build_unique()
    let b = build_shared()
    let c = build_sync_shared()
}
```

The inverse rule is [TIER-009](../errors/tier.md#tier-009): `List`,
`Dictionary`, `Queue` and `Stack` cannot be *constructed* inside LOW, because
the ownership wrappers are LOW's memory model.

## Moves

Using a `Unique<T>` by value moves it. Using the old name afterwards is an
error, because the value is no longer there:

<!-- ubel-test: error MOVE-001 -->
```ubel
@tier(low)
fn main() {
    let a = Unique.new(1)
    let b = a              // moves `a` into `b`
    let c = a              // error: `a` was already moved
}
```

```text
>>> error[MOVE-001]: use of `a` after it was already moved
```

A moved name may be given a fresh value and used again:

<!-- ubel-test: run -->
```ubel
@tier(low)
fn main() {
    let a = Unique.new(1)
    let b = a
    a = Unique.new(2)      // `a` owns something again
    let c = a
    println($"{b:?} {c:?}")
}
```

```text
Unique(1) Unique(2)
```

The check follows every path through the function. If a value *might* already
be moved (it was moved in one branch of an `if`, or in an earlier loop
iteration), it is rejected. The details and limits are in
[MOVE-001](../errors/ownership.md#move-001); to avoid a move, borrow instead:
see [References, borrowing and lifetimes](./references-and-lifetimes.md).

`Shared<T>` and `SyncShared<T>` are not moved by use: any number of names
refer to the same value.

<!-- ubel-test: ok -->
```ubel
@tier(low)
fn main() {
    let s = Shared.new(5)
    let t = s
    let u = s              // fine: shared ownership
}
```

## Each wrapper is its own type

`Unique<int>` and `Shared<int>` do not mix, even though both hold an `int`:

<!-- ubel-test: error TYPE-101 -->
```ubel
@tier(low)
fn main() {
    let s: Shared<int> = Unique.new(1)
}
```

## Using the value inside

Methods of the wrapped type work straight through the wrapper, so a
`Unique<List<int>>` can be pushed to directly:

<!-- ubel-test: ok -->
```ubel
@tier(low)
fn push_one(items: Unique<List<int>>) {
    items.push(1)
}

fn main() {
}
```

Calling a built-in method does not move the receiver. Calling a user-declared
method does count as a move of it, and the checker goes by the method's name.

## What is not finished

- `SyncShared<T>` is a separate type so the compiler can tell it apart, but
  in the interpreter it is backed by the same single-threaded representation
  as `Shared<T>`, and it can only be created in LOW tier. The design for
  making it genuinely thread-safe, and creatable in every tier, is decided but
  not built: see [Async](../guide/async.md).
- Move checking tracks locals bound with `let` inside one function. A value
  that arrives indirectly (returned from another call, taken out of a field)
  is not tracked.
- LOW-tier *methods* are not borrow- or move-checked yet, only free functions.

[Project Status](../project-status.md) tracks these.
