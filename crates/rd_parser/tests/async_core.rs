//! The async core (docs/ASYNC_SYNC_DESIGN.md, AS2): `async fn f() Task<T>`, cold tasks,
//! `await` that runs a task where it is awaited, `async { }` blocks, and `async` and
//! `await` in every tier. No threads and no spawn yet.
//!
//! The `.ubl` fixtures show whole programs accepted or rejected. These tests pin what they
//! cannot see: that a call runs nothing, the order things happen in, that a task is
//! awaited at most once (a run-time failure no fixture can express), and the exact code
//! and count each new rule reports.

use ubel_stratum::ast::arena::AstArena;
use ubel_stratum::interpreter::Interpreter;
use ubel_stratum_rd::{check_source, Stage};

fn codes(source: &str) -> (Stage, Vec<&'static str>) {
    let report = check_source(source);
    let codes = report.diagnostics.iter().map(|d| d.code).collect();
    (report.stage, codes)
}

fn messages(source: &str) -> Vec<String> {
    check_source(source).diagnostics.iter().map(|d| d.message.clone()).collect()
}

fn is_clean(source: &str) -> bool {
    matches!(check_source(source).stage, Stage::Clean)
}

/// Run a program that passes `check_source` and return what it printed.
fn run(source: &str) -> Result<String, String> {
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
    let result = interp.run_program(&program);
    let output = ubel_stratum::builtins::global::io::take_captured_output();
    result.map(|_| output)
}

// ── Cold tasks and await ─────────────────────────────────────────

#[test]
fn a_call_runs_nothing_and_await_runs_the_body_where_it_is_awaited() {
    let src = "async fn work() Task<int> {\n    println(\"body\")\n    return 7\n}\n\
               async fn main() {\n    println(\"before\")\n    let t = work()\n    println(\"built\")\n\
                   let n = await t\n    println(\"after\")\n    println(n)\n}\n";
    assert_eq!(run(src).unwrap(), "before\nbuilt\nbody\nafter\n7\n");
}

#[test]
fn a_task_that_is_never_awaited_never_runs() {
    let src = "async fn work() Task<int> {\n    println(\"ran\")\n    return 1\n}\n\
               fn main() void {\n    let t = work()\n    println(\"done\")\n}\n";
    assert_eq!(run(src).unwrap(), "done\n");
}

#[test]
fn a_task_is_awaited_at_most_once() {
    let src = "async fn work() Task<int> { return 1 }\n\
               async fn main() {\n    let t = work()\n    await t\n    await t\n}\n";
    let err = run(src).unwrap_err();
    assert!(err.contains("a task can be awaited only once"), "got: {err}");
}

#[test]
fn a_copy_of_a_task_is_the_same_task() {
    let src = "async fn work() Task<int> { return 1 }\n\
               async fn main() {\n    let t = work()\n    let u = t\n    await t\n    await u\n}\n";
    let err = run(src).unwrap_err();
    assert!(err.contains("a task can be awaited only once"), "got: {err}");
}

#[test]
fn awaiting_two_calls_of_the_same_function_is_two_tasks() {
    let src = "static RAN: int = 0\nasync fn work() Task<int> { RAN += 1\n return RAN }\n\
               async fn main() {\n    let a = await work()\n    let b = await work()\n    println(a + b)\n}\n";
    assert_eq!(run(src).unwrap(), "3\n");
}

#[test]
fn a_panic_inside_a_task_stops_the_awaiter() {
    // Capturing the panic as a value is AS6; until then it propagates.
    let src = "async fn boom() Task<int> {\n    assert(false, \"boom inside a task\")\n    return 1\n}\n\
               async fn main() {\n    println(\"before\")\n    await boom()\n    println(\"after\")\n}\n";
    let err = run(src).unwrap_err();
    assert!(err.contains("boom inside a task"), "got: {err}");
}

#[test]
fn return_inside_an_async_function_ends_it() {
    let src = "async fn early() Task<int> {\n    return 1\n    println(\"never\")\n}\n\
               async fn main() { println(await early()) }\n";
    assert_eq!(run(src).unwrap(), "1\n");
}

#[test]
fn an_async_method_acts_on_its_receiver_when_awaited() {
    let src = "struct Cnt { n: int }\nimpl Cnt { async fn bump(self) { self.n += 1 } }\n\
               async fn main() {\n    let c = Cnt { n = 0 }\n    let t = c.bump()\n    println(c.n)\n\
                   await t\n    println(c.n)\n}\n";
    assert_eq!(run(src).unwrap(), "0\n1\n");
}

#[test]
fn a_bare_async_fn_and_task_void_are_the_same() {
    let src = "async fn a() { println(\"a\") }\nasync fn b() Task<void> { println(\"b\") }\n\
               async fn main() {\n    await a()\n    await b()\n}\n";
    assert_eq!(run(src).unwrap(), "a\nb\n");
    let kinds = "async fn a() { }\nfn main() void { let t: Task<void> = a() }\n";
    assert!(is_clean(kinds), "{:?}", codes(kinds).1);
}

