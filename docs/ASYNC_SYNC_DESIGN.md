# Async and Synchronization: Design Record

> **Status: decisions made 2026-11-14 in a design session; nothing is built.**
> This document records what the compiler and interpreter do with `async`, `await`, `Task` and
> shared state today (read from source and confirmed by running programs), the decisions taken,
> the options that were weighed, what each choice costs, the outside work that was checked
> against the design, the details that are still open (each with a recommended default), and a
> delivery order. Modules (`summon`) are a separate design session and are not covered here.
> Once a slice is built, its entry moves to a "What ASn settled" note in section 0, as in
> `docs/TRAITS_DESIGN.md`.

---

## 0. Decisions (2026-11-14)

Every row was a question with options and a recommendation. A row marked **against the
recommendation** records a choice that differs from the recommended option; section 3 states what
those choices cost.

| Id | Question | Chosen |
|---|---|---|
| **A1** | What "sync" covers | All three senses: locks, atomics, channels and thread safety; blocking code beside async in MID and LOW; frame lock-step in the game loop. |
| **A2** | Order of design topics | Async and synchronization together, first (**against the recommendation**, which was modules first). Modules follow. |
| **A3** | Concurrency in the interpreter | Real threads in the interpreter too (**against the recommendation**, which was a single-threaded interpreter with cooperative tasks). |
| **A4** | Requirement on concurrency | Genuine parallelism: tasks run at the same time on several OS threads. A single-threaded event loop that interleaves tasks does not meet the requirement. |
| **A5** | Which tiers can start parallel work (spawn, channels, locks, atomics) | All three tiers. In MID and LOW the primary form is scoped. |
| **A6** | Which tiers get `async`/`await` and detached tasks | All three tiers, Rust-like (**against the recommendation**, which was HIGH only). |
| **A7** | How sharing between parallel tasks is kept safe | A compile-time rule: only sendable values cross into a task. Ordinary HIGH data (a `List`, a struct) is copied at the boundary. Sharing goes through `SyncShared`, `Channel`, `Atomic` and the lock types. `Unique` is moved. |
| **A8** | How a task suspends | Compiler-generated state machines (stackless). |
| **A9** | Hot or cold tasks | Cold. Calling an async function does nothing until the task is awaited or spawned. |
| **A10** | Where a detached MID task allocates | It owns a private arena, freed when the task ends. |
| **A11** | Which synchronization primitives ship | The Rust-like full set: `Mutex`, `RwLock`, `Condvar`, `Semaphore`, plus `Barrier`, `Atomic`, `Channel` (**against the recommendation**, which was a small core). |
| **A12** | Who runs the tasks | A built-in thread pool that the engine can replace. |
| **A13** | What `spawn` guarantees | Real parallelism or an error. The wasm playground, which has no threads, gets the error. |
| **A14** | How a scope whose tasks borrow waits | It is a blocking scope: the parent waits and helps run tasks, and a cancelled scope waits for its children. |
| **A15** | Lock guards across `await` | Forbidden at compile time. An async-aware `Mutex` is provided for the cases that need to hold a lock across `await`. |
| **A16** | What `SyncShared<T>` is | Shared ownership plus a built-in lock, in one wrapper, matching the built-in mutability of `Shared<T>`. `Mutex` and `RwLock` stay for standalone use. `SyncShared` becomes creatable in every tier. |
| **A17** | Task failure and cancellation | A panic inside a task is captured and returned as an error when the task is awaited or joined. A detached task that nobody awaits reports the failure loudly. Cancellation is cooperative: checked at each `await` and by an explicit check in long loops. A failing task in a scope cancels its siblings. |
| **A18** | `async` methods in traits | Allowed, and `dyn`-compatible through automatic boxing of the returned task (**against the recommendation**, which was not `dyn`-compatible in v1). |
| **A19** | Syntax for lock guards and scopes | `using` blocks. The `using` cleanup is built first. |
| **A20** | Atomic memory orderings | Sequentially consistent by default, with an optional explicit ordering. |
| **A21** | Channel flavours | Multi-producer multi-consumer, bounded and unbounded. `select` is added later. |

---

### AS2 decisions (2026-11-17)

- An async function is written in the C# form, `async fn f() Task<T>`, with a body that returns
  `T`. A task is awaited at most once. The cleanup trait that `using` calls is `Dispose`. These are
  items 2 (signature part), 4 and 5 of section 5, as recommended.
