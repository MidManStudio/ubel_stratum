# Type errors (TYPE)

Type checking covers ordinary typing and the trait system. The 1xx numbering is historical: the old 2xx tier errors moved to their own [TIER](./tier.md) family and kept no gap-filling.

Every example below was compiled with the real `ubel` binary, and the output shown is exactly what it printed. Entries marked *no verified example yet* come from the compiler's diagnostics registry (`docs/DIAGNOSTICS_RULES.md`) but could not be reproduced from ordinary source when this page was written; the registry text is given as is.

## Mismatches, calls and inference

### TYPE-101

**TypeMismatch** — `` type mismatch: expected `x`, found `y` ``

A value has a different type from the one the context needs: a `let` with a written type, an argument, a `return`, an operand. When the expected type was fixed somewhere else, a note points at that place.

<!-- ubel-test: error TYPE-101 -->
```ubel
fn main() void {
    let f: float = 5
    println($"{f}")
}
```

```text
>>> error[TYPE-101]: type mismatch: expected `float`, found `int`
  --> 2:5
    |
  2 |     let f: float = 5
    |     ^^^^^^^^^^^^^^^^
<<<
```

**Fix:** Change the value, or the annotation. Convert explicitly where a conversion is meant.

### TYPE-102

**ArgumentCountMismatch** — `expected N argument(s), found M`

A call passes a different number of arguments than the function, method or built-in takes.

<!-- ubel-test: error TYPE-102 -->
```ubel
fn main() void {
    let items = List.new()
    items.push()
}
```

```text
>>> error[TYPE-102]: expected 1 argument(s), found 0
  --> 3:5
    |
  3 |     items.push()
    |     ^^^^^^^^^^^^
<<<
```

**Fix:** Add or remove arguments.

### TYPE-103

**NoSuchField** — `` type `T` has no field `f` ``

A field access or struct literal names a field the struct does not declare.

<!-- ubel-test: error TYPE-103 -->
```ubel
struct Rectangle {
    width:  int
    height: int

    pub fn new(w: int, h: int) Rectangle {
        return Rectangle { width = w, height = h }
    }
}

fn main() void {
    let rect = Rectangle.new(10, 5)
    let d = rect.depth
}
```

```text
>>> error[TYPE-103]: type `Rectangle` has no field `depth`
  --> 12:13
     |
  12 |     let d = rect.depth
     |             ^^^^^^^^^^
<<<
```

**Fix:** Check the spelling against the struct declaration.

### TYPE-104

**NoSuchMethod** — `` type `T` has no method `m` ``

