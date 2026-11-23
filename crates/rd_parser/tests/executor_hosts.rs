//! A host that lends threads to the worker pool (docs/ASYNC_SYNC_DESIGN.md, AS3b).
//!
//! The default executor is exercised by every spawn test. These tests run whole programs on
//! executors written here, to pin the contract between the runtime and a host: how many workers
//! the host is asked to start, who decides that number, that every wait and wake of the pool goes
//! through the host, what happens when the host cannot start a worker, and that a host that
//! polls instead of blocking is as good as one that blocks.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};

use ubel_stratum::ast::arena::AstArena;
use ubel_stratum::interpreter::{Executor, Interpreter, Lend};
use ubel_stratum::stack::WORKER_STACK_BYTES;
use ubel_stratum_rd::{check_source, Stage};

// ── Programs ─────────────────────────────────────────────────────

const SPAWN_ONE: &str = "async fn work(n: int) Task<int> {\n    return n * 2\n}\n\n\
                         fn main() void {\n    let t = Task.spawn(work(21))\n    println(t.join())\n}\n";

const NO_SPAWN: &str = "fn main() void {\n    println(\"no tasks\")\n}\n";

/// Eight tasks, spawned and then joined in order. The sum of the squares of 1 to 8 is 204.
const MANY: &str = "async fn square(n: int) Task<int> {\n    return n * n\n}\n\n\
                    fn main() void {\n    let tasks: List<Task<int>> = List.new()\n    let mut i = 1\n\
                    \x20   while i <= 8 {\n        tasks.push(Task.spawn(square(i)))\n        i += 1\n    }\n\
                    \x20   let mut sum = 0\n    for t in tasks {\n        sum += t.join()\n    }\n    println(sum)\n}\n";
const MANY_TASKS: usize = 8;

/// Prints before it spawns, so that any output shows that `main` ran.
const PRINT_THEN_SPAWN: &str = "async fn work(n: int) Task<int> {\n    return n\n}\n\n\
                                fn main() void {\n    println(\"main ran\")\n    let t = Task.spawn(work(1))\n    println(t.join())\n}\n";

const PRINTING: &str = "async fn shout() {\n    println(\"in task\")\n}\n\n\
                        fn main() void {\n    let t = Task.spawn(shout())\n    t.join()\n    println(\"main\")\n}\n";

// ── Hosts ────────────────────────────────────────────────────────

/// Start `lend.workers()` workers on threads of `WORKER_STACK_BYTES`, run `main`, stop. With
/// `fail_after`, fail to start worker number `n` (counting from zero): stop and report.
fn run_on_threads(
    mut lend: Lend<'_>,
    fail_after: Option<usize>,
    started: &AtomicUsize,
    returned: &AtomicUsize,
) -> Result<(), String> {
    let worker = lend.worker();
    std::thread::scope(|scope| {
        for i in 0..lend.workers() {
            if fail_after == Some(i) {
                lend.stop();
                return Err("the host has no thread to give".to_string());
            }
            std::thread::Builder::new()
                .name(format!("host-{i}"))
                .stack_size(WORKER_STACK_BYTES)
                .spawn_scoped(scope, move || {
                    started.fetch_add(1, Ordering::SeqCst);
                    worker(i);
                    // A host that keeps this thread for other work must find it free of the
                    // buffer the run captured its output in. If the worker left it attached,
                    // taking the capture here would take the output of the whole run.
                    let leftover = ubel_stratum::builtins::global::io::take_captured_output();
                    assert_eq!(leftover, "", "a worker left the output capture attached to its thread");
                    returned.fetch_add(1, Ordering::SeqCst);
                })
                .unwrap();
        }
        lend.run_main();
        lend.stop();
        Ok(())
    })
}

/// A host that blocks on a condition variable, and counts everything the runtime asks of it.
#[derive(Default)]
struct Host {
    preferred:  Option<usize>,
    fail_after: Option<usize>,
    runs:       AtomicUsize,
    requested:  AtomicUsize,
    started:    AtomicUsize,
    returned:   AtomicUsize,
    parks:      AtomicUsize,
    wakes:      AtomicUsize,
    lock:       Mutex<()>,
    cv:         Condvar,
}