- With no result, an async function is written bare: `async fn run()` means `Task<void>`
  (**against the recommendation**, which was an explicit `Task<void>` and an error for a bare
  `async fn`). Cost: two spellings of one type are accepted, and only functions with a result show
  the task in their signature.
- `await` outside an async function or block is an error in every tier, as recommended.
- `async { ... }` blocks are built in AS2 (**against the recommendation**, which was an explicit
  error until a later slice). Cost: a flag on the lambda node that every pass has to respect,
  closure capture rules applied to tasks, and the sendable rule at AS4 has to cover them.
- Settled without a question: methods in an `impl` or `extend` block may be async; `async` on a
  trait method or on a method of a trait impl is `PARSE-004` until AS7; a task that is never
  awaited does nothing and is not reported; `async fn main` is allowed and run to completion,
  because with `await` limited to async code and no spawn before AS3, nothing else could start an
  async function from a synchronous `main`.

### AS1 decisions (2026-11-18)

- A `using` binding may hold a value of any type, and the cleanup runs only when the type of the
  value implements `Dispose` (**against the recommendation**, which was an error for a type without
  `Dispose`). Cost: section 3. The cleanup method is `fn dispose(self) void`, and the cleanup also
  runs when a panic unwinds through the block, both as recommended. These are the AS1 row of
  section 7 and item 5 of section 5.
- Settled without a question:
  - `Dispose` is a prelude trait written in Ubel source and `@tier(low)`, like the other prelude
    traits. The body of a `dispose` is therefore LOW-tier code (a `List` cannot be built in it,
    `TIER-009`), and a `using` block in any tier may call it. It cannot be derived (`TYPE-116`).
  - Several bindings in one `using` are cleaned up in reverse order of declaration. A binding whose
    initializer fails, and every binding after it, is never cleaned up. The bindings before it are.
  - The cleanup runs on every way out of the block: the end, `return` (its value is computed
    first), `break`, `continue`, `fail` and a panic. The `defer` expressions of the body run before
    the cleanup.
  - The value cleaned up is the one the binding was initialized with. Assigning to a `let mut`
    binding inside the block does not change what is cleaned up.
  - A `Unique<T>` is looked through, so its `T` is cleaned up. A `Shared<T>` or a `SyncShared<T>`
    is not, because it is one of several owners and no one block is where the value ends.
  - Every cleanup runs even when one of them panics. A panic that is already on its way out is
    kept. Otherwise the first panic raised by a cleanup replaces whatever the block was leaving
    with (a `return`, a `break`, a `fail`).
  - The interpreter finds the cleanup by the trait name `Dispose`, as it finds `Display` and
    `Clone`, so a program that declares its own trait `Dispose` gets that trait used.
  - A `using` block in an `async` function cleans up after the awaits inside it.
- Known gaps, recorded in `docs/PARKED_IDEAS.md`: a binding moved out inside the block is still
  cleaned up at the end, `fail` is accepted in a function that is not fallible (so a `dispose` can
  fail at run time), and the native backend reports `using` as unsupported.

## 1. Where things stand (probed 2026-11-14 on live master; async updated when AS2 landed, 2026-11-17, `using` when AS1 landed, 2026-11-18)

Each line was confirmed by running a program through `ubel check` or `ubel run`, or by reading
the source named.

**Async and tasks**

- `Task<T>` is a type in sema (`SemaType::Task`) and `await` is a unary operator typed by
  `unwrap_task`. `TYPE-106` is "`await` requires `Task<T>`".
- **A task can be built (AS2, 2026-11-17).** `async fn f() Task<int>` has the call type
  `Task<int>` and a body that returns `int`; a bare `async fn f()` is `Task<void>`; `async { }` is a
  `Task` over a block. A declared result that is not a task is `TYPE-144`. A call or a block builds
  a cold task and runs nothing.
- `async fn` and `await` are legal in every tier. `await` outside an async function or block is
  `TYPE-143`, and a plain lambda inside an async function is not an async context. `TIER-002`,
  `TIER-003` and the `PARSE-004` for `async fn` outside HIGH are retired. `async` on a trait method,
  or on a method of a trait impl, is `PARSE-004` until AS7.
