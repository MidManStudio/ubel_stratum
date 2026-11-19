//! The `using` cleanup (docs/ASYNC_SYNC_DESIGN.md, AS1): a `using` block calls the `dispose` of
//! the `Dispose` impl of every value it binds, last binding first, on every way out of the
//! block, and a value with no `Dispose` impl has no cleanup.
//!
//! The `.ubl` fixtures show whole programs accepted or rejected, and check the order of cleanups
//! through a shared log. These tests pin what they cannot see: the exact output order around
//! each exit (the value of a `return` is computed before the cleanup runs, a panic still reaches
//! the end of the program), the panic rules for a cleanup that panics, which values have a
//! cleanup at all, and the exact code and count each diagnostic reports.

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

/// Run a program that passes `check_source`. Returns what it printed, also when it ended in
/// a panic, and the error text if it did.
fn run_all(source: &str) -> (String, Result<(), String>) {
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
    let result = interp.run_program(&program).map(|_| ());
    let output = ubel_stratum::builtins::global::io::take_captured_output();
    (output, result)
}

/// A program that runs to the end: what it printed.
fn run(source: &str) -> String {
    let (output, result) = run_all(source);
    result.unwrap_or_else(|e| panic!("program should finish, ended with: {e}"));
    output
}

/// A guard that prints its name when it is disposed, and one that panics when it is.
const GUARDS: &str = r#"struct Guard { name: string }

impl Dispose for Guard {
    fn dispose(self) void { println($"dispose {self.name}") }
}

struct Loud { name: string }

impl Dispose for Loud {
    fn dispose(self) void {
        println($"dispose {self.name}")
        assert(false, $"cleanup panic {self.name}")
    }
}

"#;

fn with_guards(body: &str) -> String {
    format!("{GUARDS}{body}")
}

// ── The end of the block ─────────────────────────────────────────

#[test]
fn the_cleanup_runs_when_the_block_ends() {
    let src = with_guards(
        "fn main() void {\n    using let g = Guard { name = \"a\" } {\n        println(\"body\")\n    }\n    println(\"after\")\n}\n",
    );
    assert_eq!(run(&src), "body\ndispose a\nafter\n");
}

#[test]
fn several_bindings_are_cleaned_up_last_first() {
    let src = with_guards(
        "fn main() void {\n    using let a = Guard { name = \"a\" }, let b = Guard { name = \"b\" }, let c = Guard { name = \"c\" } {\n        println(\"body\")\n    }\n}\n",
    );
    assert_eq!(run(&src), "body\ndispose c\ndispose b\ndispose a\n");
}

#[test]
fn nested_blocks_clean_up_the_inner_one_first() {
    let src = with_guards(
        "fn main() void {\n    using let outer = Guard { name = \"outer\" } {\n        using let inner = Guard { name = \"inner\" } {\n            println(\"body\")\n        }\n        println(\"between\")\n    }\n}\n",
    );
    assert_eq!(run(&src), "body\ndispose inner\nbetween\ndispose outer\n");
}

#[test]
fn a_later_binding_sees_an_earlier_one() {
    let src = with_guards(
        "struct Pair { first: Guard, label: string }\n\
         fn main() void {\n    using let a = Guard { name = \"a\" }, let p = Pair { first = a, label = \"pair\" } {\n        println(p.first.name)\n    }\n}\n",
    );
    assert_eq!(run(&src), "a\ndispose a\n");
}

#[test]
fn a_binding_of_the_block_is_not_visible_after_it() {
    let src = "fn main() void {\n    let g = 1\n    using let g = 5 {\n        println(g)\n    }\n    println(g)\n}\n";
    assert_eq!(run(src), "5\n1\n");
}

#[test]
fn the_defers_of_the_body_run_before_the_cleanup() {
    let src = with_guards(
        "fn main() void {\n    using let g = Guard { name = \"d\" } {\n        defer println(\"deferred\")\n        println(\"body\")\n    }\n}\n",
    );
    assert_eq!(run(&src), "body\ndeferred\ndispose d\n");
}

