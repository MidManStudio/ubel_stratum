//! `Task.spawn` and `join` on real threads (docs/ASYNC_SYNC_DESIGN.md, AS3a).
//!
//! The `.ubl` fixtures show whole programs accepted, run or rejected. These tests pin what a
//! fixture cannot see: that tasks run at the same time on different threads, that a value is
//! copied (not shared) at the task boundary while what it shared inside stays shared, what a
//! panic or a `fail` in a task does to the one that waits for it, which values cannot be sent
//! yet, and what is and is not visible to a task (constants, statics).

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use ubel_stratum::ast::arena::AstArena;
use ubel_stratum::interpreter::Interpreter;
use ubel_stratum_rd::{check_source, Stage};

fn codes(source: &str) -> (Stage, Vec<&'static str>) {
    let report = check_source(source);
    let codes = report.diagnostics.iter().map(|d| d.code).collect();
    (report.stage, codes)
}

fn is_clean(source: &str) -> bool {
    matches!(check_source(source).stage, Stage::Clean)
}

/// Run a program that passes `check_source` with `workers` worker threads. Returns what it printed,
/// also when it ended in a panic, and the error text if it did.
fn run_with(source: &str, workers: usize) -> (String, Result<(), String>) {
    run_with_hook(source, workers, None)
}

fn run_with_hook(
    source: &str,
    workers: usize,
    hook: Option<Arc<dyn Fn() + Send + Sync>>,
) -> (String, Result<(), String>) {
    assert!(is_clean(source), "program should check clean: {:?}", codes(source).1);
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
    interp.set_worker_count(workers);
    if let Some(h) = hook {
        interp.set_task_start_hook(h);
    }
    let result = interp.run_program(&program).map(|_| ());
    let output = ubel_stratum::builtins::global::io::take_captured_output();
    (output, result)
}

fn run(source: &str, workers: usize) -> String {
    let (output, result) = run_with(source, workers);
    result.unwrap_or_else(|e| panic!("program should finish, ended with: {e}"));
    output
}

fn panic_text(source: &str, workers: usize) -> String {
    run_with(source, workers).1.expect_err("program should panic")
}

// ── Spawn, join, await ───────────────────────────────────────────

const WORK: &str = "async fn work(n: int) Task<int> {\n    return n * 2\n}\n\n";

#[test]
fn a_spawned_task_is_joined_for_its_result() {
    let src = format!("{WORK}fn main() void {{\n    let t = Task.spawn(work(21))\n    println(t.join())\n}}\n");
    assert_eq!(run(&src, 2), "42\n");
}

#[test]
fn a_spawned_task_is_awaited_in_async_code() {
    let src = format!("{WORK}async fn main() {{\n    let t = Task.spawn(work(1))\n    let v = await t\n    println(v)\n}}\n");
    assert_eq!(run(&src, 2), "2\n");
}

#[test]
fn join_on_a_task_that_was_never_spawned_runs_it_in_place() {
    let src = format!("{WORK}fn main() void {{\n    println(work(5).join())\n}}\n");
    // No `Task.spawn` anywhere, so the program starts no workers at all.
    let started = Arc::new(AtomicUsize::new(0));
    let counter = started.clone();
    let (out, result) = run_with_hook(&src, 2, Some(Arc::new(move || { counter.fetch_add(1, Ordering::SeqCst); })));
    result.unwrap();
    assert_eq!(out, "10\n");
    assert_eq!(started.load(Ordering::SeqCst), 0);
}

#[test]
fn a_task_returns_the_task_it_was_given() {
    // `Task.spawn(t)` gives `t` back, running, so `t` itself can be joined.
    let src = format!(
        "{WORK}fn main() void {{\n    let t = work(4)\n    let same = Task.spawn(t)\n    println(t.join())\n    println(\"done\")\n}}\n"
    );
    assert_eq!(run(&src, 2), "8\ndone\n");
}

#[test]
fn several_tasks_come_back_in_the_order_they_are_joined() {
    let src = "async fn square(n: int) Task<int> {\n    return n * n\n}\n\n\
               fn main() void {\n    let tasks: List<Task<int>> = List.new()\n    let mut i = 1\n    while i <= 6 {\n        tasks.push(Task.spawn(square(i)))\n        i += 1\n    }\n    for t in tasks {\n        println(t.join())\n    }\n}\n";
    assert_eq!(run(src, 3), "1\n4\n9\n16\n25\n36\n");
}

