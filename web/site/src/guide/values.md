# Values and variables

## Comments

```text
// a line comment
/* a block comment, which /* nests */ properly */
```

Block comments nest, so you can comment out code that already contains a
comment. An unclosed one is [LEX-003](../errors/lex.md#lex-003).

## Variables: `let` and `let mut`

`let` introduces a variable. It is immutable unless you write `mut`.

<!-- ubel-test: run -->
```ubel
fn main() {
    let name = "Ubel"          // immutable, type inferred: string
    let mut count = 0          // mutable
    count += 1
    count = count * 2

    let ratio: double = 0.75   // an explicit type annotation
    let flag = true
    let initial = 'U'          // a character literal

    println($"{name} {count} {ratio} {flag} {initial}")
}
```

```text
Ubel 2 0.75 true U
```

A type annotation is optional where the compiler can infer the type. It is
checked where you write one: `let n: int = "three"` is
[TYPE-101](../errors/type.md#type-101).

A name can be defined once per scope. A second `let` of the same name in the
same block is an error; a block nested inside may define the name again:

<!-- ubel-test: error NAME-002 -->
```ubel
fn main() {
    let x = 1
    let x = 2
}
```

An undefined name is [NAME-001](../errors/name.md#name-001), and the
diagnostic suggests a close match if there is one.

## Constants and statics

`const` is a compile-time-style global: initialised once, readable from every
tier, never assigned. `static` is a mutable global that lives in the HIGH tier.

<!-- ubel-test: run -->
```ubel
const MAX_USERS: int = 100
static VISITS: int = 0

fn main() {
    VISITS += 1
    println($"visits = {VISITS} of {MAX_USERS}")
}
```

A constant is evaluated once at startup. Two rules are enforced:

<!-- ubel-test: error NAME-007 -->
```ubel
const LIMIT: int = 5

fn main() {
    LIMIT = 6
}
```

- You cannot assign to a constant ([NAME-007](../errors/name.md#name-007)).
- A constant cannot read a `static`, because a static's value changes
  ([NAME-008](../errors/name.md#name-008)).

A `static` can only be used from `@tier(high)` code; reading one from a MID
or LOW function is [TIER-015](../errors/tier.md#tier-015).

## Numbers

| Type | Meaning |
|------|---------|
| `int`, `long` | signed integers |
| `i8` `i16` `i32` `i64` `isize` | fixed-width signed integers |
| `u8` `u16` `u32` `u64` `usize` | fixed-width unsigned integers |
| `double`, `float` | floating point |

An integer literal can carry a suffix (`300u8`) or be given its type by
context (`let x: u8 = 200`). Underscores group digits (`1_000_000`), and
there are `0x` hexadecimal and `0b` binary literals.

<!-- ubel-test: run -->
```ubel
fn main() {
    let big = 1_000_000
    let small: u8 = 200
    let mask = 0xFF
    let bits = 0b1010
    let tiny = 1.5e-3
    println($"{big} {small} {mask} {bits} {tiny}")
}
```

A literal that does not fit is rejected instead of wrapping:

<!-- ubel-test: error TYPE-120 -->
```ubel
fn main() {
    let x: u8 = 300
}
```

See [TYPE-120](../errors/type.md#type-120). There is no implicit conversion
between `int` and `double` (`let b: double = a` with an `int` `a` is
[TYPE-101](../errors/type.md#type-101)); use `to_int`, `to_double` and friends
([built-in functions](../reference/builtins.md)).

## Operators

<!-- ubel-test: run -->
```ubel
fn main() {
    let a = 7
    let b = 2
    println($"{a + b} {a - b} {a * b} {a / b} {a % b}")     // arithmetic
    println($"{a > b} {a == b} {a != b} {a >= 7}")          // comparison
    println($"{a & b} {a | b} {a ^ b} {a << 2} {a >> 1}")   // bitwise
    let ok = a > 1 and b < 5      // `and`, `or`, `not` are the logical operators
    println($"{ok} {not ok} {a > 100 or b == 2}")
}
```

```text
9 5 14 3 1
true false true true
2 7 5 28 3
true false true
```

`and`, `or` and `not` are the logical operators (`&&` and `||` are accepted
as aliases). `&` and `|` are the *bitwise* operators and only accept integers
([TYPE-139](../errors/type.md#type-139)).

Using an operator on a type it is not defined for is an error, for example
adding two lists. For your own types, operators are defined through
[traits](./generics-traits.md).

## Ranges

`a..b` is a half-open range (excludes `b`) and `a..=b` includes it. They are
what `for` loops walk; see [Control flow](./control-flow.md).

## Optional values

A type written `T?` may hold `null`. See
[Optionals and failure](./errors-and-optionals.md).