#[test]
fn async_main_is_run_to_completion() {
    let src = "async fn main() {\n    println(\"in main\")\n}\n";
    assert_eq!(run(src).unwrap(), "in main\n");
}

// ── The declared result ──────────────────────────────────────────

#[test]
fn an_async_function_that_does_not_declare_a_task_is_one_error() {
    for (src, found) in [
        ("async fn f() int { return 1 }\nfn main() void { }\n", "int"),
        ("async fn f() string { return \"a\" }\nfn main() void { }\n", "string"),
        ("async fn f() void { }\nfn main() void { }\n", "void"),
        ("async fn f<T>(x: T) T { return x }\nfn main() void { }\n", "T"),
    ] {
        assert_eq!(codes(src), (Stage::Sema, vec!["TYPE-144"]), "{src}");
        assert_eq!(messages(src), vec![format!("an async function returns `Task<T>`, found `{found}`")]);
    }
}

#[test]
fn a_fallible_task_is_not_a_declared_task() {
    // `Task<int>!` is a fallible call, which is not what an async fn declares.
    // `Task<int!>`, a task that yields a fallible value, is.
    let bad = "async fn f() Task<int>! { return 1 }\nfn main() void { }\n";
    assert_eq!(codes(bad), (Stage::Sema, vec!["TYPE-144"]));
    let good = "async fn f() Task<int!> { return 1 }\nfn main() void { }\n";
    assert!(is_clean(good), "{:?}", codes(good).1);
}

#[test]
fn the_body_is_checked_against_the_inner_type_and_a_call_has_the_task_type() {
    let body = "async fn f() Task<int> { return \"x\" }\nfn main() void { }\n";
    assert_eq!(codes(body), (Stage::Sema, vec!["TYPE-101"]));
    assert_eq!(messages(body), vec!["type mismatch: expected `int`, found `string`".to_string()]);
    let call = "async fn f() Task<int> { return 1 }\nfn main() void { let n: int = f() }\n";
    assert_eq!(messages(call), vec!["type mismatch: expected `int`, found `Task<int>`".to_string()]);
}

// ── await only inside an async context ───────────────────────────

#[test]
fn await_outside_an_async_function_is_one_error_each() {
    let one = "async fn one() Task<int> { return 1 }\n";
    for (tail, n) in [
        ("fn plain() int { return await one() }\nfn main() void { }\n", 1),
        ("struct S { x: int }\nimpl S { fn m(self) int { return await one() } }\nfn main() void { }\n", 1),
        ("const C = await one()\nfn main() void { }\n", 1),
        ("static S: int = await one()\nfn main() void { }\n", 1),
        ("fn a() int { return await one() }\nfn b() int { return await one() }\nfn main() void { }\n", 2),
    ] {
        let src = format!("{one}{tail}");
        assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-143"; n]), "{src}");
    }
}

#[test]
fn await_does_not_reach_through_a_plain_lambda() {
    let src = "async fn one() Task<int> { return 1 }\nasync fn f() {\n    let g = fn() await one()\n}\nfn main() void { }\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["TYPE-143"]));
}

#[test]
fn an_async_method_does_not_leave_its_context_behind() {
    // The async context ends with the method that set it: neither the method after it
    // nor a constant initializer checked later is inside one.
    let src = "async fn one() Task<int> { return 1 }\nstruct S { x: int }\n\
               impl S {\n    async fn a(self) Task<int> { return await one() }\n\
                   fn b(self) int { return await one() }\n}\nconst C = await one()\nfn main() void { }\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["TYPE-143", "TYPE-143"]));
}

#[test]
fn await_is_legal_in_async_functions_methods_and_blocks() {
    let src = "async fn one() Task<int> { return 1 }\nstruct S { x: int }\n\
               impl S { async fn m(self) Task<int> { return await one() } }\n\
               async fn f() Task<int> {\n    let t = async { return await one() }\n    return await t\n}\n\
               fn main() void { }\n";
    assert!(is_clean(src), "{:?}", codes(src).1);
}

#[test]
fn await_on_a_non_task_is_still_its_own_error() {
    let src = "async fn f() {\n    let n = await 5\n}\nfn main() void { }\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["TYPE-106"]));
}

// ── async blocks ─────────────────────────────────────────────────

#[test]
fn an_async_block_is_cold_and_captures_its_scope() {
    let src = "async fn main() {\n    let offset = 2\n    let t = async {\n        println(\"in block\")\n\
                   return 40 + offset\n    }\n    println(\"built\")\n    println(await t)\n}\n";
    assert_eq!(run(src).unwrap(), "built\nin block\n42\n");
}

#[test]
fn return_inside_an_async_block_ends_the_block_not_the_function() {
    let src = "async fn main() {\n    let t = async {\n        return 1\n        println(\"never\")\n    }\n\
                   let n = await t\n    println(n + 1)\n}\n";
    assert_eq!(run(src).unwrap(), "2\n");
}