#[test]
fn many_more_tasks_than_workers_all_finish() {
    let src = "async fn bump(n: int) Task<int> {\n    return n + 1\n}\n\n\
               fn main() void {\n    let tasks: List<Task<int>> = List.new()\n    let mut i = 0\n    while i < 100 {\n        tasks.push(Task.spawn(bump(i)))\n        i += 1\n    }\n    let mut sum = 0\n    for t in tasks {\n        sum += t.join()\n    }\n    println(sum)\n}\n";
    assert_eq!(run(src, 4), "5050\n");
}

#[test]
fn an_async_method_runs_with_a_copy_of_its_receiver() {
    let src = "struct Counter { n: int }\n\n\
               extend Counter {\n    async fn plus(self, k: int) Task<int> {\n        self.n += k\n        return self.n\n    }\n}\n\n\
               fn main() void {\n    let c = Counter { n = 10 }\n    println(Task.spawn(c.plus(5)).join())\n    println(c.n)\n}\n";
    assert_eq!(run(src, 2), "15\n10\n");
}

#[test]
fn spawn_works_in_every_tier() {
    // `work` is LOW so that all three functions may call it (a LOW function cannot call a HIGH one).
    let src = format!(
        "@tier(low)\nasync fn work(n: int) Task<int> {{\n    return n * 2\n}}\n\n@tier(low)\nfn in_low() int {{\n    return Task.spawn(work(1)).join()\n}}\n\
         @tier(mid)\nfn in_mid() int {{\n    return Task.spawn(work(2)).join()\n}}\n\
         @tier(high)\nfn in_high() int {{\n    return Task.spawn(work(3)).join()\n}}\n\
         fn main() void {{\n    println(in_low() + in_mid() + in_high())\n}}\n"
    );
    assert_eq!(run(&src, 2), "12\n");
}

// ── Tasks run at the same time ───────────────────────────────────

#[test]
fn two_tasks_run_at_the_same_time_on_two_workers() {
    // Each worker waits, before it starts its task, until the other one has started too. If the
    // tasks ran one after the other the first would wait out the whole time limit.
    let started = Arc::new(AtomicUsize::new(0));
    let timed_out = Arc::new(AtomicBool::new(false));
    let (s, t) = (started.clone(), timed_out.clone());
    let hook: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
        s.fetch_add(1, Ordering::SeqCst);
        let deadline = Instant::now() + Duration::from_secs(10);
        while s.load(Ordering::SeqCst) < 2 {
            if Instant::now() > deadline {
                t.store(true, Ordering::SeqCst);
                return;
            }
            std::thread::yield_now();
        }
    });
    let src = format!(
        "{WORK}fn main() void {{\n    let a = Task.spawn(work(1))\n    let b = Task.spawn(work(2))\n    println(a.join() + b.join())\n}}\n"
    );
    let (out, result) = run_with_hook(&src, 2, Some(hook));
    result.unwrap();
    assert_eq!(out, "6\n");
    assert!(!timed_out.load(Ordering::SeqCst), "the two tasks did not run at the same time");
    assert_eq!(started.load(Ordering::SeqCst), 2);
}

#[test]
fn output_printed_before_the_workers_start_is_kept() {
    // A static's initializer runs before the pool starts, so what it prints was captured by then.
    // Moving the capture to the buffer the workers share must not lose it.
    let src = "fn init() int {\n    println(\"initializing\")\n    return 7\n}\n\nstatic START: int = init()\n\nasync fn read() Task<int> {\n    return START\n}\n\nfn main() void {\n    println(Task.spawn(read()).join())\n}\n";
    assert_eq!(run(src, 2), "initializing\n7\n");
}

#[test]
fn a_worker_prints_into_the_output_of_the_run() {
    let src = "async fn noisy() Task<void> {\n    println(\"from the task\")\n}\n\n\
               fn main() void {\n    println(\"before\")\n    Task.spawn(noisy()).join()\n    println(\"after\")\n}\n";
    assert_eq!(run(src, 2), "before\nfrom the task\nafter\n");
}

// ── The copy at the boundary ─────────────────────────────────────

#[test]
fn a_change_made_in_a_task_is_not_seen_by_the_one_that_spawned_it() {
    let src = "async fn push_it(xs: List<int>) Task<int> {\n    xs.push(99)\n    return xs.len()\n}\n\n\
               fn main() void {\n    let xs: List<int> = List.new()\n    xs.push(1)\n    println(Task.spawn(push_it(xs)).join())\n    println(xs.len())\n}\n";
    assert_eq!(run(src, 2), "2\n1\n");
}