impl Host {
    fn new() -> Arc<Host> {
        Arc::new(Host::default())
    }
    fn preferring(n: usize) -> Arc<Host> {
        Arc::new(Host { preferred: Some(n), ..Host::default() })
    }
    fn failing_after(n: usize) -> Arc<Host> {
        Arc::new(Host { fail_after: Some(n), ..Host::default() })
    }
    fn get(counter: &AtomicUsize) -> usize {
        counter.load(Ordering::SeqCst)
    }
}

impl Executor for Host {
    fn run(&self, lend: Lend<'_>) -> Result<(), String> {
        self.runs.fetch_add(1, Ordering::SeqCst);
        self.requested.store(lend.workers(), Ordering::SeqCst);
        run_on_threads(lend, self.fail_after, &self.started, &self.returned)
    }

    fn park_until(&self, ready: &dyn Fn() -> bool) {
        self.parks.fetch_add(1, Ordering::SeqCst);
        let mut guard = self.lock.lock().unwrap();
        while !ready() {
            guard = self.cv.wait(guard).unwrap();
        }
    }

    fn wake(&self) {
        self.wakes.fetch_add(1, Ordering::SeqCst);
        drop(self.lock.lock().unwrap());
        self.cv.notify_all();
    }

    fn preferred_workers(&self) -> Option<usize> {
        self.preferred
    }
}

/// A host that returns without ever running the program, which the contract forbids.
struct SkipMain;

impl Executor for SkipMain {
    fn run(&self, _lend: Lend<'_>) -> Result<(), String> {
        Ok(())
    }
    fn park_until(&self, _ready: &dyn Fn() -> bool) {}
    fn wake(&self) {}
}

/// A host for a scheduler that never blocks a thread: a parked caller polls, and a wake is a no-op.
struct SpinHost;

impl Executor for SpinHost {
    fn run(&self, lend: Lend<'_>) -> Result<(), String> {
        run_on_threads(lend, None, &AtomicUsize::new(0), &AtomicUsize::new(0))
    }

    fn park_until(&self, ready: &dyn Fn() -> bool) {
        while !ready() {
            std::thread::yield_now();
        }
    }

    fn wake(&self) {}
}

// ── Running a program on a host ──────────────────────────────────

fn is_clean(source: &str) -> bool {
    matches!(check_source(source).stage, Stage::Clean)
}

/// Run a program that passes `check_source`. Returns what it printed, also when it ended in an
/// error, and the error text if it did.
fn run_on(source: &str, host: Arc<dyn Executor>, workers: Option<usize>) -> (String, Result<(), String>) {
    assert!(is_clean(source), "program should check clean");
    let tokens = ubel_stratum::lexer::tokenize(source).expect("source should lex");
    let arena = AstArena::new();
    let program = ubel_stratum_rd::parse(&arena, &tokens, source.to_string())
        .unwrap_or_else(|_| panic!("source should parse"));
    let ctx = ubel_stratum::sema::analyse(&program, &arena, source.to_string())
        .unwrap_or_else(|_| panic!("source should pass sema"));

    ubel_stratum::builtins::global::io::start_output_capture();
    let mut interp = Interpreter::new(&arena);
    interp.set_int_literal_types(ctx.int_literal_types);
    interp.set_trait_call_sites(ctx.trait_call_sites);
    interp.set_executor(host);
    if let Some(n) = workers {
        interp.set_worker_count(n);
    }
    let result = interp.run_program(&program).map(|_| ());
    let output = ubel_stratum::builtins::global::io::take_captured_output();
    (output, result)
}

fn the_default_count() -> usize {
    std::thread::available_parallelism().map(|n| n.get()).unwrap_or(2).saturating_sub(1).max(1)
}

// ── The host runs the program ────────────────────────────────────

#[test]
fn an_executor_set_on_the_interpreter_runs_the_program() {
    let host = Host::new();
    let (out, result) = run_on(SPAWN_ONE, host.clone(), Some(2));
    result.unwrap();
    assert_eq!(out, "42\n");
    assert_eq!(Host::get(&host.runs), 1);
}

#[test]
fn a_program_that_never_spawns_never_calls_the_executor() {
    let host = Host::new();
    let (out, result) = run_on(NO_SPAWN, host.clone(), Some(2));
    result.unwrap();
    assert_eq!(out, "no tasks\n");
    assert_eq!(Host::get(&host.runs), 0);
}

