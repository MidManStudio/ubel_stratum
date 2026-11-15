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