- The interpreter has a task value (`Value::Task`). `await` runs the task where it is awaited and
  marks it awaited, and a second `await` of the same task is a run-time error. The move checker
  tracks only LOW-tier `Unique` locals, so there is no compile-time error for a second `await` yet.
  `main` may be async and is run to completion. The native backend reports async functions, `await`
  and async blocks as unsupported.
- The earlier public tour wrote an `async fn` declared `Task<User>!` with a body returning the user;
  that example did not run when it was written. The Async guide now has examples that do.

**Synchronization and sharing**

- No `Mutex`, `Channel`, `spawn`, `Thread` or `Atomic` exists (`NAME-001` for each).
- `SyncShared.new` is LOW-only (`TIER-014` in other tiers). `SyncShared` is backed by the same
  `Rc<RefCell<Value>>` as `Shared`; `docs/MEMORY_MODEL.md` already records that it is "not
  actually `Arc<Mutex<_>>` yet".
- A HIGH `List` is a shared reference: after `let b = a`, `b.push(2)` is visible through `a`.
  That is why a plain value cannot simply be handed to another task.
- `using` calls the `dispose` of each bound value whose type implements `Dispose` (AS1, 2026-11-18,
  see the AS1 decisions above). `with` is an allocator block only (`arena`, `pool`, `gc`, `heap`).

**Interpreter structure** (`crates/core/src/interpreter/`)

- Collections sit behind `Rc<RefCell<..>>`, so `Value` is not `Send`.
- `Interpreter<'ast>` is one mutable struct: the current `env` (replaced by a clone of the
  closure's environment on every call), the `functions` table (it grows at run time, because each
  lambda evaluated allocates an entry), `statics`, and `pool_capacity_stack`.
- The AST is read-only at run time. Interpolation holes are parsed at parse time, and the
  `arena` field of the interpreter is not used while a program runs.
- The only global state is the thread-local `OUTPUT_CAPTURE` (output capture for tests and the
  playground) and an atomic logger flag. A spawned thread would not see the capture.

**Native backend** (`crates/ubel_llvm`, from another chat)

- It emits textual LLVM IR, assembled by clang 15 or newer, with a runtime written in C. The
  collector is single-threaded and stop-the-world with a conservative stack scan, and the
  roadmap lists concurrency last.

**Measured cost of a thread-safe `Value`** (scratch copy outside the repository, discarded)

A mechanical swap of `Rc` for `Arc` and `RefCell` for `RwLock` in the interpreter and builtins
compiled with no errors. Release builds, one thread:

| Workload | `Rc` + `RefCell` (live) | `Arc` + `RwLock` |
|---|---|---|
| Mixed: list push and index, struct field write, calls, closure calls | 1.156 s | 1.152 s |
| Worst case for reference counting: a closure that captured 40 lists, called 150 000 times | 0.457 s | 0.511 s (+12%) |

All 369 fixtures in `tests/fixtures` gave identical output and exit status, with no hangs. This
measures uncontended cost in an interpreter whose own overhead hides it. It says nothing about
contention between threads or about native code, where a lock or atomic on every list access
would matter far more.

---

## 2. The model

### 2.1 Tasks

- `async fn` declares an asynchronous function. Calling it builds a `Task<T>` and does nothing
  else (A9). A task is a compiler-generated state machine that holds only the variables alive
  across a wait (A8). An async function is written `async fn f() Task<T>` and its body returns
  `T`; with no result it is written bare, `async fn run()`, and means `Task<void>`. `async { ... }`
  is a cold task over a block that captures its scope as a closure does.
- `await task` runs the task to completion from the awaiting task and yields its result. Awaiting
  a cold task inline allocates nothing; the machine runs inside the awaiting task's frame.
- `await` is legal only directly inside an async function or an async block. `main` may be async;
  the interpreter runs it to completion, because nothing else can start async code before
  `Task.spawn` exists (AS3).
- `Task.spawn(task)` starts a task on the pool and returns a handle that can be awaited, or
  joined from synchronous code.
- A task frame is allocated once by the runtime and does not move after it first runs. A borrow
  of a task's own local that is live across `await` is therefore valid, and no `Pin`-like
  construct is needed. (Analysis, to be checked against `borrow_check.rs` when built.)