// ── Every way out ────────────────────────────────────────────────

#[test]
fn a_return_computes_its_value_before_the_cleanup_runs() {
    let src = with_guards(
        "fn value() int {\n    println(\"value computed\")\n    return 7\n}\n\
         fn f() int {\n    using let g = Guard { name = \"r\" } {\n        return value()\n    }\n    return 0\n}\n\
         fn main() void {\n    let n = f()\n    println(\"returned\")\n    println(n)\n}\n",
    );
    assert_eq!(run(&src), "value computed\ndispose r\nreturned\n7\n");
}

#[test]
fn break_and_continue_clean_up_each_pass_of_a_loop() {
    let src = with_guards(
        "fn main() void {\n    let mut i = 0\n    while i < 5 {\n        i += 1\n        using let g = Guard { name = $\"pass{i}\" } {\n            if i == 1 { continue }\n            if i == 3 { break }\n            println($\"body {i}\")\n        }\n    }\n    println(\"after loop\")\n}\n",
    );
    assert_eq!(run(&src), "dispose pass1\nbody 2\ndispose pass2\ndispose pass3\nafter loop\n");
}

#[test]
fn a_fail_in_the_body_cleans_up_before_the_catch_runs() {
    let src = with_guards(
        "fn work() void! {\n    using let g = Guard { name = \"f\" } {\n        fail \"body failed\"\n    }\n}\n\
         fn main() void {\n    try {\n        work()?\n    } catch (e) {\n        println(\"caught\")\n    }\n    println(\"end\")\n}\n",
    );
    assert_eq!(run(&src), "dispose f\ncaught\nend\n");
}

#[test]
fn a_panic_in_the_body_cleans_up_and_then_reaches_the_end_of_the_program() {
    let src = with_guards(
        "fn main() void {\n    using let g = Guard { name = \"p\" } {\n        assert(false, \"boom inside using\")\n    }\n    println(\"unreachable\")\n}\n",
    );
    let (output, result) = run_all(&src);
    assert_eq!(output, "dispose p\n");
    assert!(result.unwrap_err().contains("boom inside using"));
}

// ── A binding that never finished ────────────────────────────────

#[test]
fn a_failing_initializer_cleans_up_the_earlier_bindings_only() {
    let src = with_guards(
        "fn boom() Guard! {\n    println(\"init b\")\n    fail \"second failed\"\n}\n\
         fn make() Guard {\n    println(\"init c\")\n    return Guard { name = \"c\" }\n}\n\
         fn two() void! {\n    using let a = Guard { name = \"a\" }, let b = boom()?, let c = make() {\n        println(\"never\")\n    }\n}\n\
         fn main() void {\n    try {\n        two()?\n    } catch (e) {\n        println(\"caught\")\n    }\n}\n",
    );
    assert_eq!(run(&src), "init b\ndispose a\ncaught\n");
}

// ── Which values have a cleanup ──────────────────────────────────

#[test]
fn a_value_with_no_dispose_impl_has_no_cleanup() {
    let src = "struct Plain { n: int }\n\
               fn main() void {\n    using let a = 5, let s = \"text\", let p = Plain { n = 1 } {\n        println(\"body\")\n    }\n    println(\"after\")\n}\n";
    assert_eq!(run(src), "body\nafter\n");
}

#[test]
fn a_method_named_dispose_is_not_the_cleanup_unless_it_is_the_dispose_impl() {
    let src = "struct Guard { name: string }\n\
               extend Guard {\n    fn dispose(self) void { println(\"inherent dispose\") }\n}\n\
               fn main() void {\n    using let g = Guard { name = \"a\" } {\n        println(\"body\")\n    }\n}\n";
    assert_eq!(run(src), "body\n");
}