A method call on a value whose type has no method of that name. This covers user structs and enums (including an enum with no `extend` or `impl` block at all), primitives and built-in collections. A struct field of function type is callable and is not a method: a method of the same name wins, then a field. Calling a value that is not a function at all is [TYPE-140](#type-140).

<!-- ubel-test: error TYPE-104 -->
```ubel
fn main() void {
    let items = List.new()
    items.push(1)
    items.frobnicate()
}
```

```text
>>> error[TYPE-104]: type `List<?T8>` has no method `frobnicate`
  --> 4:5
    |
  4 |     items.frobnicate()
    |     ^^^^^^^^^^^^^^^^^
<<<
```

**Fix:** Use a method that exists, add one with `extend` or `impl`, or correct the receiver.

### TYPE-105

**TryOnNonFallible** — `` `?` requires a fallible type (`T!`), found `x` ``

The `?` operator was applied to a value that is not fallible.

*No verified example yet.*

**Fix:** Apply `?` only to a value of a fallible type `T!`.

### TYPE-106

**AwaitOnNonTask** — `` `await` requires `Task<T>`, found `x` ``

`await` was applied to something that is not a `Task<T>`.

<!-- ubel-test: error TYPE-106 -->
```ubel
@tier(high)
async fn run() {
    let n = 5
    let m = await n
}

fn main() {
}
```

```text
>>> error[TYPE-106]: `await` requires `Task<T>`, found `int`
  --> 4:13
    |
  4 |     let m = await n
    |             ^^^^^^^
<<<
```

**Fix:** Await the result of an `async` function call, or drop the `await`.

### TYPE-107

**CannotInferType** — `cannot infer type — add an explicit type annotation`

Nothing in the program fixes this value's type.

*No verified example yet.*

**Fix:** Add a type annotation.

### TYPE-108

**GenericArgCountMismatch** — `` `T` expects N type argument(s), found M ``

A generic type, or a trait, is written with the wrong number of type arguments, including none where some are required (`Unique` needs one). It also covers a trait in an `impl` header or a bound with the wrong count, and a generic type written without its arguments in a trait impl.

<!-- ubel-test: error TYPE-108 -->
```ubel
fn bad(x: Unique) void {}

fn main() void {
}
```

```text
>>> error[TYPE-108]: `Unique` expects 1 type argument(s), found 0
  --> 1:11
    |
  1 | fn bad(x: Unique) void {}
    |           ^^^^^^
<<<
```

**Fix:** Write exactly the number of type arguments the declaration has.

### TYPE-113

**InlineListCapacityNotLiteral** — `` `InlineList.new(...)` requires a literal integer capacity ``

`InlineList<T>` lives on the stack and is bounded, so its capacity must be known at compile time. A variable or computed expression cannot be used.

<!-- ubel-test: error TYPE-113 -->
```ubel
fn main() void {
    let n = 10
    let v = InlineList.new(n)
}
```

```text
>>> error[TYPE-113]: InlineList.new(capacity) requires a literal integer capacity
  --> 3:13
    |
  3 |     let v = InlineList.new(n)
    |             ^^^^^^^^^^^^^^^^^
  ~> help: write the capacity as a plain integer literal, e.g. InlineList.new(64) — a variable or computed expression can't be used here
  <~
<<<
```

**Fix:** Write the capacity as an integer literal, for example `InlineList.new(64)`.

### TYPE-114

**DerefOnNonReference** — `` `*`/`deref` requires a reference type (`&T`/`ref T`), found `x` ``

The dereference operator was applied to a value that is not a reference.

<!-- ubel-test: error TYPE-114 -->
```ubel
fn main() {
    let n = 5
    let m = *n
}
```

```text
>>> error[TYPE-114]: `*`/`deref` requires a reference type (`&T`/`ref T`), found `int`
  --> 3:13
    |
  3 |     let m = *n
    |             ^^
<<<
```

**Fix:** Dereference a `&T` or `ref T`, or remove the `*`.

### TYPE-120

**IntLiteralOutOfRange** — `` literal `N` is out of range for `T` ``

An integer literal does not fit its type. That covers a suffixed literal (`300u8`) and an unsuffixed one whose context picked a type too narrow for it (`let x: u8 = 300`, an argument for a `u8` parameter). A literal directly under unary `-` is checked against the signed range, so `-128i8` is accepted.

<!-- ubel-test: error TYPE-120 -->
```ubel
fn main() void {
    let bad: u8 = 300
    println($"{bad}")
}
```

```text
>>> error[TYPE-120]: literal `300` is out of range for `u8`
  --> 2:19
    |
  2 |     let bad: u8 = 300
    |                   ^^^
  ~> help: use a value that fits `u8`, or give the binding a wider type
  <~
<<<
```

**Fix:** Use a value that fits, drop the suffix, or choose a wider type.

### TYPE-121

**TypeAliasCycle** — `` type alias `A` refers to itself, directly or through other aliases ``

A `type` alias chain never ends in a real type.

<!-- ubel-test: error TYPE-121 -->
```ubel
type A = A
type B = C
type C = B
type D = int

fn main() void {
    let x: D = 1
}
```

```text
>>> error[TYPE-121]: type alias `A` refers to itself, directly or through other aliases
  --> 1:1
    |
  1 | type A = A
    | ^^^^^^^^^^
  ~> help: an alias is just another name for a type, so it must end in a type that is not itself an alias of the same chain
  <~
<<<
>>> error[TYPE-121]: type alias `B` refers to itself, directly or through other aliases
  --> 2:1
    |
  2 | type B = C
    | ^^^^^^^^^^
  ~> help: an alias is just another name for a type, so it must end in a type that is not itself an alias of the same chain
  <~
<<<
>>> error[TYPE-121]: type alias `C` refers to itself, directly or through other aliases
  --> 3:1
    |
  3 | type C = B
    | ^^^^^^^^^^
  ~> help: an alias is just another name for a type, so it must end in a type that is not itself an alias of the same chain
  <~
<<<
```

The compiler printed 3 diagnostics for this example; the others follow from the first.

**Fix:** An alias is another name for a type, so it must end in a type that is not itself an alias of the same chain.

### TYPE-140

**NotCallable** — `` cannot call a value of type `T` ``

The callee of a call has a type that is never a function: a number, string, collection, tuple or struct value.

<!-- ubel-test: error TYPE-140 -->
```ubel
fn main() void {
    let n = 5
    n()
}
```

```text
>>> error[TYPE-140]: cannot call a value of type `{integer}`
  --> 3:5
    |
  3 |     n()
    |     ^
  ~> help: only a function, a closure, a method or an enum variant can be called
  <~
<<<
```

**Fix:** Only a function, closure, method or enum variant can be called.

### TYPE-141

**TupleIndexOutOfRange** — `` tuple index N is out of range for `(...)`, which has M element(s) ``

A literal index is past the end of a tuple, whose size is part of its type. A computed index is checked when the program runs, and a list is not a tuple.

<!-- ubel-test: error TYPE-141 -->
```ubel
fn main() void {
    let t = (1, 2)
    println(t[5])
}
```

```text
>>> error[TYPE-141]: tuple index 5 is out of range for `(?T7, ?T8)`, which has 2 element(s)
  --> 3:15
    |
  3 |     println(t[5])
    |               ^
  ~> help: the valid indexes are 0 to 1
  <~
<<<
```

**Fix:** Use an index from 0 to the tuple's last element.

### TYPE-142

**NotAStruct** — `` `Score` is an alias for `int`, which is not a struct or an enum ``

A struct literal is written with the name of an alias whose target is neither a struct nor an enum. An alias of a struct is seen through.

<!-- ubel-test: error TYPE-142 -->
```ubel
type Score = int

fn main() void {
    let s = Score { x = 1 }
    println(1)
}
```

```text
>>> error[TYPE-142]: `Score` is an alias for `int`, which is not a struct or an enum
  --> 4:13
    |
  4 |     let s = Score { x = 1 }
    |             ^^^^^^^^^^^^^^^
  ~> help: a struct literal needs the name of a struct; write a value of `int` directly, or alias a struct
  <~
<<<
```

**Fix:** Write the name of a struct, or write the value directly.

## Enums and match

### TYPE-109

**UnknownVariant** — `` enum `E` has no variant `V` ``

A variant name that the enum does not declare, in an expression or in a pattern (including a struct-payload pattern that names an undeclared field).

<!-- ubel-test: error TYPE-109 -->
```ubel
enum Direction {
    North,
    South,
    East,
    West
}

fn main() void {
    let dir = Direction.Northeast
}
```

```text
>>> error[TYPE-109]: enum `Direction` has no variant `Northeast`
  --> 9:15
    |
  9 |     let dir = Direction.Northeast
    |               ^^^^^^^^^^^^^^^^^^^
<<<
```

**Fix:** Use a declared variant name.

### TYPE-110

**VariantArityMismatch** — `` `E.V` expects N value(s), found M ``

A tuple-payload variant is built or matched with the wrong number of values. A bare name against a variant that has a payload counts as zero values.

<!-- ubel-test: error TYPE-110 -->
```ubel
enum Result {
    Ok(int),
    Err(string)
}

fn main() void {
    let r = Result.Ok(1, 2)
}
```

```text
>>> error[TYPE-110]: `Result.Ok` expects 1 value(s), found 2
  --> 7:13
    |
  7 |     let r = Result.Ok(1, 2)
    |             ^^^^^^^^^^^^^^^
<<<
```

**Fix:** Match the shape declared on the enum.

### TYPE-111

**NonExhaustiveMatch** — `match is not exhaustive — missing variant(s): ...`

A `match` on an enum does not cover every variant. The check looks at the top-level variant of each arm, and an arm with a `where` guard never counts toward coverage because the guard can fail.

<!-- ubel-test: error TYPE-111 -->
```ubel
enum Direction {
    North,
    South,
    East,
    West
}

fn describe(d: Direction) string {
    match d {
        North => return "heading north",
        South => return "heading south",
        East  => return "heading east",
    }
}

fn main() void {
    let desc = describe(Direction.North)
}
```

```text
>>> error[TYPE-111]: match is not exhaustive — missing variant(s): West
  --> 9:5
    |
  9 |     match d {
    |     ^^^^^^^^^
  ~> help: add arms for the missing variant(s), or a wildcard `_ => ...` arm to cover the rest
  <~
<<<
```

**Fix:** Add arms for the missing variants, or a wildcard `_ => ...` arm.

### TYPE-112

**MixedDiscriminantAndPayload** — `` enum `E` mixes an explicit discriminant variant with a payload-carrying variant ``

An enum may give explicit numeric values to its variants, or carry payloads, but not both.

<!-- ubel-test: error TYPE-112 -->
```ubel
enum Confused {
    Active = 1,
    Holding(int)
}

fn main() void {
    let c = Confused.Active
}
```

```text
>>> error[TYPE-112]: enum `Confused` mixes an explicit discriminant variant with a payload-carrying variant
  --> 1:1
    |
  1 | enum Confused {
    | ^^^^^^^^^^^^^^^
  ~> help: use either explicit discriminants on every fieldless variant, or payload variants — not both in the same enum
  <~
<<<
```

**Fix:** Use explicit discriminants on every fieldless variant, or payload variants, not both in one enum.

## Formatting, deriving and ordering

### TYPE-115

**InvalidFormatSpec** — `` `.precision` in a format spec doesn't apply to type `T` ``

A format spec part that does not apply to the value's type. Width, alignment and `?` apply to any type; `.precision` applies only to `float`, `double` and strings.

<!-- ubel-test: error TYPE-115 -->
```ubel
fn main() void {
    let n = 42
    println($"bad: {n:.2}")
}
```

```text
>>> error[TYPE-115]: `precision` in a format spec doesn't apply to type `int`
  --> 3:21
    |
  3 |     println($"bad: {n:.2}")
    |                     ^
  ~> help: `.precision` only applies to float/double (decimal places) or string (max length) — drop it, or format a value of one of those types
  <~
<<<
```

**Fix:** Drop the `.precision`, or format a value of one of those three types.

### TYPE-116

**UnknownDeriveTrait** — `` unknown derive trait `X` ``

`@derive(...)` names something the compiler does not derive. `Debug` and `Display` are automatic for every struct and enum and need no derive, and `PartialEq` on an enum is automatic too, so those are reported as unnecessary. `PartialEq` on a struct is accepted: it makes `==` structural instead of identity-based.

<!-- ubel-test: error TYPE-116 -->
```ubel
@derive(Comparable)
struct Money {
    cents: int
}

fn main() void {
    let a = Money { cents = 500 }
    println($"{a.cents}")
}
```

```text
>>> error[TYPE-116]: unknown derive trait `Comparable`
  --> 1:1
    |
  1 | @derive(Comparable)
    | ^^^^^^^^^^^^^^^^^^^
  ~> help: supported derive traits: `PartialEq`, `Eq`, `Hash`, `Ord`, `PartialOrd`, `Clone`
  <~
<<<
```

**Fix:** Remove the unneeded derive, or use a trait the compiler supports.

### TYPE-117

**DeriveRequiresOther** — `` `@derive(X)` needs another trait first ``

A derive is requested without a trait it builds on; for example `Ord` builds on `PartialOrd` and `Eq`, and `Hash` builds on `Eq`.

<!-- ubel-test: error TYPE-117 -->
```ubel
@derive(Ord)
struct Rank {
    value: int
}

fn main() void {
}
```

```text
>>> error[TYPE-117]: `@derive(Ord)` also needs `@derive(PartialOrd)`
  --> 1:1
    |
  1 | @derive(Ord)
    | ^^^^^^^^^^^^
  ~> help: add `@derive(PartialOrd, Ord)`
  <~
<<<
>>> error[TYPE-117]: `@derive(Ord)` also needs `@derive(Eq)`
  --> 1:1
    |
  1 | @derive(Ord)
    | ^^^^^^^^^^^^
  ~> help: add `@derive(Eq, Ord)`
  <~
<<<
```

The compiler printed 2 diagnostics for this example; the others follow from the first.

**Fix:** Add the prerequisite derives. The diagnostic's suggestion names them.

### TYPE-118

**TypeNotOrderable** — `` type `T` doesn't support ordering comparisons ``

`<`, `<=`, `>` or `>=` was used on a struct that has no ordering.

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

```text
>>> error[TYPE-118]: type `Unranked` doesn't support ordering comparisons
  --> 8:8
    |
  8 |     if a < b {
    |        ^^^^^
  ~> help: add `@derive(PartialOrd)` (or `@derive(Ord)`) to the struct, or compare a different field
  <~
<<<
```

**Fix:** Derive or implement the ordering traits for the type, or compare a field instead.

### TYPE-119

**AlternateFormatWithoutBase** — `` `#` in a format spec needs a numeric base (`x`/`X`/`o`/`b`) ``

The alternate form `#` adds a base prefix such as `0x`, so it only makes sense together with a base.

<!-- ubel-test: error TYPE-119 -->
```ubel
fn main() {
    let value = 42
    println($"{value:#}")
}
```

```text
>>> error[TYPE-119]: `#` in a format spec needs a numeric base (`x`/`X`/`o`/`b`)
  --> 3:16
    |
  3 |     println($"{value:#}")
    |                ^^^^^
  ~> help: add a trailing base, e.g. `{value:#x}`, or drop the `#`
  <~
<<<
```

**Fix:** Add a base: `{value:#x}`, or drop the `#`.

## Traits, impls and bounds

### TYPE-122

**NotATrait** — `` unknown trait `X` / `X` is a struct, not a trait ``

A name after `impl`, in a bound, or in a supertrait list is either not declared at all or names something that is not a trait.

<!-- ubel-test: error TYPE-122 -->
```ubel
struct Plain {
    p: int,
}

fn a<T: Missing>(x: T) int { return 1 }

fn b<T: Plain>(x: T) int { return 1 }

struct Holder<T: Nothing> {
    v: T,
}

enum Slot<T: Plain> {
    Full(T),
}

fn main() void {
    println(1)
}
```

```text
>>> error[TYPE-122]: unknown trait `Missing`
  --> 5:6
    |
  5 | fn a<T: Missing>(x: T) int { return 1 }
    |      ^
  ~> help: declare it with `trait Name { .. }`, or check the spelling
  <~
<<<
>>> error[TYPE-122]: `Plain` is a struct, not a trait
  --> 7:6
    |
  7 | fn b<T: Plain>(x: T) int { return 1 }
    |      ^
<<<
>>> error[TYPE-122]: unknown trait `Nothing`
  --> 9:15
    |
  9 | struct Holder<T: Nothing> {
    |               ^
  ~> help: declare it with `trait Name { .. }`, or check the spelling
  <~
<<<
>>> error[TYPE-122]: `Plain` is a struct, not a trait
  --> 13:11
     |
  13 | enum Slot<T: Plain> {
     |           ^
<<<
```

The compiler printed 4 diagnostics for this example; the others follow from the first.

**Fix:** Declare the trait with `trait X { ... }`, or fix the spelling.

### TYPE-123

**TraitMethodMissing** — `` `T` does not implement `m`, which trait `Tr` requires ``

An `impl` leaves out a method the trait declares without a default body. Reported once per missing method, at the `impl`.

<!-- ubel-test: error TYPE-123 -->
```ubel
trait Shape {
    fn area(self) int
    fn perimeter(self) int
}

struct Sq {
    s: int
}

impl Shape for Sq {
    fn area(self) int { return self.s * self.s }
}

fn main() {
}
```

```text
>>> error[TYPE-123]: `Sq` does not implement `perimeter`, which trait `Shape` requires
  --> 10:1
     |
  10 | impl Shape for Sq {
     | ^^^^^^^^^^^^^^^^^^^
  ~> help: add `fn perimeter(..)` to the impl, or give the method a default body in the trait
  <~
<<<
```

**Fix:** Add the method to the impl, or give it a default body in the trait.

### TYPE-124

**UnknownTraitMethod** — `` `m` is not a method of trait `Tr` ``

An `impl Trait for Type` block defines a method the trait does not declare.

<!-- ubel-test: error TYPE-124 -->
```ubel
trait Shape {
    fn area(self) int
}

struct Sq {
    s: int
}

impl Shape for Sq {
    fn area(self) int { return self.s * self.s }
    fn extra(self) int { return 2 }
}

fn main() {
}
```

```text
>>> error[TYPE-124]: `extra` is not a method of trait `Shape`
  --> 11:5
     |
  11 |     fn extra(self) int { return 2 }
     |     ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
  ~> help: move it to an inherent `extend` block, or declare it in the trait
  <~
<<<
```

**Fix:** Move it to an inherent `extend` block, or declare it in the trait.

### TYPE-125

**TraitMethodSignatureMismatch** — `` method `m` does not match trait `Tr`: expected `fn(self) int`, found `fn(self) string` ``

An impl method's signature differs from the trait's, with `Self` replaced by the implementing type: parameter count, parameter types, return type, whether it takes `self`, and fallibility. Parameter names do not matter.

<!-- ubel-test: error TYPE-125 -->
```ubel
trait Shape {
    fn area(self) int
}

struct Sq {
    s: int,
}

impl Shape for Sq {
    fn area(self) string { return "x" }
}

fn main() void {
}
```

```text
>>> error[TYPE-125]: method `area` does not match trait `Shape`: expected `fn(self) int`, found `fn(self) string`
  --> 10:5
     |
  10 |     fn area(self) string { return "x" }
     |     ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
<<<
```

**Fix:** Make the impl's signature match the trait's.

### TYPE-126

**UnsatisfiedBound** — `` the trait `Tr` is not implemented for `T` ``

A type is used where a trait bound requires an impl it does not have: a call to a generic function, a struct literal for a bounded struct, a qualified call `Tr.m(value)`, or a coercion to `dyn Tr`. A type parameter satisfies a bound only if it is declared with it.

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

```text
>>> error[TYPE-126]: the trait `Show` is not implemented for `Plain`
  --> 14:14
     |
  14 |     print_it(Plain { n = 1 })
     |              ^^^^^^^^^^^^^^^^
  ~> help: add `impl Show for Plain { .. }`
  <~
<<<
```

**Fix:** Add `impl Tr for T { ... }`.

### TYPE-127

**AmbiguousTraitMethod** — `` method `m` on `T` is supplied by more than one trait (`A`, `B`) ``

An unqualified call when two or more traits supply the name to the type and the type has no inherent method of that name.

<!-- ubel-test: error TYPE-127 -->
```ubel
trait A {
    fn go(self) int
}

trait B {
    fn go(self) int
}

struct T {
    v: int,
}

impl A for T { fn go(self) int { return 1 } }
impl B for T { fn go(self) int { return 2 } }

fn main() void {
    let t = T { v = 0 }
    println(t.go())
}
```

```text
>>> error[TYPE-127]: method `go` on `T` is supplied by more than one trait (`A`, `B`)
  --> 18:13
     |
  18 |     println(t.go())
     |             ^^^^^
  ~> help: name the trait: `A.go(value)`
  <~
<<<
```

**Fix:** Name the trait: `A.m(value)`.

### TYPE-128

**OverlappingImpl** — `` `Tr` is implemented for `T` more than once ``

A type has exactly one impl of a trait, whatever the type arguments. The second impl is skipped. Also reported for a hand-written impl of a prelude trait on a struct that already derives it. Operator traits are the exception: a type has one impl per kind of right-hand operand.

<!-- ubel-test: error TYPE-128 -->
```ubel
trait Shape {
    fn area(self) int
}

struct Sq {
    s: int
}

impl Shape for Sq { fn area(self) int { return 1 } }
impl Shape for Sq { fn area(self) int { return 2 } }

fn main() {
}
```

```text
>>> error[TYPE-128]: `Shape` is implemented for `Sq` more than once
  --> 10:1
     |
  10 | impl Shape for Sq { fn area(self) int { return 2 } }
     | ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
<<<
```

**Fix:** Remove or merge the duplicate impl.

### TYPE-129

**UnsupportedTraitFeature** — `<feature> is not supported yet`

A trait construct that parses but is not built yet, reported instead of being silently ignored. Examples: an associated type, a blanket impl (a type parameter as the impl target), an impl for a `Pool` or `Handle`, a hand-written `Display` for a built-in type, an operator impl whose right operand is a bare type parameter. The list shrinks as each feature is built.

<!-- ubel-test: error TYPE-129 -->
```ubel
trait Shape {
    fn area(self) int
}

impl Shape for int {
    fn area(self) int { return 1 }
}

trait Conv<T> {
    fn conv(self) T
}

trait Holder {
    type Item
    fn get(self) int
}

fn main() void {
}
```

```text
>>> error[TYPE-129]: an associated type is not supported yet
  --> 14:10
     |
  14 |     type Item
     |          ^^^^
<<<
```

**Fix:** Use a supported form; the message names the feature.

### TYPE-130

**MethodOnUnboundedParam** — `` cannot call `m` on a value of type parameter `T`: it has no trait bound ``

A method is called on a value whose type is a type parameter with no bound in force. A parameter that has bounds, none of which declares the method, is [TYPE-104](#type-104) instead.

<!-- ubel-test: error TYPE-130 -->
```ubel
fn total<T>(x: T) int {
    return x.area()
}

struct Box<T> {
    v: T,
    fn show(self) int { return self.v.area() }
}

struct Crate<T> {
    v: T,
}

extend Crate {
    fn show(self) int { return self.v.area() }
}

fn main() void {
    println(1)
}
```

```text
>>> error[TYPE-130]: cannot call `area` on a value of type parameter `T`: it has no trait bound
  --> 2:12
    |
  2 |     return x.area()
    |            ^^^^^^^
  ~> help: declare a bound that provides `area`, as in `<T: SomeTrait>`
  <~
<<<
>>> error[TYPE-130]: cannot call `area` on a value of type parameter `T`: it has no trait bound
  --> 7:32
    |
  7 |     fn show(self) int { return self.v.area() }
    |                                ^^^^^^^^^^^
  ~> help: declare a bound that provides `area`, as in `<T: SomeTrait>`
  <~
<<<
>>> error[TYPE-130]: cannot call `area` on a value of type parameter `T`: it has no trait bound
  --> 15:32
     |
  15 |     fn show(self) int { return self.v.area() }
     |                                ^^^^^^^^^^^
  ~> help: declare a bound that provides `area`, as in `<T: SomeTrait>`
  <~
<<<
```

The compiler printed 3 diagnostics for this example; the others follow from the first.

**Fix:** Declare a bound that provides the method, as in `<T: SomeTrait>`.

### TYPE-131

**OperatorNeedsBound** — `` `<op>` on a value of type parameter `T` needs a `<Trait>` bound ``

An operator or index is used on a value of a type parameter that lacks the bound for it: `==` and `!=` need `PartialEq`, `<` and friends need `PartialOrd`, arithmetic needs `Add`, `Sub` and so on, `a[i]` needs `Index`. A bound of `Eq` or `Ord` implies the comparison ones.

<!-- ubel-test: error TYPE-131 -->
```ubel
fn at<T>(a: T) int {
    return a[0]
}

fn main() void {
    println(at([1, 2, 3]))
}
```

```text
>>> error[TYPE-131]: `[]` on a value of type parameter `T` needs a `Index` bound
  --> 2:12
    |
  2 |     return a[0]
    |            ^^^^
  ~> help: declare the bound, as in `<T: Index<Idx, Out>>`
  <~
<<<
```

**Fix:** Declare the bound, as in `<T: PartialEq>`.

### TYPE-132

**MissingSupertraitImpl** — `` `impl Tr for T` needs `Super`, which `T` does not implement ``

An impl of a trait whose supertrait the type lacks. For the prelude, `impl Ord` needs `PartialOrd` and `Eq`; `impl Hash` needs `Eq`. For `trait B: A`, `impl B` needs `A`. Reported once per impl, for the first missing supertrait.

<!-- ubel-test: error TYPE-132 -->
```ubel
trait Named {
    fn name(self) string
}

trait Greeter: Named {
    fn greet(self) string
}

struct Person {
    n: int
}

impl Greeter for Person {
    fn greet(self) string { return "hi" }
}

fn main() {
}
```

```text
>>> error[TYPE-132]: `impl Greeter for Person` needs `Named`, which `Person` does not implement
  --> 13:1
     |
  13 | impl Greeter for Person {
     | ^^^^^^^^^^^^^^^^^^^^^^^^^
  ~> help: add `impl Named for Person { .. }`
  <~
<<<
```

**Fix:** Add `@derive(...)` or an `impl` for the missing trait.

### TYPE-133

**SupertraitCycle** — `` trait `A` is its own supertrait: A -> B -> A ``

The chain of `: ...` supertrait links loops back on itself, directly (`trait A: A`) or through other traits. Reported once per cycle.

<!-- ubel-test: error TYPE-133 -->
```ubel
trait A: B {
    fn a(self) int
}

trait B: A {
    fn b(self) int
}

fn main() {
}
```

```text
>>> error[TYPE-133]: trait `A` is its own supertrait: A -> B -> A
  --> 5:10
    |
  5 | trait B: A {
    |          ^
  ~> help: a supertrait is required by the trait that builds on it, so the chain of `: ...` links must end; remove one of them
  <~
<<<
```

**Fix:** Remove one of the links.

### TYPE-134

**ConflictingBounds** — `` `T` is bounded by both `Convert<int>` and `Convert<string>`, and no type implements both ``

Two bounds on one type parameter apply the same trait to different arguments. A type has one impl of a trait, so no type can meet both.

<!-- ubel-test: error TYPE-134 -->
```ubel
trait Convert<T> {
    fn convert(self) T
}

fn both<X: Convert<int> + Convert<string>>(x: X) {
}

fn main() {
}
```

```text
>>> error[TYPE-134]: `X` is bounded by both `Convert<int>` and `Convert<string>`, and no type implements both
  --> 5:27
    |
  5 | fn both<X: Convert<int> + Convert<string>>(x: X) {
    |                           ^^^^^^^
  ~> help: a type has one impl of `Convert`, so a parameter can be bounded by it once; keep one of the two
  <~
<<<
```

**Fix:** Keep one of the two bounds. (Operator traits are the exception: a parameter can be bounded once per right operand.)

### TYPE-138

**NotObjectSafe** — `` the trait `Animal` cannot be used as `dyn Animal`: `Animal.kind` has no `self` receiver ``

A `dyn Trait` type names a trait with a method that has no `self`, has generic parameters of its own, or mentions `Self` outside the receiver. Of the prelude traits, `PartialEq`, `Eq`, `PartialOrd`, `Ord`, `Clone` and `Hash` are not object safe; `Display`, `Iterator`, the operator traits and `Index` are.

<!-- ubel-test: error TYPE-138 -->
```ubel
fn compare(a: dyn PartialEq) int {
    return 0
}

fn main() void {
    println(1)
}
```

```text
>>> error[TYPE-138]: the trait `PartialEq` cannot be used as `dyn PartialEq`: `PartialEq.eq` mentions `Self` outside its receiver; `PartialEq.ne` mentions `Self` outside its receiver
  --> 1:15
    |
  1 | fn compare(a: dyn PartialEq) int {
    |               ^^^
  ~> help: a trait can be `dyn` when every method takes `self`, has no generic parameters of its own and does not mention `Self` outside the receiver; move the other methods to a separate trait
  <~
<<<
```

**Fix:** Move the offending methods to a separate trait.

## Operators, indexing and iteration

### TYPE-135

**NotIterable** — `` cannot iterate over a value of type `T` ``

A `for` loop over a value the runtime cannot walk. A loop walks a list or array, `Queue`, `Stack`, `Dictionary` (as `(key, value)` pairs), `InlineList`, `Pool`, string, tuple, range, a struct or enum that implements `Iterator<T>`, and a type parameter bounded by it. A number, a function, a `Linqerizer` and a struct with no `Iterator` impl are not iterable.

<!-- ubel-test: error TYPE-135 -->
```ubel
struct Inventory {
    count: int,
}

fn main() void {
    let inv = Inventory { count = 3 }
    for item in inv {
        println(item)
    }
}
```

```text
>>> error[TYPE-135]: cannot iterate over a value of type `Inventory`
  --> 7:17
    |
  7 |     for item in inv {
    |                 ^^^
  ~> help: add `impl Iterator<Item> for Inventory { fn next(self) Item? { .. } }`, or iterate one of its collection fields
  <~
<<<
```

**Fix:** Iterate a collection, or add `impl Iterator<Item> for T { fn next(self) Item? { ... } }`.

### TYPE-136

**OperatorNotImplemented** — `` cannot apply `*` to `Vec2` and `Vec2`: no `Mul<R, Out>` impl for this pair ``

An operator, unary `-` or `~`, compound assignment or index is applied to a struct or enum with no impl of the operator's trait for that right operand. The suggestion shows the impl to add, or lists the right operands the type already supports.

<!-- ubel-test: error TYPE-136 -->
```ubel
struct V {
    x: int,
}

impl Add<V, V> for V {
    fn add(self, rhs: V) V {
        return V { x = self.x + rhs.x }
    }
}

fn main() void {
    let a: dyn Add<V, V> = V { x = 5 }
    let s = a + 3
    println(1)
}
```

```text
>>> error[TYPE-136]: cannot apply `+` to `dyn Add<V, V>` and `int`: no `Add<R, Out>` impl for this pair
  --> 13:13
     |
  13 |     let s = a + 3
     |             ^^^^^
  ~> help: `+` on `dyn Add<V, V>` is implemented with V; add `impl Add<int, Out> for dyn Add<V, V> { .. }` for `int`, or use one of those
  <~
<<<
```

**Fix:** Add the impl the suggestion shows, or use one of the supported operand types.

### TYPE-137

**NotIndexable** — `` cannot index a value of type `T` ``

An index is applied to a value the runtime cannot index (a number, `bool`, `Queue`, `Stack`, `Set`, `Pool`, a function), or an assignment goes through the index of a string or tuple, which are read only.

<!-- ubel-test: error TYPE-137 -->
```ubel
fn main() void {
    let q = Queue.new()
    q.enqueue(1)
    q.enqueue(2)
    println(q[0])
}
```

```text
>>> error[TYPE-137]: cannot index a value of type `Queue<?T8>`
  --> 5:13
    |
  5 |     println(q[0])
    |             ^^^^^
  ~> help: a `List`, an array, a `Dictionary`, a `string`, a tuple, and a struct or enum that implements `Index<Idx, Out>` can be indexed
  <~
<<<
```

**Fix:** Index a list, array, `Dictionary`, string or tuple, or a type that implements `Index<Idx, Out>`.

### TYPE-139

**OperatorNotSupported** — `` operator `+` is not defined for `List<int>` ``

A built-in operator applied to a built-in type it is not defined for: `[1] + [2]`, `true & false`, `1.5 & 2.5`, `-x` for an unsigned `x`, `~true`. `+` takes numbers, and a string with a string; `-` `*` `/` `%` take numbers; `&` `|` `^` `<<` `>>` and `~` take integers.

<!-- ubel-test: error TYPE-139 -->
```ubel
fn main() void {
    println(1.5 & 2.5)
}
```

```text
>>> error[TYPE-139]: operator `&` is not defined for `double`
  --> 2:13
    |
  2 |     println(1.5 & 2.5)
    |             ^^^^^^^^^
  ~> help: `&` works on integers
  <~
<<<
```

**Fix:** Use an operator that fits the type; for booleans use `and` and `or`.

### TYPE-143

**AwaitOutsideAsync** — `` `await` is only valid inside an async function or an async block ``

An `await` that is not directly inside an async function or an async block: in a plain function, in a method that is not async, in a constant or static initializer, or inside a plain lambda. A lambda inside an async function is not an async context, because `await` does not reach through a closure. It is reported in every tier.

<!-- ubel-test: error TYPE-143 -->
```ubel
async fn one() Task<int> {
    return 1
}

fn plain() int {
    return await one()
}

fn main() {
}
```

```text
>>> error[TYPE-143]: `await` is only valid inside an async function or an async block
  --> 6:12
    |
  6 |     return await one()
    |            ^^^^^^^^^^^
<<<
```

**Fix:** Make the enclosing function `async` and declare the task it returns, or build the work as an `async { ... }` block and await that from async code.

### TYPE-144

**AsyncReturnNotTask** — `` an async function returns `Task<T>`, found `int` ``

An `async fn` whose declared result is not a `Task<T>`. The declared type is the task and the body returns the `T`: `async fn count() Task<int>` with `return 1`. With no declared result, `async fn run()` means `Task<void>` and is fine. A fallible task, `Task<int>!`, is not a task result either; a task that yields a fallible value, `Task<int!>`, is.

<!-- ubel-test: error TYPE-144 -->
```ubel
async fn count() int {
    return 1
}

fn main() {
}
```

```text
>>> error[TYPE-144]: an async function returns `Task<T>`, found `int`
  --> 1:1
    |
  1 | async fn count() int {
    | ^^^^^^^^^^^^^^^^^^^^^^
<<<
```

**Fix:** Declare the task: `async fn count() Task<int>`. The body still returns the `int`.

### TYPE-145

**MarkerImplField** — `` `impl Send for Holder`: field `cell` has type `Shared<int>`, which is not `Send` ``

An `impl Send` or an `impl Sync` for a struct or an enum where a field, or the payload of a variant, has a type without that marker. The impl is checked against every field of a struct and every payload of every variant of an enum, under the conditions the impl wrote on its type parameters, and only the first part that fails is named. `Shared<T>`, references, borrowed views, function values and tasks have neither marker, and a struct or an enum has one only through an impl of its own. An `unsafe impl` is not checked.

<!-- ubel-test: error TYPE-145 -->
```ubel
struct Holder { cell: Shared<int> }

impl Send for Holder {
}

fn main() {
}
```

```text
>>> error[TYPE-145]: `impl Send for Holder`: field `cell` has type `Shared<int>`, which is not `Send`
  --> 3:1
    |
  3 | impl Send for Holder {
    | ^^^^^^^^^^^^^^^^^^^^^^
  ~> help: give the field a type that is `Send`, or, if the type is sound to use that way, write `unsafe impl Send for ...` (trusted, not checked)
  <~
<<<
```

**Fix:** Give the field a type that is `Send`. If the type is sound to use that way in spite of what the compiler can see, write `unsafe impl Send for Holder {}`, which is trusted and not checked.

### TYPE-146

**UnsafeImplNotMarker** — `` `unsafe impl` is only for `Send` and `Sync`, found an inherent impl ``

An `unsafe impl` of anything but the prelude's `Send` or `Sync`: an impl of another trait, or an impl with no trait. The impl is still read as an ordinary impl, so only the keyword is reported.

<!-- ubel-test: error TYPE-146 -->
```ubel
struct Gauge { reading: int }

unsafe impl Gauge {
    fn read(self) int { return self.reading }
}

fn main() {
}
```

```text
>>> error[TYPE-146]: `unsafe impl` is only for `Send` and `Sync`, found an inherent impl
  --> 3:1
    |
  3 | unsafe impl Gauge {
    | ^^^^^^^^^^^^^^^^^^^
  ~> help: drop `unsafe`: an impl of any other trait is checked like every impl
  <~
<<<
```

**Fix:** Drop `unsafe`. An impl of any other trait is checked like every impl.