#[test]
fn what_was_shared_inside_the_copy_stays_shared() {
    let src = "async fn probe(pair: (List<int>, List<int>)) Task<int> {\n    pair[0].push(7)\n    return pair[1].len()\n}\n\n\
               fn main() void {\n    let shared: List<int> = List.new()\n    shared.push(1)\n    println(Task.spawn(probe((shared, shared))).join())\n    println(shared.len())\n}\n";
    // One list reached through two names in the tuple is still one list in the task.
    assert_eq!(run(src, 2), "2\n1\n");
}

#[test]
fn two_lists_that_were_separate_stay_separate() {
    let src = "async fn probe(pair: (List<int>, List<int>)) Task<int> {\n    pair[0].push(7)\n    return pair[1].len()\n}\n\n\
               fn main() void {\n    let a: List<int> = List.new()\n    let b: List<int> = List.new()\n    println(Task.spawn(probe((a, b))).join())\n}\n";
    assert_eq!(run(src, 2), "0\n");
}

#[test]
fn structs_enums_tuples_and_collections_cross_both_ways() {
    let src = "enum Shape { Dot, Sized(int), Box { w: int, h: int } }\n\
               struct Entry { name: string, tags: List<string>, sizes: Dictionary<string, int>, shape: Shape, pos: (int, double) }\n\n\
               async fn grow(e: Entry) Task<Entry> {\n    e.tags.push(\"seen\")\n    e.sizes.set(\"later\", 9)\n    return Entry { name = e.name, tags = e.tags, sizes = e.sizes, shape = Shape.Box { w = 3, h = 4 }, pos = (e.pos[0] + 1, e.pos[1] * 2.0) }\n}\n\n\
               fn main() void {\n    let tags: List<string> = List.new()\n    tags.push(\"a\")\n    let sizes: Dictionary<string, int> = Dictionary.new()\n    sizes.set(\"first\", 1)\n    let e = Entry { name = \"x\", tags = tags, sizes = sizes, shape = Shape.Sized(2), pos = (1, 1.5) }\n    let r = Task.spawn(grow(e)).join()\n    println(r.name)\n    println(r.tags.len())\n    println(r.sizes.len())\n    println(r.pos[0])\n    println(r.pos[1])\n    println(tags.len())\n    println(sizes.len())\n    match r.shape {\n        Shape.Box { w, h } => println(w * h),\n        _ => println(\"other\"),\n    }\n}\n";
    assert_eq!(run(src, 2), "x\n2\n2\n2\n3\n1\n1\n12\n");
}

#[test]
fn an_async_block_takes_only_the_names_it_uses() {
    let src = "fn main() void {\n    let base = 10\n    let items: List<int> = List.new()\n    items.push(1)\n    items.push(2)\n    let unused: List<int> = List.new()\n    let t = Task.spawn(async { base + items.len() })\n    println(t.join())\n    println(unused.len())\n}\n";
    assert_eq!(run(src, 2), "12\n0\n");
}

#[test]
fn an_async_block_can_use_a_name_inside_an_interpolated_string() {
    let src = "fn main() void {\n    let who = \"task\"\n    let t = Task.spawn(async { $\"hello {who}\" })\n    println(t.join())\n}\n";
    assert_eq!(run(src, 2), "hello task\n");
}

#[test]
fn an_async_block_can_use_the_names_of_the_program() {
    let src = "const K = 5\n\nfn triple(n: int) int {\n    return n * 3\n}\n\n\
               fn main() void {\n    let t = Task.spawn(async { triple(K) })\n    println(t.join())\n}\n";
    assert_eq!(run(src, 2), "15\n");
}

#[test]
fn an_async_block_can_spawn_tasks_of_its_own() {
    let src = format!(
        "{WORK}fn main() void {{\n    let t = Task.spawn(async {{\n        let inner = Task.spawn(work(4))\n        return await inner\n    }})\n    println(t.join())\n}}\n"
    );
    assert_eq!(run(&src, 2), "8\n");
}

// ── A task that waits for another ────────────────────────────────