#[test]
fn an_enum_can_implement_dispose() {
    let src = "enum Mode { On, Off }\n\
               impl Dispose for Mode {\n    fn dispose(self) void { println(\"dispose mode\") }\n}\n\
               fn main() void {\n    using let m = Mode.On {\n        println(\"body\")\n    }\n}\n";
    assert_eq!(run(src), "body\ndispose mode\n");
}

#[test]
fn the_value_the_binding_started_with_is_the_one_cleaned_up() {
    let src = with_guards(
        "fn main() void {\n    using let mut g = Guard { name = \"first\" } {\n        g = Guard { name = \"replacement\" }\n        println(g.name)\n    }\n}\n",
    );
    assert_eq!(run(&src), "replacement\ndispose first\n");
}

#[test]
fn a_unique_is_looked_through_and_a_shared_is_not() {
    let src = with_guards(
        "@tier(low)\nfn run() void {\n    using let u = Unique.new(Guard { name = \"unique\" }) {\n        println(\"body u\")\n    }\n    using let s = Shared.new(Guard { name = \"shared\" }) {\n        println(\"body s\")\n    }\n    using let ss = SyncShared.new(Guard { name = \"sync\" }) {\n        println(\"body ss\")\n    }\n    println(\"end\")\n}\n\
         fn main() void {\n    run()\n}\n",
    );
    assert_eq!(run(&src), "body u\ndispose unique\nbody s\nbody ss\nend\n");
}

#[test]
fn a_generic_function_cleans_up_what_it_is_given_bounded_or_not() {
    let src = with_guards(
        "fn scoped<T: Dispose>(x: T) void {\n    using let held = x {\n        println(\"bounded body\")\n    }\n}\n\
         fn anything<T>(x: T) void {\n    using let held = x {\n        println(\"unbounded body\")\n    }\n}\n\
         fn main() void {\n    scoped(Guard { name = \"g1\" })\n    anything(Guard { name = \"g2\" })\n    anything(5)\n}\n",
    );
    assert_eq!(
        run(&src),
        "bounded body\ndispose g1\nunbounded body\ndispose g2\nunbounded body\n"
    );
}

// ── A cleanup that panics ────────────────────────────────────────

#[test]
fn every_cleanup_runs_when_one_of_them_panics() {
    let src = with_guards(
        "fn leaving() int {\n    using let a = Guard { name = \"a\" }, let b = Loud { name = \"b\" }, let c = Guard { name = \"c\" } {\n        return 1\n    }\n    return 0\n}\n\
         fn main() void {\n    println(leaving())\n}\n",
    );
    let (output, result) = run_all(&src);
    assert_eq!(output, "dispose c\ndispose b\ndispose a\n");
    assert!(result.unwrap_err().contains("cleanup panic b"));
}

#[test]
fn a_panic_already_on_its_way_out_wins_over_a_panic_in_a_cleanup() {
    let src = with_guards(
        "fn main() void {\n    using let a = Loud { name = \"x\" }, let b = Loud { name = \"y\" } {\n        assert(false, \"body panic\")\n    }\n}\n",
    );
    let (output, result) = run_all(&src);
    assert_eq!(output, "dispose y\ndispose x\n");
    let err = result.unwrap_err();
    assert!(err.contains("body panic"), "{err}");
    assert!(!err.contains("cleanup panic"), "{err}");
}

#[test]
fn the_first_of_two_cleanup_panics_is_the_one_reported() {
    let src = with_guards(
        "fn main() void {\n    using let a = Loud { name = \"x\" }, let b = Loud { name = \"y\" } {\n        println(\"body\")\n    }\n}\n",
    );
    let (output, result) = run_all(&src);
    assert_eq!(output, "body\ndispose y\ndispose x\n");
    let err = result.unwrap_err();
    assert!(err.contains("cleanup panic y"), "{err}");
    assert!(!err.contains("cleanup panic x"), "{err}");
}

