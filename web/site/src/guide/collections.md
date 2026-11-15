# Collections

The built-in collections have C#-style names. Each is created with `.new()`
and grows as needed (except `InlineList`, which is fixed-size).

| Collection | What it is |
|------------|------------|
| `List<T>` | an ordered, growable sequence; also written as an array literal `[1, 2, 3]` |
| `Dictionary<K, V>` | key/value pairs |
| `Queue<T>` | first in, first out |
| `Stack<T>` | last in, first out |
| `InlineList<T>` | a fixed-capacity list that lives on the stack |
| `Pool<T>` / `Handle<T>` | slot storage with generational handles; MID tier only (see [The Tier Model](../tier-model.md)) |

The element type is inferred from what you put in. To state it, annotate the
variable (the type arguments go on the type, not on `.new()`):

<!-- ubel-test: run -->
```ubel
fn main() {
    let scores: Dictionary<string, int> = Dictionary.new()
    scores.set("ada", 100)
    println(scores.get("ada"))
}
```

## Lists

<!-- ubel-test: run -->
```ubel
fn main() {
    let nums = List.new()
    nums.push(3)
    nums.push(1)
    nums.push(2)
    println($"{nums.len()} {nums.first()} {nums.last()} {nums.contains(2)}")

    let literal = [10, 20, 30]          // an array literal
    println(literal[1])                 // indexing
    println(literal.len())

    for x in nums {                     // iterate
        println(x)
    }
}
```

```text
3 3 2 true
20
3
3
1
2
```

The list methods are `len`, `push`, `pop`, `contains`, `first`, `last`,
`is_empty`, `reverse`, `get`, `set`, `find`, `find_all` and `query`.

## Dictionaries

A dictionary is written to with `set` (not `insert`), which keeps `get` and
`set` symmetrical.

<!-- ubel-test: run -->
```ubel
fn main() {
    let ages = Dictionary.new()
    ages.set("ada", 36)
    ages.set("alan", 41)
    println(ages.get("ada"))
    println(ages.contains_key("grace"))
    println(ages.len())
    for pair in ages {                  // each item is a (key, value) pair
        println(pair)
    }
}
```

The dictionary methods are `get`, `set`, `contains_key`, `keys`, `values`,
`len` and `is_empty`.

## Queues and stacks

<!-- ubel-test: run -->
```ubel
fn main() {
    let q = Queue.new()
    q.enqueue(1)
    q.enqueue(2)
    println(q.dequeue())      // 1: first in, first out
    println(q.peek())         // 2: look without removing

    let s = Stack.new()
    s.push(1)
    s.push(2)
    println(s.pop())          // 2: last in, first out
}
```

`Queue` has `enqueue`, `dequeue`, `peek`, `len`, `is_empty`, `contains` and
`clear`. `Stack` has `push`, `pop`, `peek`, `len`, `is_empty`, `contains` and
`clear`.

## Fixed-size lists

`InlineList<T>` stores its items inline with a capacity fixed at compile time,
so it needs no allocation. The capacity must be an integer literal:

<!-- ubel-test: error TYPE-113 -->
```ubel
fn main() {
    let n = 8
    let buf = InlineList.new(n)
}
```

See [TYPE-113](../errors/type.md#type-113). Write `InlineList.new(8)`. It has
`len`, `push`, `pop`, `contains`, `first`, `last`, `is_empty`, `reverse` and
`capacity`.

## Queries

A list can be turned into a lazy query with `.query()`. Nothing runs until a
terminal call such as `.to_list()` or `.count()` walks the pipeline, and each
step returns a new pipeline rather than changing the old one.

<!-- ubel-test: run -->
```ubel
fn main() {
    let list = List.new()
    for n in [1, 2, 3, 4, 5, 6] {
        list.push(n)
    }
    let evens = list.query()
        .where(fn(x) x % 2 == 0)
        .select(fn(x) x * 10)
        .to_list()
    println(evens.len())            // 3
    println(evens.get(0))           // 20
    println(list.query().where(fn(x) x > 4).count())   // 2
}
```

The query methods are `where`, `select`, `order_by`, `order_by_desc`,
`to_list`, `first`, `count` and `group_by`. Queries are HIGH-tier only;
calling one from MID or LOW is [TIER-008](../errors/tier.md#tier-008).

## Collections and tiers

`List`, `Dictionary`, `Queue` and `Stack` allocate, and LOW tier has no
allocation of its own, so constructing one in a `@tier(low)` function is
[TIER-009](../errors/tier.md#tier-009). A LOW function can receive one as a
parameter.

`Set` is a reserved type name; `Set.new()` is not available yet.
