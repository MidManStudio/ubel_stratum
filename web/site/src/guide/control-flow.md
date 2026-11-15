# Control flow

## `if`, `elif`, `else`

Conditions need no parentheses. `if` can also be used as an expression,
which gives the value of whichever branch ran.

<!-- ubel-test: run -->
```ubel
fn classify(n: int) string {
    if n < 0 {
        return "negative"
    } elif n == 0 {
        return "zero"
    } else {
        return "positive"
    }
}

fn main() {
    println(classify(-3))
    let n = 12
    let size = if n > 10 { "big" } else { "small" }
    println(size)
}
```

```text
negative
big
```

## Loops

`while` repeats while a condition holds, `loop` repeats until a `break`, and
`for` walks a range or a collection.

<!-- ubel-test: run -->
```ubel
fn main() {
    let mut i = 0
    while i < 3 {
        i += 1
    }

    let mut n = 0
    loop {
        n += 1
        if n == 4 {
            break
        }
    }

    for k in 0..10 {          // 0, 1, ... 9
        if k % 2 == 0 {
            continue
        }
        println(k)
    }

    for k in 1..=3 {          // 1, 2, 3: `..=` includes the end
        println(k)
    }
    println($"{i} {n}")
}
```

```text
1
3
5
7
9
1
2
3
3 4
```

A `for` loop can walk a list or array, a dictionary (as `(key, value)`
pairs), a queue, a stack, an `InlineList`, a string, a tuple, a range, or
any struct that implements `Iterator`
([TYPE-135](../errors/type.md#type-135) lists what cannot be walked).

## `match`

`match` compares a value against patterns, top to bottom. It works on
numbers, strings and enums.

<!-- ubel-test: run -->
```ubel
fn main() {
    let n = 7
    match n {
        1 => println("one"),
        7 => println("seven"),
        _ => println("something else"),   // `_` matches anything
    }
}
```

On an enum, the compiler checks that every variant is covered; see
[Structs and enums](./structs-enums.md) for patterns that bind payloads, and
[TYPE-111](../errors/type.md#type-111) for the non-exhaustive error.

### Guards

An arm can carry a `where` guard. A guarded arm only matches when the guard
is true, and it does not count toward exhaustiveness, because the guard can
fail.

<!-- ubel-test: run -->
```ubel
enum Reading {
    Ok(int),
    Bad(string)
}

fn describe(r: Reading) string {
    match r {
        Ok(v) where v > 100 => return "high",
        Ok(v) => return "fine",
        Bad(why) => return "bad: " + why,
    }
}

fn main() {
    println(describe(Reading.Ok(500)))
    println(describe(Reading.Ok(5)))
    println(describe(Reading.Bad("sensor")))
}
```

```text
high
fine
bad: sensor
```
