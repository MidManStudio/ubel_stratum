# Language Tour

A tour of the surface syntax. Every example below is real syntax the
project documents; none is invented for this page.

## Tier annotations

```ubel
// Default: HIGH, no annotation needed for most code
fn handle_request(req: Request) Response {
    let user = fetch_user(req.user_id)
    return Response.ok(user.to_json())
}

// Opt into MID for a hot path
@tier(mid)
fn parse_payload(body: string) ParsedData {
    with arena(1MB) { }
}

// Opt into LOW for systems code
@tier(low)
fn write_packet(buf: &mut [u8]) usize {
    // raw ownership; borrow checker enforcement is still in progress
}
```

## Collections

Collection names follow C# convention rather than Rust's.

```ubel
let mut numbers = List.new()
numbers.push(1)
numbers.push(2)

let mut scores = Dictionary<string, int>.new()
scores.set("Alice", 100)   // set/get, not insert, for symmetry

let items = [1, 2, 3, 4, 5]
let names = ["Alice", "Bob"]   // inferred as List<string>
```

## Unified member access

There is no `::`. Type-level calls and instance calls both use `.`.

```ubel
summon std.collections.List
let list = List.new()   // type-level call
list.push(42)            // instance call
```

## Error handling

A trailing `!` on a return type means the function may fail; `?`
propagates a failure out of the current function.

```ubel
fn parse_int(s: string) int! {
    // returns a Result-like value
}

fn process(s: string) int! {
    let n = parse_int(s)?
    return n * 2
}
```

## Async

Async is HIGH tier only. MID and LOW are synchronous by design: arenas
have lexical lifetimes, which do not compose with the way an `async`
function suspends and resumes across `await` points.

```ubel
@tier(high)
async fn fetch_user(id: int) Task<User>! {
    let resp = await http_get($"/users/{id}")?
    return await parse_user(resp.body)?
}
```

## Structs and methods

```ubel
struct Rectangle {
    width: int,
    height: int

    pub fn new(w: int, h: int) Rectangle {
        return Rectangle { width = w, height = h }
    }

    pub fn area(self) int {
        return self.width * self.height
    }
}
```

## Pattern matching

```ubel
match response {
    Ok(data) where data.status == 200 => process_success(data),
    Ok(data) => log_warning($"Status: {data.status}"),
    Err(NetworkError(extract { code, message })) => {
        log_error($"Network error {code}: {message}")
    }
    Err(e) => log_error($"Unknown: {e}"),
}
```

## Pipe operator

```ubel
let result = data
    |> parse?
    |> validate?
    |> transform
    |> save
```

## Extension functions

```ubel
extend int {
    fn is_even(self) bool { return self % 2 == 0 }
}

if 42.is_even() { println("Even!") }
```

## References and lifetimes (LOW tier)

`&`/`ref` and `&mut`/`ref mut` are dual spellings of the same borrow
operator, `*`/`deref` likewise for dereference, the same relationship
`and`/`&&` and `or`/`||` already have. Either spelling compiles to the
identical AST node; named lifetimes are only needed for cross-function
borrows complex enough that inference cannot resolve them alone.

```ubel
// inferred, no annotation needed
fn first(list: &List<int>) &int {
    return &list[0]
}

// same thing, keyword spelling
fn first(list: ref List<int>) ref int {
    return ref list[0]
}

// explicit lifetime for a genuinely ambiguous case
fn longest[lifetime L](x: &L str, y: &L str) &L str {
    if x.len() > y.len() { return x } else { return y }
}
```

The syntax and structural typing for references are complete. The borrow
checker that actually verifies a program's borrows are sound (loan
tracking, liveness, move checking) is still in progress; see
[The Tier Model](./tier-model.md#what-is-enforced-today) for exactly
where that line sits today.

## RAII with `using`

```ubel
using let file = File.open("data.txt") {
    let content = file.read()
    process(content)
}   // file.close() runs automatically
```

## Query pipelines

`Linqerizer<T>` is a lazy, chainable query pipeline over any collection,
HIGH tier only. `.query()` snapshots the source once; nothing runs until
a terminal call (`.to_list()`, `.first()`, `.count()`) walks the
pipeline, and each chained call returns a new pipeline rather than
mutating in place.

```ubel
@tier(high)
fn active_adult_names(users: List<User>) List<string> {
    return users.query()
        .where(fn(u) u.age >= 18 and u.status == "active")
        .order_by(fn(u) u.name)
        .select(fn(u) u.name)
        .to_list()
}
```

`.group_by(...)` produces a real `Dictionary<Key, List<Value>>`. An
earlier query-comprehension grammar (`from x in ... where ... select`,
closer to C#'s LINQ syntax) was removed outright rather than kept
alongside this: it was eager, List-only, `group_by` was a stub, and it
had no fixture coverage. `Linqerizer<T>` needs no dedicated grammar at
all; it parses as ordinary method calls.