- Frame storage per tier: HIGH keeps the frame on the GC heap, LOW in an owned block, and a
  detached MID task in a private arena owned by the task (A10). A scoped MID task allocates from
  the enclosing arena, which therefore needs thread-safe allocation.

### 2.2 What each tier gets

| | HIGH | MID | LOW |
|---|---|---|---|
| Scoped parallel work (borrows the enclosing scope) | yes | yes | yes |
| Detached tasks | yes | yes | yes |
| `async fn` and `await` | yes | yes | yes |
| Task frame | GC heap | private arena (detached), enclosing arena (scoped) | owned block |
| May a detached task capture | sendable values only | sendable values only | sendable values only |

### 2.3 The sendable rule (A7)

Only sendable values cross into a task. Arena references and borrows are never sendable into a
detached task. A borrow of the enclosing scope is allowed into a scoped task when the borrowed
type is `Sync`.

| Value | Crosses by |
|---|---|
| `int`, `bool`, `double`, immutable `string`, enum without payload | copy |
| Struct, tuple, enum with payload, `List`, `Dictionary`, `Queue`, `Stack`, `Set`, `InlineList` of sendable parts (HIGH) | deep copy at the boundary |
| `Unique<T>` with sendable `T` | move |
| `SyncShared<T>`, `Mutex<T>`, `RwLock<T>`, `Atomic<T>`, `Channel<T>`, `Semaphore`, `Barrier`, `Condvar` | shared handle |
| `Shared<T>` (not thread-safe), `GcRef` into another task's heap, `ArenaRef`, a borrow into a detached task | not sendable: compile error |
| A lock guard | not sendable, and may not be live across `await` (A15) |

Copy at the boundary means a task never sees later mutations by its parent and the parent never
sees the task's. Results come back by the same rule. The copy is cycle-aware.

### 2.4 Scopes (A14, A19)

```ubel
using let s = Task.scope() {
    s.spawn(crunch(left))
    s.spawn(crunch(right))
}
```

This is a proposed form, not implemented. The scope is a blocking scope: when the block ends, the
parent waits for every task spawned in it and runs queued tasks while it waits, so the pool cannot
deadlock on itself. A task in a scope may borrow data of the enclosing function. A cancelled scope
waits for its children before it returns. A scope is never an awaitable value in v1.

### 2.5 Synchronization primitives (A11, A16, A19, A20, A21)

- `Mutex<T>`, `RwLock<T>`: owned lock containers. `lock`, `read` and `write` return a guard bound
  with `using`. A guard may not be live across `await` (A15). `AsyncMutex<T>` (name open) is the
  variant whose `lock` is awaited.
- `SyncShared<T>`: shared ownership with a built-in lock (A16). Cloning the handle shares it.
- `Condvar`, `Semaphore`, `Barrier`: as in Rust's standard library and the usual semaphore.
  `Barrier` together with a scope is the frame lock-step primitive (2.9).
- `Atomic<T>` for integers and `bool`. Operations take an optional ordering (`Relaxed`,
  `Acquire`, `Release`, `AcqRel`, `SeqCst`) and default to `SeqCst` (A20). The interpreter makes
  every operation sequentially consistent, so a weaker ordering is only observable in native code,
  for example on ARM.
- `Channel<T>`: multi-producer multi-consumer, bounded (with backpressure) and unbounded (A21).
  `try_recv` and timeouts are included. `select` is deferred.
- These are compiler built-ins in the same way as `SyncShared.new` today, and move into a
  `std.sync` namespace when the module system exists.

