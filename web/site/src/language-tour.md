# A Tour of Ubel

A quick pass over the language, with one runnable program per idea. Each
section links to the Guide chapter that goes deep, and every example here is
compiled by the project's CI.

## Hello, tiers

Functions are HIGH tier (garbage collected) unless you say otherwise.
`@tier(mid)` allocates in arenas and `@tier(low)` uses ownership and
borrowing. [The Tier Model](./tier-model.md) is the full story.

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

fn main() {
    println($"sum of squares: {sum_squares(10)}")
}
```

## Values, structs and enums

[Values and variables](./guide/values.md), [Structs and enums](./guide/structs-enums.md)

<!-- ubel-test: run -->
```ubel
struct Rectangle {
    width: int
    height: int

    pub fn new(w: int, h: int) Rectangle {
        return Rectangle { width = w, height = h }
    }

    pub fn area(self) int {
        return self.width * self.height
    }
}

enum Shape {
    Circle(int),
    Square(int),
    Nothing
}

fn describe(s: Shape) string {
    match s {
        Circle(r) => return $"circle, radius {r}",
        Square(side) => return $"square, side {side}",
        Nothing => return "no shape",
    }
}

fn main() {
    let r = Rectangle.new(3, 4)
    println($"area = {r.area()}")
    println(describe(Shape.Circle(2)))
    println(describe(Shape.Nothing))
}
```

Names use `.` everywhere: `Rectangle.new(...)`, `value.method()`,
`Shape.Circle(2)`. There is no `::`.

## Collections and queries

[Collections](./guide/collections.md)

<!-- ubel-test: run -->
```ubel
fn main() {
    let scores = Dictionary.new()      // get/set, not insert
    scores.set("ada", 100)
    scores.set("alan", 90)
    println(scores.get("ada"))

    let names = ["ada", "alan", "grace"]
    let long_names = List.new()
    for n in names {
        long_names.push(n)
    }
    let shout = long_names.query()
        .where(fn(n) n.len() > 3)
        .select(fn(n) n.to_upper())
        .to_list()
    println(shout.len())
}
```

## Functions, closures and the pipe

[Functions and closures](./guide/functions.md)

<!-- ubel-test: run -->
```ubel
fn twice(x: int) int { return x * 2 }
fn add(x: int, y: int) int { return x + y }

extend int {
    fn is_even(self) bool { return self % 2 == 0 }
}

fn main() {
    let r = 5 |> twice |> add(1)       // add(twice(5), 1)
    println(r)
    println(42.is_even())

    let offset = 10
    let shifted = fn(x: int) x + offset
    println(shifted(5))
}
```

## Generics and traits

[Generics and traits](./guide/generics-traits.md)

<!-- ubel-test: run -->
```ubel
trait Shape {
    fn area(self) int
}

struct Sq {
    s: int
}

impl Shape for Sq {
    fn area(self) int { return self.s * self.s }
}

fn total_area<T: Shape>(a: T, b: T) int {
    return a.area() + b.area()
}

fn main() {
    println(total_area(Sq { s = 2 }, Sq { s = 3 }))
}
```

## Failure and absence

[Optionals and failure](./guide/errors-and-optionals.md)

<!-- ubel-test: run -->
```ubel
fn parse_positive(n: int) int! {
    if n < 0 {
        fail "negative"
    }
    return n
}

fn double_it(n: int) int! {
    let v = parse_positive(n)?
    return v * 2
}

fn main() {
    let r = double_it(4)
    println("ok")
}
```

## Where next

- Something fails? [The error reference](./errors/index.md) explains every code.
- Try it without installing anything: [the playground](./tooling/playground.md).
- Build the tools: [Getting Started](./getting-started.md).
