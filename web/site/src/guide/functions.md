# Functions and closures

## Declaring a function

The return type follows the parameter list with no arrow. A function that
returns nothing omits it (or writes `void`).

<!-- ubel-test: run -->
```ubel
fn add(a: int, b: int) int {
    return a + b
}

fn greet(name: string) {
    println($"hello, {name}")
}

fn fact(n: int) int {
    if n <= 1 {
        return 1
    }
    return n * fact(n - 1)
}

fn main() {
    greet("world")
    println(add(2, 3))
    println(fact(5))
}
```

```text
hello, world
5
120
```

Functions can be declared in any order, so `main` can come first. Execution
starts at `fn main()`.

### Tiers

A function may carry a tier annotation (`@tier(mid)`, `@tier(low)`); without
one it is HIGH. See [The Tier Model](../tier-model.md) for what each tier
allows and which tiers may call which.

## Calling with the wrong shape

<!-- ubel-test: error TYPE-102 -->
```ubel
fn add(a: int, b: int) int {
    return a + b
}

fn main() {
    println(add(1, 2, 3))
}
```

A wrong argument count is [TYPE-102](../errors/type.md#type-102); an argument
of the wrong type or a wrong return value is
[TYPE-101](../errors/type.md#type-101):

<!-- ubel-test: error TYPE-101 -->
```ubel
fn half(n: int) int {
    return "a half"
}

fn main() {
    println(half(4))
}
```

## Functions are values

A parameter can have a function type, written `fn(int) int`.

## Closures

A closure is an anonymous function written `fn(params) expression`. It can
use variables from the function around it.

<!-- ubel-test: run -->
```ubel
fn apply(f: fn(int) int, x: int) int {
    return f(x)
}

fn main() {
    let inc = fn(x: int) x + 1
    println(apply(inc, 41))

    let scale = 3
    let times = fn(x: int) x * scale     // captures `scale`
    println(times(7))

    println(apply(fn(x: int) x * x, 9))  // a closure passed directly
}
```

```text
42
21
81
```

A closure has the tier of the function that created it. LOW-tier code may
only call a closure that it bound itself with an immutable `let`
([TIER-017](../errors/tier.md#tier-017)). Calling something that is not a
function at all is [TYPE-140](../errors/type.md#type-140).

## The pipe operator

`a |> f` calls `f` with `a` as its first argument, so a chain reads in the
order things happen:

<!-- ubel-test: run -->
```ubel
fn twice(x: int) int { return x * 2 }
fn add(x: int, y: int) int { return x + y }

fn main() {
    // twice(5) = 10, add(10, 1) = 11, twice(11) = 22
    let r = 5 |> twice |> add(1) |> twice
    println(r)
}
```

## Extension methods

`extend` adds methods to an existing type, including the built-in ones:

<!-- ubel-test: run -->
```ubel
extend int {
    fn is_even(self) bool { return self % 2 == 0 }
}

fn main() {
    println(42.is_even())
    println(7.is_even())
}
```

`self` is only valid inside a method ([NAME-005](../errors/name.md#name-005)).
