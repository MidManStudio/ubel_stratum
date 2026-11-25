# Async

> **Status: the async core runs.** A call to an `async fn` builds a task that has not started,
> `await` runs it where it is awaited, and `async { ... }` makes a task from a block. All of it
> works in every tier. A task that is only awaited runs inline where it is awaited, and
> `Task.spawn` runs one on a pool of worker threads (see below). Sharing data between tasks is the
> next stage of a larger design, summarised below.

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

## Running tasks on other threads

`Task.spawn(task)` hands a task to a pool of worker threads and gives the same task back, now
running. Wait for it with `await` in async code, or with `.join()` anywhere. Both give the result
of the task, and a task is awaited or joined once.

<!-- ubel-test: run -->
```ubel
async fn double(n: int) Task<int> {
    return n * 2
}

fn main() void {
    let a = Task.spawn(double(1))
    let b = Task.spawn(double(2))
    println(a.join() + b.join())
}
```

```text
6
```

A program that never writes `Task.spawn` starts no threads, and `.join()` on a task that was never
spawned runs it right there. An `async { }` block can be spawned too, and it takes the names it
uses from the code around it.

What goes into a task is **copied**. A change made inside a task is not seen by the code that
spawned it, what was shared inside the copy stays shared, and the result comes back as a copy. A
value that cannot be copied is refused, when the program runs, with an error that names it: a
`Shared`, a `SyncShared`, a task, a `Pool` or a closure. A function declared in the program can be
sent. A task sees the constants and statics as they were when the pool started, and it cannot
write a static.

A panic in a task reaches the one that awaits it, with its message, and a `fail` arrives as a
failure. The pool has one worker fewer than the machine has logical cores, and at least one. `ubel run
--workers N` or the `UBEL_WORKERS` environment variable sets another number, and a program that
embeds the interpreter calls `set_worker_count`. There are no threads in the browser playground, so
`Task.spawn` reports an error there.

### Who runs the workers

The pool does not start threads itself. It hands the worker loops, and the program's own `main`, to
an `Executor`, which decides where they run. The default executor gives every worker an operating
system thread. An engine that owns its threads, to pin them to cores or to run the workers as fibers
of its job system, brings an executor of its own and gives it to the interpreter with
`set_executor`. Every wait of the pool goes through the executor (`park_until`), and so does every
change a waiter could be waiting for (`wake`), so a fiber host can park a fiber where a thread host
would block a thread. An executor may also say how many workers it wants (`preferred_workers`); an
explicit `--workers` or `UBEL_WORKERS` count wins over that. A thread or fiber that runs a worker
needs at least 128 MiB of stack, which the runtime cannot check.

### Send and Sync

A value can go into a task only when its type is `Send`, and a task can use a shared view of a value
only when its type is `Sync`. Both are traits with no methods. A struct or an enum has one only when
it says so with an `impl`, and nothing is worked out from its fields, so forgetting the impl is an
error wherever `Send` is asked for and not a quiet default. The impl is checked against the fields:

<!-- ubel-test: run -->
```ubel
struct Job { id: int, name: string }

impl Send for Job {
}

fn main() {
    let job = Job { id = 1, name = "index" }
    println(job.name)
}
```

`impl Send for Job` is accepted because every field of `Job` is `Send`. A field that is not, such as
a `Shared<int>`, makes the impl an error (`TYPE-145`) that names the field:

<!-- ubel-test: error TYPE-145 -->
```ubel
struct Holder { cell: Shared<int> }

impl Send for Holder {
}

fn main() {
}
```

A type that is safe to send in spite of what the compiler can see takes `unsafe impl Send for Holder
{}`. That impl is trusted and not checked, so it is the one place a data race can be introduced, and
`unsafe` makes it easy to find. `unsafe impl` is for `Send` and `Sync` only.

A built-in type needs no impl and cannot be given one. Numbers, `bool`, `char` and `string` have both
markers. A tuple, an optional, a `List`, a `Dictionary` and the other collections have a marker when
every part has it. `Unique<T>` has what `T` has, and `SyncShared<T>` has both when `T` is `Send`.
`Shared<T>`, references, borrowed views, function values and tasks have neither. A generic function
asks for a marker with a bound, `fn run<T: Send>(x: T)`, and a generic type makes its impl
conditional, `impl<T: Send> Send for Box<T> {}`.

The compiler does not yet refuse a value that is not `Send` at `Task.spawn`: that check is the next
step. Until then a closure, a `Shared`, a task or a pool is still refused when the task starts.

## The decided design (the async core and real threads are built, sharing is not)

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

The async core, the `using` cleanup and tasks on real threads, on a pool an engine can replace, are built (see
[Generics and traits](./generics-traits.md#cleanup-with-using-and-dispose) for `using`). Delivery
continues with the sharing rule and the lock types. [Project Status](../project-status.md) tracks it, and this guide
grows as those land.
