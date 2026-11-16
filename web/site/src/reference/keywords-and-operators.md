# Keywords and operators

## Keywords

These words are reserved: they cannot be used as variable, function or type
names. The list is the lexer's own table (`crates/core/src/lexer/keywords.rs`).

| Group | Keywords |
|-------|----------|
| Bindings | `let` `mut` `const` `static` `type` |
| Control flow | `if` `elif` `else` `match` `where` `then` `for` `in` `while` `loop` `break` `continue` `return` |
| Functions and failure | `fn` `fail` `try` `catch` `defer` `unsafe` |
| Types and items | `struct` `enum` `trait` `impl` `extend` `edge` `pub` `self` `lifetime` |
| Modules | `package` `summon` `from` `as` |
| Async | `async` `await` `Task` |
| Operators in word form | `and` `or` `not` |
| Values | `true` `false` `null` |
| References | `ref` `deref` |
| Other | `with` `using` `extract` `getter` `setter` |
| Tiers and memory | `tier` `high` `mid` `low` `arena` `pool` `gc` `heap` |
| Built-in collection types | `List` `Dictionary` `Set` `Queue` `Stack` `InlineList` |

A few notes:

- `Set` is reserved but has no constructor yet: `Set.new()` reports
  `undefined name` ([NAME-001](../errors/name.md#name-001)).
- `Pool`, `Handle`, `Linqerizer`, `Unique`, `Shared` and `SyncShared` are
  built-in *names*, not reserved words.
- `dyn` and `outlives` are not reserved either. They are recognised by the
  parser only in the places they mean something (a `dyn Trait` type, a
  `where L outlives M` clause), so they remain usable as ordinary names
  elsewhere.
- The tier words (`tier`, `high`, `mid`, `low`, `arena`, `pool`, `gc`, `heap`)
  are reserved, so a variable cannot be called `low` or `mid`.
- Some reserved words (for example `getter`, `setter`, `extract`, `defer`,
  `try` and `catch`) belong to features this Guide does not cover yet.

## Operators

From the loosest-binding (evaluated last) to the tightest. Operators in one row
share a level and group left to right, except assignment, which groups right to
left.

| Level | Operators | Notes |
|------:|-----------|-------|
| 1 | `=` `+=` `-=` `*=` `/=` `%=` `&=` `\|=` `^=` `<<=` `>>=` | assignment, right to left |
| 2 | `..` `..=` | range, end excluded or included |
| 3 | `or` `\|\|` | logical or |
| 4 | `and` `&&` | logical and |
| 5 | `==` `!=` | equality |
| 6 | `<` `>` `<=` `>=` | comparison |
| 7 | `\|>` | pipe: `a \|> f(b)` is `f(a, b)` |
| 8 | `\|` | bitwise or |
| 9 | `^` | bitwise xor |
| 10 | `&` | bitwise and |
| 11 | `<<` `>>` | shifts |
| 12 | `+` `-` | add, subtract |
| 13 | `*` `/` `%` | multiply, divide, remainder |
| 14 | `as` | type conversion; the right side is a type |
| 15 | `-x` `not x` `!x` `~x` `&x` `*x` and the keyword forms | prefix operators |
| 16 | `.` `()` `[]` `?` `?.` | member access, call, index, failure propagation, optional access |

Three things differ from C and its relatives:

- **The bitwise operators bind tighter than comparison.** `x & 1 == 0` means
  `(x & 1) == 0`.
- **`and` binds tighter than `or`,** as you would expect, and both are looser
  than comparison. `||` and `&&` are the same operators spelled with symbols.
- **The pipe `|>` sits between comparison and the bitwise operators,** so
  arithmetic happens before the pipe: `1 + 2 |> twice` is `twice(3)`.

<!-- ubel-test: run -->
```ubel
fn twice(x: int) int { return x * 2 }

fn main() {
    println(1 + 2 * 3)                // 7: `*` binds tighter than `+`
    println(6 & 1 == 0)               // true: (6 & 1) == 0
    println(2 + 3 << 1)               // 10: (2 + 3) << 1
    println(true or false and false)  // true: true or (false and false)
    println(5 |> twice == 10)         // true: (5 |> twice) == 10
    println(1 + 2 |> twice)           // 6: twice(1 + 2)
    println(10 - 4 - 3)               // 3: left to right
}
```

```text
7
true
10
true
true
6
3
```

The range operators bind *looser* than comparison, so a comparison inside a
range bound needs parentheses.

### What operators do on which types

An operator applied to a type it is not defined for is
[TYPE-139](../errors/type.md#type-139) for built-in types and
[TYPE-136](../errors/type.md#type-136) for your own; operators on a generic
value need the matching bound
([TYPE-131](../errors/type.md#type-131)). Integer literals must fit their
type ([TYPE-120](../errors/type.md#type-120)). See
[Values and variables](../guide/values.md#operators).

## Literals and punctuation

| Form | Is |
|------|----|
| `123` `1_000` `0xFF` `0b101` `300u8` `1.5` `1e-3` | numbers; suffixes `u8 i8 u16 i16 u32 i32 u64 i64 usize isize` on integers |
| `64KB` `1MB` `1GB` | size literals, for arena sizes |
| `'a'` | a character |
| `"..."` `@"..."` `$"..."` `$@"..."` | strings: plain, verbatim, interpolated, both; see [Strings](../guide/strings.md) |
| `// ...` `/* ... */` | comments; block comments nest |
| `@name` | an attribute, such as `@tier(mid)` or `@derive(...)` |
| `T?` `T!` | an optional type, a fallible type |
| `=>` | separates a `match` pattern from its arm |
| `_` | a wildcard pattern in `match` |
