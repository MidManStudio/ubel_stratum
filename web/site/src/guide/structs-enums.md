# Structs and enums

## Structs

A struct groups named fields. Methods live in the same block. A method that
takes `self` reads the value it is called on; a function without `self`
is an associated function, called on the type.

<!-- ubel-test: run -->
```ubel
struct Point {
    x: int
    y: int

    pub fn new(x: int, y: int) Point {
        return Point { x = x, y = y }
    }

    pub fn dist_sq(self) int {
        return self.x * self.x + self.y * self.y
    }

    pub fn shift(self, dx: int) Point {
        return Point { x = self.x + dx, y = self.y }
    }
}

fn main() {
    let p = Point.new(3, 4)        // associated function: Type.name(...)
    println(p.dist_sq())           // method: value.name(...)
    let q = p.shift(10)
    println($"{q.x}, {q.y}")
}
```

```text
25
13, 4
```

A struct literal assigns fields with `=` (`Point { x = 1, y = 2 }`). Fields
are separated by commas or newlines. Accessing a field that does not exist
is [TYPE-103](../errors/type.md#type-103), and calling a method that does
not exist is [TYPE-104](../errors/type.md#type-104):

<!-- ubel-test: error TYPE-103 -->
```ubel
struct Point {
    x: int
    y: int
}

fn main() {
    let p = Point { x = 1, y = 2 }
    println(p.z)
}
```

`Debug` and `Display` are provided for every struct and enum, so any of
them can be printed. `==` on two structs
compares them by identity unless you add `@derive(PartialEq)`, which makes
it compare field by field.

Structs that hold borrowed references, and `edge struct`, belong to the LOW
tier; see [The Tier Model](../tier-model.md).

## Enums

An enum is a type with a fixed set of variants. A variant can be a plain
name, can carry a tuple payload `Circle(int)`, or a struct payload
`Rect { w: int, h: int }`.

<!-- ubel-test: run -->
```ubel
enum Direction {
    North
    East
    South
    West
}

enum Shape {
    Circle(int),
    Rect { w: int, h: int },
    Empty
}

fn area(s: Shape) int {
    match s {
        Circle(r) => return 3 * r * r,
        Rect { w, h } => return w * h,
        Empty => return 0,
    }
}

fn turn(d: Direction) Direction {
    match d {
        North => return Direction.East,
        East => return Direction.South,
        South => return Direction.West,
        West => return Direction.North,
    }
}

fn main() {
    println(area(Shape.Circle(2)))
    println(area(Shape.Rect { w = 3, h = 4 }))
    let d = turn(Direction.North)
    println(d)
    match d {
        East => println("now facing east"),
        _ => println("elsewhere"),
    }
}
```

```text
12
12
Direction.East
now facing east
```

Variants are built with `Type.Variant(...)` and matched by their bare name.
Fieldless variants can be given explicit numbers:

<!-- ubel-test: ok -->
```ubel
enum Level {
    Low = 1
    High = 10
}

fn main() {
}
```

Numbers and payloads cannot be mixed in one enum
([TYPE-112](../errors/type.md#type-112)).

### Exhaustive matching

A `match` on an enum must cover every variant, or end with a wildcard `_`.
Forgetting one is an error that names what is missing:

<!-- ubel-test: error TYPE-111 -->
```ubel
enum Light {
    Red
    Amber
    Green
}

fn next(l: Light) Light {
    match l {
        Red => return Light.Green,
        Green => return Light.Amber,
    }
}

fn main() {
}
```

An unknown variant is [TYPE-109](../errors/type.md#type-109), and a variant
used with the wrong number of payload values is
[TYPE-110](../errors/type.md#type-110).
