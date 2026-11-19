# Generics and traits

## Generic functions and types

A type parameter is written in angle brackets after the name.

<!-- ubel-test: run -->
```ubel
struct Box<T> {
    value: T

    pub fn new(v: T) Box<T> {
        return Box { value = v }
    }

    pub fn get(self) T {
        return self.value
    }
}

fn identity<T>(x: T) T {
    return x
}

fn main() {
    let b = Box.new(42)
    println(b.get())
    println(identity("hi"))
}
```

```text
42
hi
```

Generic types and functions are checked against what the parameter is known
to support: a value of an unbounded type parameter has no methods
([TYPE-130](../errors/type.md#type-130)), and using a generic type with the
wrong number of type arguments is [TYPE-108](../errors/type.md#type-108).

## Traits

A trait is a set of methods a type can promise to provide. A method with a
body is a default that implementers inherit.

<!-- ubel-test: run -->
```ubel
trait Shape {
    fn area(self) int
    fn name(self) string {
        return "shape"
    }
}

struct Sq {
    s: int
}

impl Shape for Sq {
    fn area(self) int { return self.s * self.s }
}

fn describe<T: Shape>(x: T) string {
    return $"{x.name()} with area {x.area()}"
}

fn main() {
    println(describe(Sq { s = 4 }))
}
```

```text
shape with area 16
```

`<T: Shape>` is a **bound**: `describe` can be called only with a type that
implements `Shape`, and inside it a `T` may use `Shape`'s methods. Several
bounds are joined with `+`.

### What the compiler checks

<!-- ubel-test: error TYPE-126 -->
```ubel
trait Show {
    fn show(self) string
}

struct Plain {
    n: int
}

fn print_it<T: Show>(x: T) {
    println(x.show())
}

fn main() {
    print_it(Plain { n = 1 })
}
```

[TYPE-126](../errors/type.md#type-126): `Plain` does not implement `Show`.
The other trait errors are, in short:

- an `impl` that leaves out a required method: [TYPE-123](../errors/type.md#type-123);
- a method the trait does not declare: [TYPE-124](../errors/type.md#type-124);
- a method whose signature differs: [TYPE-125](../errors/type.md#type-125);
- two impls of the same trait for one type: [TYPE-128](../errors/type.md#type-128);
- a method call on an unbounded type parameter: [TYPE-130](../errors/type.md#type-130);
- an impl whose supertrait the type lacks: [TYPE-132](../errors/type.md#type-132).

A trait method has a tier, and an impl must declare the same one
([TIER-016](../errors/tier.md#tier-016)).

### Supertraits and trait objects

A trait can require another with `trait Greeter: Named`. A `dyn Trait` value
holds any type implementing the trait, chosen at run time; the trait must be
*object safe* ([TYPE-138](../errors/type.md#type-138)).

### Deriving

`@derive(...)` asks the compiler to write an implementation for you.

<!-- ubel-test: error TYPE-118 -->
```ubel
struct Unranked {
    label: string
}

fn main() {
    let a = Unranked { label = "first" }
    let b = Unranked { label = "second" }
    if a < b {
        println("a comes first")
    }
}
```

A struct has no ordering unless it derives or implements one
([TYPE-118](../errors/type.md#type-118)). `Debug` and `Display` need no
derive; the prerequisites of a derive are checked
([TYPE-117](../errors/type.md#type-117)).

## Cleanup with `using` and `Dispose`

A `using` block binds one or more values, runs its body, and then cleans the values up, last
binding first. A value is cleaned up by calling the `dispose` of its `Dispose` impl. The cleanup
runs on every way out of the block: the end, `return`, `break`, `continue`, a `fail` and a panic.

<!-- ubel-test: run -->
```ubel
struct Connection {
    name: string,
}

impl Dispose for Connection {
    fn dispose(self) void {
        println($"closing {self.name}")
    }
}

fn first_line() int {
    using let a = Connection { name = "a" }, let b = Connection { name = "b" } {
        println("working")
        return 1
    }
    return 0
}

fn main() void {
    let n = first_line()
    println(n)
}
```

```text
working
closing b
closing a
1
```

The value of a `return` is computed before the cleanup runs, and the `defer` expressions of the
body run before it too. A binding whose initializer fails is not cleaned up, and neither is any
binding after it, but the ones before it are.

`using` accepts a value of any type. A value whose type has no `Dispose` impl has no cleanup, and a
method that is merely named `dispose` is not called: only the `Dispose` impl is. A `Unique<T>` is
cleaned up as its `T`. A `Shared<T>` or a `SyncShared<T>` is not cleaned up, because it is one of
several owners. A `let mut` binding is cleaned up with the value it started with.

`dispose` is `fn dispose(self) void`. Any other signature is an error
([TYPE-125](../errors/type.md#type-125)).

<!-- ubel-test: error TYPE-125 -->
```ubel
struct Connection {
    name: string,
}

impl Dispose for Connection {
    fn dispose(self) int {
        return 0
    }
}

fn main() {
}
```

`Dispose` is a `@tier(low)` trait like the other prelude traits, so a `using` block in any tier
can call it, and the body of a `dispose` is LOW-tier code (it cannot build a `List`,
[TIER-009](../errors/tier.md#tier-009)). It cannot be derived
([TYPE-116](../errors/type.md#type-116)).

If a `dispose` panics, the other cleanups of the block still run. A panic that is already on its
way out is the one reported. Otherwise the first panic raised by a cleanup replaces whatever the
block was leaving with.

## Operators on your own types

The operators are traits too: implementing `Add`, `Sub`, `Mul`, `Index` and
so on makes the operator work on your type. Using an operator on a type with
no impl is [TYPE-136](../errors/type.md#type-136), and the diagnostic prints
the impl you would add. Inside a generic function the operator needs the
matching bound (`<T: PartialEq>`, [TYPE-131](../errors/type.md#type-131)).

## What is not built yet

The compiler rejects, with a clear message, trait features that parse but
are not implemented: associated types, blanket impls, and a few others
([TYPE-129](../errors/type.md#type-129)). The list shrinks as features land;
[Project Status](../project-status.md) has the current one.