#[test]
fn a_cleanup_panic_replaces_a_fail_that_was_leaving_the_block() {
    let src = with_guards(
        "fn work() void! {\n    using let l = Loud { name = \"l\" } {\n        fail \"body failed\"\n    }\n}\n\
         fn main() void {\n    try {\n        work()?\n    } catch (e) {\n        println(\"caught\")\n    }\n}\n",
    );
    let (output, result) = run_all(&src);
    assert_eq!(output, "dispose l\n");
    assert!(result.unwrap_err().contains("cleanup panic l"));
}

// ── Tiers and async ──────────────────────────────────────────────

#[test]
fn using_cleans_up_in_every_tier() {
    let src = with_guards(
        "@tier(low)\nfn in_low() void {\n    using let g = Guard { name = \"low\" } { println(\"low body\") }\n}\n\
         @tier(mid)\nfn in_mid() void {\n    using let g = Guard { name = \"mid\" } { println(\"mid body\") }\n}\n\
         @tier(high)\nfn in_high() void {\n    using let g = Guard { name = \"high\" } { println(\"high body\") }\n}\n\
         fn main() void {\n    in_low()\n    in_mid()\n    in_high()\n}\n",
    );
    assert_eq!(run(&src), "low body\ndispose low\nmid body\ndispose mid\nhigh body\ndispose high\n");
}

#[test]
fn a_using_block_in_an_async_function_cleans_up_after_the_await() {
    let src = with_guards(
        "async fn work(n: int) Task<int> {\n    return n + 1\n}\n\
         async fn run() Task<int> {\n    using let g = Guard { name = \"w\" } {\n        let v = await work(1)\n        println(\"after await\")\n        return v\n    }\n    return 0\n}\n\
         async fn main() {\n    let r = await run()\n    println(r)\n}\n",
    );
    assert_eq!(run(&src), "after await\ndispose w\n2\n");
}

// ── Diagnostics ──────────────────────────────────────────────────

#[test]
fn a_dispose_that_returns_a_value_is_a_signature_mismatch() {
    let src = "struct Guard { name: string }\n\
               impl Dispose for Guard {\n    fn dispose(self) int { return 1 }\n}\n\
               fn main() void {\n    println(\"x\")\n}\n";
    assert_eq!(codes(src).1, vec!["TYPE-125"]);
}

#[test]
fn an_impl_of_dispose_without_the_method_is_reported_once() {
    let src = "struct Guard { name: string }\n\
               impl Dispose for Guard {\n}\n\
               fn main() void {\n    println(\"x\")\n}\n";
    assert_eq!(codes(src).1, vec!["TYPE-123"]);
}

#[test]
fn a_dispose_body_is_low_tier_code() {
    let src = "struct Guard { name: string }\n\
               impl Dispose for Guard {\n    fn dispose(self) void {\n        let l = List.new()\n        l.push(1)\n    }\n}\n\
               fn main() void {\n    println(\"x\")\n}\n";
    assert_eq!(codes(src).1, vec!["TIER-009"]);
}

#[test]
fn dispose_is_not_a_derive_trait() {
    let src = "@derive(Dispose)\nstruct G { n: int }\nfn main() void {\n    println(\"x\")\n}\n";
    assert_eq!(codes(src).1, vec!["TYPE-116"]);
}

#[test]
fn using_still_accepts_a_value_of_any_type() {
    let src = "fn main() void {\n    using let a = 5, let b = [1, 2], let c = \"s\" {\n        println(\"body\")\n    }\n}\n";
    assert!(is_clean(src));
}

#[test]
fn a_dispose_impl_can_be_called_by_hand_and_through_a_bound() {
    let src = with_guards(
        "fn close<T: Dispose>(x: T) void {\n    x.dispose()\n}\n\
         fn main() void {\n    let g = Guard { name = \"manual\" }\n    g.dispose()\n    close(Guard { name = \"bound\" })\n}\n",
    );
    assert_eq!(run(&src), "dispose manual\ndispose bound\n");
}