#[test]
fn an_async_block_has_the_task_type_of_its_value() {
    for ok in [
        "async fn main() { let t: Task<int> = async { return 1 } }\n",
        "async fn main() { let t: Task<void> = async { println(1) } }\n",
        "async fn main() { let t: Task<string> = async { return \"s\" } }\n",
    ] {
        assert!(is_clean(ok), "{ok} -> {:?}", codes(ok).1);
    }
    let bad = "async fn main() { let t: Task<string> = async { return 1 } }\n";
    assert_eq!(codes(bad), (Stage::Sema, vec!["TYPE-101"]));
    let not_a_task = "async fn main() { let n: int = async { return 1 } }\n";
    assert_eq!(codes(not_a_task), (Stage::Sema, vec!["TYPE-101"]));
}

#[test]
fn an_async_block_outside_an_async_function_is_still_a_task() {
    // Building one needs no async context; awaiting one does.
    let built = "fn main() void { let t: Task<int> = async { return 1 } }\n";
    assert!(is_clean(built), "{:?}", codes(built).1);
    let awaited = "fn main() void { let t = async { return 1 }\n let n = await t }\n";
    assert_eq!(codes(awaited), (Stage::Sema, vec!["TYPE-143"]));
}

#[test]
fn an_async_block_can_be_returned_from_a_function_and_awaited_later() {
    let src = "fn later(n: int) Task<int> { return async { return n * 2 } }\n\
               async fn main() {\n    let t = later(21)\n    println(await t)\n}\n";
    assert_eq!(run(src).unwrap(), "42\n");
}

#[test]
fn a_block_inside_a_block_is_two_tasks() {
    let src = "async fn main() {\n    let outer = async {\n        let inner = async { return 5 }\n\
                   return (await inner) + 1\n    }\n    println(await outer)\n}\n";
    assert_eq!(run(src).unwrap(), "6\n");
}

#[test]
fn a_block_is_awaited_at_most_once_too() {
    let src = "async fn main() {\n    let t = async { return 1 }\n    await t\n    await t\n}\n";
    let err = run(src).unwrap_err();
    assert!(err.contains("a task can be awaited only once"), "got: {err}");
}

// ── Tiers ────────────────────────────────────────────────────────

#[test]
fn async_and_await_are_legal_in_every_tier_and_run() {
    let src = "@tier(low)\nasync fn low_task(x: int) Task<int> { return x + 1 }\n\
               @tier(mid)\nasync fn mid_task() Task<int> { return 2 }\n\
               @tier(low)\nasync fn low_runner() Task<int> { let a = await low_task(1)\n return a }\n\
               async fn main() {\n    println(await low_runner())\n    println(await mid_task())\n}\n";
    assert_eq!(run(src).unwrap(), "2\n2\n");
}

#[test]
fn an_async_block_runs_in_a_mid_function() {
    let src = "@tier(mid)\nasync fn f(x: int) Task<int> {\n    let t = async { return x * 3 }\n    return await t\n}\n\
               async fn main() { println(await f(5)) }\n";
    assert_eq!(run(src).unwrap(), "15\n");
}

#[test]
fn tier_rules_still_apply_to_the_operand_of_an_await() {
    // `await` opened up every tier, but what is inside it is checked as before: a LOW
    // function may not call a HIGH one, awaited or not.
    let src = "async fn high_task() Task<int> { return 1 }\n@tier(low)\n\
               async fn f() Task<int> { return await high_task() }\nfn main() void { }\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["TIER-005"]));
}

// ── Not built yet ────────────────────────────────────────────────

#[test]
fn async_methods_in_a_trait_or_a_trait_impl_are_explicit_errors() {
    let decl = "trait T { async fn go(self) Task<int> }\nfn main() void { }\n";
    assert_eq!(codes(decl), (Stage::Parse, vec!["PARSE-004"]));
    assert!(messages(decl)[0].contains("async methods in a trait are not supported yet"));
    let imp = "trait T { fn go(self) Task<int> }\nstruct S { x: int }\n\
               impl T for S { async fn go(self) Task<int> { return 1 } }\nfn main() void { }\n";
    assert_eq!(codes(imp), (Stage::Parse, vec!["PARSE-004"]));
    assert!(messages(imp)[0].contains("async methods in a trait impl are not supported yet"));
    let inherent = "struct S { x: int }\nimpl S { async fn go(self) Task<int> { return 1 } }\nfn main() void { }\n";
    assert!(is_clean(inherent), "an inherent async method is fine: {:?}", codes(inherent).1);
}

#[test]
fn a_plain_trait_method_may_return_a_task_from_an_async_function() {
    let src = "async fn read(n: int) Task<int> { return n }\ntrait Src { fn fetch(self) Task<int> }\n\
               struct Db { base: int }\nimpl Src for Db { fn fetch(self) Task<int> { return read(self.base) } }\n\
               async fn main() {\n    let d = Db { base = 4 }\n    println(await d.fetch())\n}\n";
    assert_eq!(run(src).unwrap(), "4\n");
}