Every blocking operation (`lock`, `read`, `write`, `Condvar.wait`, `Semaphore.acquire`,
`Barrier.wait`, `Channel` send and receive on a full or empty channel, a scope's join) parks
through the executor, never through the operating system directly (see 2.8).

### 2.6 Failure and cancellation (A17)

- A panic inside a task is captured. Awaiting or joining the task returns it as an error. The
  exact typing is an open detail (section 5, item 2).
- A detached task whose handle is dropped without being awaited reports a panic to the error
  output and the engine's hook, and the program continues.
- Cancellation is cooperative. A cancelled task stops at its next `await`. A long loop with no
  `await` checks `Task.cancelled()` itself.
- In a scope, a failing task cancels the scope's other tasks. The scope waits for all of them.

### 2.7 Traits (A18)

`async fn` can be a trait method, with its tier written as for any method and an impl matching it
exactly (S4). The returned task is a state machine of unknown size, so a call through `dyn`
boxes it: the vtable entry returns a pointer to a boxed task. Static calls never box. S6's
object-safety rule is amended so that an async method does not make a trait un-`dyn`-able; the
other S6 conditions are unchanged. The box comes from the GC in HIGH, from the caller's arena in
MID and from the default heap allocator in LOW, freed when the task completes. A note is printed
at such a call site in LOW code so the allocation is not silent.

### 2.8 The executor and the engine (A12, A13)

- The runtime supplies a work-stealing thread pool. The engine can replace it through an executor
  interface. The interface carries at least: start a task, wake a task, run queued work while
  waiting, and park and unpark the calling worker or fiber.
- Because the engine can replace the pool, the blocking primitives in 2.5 call the park hook. A
  fiber-based job system (as in the Naughty Dog engine) parks a waiting fiber and runs other work
  instead of blocking a thread; a direct operating system lock would stall its workers.
- `spawn` either runs the task in parallel or reports an error (A13). Where there are no threads
  (the wasm playground), `spawn` reports the error. Awaiting a cold task inline needs no thread and
  works everywhere.
- The default worker count and thread configuration are open (section 5, item 3).

### 2.9 Frame lock-step

A game frame is a scope. Work that must finish before the frame ends is spawned in the scope; the
scope's end is the lock-step point. `Barrier` covers phases inside a frame. A frame arena (MID)
can be shared by the scope's tasks, because the scope outlives them and the arena allocates
thread-safely. Automatic scheduling from declared read and write sets (`@system` in an ECS) is
not part of this design; it belongs to ECS code generation.

### 2.10 Memory model

Happens-before edges: a lock release before the next acquire of the same lock; an atomic store
before a load that reads it, according to the ordering; a channel send before the matching
receive; a task's spawn before its first step; a task's last step before the `await` or join that
reads its result; a scope's tasks before the scope's end. A program that type-checks and does not
use `unsafe` has no data race on language-level values.

---

## 3. What the choices made against the recommendation cost

**A3, real threads in the interpreter.** The `Rc`/`Arc` swap itself is cheap (section 1). The
work is elsewhere:

- `Interpreter` has to split into shared state (the method, enum and struct tables, the AST, an
  append-only function table behind a lock, `statics` behind a lock) and per-task state (`env`,
  the call stack, `pool_capacity_stack`).
- The thread-local output capture has to be carried into worker threads.
- Each worker needs a stack large enough for a tree-walking recursion. The entry points already
  run on a 256 MB thread; tasks need a smaller, configurable size.
- The wasm playground cannot run threaded programs (A13).
- Which values are thread-safe is an implementation choice that the language does not fix
  (section 5, item 1).

**A6, async in every tier.**

- A state-machine transform for every tier, with frame storage chosen per tier.
- The borrow checker treats `await` as a point where the task can move to another thread, and a
  new pass enforces the sendable rule.
- `TIER-002`, `TIER-003` and the parse error for `async fn` in MID or LOW are retired (done in AS2),
  and the `await` column of the tier table in `tier-model.md`, the Async guide, `MEMORY_MODEL.md`
  section 8 and `PARSER_RULES.md` section 11 say so.
- `async` becomes part of a method signature in traits (2.7).
- The interpreter pays almost nothing extra, because it does not enforce tiers at run time. The
  cost lands in sema and in the native backend.

**A11, the full set of primitives.** More to build, test and document than a small core. Locks
can deadlock, and the language does not prevent it. The blocking primitives need the executor's
park hook, so they are built after it. A deadlock-free layer in the style of Verona's
behaviour-oriented concurrency stays parked (section 6).

**A18, automatic boxing.** One allocation for each async call made through `dyn`. In LOW that
allocation is hidden, hence the note at the call site. Static dispatch is unaffected.

**AS1, `using` accepts any type.** The recommendation was an error for a binding whose type has no
`Dispose` impl. As built, such a block compiles and does nothing at its end. A forgotten
`impl Dispose`, a misspelled method, or an inherent method named `dispose` that is not the
`Dispose` impl therefore gives no error and no cleanup. That is the accept-and-ignore trap that AS1
set out to close, and it stays open for user types. The guard types of AS5 implement `Dispose`
themselves, so a lock guard is not exposed to it. If the trap shows up in practice, a warning for a
`using` binding whose type has a method named `dispose` but no `Dispose` impl is cheap to add.

---

## 4. What was checked against outside work

The design was compared with published designs before it was written down. The result: no
decision was reversed, five points were tightened, and one earlier statement was corrected.

- **A claim corrected.** During the session it was said that sending a value so that the sender
  loses it ("move") could not be made sound for HIGH without a large change. Swift's region-based
  isolation (SE-0414, SE-0430) does exactly that, with a flow-sensitive analysis. It is a large
  change, so copy at the boundary stays the v1 rule, and move is a realistic later upgrade
  (section 6).
- **A14 (blocking scopes).** Rust's async cannot offer parallelism together with borrowing from
  the parent, because a future can be forgotten or dropped, while a thread scope blocks on exit.
  Ubel has no safe way to leak a task, but cancelling a suspended parent is the same hole, so the
  scope blocks and a cancelled scope waits for its children.
- **A11 and A12 (park through the executor).** Zig 0.16 moved its mutex and condition types onto
  its `Io` interface, an interface passed like an allocator, so one program can run on threads or
  fibers. The Naughty Dog engine parks a fiber instead of blocking a thread.
- **A15 (guards across `await`).** Rust's rule, applied here, because a task can resume on another
  thread.
- **A13 (`spawn` guarantee).** Zig separates a call that may run serially from one that must run
  concurrently. The requirement in A4 is the second.
- **A7 (risk).** Kotlin/Native required freezing an object graph before sharing it between
  threads, and replaced that model in 1.7.20 with free sharing. The Ubel rule is compile-time and
  copies automatically instead of failing, which is closer to Swift's route. Error messages must
  be clear and the later upgrades matter for HIGH's easy-tier promise.
- **A8 (the field is split).** Go, Java's virtual threads and Zig 0.16 use separate stacks (Zig
  removed its stackless `async` and later returned with a colorless design). Rust, C#, Kotlin and
  Swift use state machines. The cost of A8 is that `async` shows in signatures and spreads
  through callers.