#[test]
fn output_printed_by_a_worker_on_a_host_thread_is_captured_in_order() {
    let (out, result) = run_on(PRINTING, Host::new(), Some(1));
    result.unwrap();
    assert_eq!(out, "in task\nmain\n");
}

// ── How many workers the host is asked for ───────────────────────

#[test]
fn the_host_is_asked_for_the_count_that_was_set_and_every_worker_returns_before_run_does() {
    let host = Host::new();
    let (_, result) = run_on(SPAWN_ONE, host.clone(), Some(3));
    result.unwrap();
    assert_eq!(Host::get(&host.requested), 3);
    assert_eq!(Host::get(&host.started), 3);
    assert_eq!(Host::get(&host.returned), 3);
}

#[test]
fn the_preference_of_the_host_is_used_when_no_count_is_set() {
    let host = Host::preferring(2);
    let (_, result) = run_on(SPAWN_ONE, host.clone(), None);
    result.unwrap();
    assert_eq!(Host::get(&host.requested), 2);
}

#[test]
fn an_explicit_count_beats_the_preference_of_the_host() {
    let host = Host::preferring(5);
    let (_, result) = run_on(SPAWN_ONE, host.clone(), Some(1));
    result.unwrap();
    assert_eq!(Host::get(&host.requested), 1);
}

#[test]
fn without_a_count_or_a_preference_the_runtime_default_applies() {
    let host = Host::new();
    let (_, result) = run_on(SPAWN_ONE, host.clone(), None);
    result.unwrap();
    assert_eq!(Host::get(&host.requested), the_default_count());
}

#[test]
fn a_preference_of_zero_runs_with_no_workers_and_does_not_call_the_host() {
    let host = Host::preferring(0);
    let (_, result) = run_on(SPAWN_ONE, host.clone(), None);
    assert!(result.unwrap_err().contains("no worker threads"));
    assert_eq!(Host::get(&host.runs), 0);
}

#[test]
fn an_explicit_zero_beats_a_preference_and_does_not_call_the_host() {
    let host = Host::preferring(3);
    let (_, result) = run_on(SPAWN_ONE, host.clone(), Some(0));
    assert!(result.unwrap_err().contains("no worker threads"));
    assert_eq!(Host::get(&host.runs), 0);
}

// ── A host that cannot start a worker ────────────────────────────

#[test]
fn a_host_that_cannot_start_a_worker_ends_the_run_with_its_reason_and_main_never_runs() {
    let host = Host::failing_after(1);
    let (out, result) = run_on(PRINT_THEN_SPAWN, host.clone(), Some(3));
    let err = result.unwrap_err();
    assert!(err.contains("the host has no thread to give"), "{err}");
    assert_eq!(out, "", "main must not run when the workers could not be started");
    // The worker that did start was told to stop and has returned.
    assert_eq!(Host::get(&host.started), 1);
    assert_eq!(Host::get(&host.returned), 1);
}

#[test]
fn a_host_that_returns_without_running_the_program_is_reported_as_an_error() {
    let (out, result) = run_on(SPAWN_ONE, Arc::new(SkipMain), Some(1));
    let err = result.unwrap_err();
    assert!(err.contains("without running the program"), "{err}");
    assert_eq!(out, "");
}

// ── Every wait and wake goes through the host ────────────────────

#[test]
fn every_join_of_the_main_thread_parks_through_the_host() {
    let host = Host::new();
    let (out, result) = run_on(MANY, host.clone(), Some(2));
    result.unwrap();
    assert_eq!(out, "204\n");
    assert!(
        Host::get(&host.parks) >= MANY_TASKS,
        "main joined {MANY_TASKS} spawned tasks but the host parked only {} times",
        Host::get(&host.parks)
    );
}

#[test]
fn the_host_is_woken_for_every_submit_every_finish_and_the_shutdown() {
    let host = Host::new();
    let (_, result) = run_on(MANY, host.clone(), Some(2));
    result.unwrap();
    // One wake as each task is queued, one as each ends, one when the pool shuts down.
    assert_eq!(Host::get(&host.wakes), 2 * MANY_TASKS + 1);
}

#[test]
fn a_host_that_polls_instead_of_blocking_runs_the_same_program() {
    let (out, result) = run_on(MANY, Arc::new(SpinHost), Some(3));
    result.unwrap();
    assert_eq!(out, "204\n");
}
