# Optionals and failure

Ubel has two separate ideas that other languages blur together:

- **Absence**: a value that may not exist. The type is `T?`.
- **Failure**: an operation that may go wrong. The type is `T!`.

## Optional values: `T?`

A `T?` holds a `T` or `null`. Compare against `null`, or read a field with
`?.`, which gives `null` instead of failing when the value is missing.

<!-- ubel-test: run -->
```ubel
struct User {
    name: string
}

fn find(id: int) User? {
    if id == 1 {
        return User { name = "ada" }
    }
    return null
}

fn main() {
    let found = find(1)
    let missing = find(2)
    println(missing == null)     // true
    println(found?.name)         // ada
}
```

## Fallible functions: `T!`

A function that may fail has a `!` after its return type, and reports failure
with `fail`. The caller either handles the failure or passes it on with `?`.

<!-- ubel-test: run -->
```ubel
fn parse_positive(n: int) int! {
    if n < 0 {
        fail "negative"
    }
    return n
}

fn double_it(n: int) int! {
    let v = parse_positive(n)?     // on failure, return the failure from here
    return v * 2
}

fn main() {
    let r = double_it(4)
    println("ok")
}
```

Failures travel up the call chain through `?` until something handles them.

## Async

`async fn` and `await` are part of the design and are checked by the
compiler: an `async fn` is HIGH tier only
([PARSE-004](../errors/parse.md#parse-004)), and `await` needs a `Task`
([TYPE-106](../errors/type.md#type-106)). The runtime side is not built, so
there is nothing to run yet; an Async chapter will be added when there is.
