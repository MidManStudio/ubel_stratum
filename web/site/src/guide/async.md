# Async

> **Status: the async core runs.** A call to an `async fn` builds a task that has not started,
> `await` runs it where it is awaited, and `async { ... }` makes a task from a block. All of it
> works in every tier. There are no threads yet: a task runs inline where it is awaited, so
> nothing here runs in parallel. Parallelism is the next stage of a larger design, summarised below.

## Async functions and `await`

An async function declares the task it returns, `Task<T>`, and its body returns a `T`. A call does
not run the body. It builds a task, and `await` runs the task and yields the `T`. A function with
no result is written without a return type and means `Task<void>`:

<!-- ubel-test: run -->
```ubel
async fn double(x: int) Task<int> {
    println("running")
    return x * 2
}

async fn greet() {
    println("hello")
}

async fn main() {
    let t = double(21)
    println("built, not run")
    let n = await t
    println(n)
    await greet()
}
```

```text
built, not run
running
42
hello
```

`main` may be async. It is run to completion, since nothing else could start async code yet.

## Blocks

`async { ... }` is a task over a block. It captures what it uses from the scope around it as a
closure does, `return` ends the block, and it runs nothing until it is awaited:

<!-- ubel-test: run -->
```ubel
async fn main() {
    let offset = 2
    let t = async {
        println("in the block")
        return 40 + offset
    }
    println("block built")
    println(await t)
}
```

```text
block built
in the block
42
```

## Rules

`await` is legal only directly inside an async function or an async block, in any tier. Anywhere
else, including inside a plain lambda in an async function, it is
[TYPE-143](../errors/type.md#type-143):

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

The declared result must be a task. `async fn count() int` is
[TYPE-144](../errors/type.md#type-144):

<!-- ubel-test: error TYPE-144 -->
```ubel
async fn count() int {
    return 1
}

fn main() {
}
```

`await` needs a `Task<T>`. Awaiting an ordinary value is
[TYPE-106](../errors/type.md#type-106):

<!-- ubel-test: error TYPE-106 -->
```ubel
async fn run() {
    let n = 5
    let m = await n
}

fn main() {
}
```

Other rules:

- A task is awaited at most once. A second `await` of the same task stops the program with an
  error. There is no compile-time check for it yet.
- Async methods work in an `impl` block. In a trait, or in an impl of a trait, they are
  [PARSE-004](../errors/parse.md#parse-004) until a later stage.
- `async`, `await` and `Task` are reserved words
  ([Keywords](../reference/keywords-and-operators.md)).
- The native backend does not build async code yet.

## The decided design (the async core is built, threads are not)

The record is `docs/ASYNC_SYNC_DESIGN.md`. In brief, the decisions are:

- **Real parallelism.** Tasks run at the same time on several OS threads, in
  the interpreter as well as in native code, on a built-in thread pool that an
  engine can replace. A single-threaded event loop does not meet the
  requirement.
- **All three tiers.** `async` and `await` work in HIGH, MID and LOW, not only
  in HIGH. In MID and LOW the primary way to start parallel work is a scoped
  one, so borrowed data cannot outlive the work.
- **Cold tasks, compiler-generated state machines.** Calling an async function
  does nothing until the task is awaited or spawned, and a task suspends as a
  state machine rather than on its own stack.
- **A compile-time rule for sharing.** Only values that are safe to send cross
  into a task. Ordinary HIGH data such as a `List` is copied at the boundary;
  sharing goes through `SyncShared`, channels, atomics and the lock types.
- **`SyncShared<T>` changes.** It becomes shared ownership with a built-in lock
  in one wrapper, and can be created in every tier (today it is LOW only; see
  [Ownership](../memory/ownership.md)).
- **Synchronisation primitives.** A Rust-like set: `Mutex`, `RwLock`,
  `Condvar`, `Semaphore`, `Barrier`, `Atomic` and `Channel`, with lock guards
  written as `using` blocks and forbidden across an `await`.
- **The playground.** The browser build has no threads, so starting parallel
  work there is an error.

The async core is built. Delivery continues with making `using` run its cleanup, then threads in the
interpreter and the sharing rule. [Project Status](../project-status.md) tracks it, and this guide
grows as those land.