const NESTED: &str = "async fn leaf(n: int) Task<int> {\n    return n * 10\n}\n\n\
                      async fn middle(n: int) Task<int> {\n    let inner = Task.spawn(leaf(n))\n    let v = await inner\n    return v + 1\n}\n\n\
                      fn main() void {\n    let a = Task.spawn(middle(1))\n    let b = Task.spawn(middle(2))\n    let c = Task.spawn(middle(3))\n    println(a.join() + b.join() + c.join())\n}\n";

#[test]
fn a_task_that_waits_for_a_queued_task_runs_it_itself_on_one_worker() {
    // The one worker is busy waiting, so nothing else could ever run the task it waits for.
    assert_eq!(run(NESTED, 1), "63\n");
}

#[test]
fn nested_waits_finish_on_several_workers() {
    assert_eq!(run(NESTED, 4), "63\n");
}

// ── How a task ends ──────────────────────────────────────────────

#[test]
fn a_panic_in_a_task_reaches_the_one_that_joins_it() {
    let src = "async fn boom() Task<int> {\n    assert(false, \"task panicked\")\n    return 1\n}\n\n\
               fn main() void {\n    println(\"start\")\n    println(Task.spawn(boom()).join())\n}\n";
    let (out, result) = run_with(src, 2);
    assert_eq!(out, "start\n");
    assert!(result.unwrap_err().contains("task panicked"));
}

#[test]
fn a_task_that_fails_hands_its_failure_to_the_question_mark() {
    let src = "async fn risky(n: int) Task<int!> {\n    if n < 0 {\n        fail \"negative\"\n    }\n    return n + 1\n}\n\n\
               fn run(n: int) int! {\n    let t = Task.spawn(risky(n))\n    let v = t.join()?\n    return v\n}\n\n\
               fn main() void {\n    try {\n        println(run(4)?)\n        println(run(-1)?)\n        println(\"not reached\")\n    } catch (e) {\n        println($\"caught {e}\")\n    }\n}\n";
    assert_eq!(run(src, 2), "5\ncaught negative\n");
}

#[test]
fn a_task_can_be_awaited_or_spawned_only_once() {
    let twice = format!("{WORK}fn main() void {{\n    let t = work(1)\n    let h = Task.spawn(t)\n    println(h.join())\n    println(t.join())\n}}\n");
    let (out, result) = run_with(&twice, 2);
    assert_eq!(out, "2\n");
    assert!(result.unwrap_err().contains("awaited only once"));

    let spawn_twice = format!("{WORK}fn main() void {{\n    let t = work(1)\n    let a = Task.spawn(t)\n    let b = Task.spawn(t)\n    println(a.join())\n}}\n");
    assert!(panic_text(&spawn_twice, 2).contains("spawned only once"));

    let after_await = format!("{WORK}fn main() void {{\n    let t = work(1)\n    println(t.join())\n    let h = Task.spawn(t)\n}}\n");
    assert!(panic_text(&after_await, 2).contains("spawned only once"));
}

#[test]
fn a_run_with_no_workers_cannot_spawn() {
    let src = format!("{WORK}fn main() void {{\n    println(Task.spawn(work(1)).join())\n}}\n");
    assert!(panic_text(&src, 0).contains("no worker threads"));
}

// ── What cannot cross yet ────────────────────────────────────────

#[test]
fn a_shared_cannot_be_sent_to_a_task() {
    let src = "@tier(low)\nfn run() void {\n    let s = Shared.new(5)\n    let t = Task.spawn(async { s })\n    t.join()\n}\n\nfn main() void {\n    run()\n}\n";
    assert!(panic_text(src, 2).contains("a Shared cannot be sent to another task"));
}

#[test]
fn a_closure_cannot_be_sent_to_a_task() {
    let src = "async fn call(f: fn(int) int) Task<int> {\n    return f(1)\n}\n\n\
               fn main() void {\n    let k = 3\n    let add = fn(x: int) x + k\n    println(Task.spawn(call(add)).join())\n}\n";
    assert!(panic_text(src, 2).contains("a closure cannot be sent to another task"));
}

#[test]
fn a_function_of_the_program_can_be_sent() {
    let src = "fn double(x: int) int {\n    return x * 2\n}\n\n\
               async fn call(f: fn(int) int) Task<int> {\n    return f(21)\n}\n\n\
               fn main() void {\n    println(Task.spawn(call(double)).join())\n}\n";
    assert_eq!(run(src, 2), "42\n");
}

