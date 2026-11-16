# Async

> **Status: checked in a narrow form, not runnable.** `async` and `await` are
> parsed and type-checked, but there is no async runtime, and the project's
> test suite has no program that runs async code. A much larger design was
> decided on 2026-11-14 and is summarised below; none of it is built. This page
> separates what the compiler does today from what has only been decided.

## What the compiler checks today

An `async fn` outside HIGH tier is rejected by the parser
([PARSE-004](../errors/parse.md#parse-004)):

<!-- ubel-test: error PARSE-004 -->
```ubel
@tier(mid)
async fn run() {
}

fn main() {
}
```

`await` needs a `Task<T>` to wait for. Awaiting an ordinary value is
[TYPE-106](../errors/type.md#type-106):

<!-- ubel-test: error TYPE-106 -->
```ubel
@tier(high)
async fn run() {
    let n = 5
    let m = await n
}

fn main() {
}
```

The registry also defines tier diagnostics for `await` and `async fn` in the
wrong tier ([TIER-002](../errors/tier.md#tier-002) and
[TIER-003](../errors/tier.md#tier-003)); in the current compiler the parser's
check above reports the `async fn` case first. `async`, `await` and `Task` are
reserved words ([Keywords](../reference/keywords-and-operators.md)).

**The HIGH-only rule is expected to change.** See the next section.

## The decided design (not built)

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

The order of delivery starts with making `using` run its cleanup, then the
async core. [Project Status](../project-status.md) tracks it. An Async chapter
with working examples will replace this page when there is a runtime to
exercise.
