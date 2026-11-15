# Built-in functions

These functions are available everywhere with no import. There is no standard
library yet; this is the whole set the interpreter provides as global
functions. Methods on strings and collections are listed in
[Strings](../guide/strings.md) and [Collections](../guide/collections.md).

| Function | What it does |
|----------|--------------|
| `println(...)` | print the arguments, then a newline |
| `print(...)` | print the arguments with no newline |
| `assert(cond, ...)` | stop with a runtime error if `cond` is false; the rest is the message |
| `panic(...)` | stop with a runtime error and the given message |
| `abs(x)` | absolute value |
| `max(a, b)`, `min(a, b)` | larger or smaller of two numbers |
| `sqrt(x)` | square root |
| `floor(x)`, `ceil(x)` | round down or up |
| `to_int(x)`, `to_double(x)`, `to_float(x)` | numeric conversion |
| `to_string(x)` | text form of a value |
| `typeof(x)` | the name of a value's type, as a string |
| `range(start, end)` | the integers from `start` up to, not including, `end` |
| `log(x)` | a debug print to the error stream (prefixed `[log]`); not a logarithm |

<!-- ubel-test: run -->
```ubel
fn main() {
    println(abs(-5))
    println(max(3, 9))
    println(min(3, 9))
    println(sqrt(16.0))
    println(floor(2.7))
    println(ceil(2.1))
    println(to_int(3.9))
    println(to_double(2))
    println(to_string(42) + "!")
    println(typeof(42))
    println(typeof("s"))
    println(typeof(1.5))
    assert(1 + 1 == 2, "math works")
    for i in range(1, 4) {
        println(i)
    }
}
```

```text
5
9
3
4
2
3
3
2
42!
int
string
double
1
2
3
```

`range` takes exactly two integers; `range(3)` is a runtime error. When
`assert` or `panic` fires, the program stops and `ubel run` exits with
code 1:

```text
✗ runtime error: panic: math is broken
```

There is no way to read input or touch the file system from Ubel code yet.