- **State-machine size.** Rust's project goals for 2026 list async future bloat: nested futures
  grow quickly, and locals that live across several awaits keep their slots for the whole life of
  the future. The plan therefore includes a size report and a rule that a recursive async function
  boxes its recursive call.
- **LLVM coroutines.** The frontend supplies the frame's allocation, so a frame can live in the
  GC heap, a task arena or an owned block per tier. The LLVM documentation states that
  compatibility across releases is not guaranteed, so the clang version is pinned.

Sources: `without.boats/blog/the-scoped-task-trilemma`;
`internals.rust-lang.org/t/blog-post-a-formulation-for-scoped-tasks/18448`;
`github.com/apple/swift-evolution/blob/main/proposals/0414-region-based-isolation.md`;
`github.com/swiftlang/swift-evolution/blob/main/proposals/0430-transferring-parameters-and-results.md`;
`arxiv.org/pdf/2309.02983` (reference capabilities for flexible memory management, the Verona
region model); `microsoft.github.io/bocpy`;
`blog.jetbrains.com/kotlin/2020/07/kotlin-native-memory-management-roadmap/`;
`blog.bokvi.com/blog/zig-in-2026/`;
`gdcvault.com/play/1022187/Parallelizing-the-Naughty-Dog-Engin`;
`goals.rust-lang.org/2026/async-future-memory-optimisation.html`;
`releases.llvm.org/11.0.1/docs/Coroutines.html`.

---

## 5. Open details, with a recommended default

None of these has been confirmed. Each is settled before the slice that needs it, as options with
a recommendation.

1. **Interpreter strategy for threads.** (a) Make every `Value` thread-safe with `Arc` and
   `RwLock` (measured in section 1). (b) Keep `Rc` inside each task and give shared payloads
   (`SyncShared`, `Mutex`, `Channel`, `Atomic`) a thread-safe backing, with a deep copy at the
   boundary. Recommended: (b). It models the native design (per-task heaps, copies at the
   boundary) and makes a program that relied on accidental sharing fail in the interpreter
   instead of racing in native code. (a) stays the fallback because it is mechanical and cheap.
