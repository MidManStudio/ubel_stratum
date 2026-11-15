# Strings and formatting

A `string` is a sequence of Unicode characters.

## Literals

<!-- ubel-test: run -->
```ubel
fn main() {
    let plain = "tab\there, quote \" and backslash \\"
    let verbatim = @"C:\temp\notes.txt"    // backslashes are ordinary
    let name = "ada"
    let greeting = $"hello, {name}!"       // interpolation
    println(plain)
    println(verbatim)
    println(greeting)
}
```

| Form | Meaning |
|------|---------|
| `"..."` | plain string; escapes `\"` `\\` `\n` `\r` `\t` (an interpolated string also takes `\{` and `\}`) |
| `@"..."` | verbatim: no escapes, write `""` for a quote, may span lines |
| `$"..."` | interpolated: `{expr}` holes hold any expression |
| `$@"..."` | interpolated and verbatim |

Inside `$"..."`, write `\{` and `\}` for literal braces (`{{` is not an escape: it opens a hole). A hole that is never closed
is [LEX-006](../errors/lex.md#lex-006), and a name inside a hole that does
not exist is a normal [NAME-001](../errors/name.md#name-001).

## Methods

<!-- ubel-test: run -->
```ubel
fn main() {
    let s = "  Hello, Ubel  "
    println(s.trim())                      // "Hello, Ubel"
    println(s.trim().to_upper())
    println(s.trim().to_lower())
    println(s.len())
    println(s.contains("Ubel"))
    println(s.trim().starts_with("Hello"))
    println(s.trim().ends_with("Ubel"))
    println(s.replace("Ubel", "world"))
    println("a,b,c".split(",").len())      // split returns a list
    println("ab" + "cd")                   // concatenation
    println("apple" < "banana")            // ordering comparison
}
```

The string methods are `len`, `is_empty`, `to_upper`, `to_lower`, `trim`,
`trim_start`, `trim_end`, `chars`, `contains`, `starts_with`, `ends_with`,
`split` and `replace`. A method that does not exist is
[TYPE-104](../errors/type.md#type-104).

Strings are compared by their characters, and `+` joins two strings. You
cannot add a number to a string ([TYPE-101](../errors/type.md#type-101));
convert it with `to_string` first.

## Format specs

A hole may carry a spec after a colon: `{expr:spec}`, with the parts in this
order: **alignment, width, `.precision`, `?`** and, for integers, a sign,
zero-padding or base.

<!-- ubel-test: run -->
```ubel
fn main() {
    let pi = 3.14159
    println($"pi = {pi:.2}")                    // precision
    println($"[{42:>6}] [{42:<6}] [{42:^6}]")   // right, left, centre in 6
    println($"[{pi:>10.2}]")                    // width and precision together
    let phrase = "hello there"
    println($"{phrase:.5}")                     // precision truncates a string
    println($"hex {255:x}  binary {5:b}")
}
```

```text
pi = 3.14
[    42] [42    ] [  42  ]
[      3.14]
hello
hex ff  binary 101
```

Two mistakes the type checker catches:

<!-- ubel-test: error TYPE-115 -->
```ubel
fn main() {
    println($"{42:.2}")
}
```

`.precision` only applies to floating-point numbers and strings
([TYPE-115](../errors/type.md#type-115)).

<!-- ubel-test: error TYPE-119 -->
```ubel
fn main() {
    println($"{42:#}")
}
```

`#` adds a base prefix and so needs a base: write `{42:#x}`
([TYPE-119](../errors/type.md#type-119)).

> Known limit: the sign, zero-pad and base specs (`{x:+}`, `{x:05}`,
> `{x:x}`) work on plain `int`, not yet on the fixed-width integer types.
> See [Project Status](../project-status.md#known-gaps-tracked-rather-than-hidden).