#[test]
fn a_task_cannot_be_sent_to_a_task() {
    let src = format!(
        "{WORK}async fn outer(t: Task<int>) Task<int> {{\n    return await t\n}}\n\n\
         fn main() void {{\n    println(Task.spawn(outer(work(1))).join())\n}}\n"
    );
    assert!(panic_text(&src, 2).contains("a task cannot be sent to another task"));
}

#[test]
fn a_task_that_could_not_send_its_result_says_so() {
    let src = "async fn make() Task<fn(int) int> {\n    let k = 2\n    return fn(x: int) x + k\n}\n\n\
               fn main() void {\n    let f = Task.spawn(make()).join()\n}\n";
    assert!(panic_text(src, 2).contains("could not return its result"));
}

// ── What a task sees of the program ──────────────────────────────

#[test]
fn a_task_sees_the_constants_and_the_statics_the_program_started_with() {
    let src = "const K = 5\nstatic START: int = 7\n\n\
               async fn read() Task<int> {\n    return K * 10 + START\n}\n\n\
               fn main() void {\n    println(Task.spawn(read()).join())\n}\n";
    assert_eq!(run(src, 2), "57\n");
}

#[test]
fn a_task_cannot_write_a_static_and_the_main_thread_is_left_alone() {
    let src = "static COUNT: int = 0\n\n\
               async fn bump() Task<int> {\n    COUNT = COUNT + 100\n    return COUNT\n}\n\n\
               fn main() void {\n    COUNT = COUNT + 1\n    println(Task.spawn(bump()).join())\n}\n";
    let (out, result) = run_with(src, 1);
    assert_eq!(out, "");
    assert!(result.unwrap_err().contains("a task cannot write the static `COUNT`"));
}

#[test]
fn the_main_thread_waits_instead_of_running_a_queued_task() {
    // With one worker held up, the task the main thread joins is still queued. Running it on the
    // main thread would let it write the program's static; it must wait for the worker instead.
    let src = "static COUNT: int = 0\n\n\
               async fn bump() Task<int> {\n    COUNT = COUNT + 100\n    return COUNT\n}\n\n\
               fn main() void {\n    let t = Task.spawn(bump())\n    println(t.join())\n}\n";
    assert!(panic_text(src, 1).contains("a task cannot write the static"));
}

// ── Diagnostics ──────────────────────────────────────────────────

#[test]
fn spawning_something_that_is_not_a_task_is_a_type_mismatch() {
    let src = "fn main() void {\n    let a = Task.spawn(5)\n}\n";
    assert_eq!(codes(src).1, vec!["TYPE-101"]);
}

#[test]
fn spawn_takes_one_argument() {
    let none = format!("{WORK}fn main() void {{\n    let a = Task.spawn()\n}}\n");
    assert_eq!(codes(&none).1, vec!["TYPE-102"]);
    let two = format!("{WORK}fn main() void {{\n    let a = Task.spawn(work(1), work(2))\n}}\n");
    assert_eq!(codes(&two).1, vec!["TYPE-102"]);
}

#[test]
fn join_takes_no_arguments_and_gives_the_result_type() {
    let ok = format!("{WORK}fn main() void {{\n    let n: int = work(1).join()\n}}\n");
    assert!(is_clean(&ok));
    let arg = format!("{WORK}fn main() void {{\n    let n = work(1).join(3)\n}}\n");
    assert_eq!(codes(&arg).1, vec!["TYPE-102"]);
    let wrong = format!("{WORK}fn main() void {{\n    let s: string = work(1).join()\n}}\n");
    assert_eq!(codes(&wrong).1, vec!["TYPE-101"]);
}

#[test]
fn task_is_a_name_only_before_a_dot() {
    let src = "fn main() void {\n    let x = Task\n}\n";
    assert!(!is_clean(src));
}

#[test]
fn a_spawned_task_has_the_type_of_the_task_it_was_given() {
    let src = format!("{WORK}fn main() void {{\n    let t: Task<int> = Task.spawn(work(1))\n    let n: int = await_it(t)\n}}\n\nfn await_it(t: Task<int>) int {{\n    return t.join()\n}}\n");
    assert!(is_clean(&src));
    let wrong = format!("{WORK}fn main() void {{\n    let t: Task<string> = Task.spawn(work(1))\n}}\n");
    assert_eq!(codes(&wrong).1, vec!["TYPE-101"]);
}

#[test]
fn the_default_pool_has_at_least_one_worker() {
    assert!(ubel_stratum::interpreter::eval::spawn::default_worker_count() >= 1);
}