2. **Signature and typing of async functions and failed tasks.** *Signature decided 2026-11-17:*
   `async fn f() Task<T>` with a body that returns `T`, and a bare `async fn run()` for no result,
   which means `Task<void>` (against the recommendation of an explicit `Task<void>`). *Failure
   typing is still open and belongs to AS6:* the placement of `!` for expected errors, `Task<User!>`
   against `Task<User>!` (the earlier tour wrote the second form, which then means the call itself
   can fail). Recommended: `Task<User!>`, with `await` re-raising a captured panic and `Task.join`
   returning the error. AS2 already accepts `Task<User!>` as a declared result and rejects
   `Task<User>!` (`TYPE-144`).
3. **Executor interface shape.** The methods in 2.8, the default worker count (recommended:
   logical cores minus one, at least one), thread names and affinity for engines that pin
   threads.
4. **Awaiting a task more than once.** *Decided 2026-11-17:* a task is awaited at most once. A second
   await is a run-time error in every tier. The compile-time error where the move checker can see
   it is not built: the move checker tracks only LOW-tier `Unique` locals, so it needs a pass over
   task values first.
5. **Cleanup trait name.** *Decided 2026-11-17:* `Dispose`, matching `using`. Built in AS1 as a `@tier(low)` prelude trait with
   `fn dispose(self) void`.
6. **Marker traits.** Whether `Send` and `Sync` are written in bounds of generic library code.
   Recommended: compiler-known marker traits, derived structurally, writable as bounds.
7. **Names.** `Task.spawn`, `Task.scope`, `Task.cancelled`, the async-aware lock type, the
   ordering names. `Task.spawn` needs the parser to accept `Task` followed by `.` in expression
   position.
8. **Diagnostics.** Codes for: a value that is not sendable, a lock guard live across `await`,
   an awaited-twice task, `spawn` where threads do not exist, a captured panic that was never
   awaited. Allocated in `DIAGNOSTICS_RULES.md` at the slice that introduces each.
9. **Native work.** A thread-aware collector (thread registry, stop-the-world across threads,
   per-thread allocation) is a prerequisite for parallel HIGH code in native builds; parallel MID
   and LOW code does not depend on it. Coroutine lowering and the pinned LLVM version belong to
   the backend's own roadmap. Neither is touched by this document.

---

## 6. Parked

- **Awaitable borrowing scopes.** Would need tasks that cannot be dropped while their children
  run.
- **Move into a task without a copy,** by region analysis as in Swift, and **sending a whole arena
  to a task in one step,** as Verona does with regions.
- **A deadlock-free layer** in the style of behaviour-oriented concurrency (Verona), over the lock
  types.
- **`select`** over several channels.
- **Automatic parallel scheduling** from declared read and write sets.

---

## 7. Delivery order

Design first, then slices, each with fixtures, mutation checks and a fresh-clone verification, as
for traits.

| Slice | Content | Depends on |
|---|---|---|
| **AS0** | This document and the public status page. | none |
| **AS1** | **Done 2026-11-18.** `using` cleanup: the `Dispose` trait, run on every exit path (`return`, `break`, `continue`, `?`, failure, panic), reverse order for several bindings. The cleanup runs for a bound value whose type implements `Dispose`, and a binding of any other type is accepted and has none (**against the recommendation**, which was a type check). Cancellation arrives with AS6. Useful on its own. | AS0 |
| **AS2** | **Done 2026-11-17.** Async core in sema and the interpreter: `async fn` typing, cold `Task<T>`, `await` that runs a task inline, tier rules opened. No threads yet. | AS0 |
| **AS3** | Threads in the interpreter: the state split, the pool, `Task.spawn`, join, the executor interface, the park hook. | AS2 |
| **AS4** | The sendable rule: marker traits, the sema pass, copy at the boundary, a real `SyncShared`, `TIER-014` lifted for it. | AS3 |
| **AS5** | The lock types, `Condvar`, `Semaphore`, `Barrier`, `Atomic`, `Channel`, parking through the executor, the guard rules and the async-aware lock. | AS1, AS3, AS4 |
| **AS6** | Scopes, cancellation, failure capture. | AS1, AS3, AS5 |
| **AS7** | `async` methods in traits and boxed `dyn` calls. | AS2 |
| **AS8** | Native backend items: thread-aware collector, coroutine lowering. Planned in the backend's own documents. | AS2 and the backend |
