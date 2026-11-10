//! The pipe operator `|>`.
//!
//! The parser lowers `left |> stage` to the call it stands for, so every later
//! pass sees an ordinary call:
//!
//! ```text
//! x |> f         =>  f(x)
//! x |> f(a, b)   =>  f(x, a, b)      the piped value is the first argument
//! x |> f?        =>  (f(x))?
//! x |> f(a)?     =>  (f(x, a))?
//! ```
//!
//! Before the lowering sema typed `x |> f` as `f` itself (the function type, not
//! its result) and reported an argument-count error for `x |> f(a)` because it
//! checked `f(a)` as a call of its own. Move and borrow checking never saw the
//! piped value as an argument at all.
//!
//! See docs/PARSER_RULES.md, section 5.13.

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

fn run(source: &str) -> Result<String, String> {
    assert!(matches!(check_source(source).stage, Stage::Clean), "program should check clean: {:?}\n{source}", messages(source));
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
    let result = interp.run_program(&program);
    let output = ubel_stratum::builtins::global::io::take_captured_output();
    result.map(|_| output)
}

fn out(source: &str) -> String {
    run(source).unwrap_or_else(|e| panic!("run failed: {e}\n{source}"))
}

fn one_sema(source: &str, code: &str) -> Vec<String> {
    let (stage, c) = codes(source);
    assert!(matches!(stage, Stage::Sema), "{stage:?} {c:?}\n{source}");
    assert_eq!(c, vec![code], "{:?}\n{source}", messages(source));
    messages(source)
}

const FNS: &str = r#"
fn double(x: int) int { return x * 2 }
fn add(x: int, y: int) int { return x + y }
fn sub(x: int, y: int) int { return x - y }
fn half(x: int) int! {
    if x % 2 != 0 { fail "odd" }
    return x / 2
}
"#;

fn with_main(body: &str) -> String {
    format!("{FNS}\nfn main() void {{\n{body}\n}}\n")
}

// ── The stage forms ──────────────────────────────────────────────────────────

#[test]
fn a_bare_function_is_called_with_the_piped_value() {
    assert_eq!(out(&with_main("    println(5 |> double)")), "10\n");
}

#[test]
fn a_call_with_no_arguments_gets_the_piped_value() {
    assert_eq!(out(&with_main("    println(5 |> double())")), "10\n");
}

#[test]
fn the_piped_value_is_the_first_argument() {
    // sub(10, 3), not sub(3, 10)
    assert_eq!(out(&with_main("    println(10 |> sub(3))")), "7\n");
}

#[test]
fn a_chain_runs_left_to_right() {
    assert_eq!(out(&with_main("    println(5 |> double |> double)")), "20\n");
}

#[test]
fn a_chain_of_mixed_stages() {
    assert_eq!(out(&with_main("    println(5 |> add(1) |> double |> sub(2))")), "10\n");
}

#[test]
fn a_lambda_stage() {
    assert_eq!(out(&with_main("    println(5 |> fn(x: int) x + 1)")), "6\n");
}

#[test]
fn the_piped_value_may_be_any_expression() {
    assert_eq!(out(&with_main("    let a = 2\n    println((a + 3) |> double)")), "10\n");
}

#[test]
fn a_pipe_as_a_let_value_and_as_an_argument() {
    let body = "    let a = 4 |> double\n    println(add(a, 1 |> double))";
    assert_eq!(out(&with_main(body)), "10\n");
}

// ── Precedence ───────────────────────────────────────────────────────────────

#[test]
fn arithmetic_binds_tighter_than_the_pipe() {
    // (1 + 2) |> double
    assert_eq!(out(&with_main("    println(1 + 2 |> double)")), "6\n");
}

#[test]
fn comparison_binds_looser_than_the_pipe() {
    // (5 |> double) == 10
    assert_eq!(out(&with_main("    println(5 |> double == 10)")), "true\n");
}

// ── Types ────────────────────────────────────────────────────────────────────

#[test]
fn the_result_has_the_return_type_of_the_stage() {
    // The result is `int`, not the function type `fn(int) int` it was.
    let m = one_sema(&with_main("    let a: string = 5 |> double\n    println(a)"), "TYPE-101");
    assert!(m[0].contains("expected `string`, found `int`"), "{m:?}");
}

#[test]
fn the_piped_value_is_checked_against_the_first_parameter() {
    let m = one_sema(&with_main("    println(\"a\" |> double)"), "TYPE-101");
    assert!(m[0].contains("expected `int`, found `string`"), "{m:?}");
}

#[test]
fn a_stage_that_needs_more_arguments_is_an_arity_error() {
    // add(5) has one argument, add wants two.
    let m = one_sema(&with_main("    println(5 |> add)"), "TYPE-102");
    assert!(m[0].contains("expected 2") && m[0].contains("found 1"), "{m:?}");
}

#[test]
fn extra_arguments_count_with_the_piped_one() {
    let m = one_sema(&with_main("    println(5 |> double(1))"), "TYPE-102");
    assert!(m[0].contains("expected 1") && m[0].contains("found 2"), "{m:?}");
}

// ── `?` on a stage ───────────────────────────────────────────────────────────

#[test]
fn a_try_stage_unwraps_the_result() {
    // `8 |> half?` is `half(8)?`, an `int`, not an `int!`.
    let src = format!(
        "{FNS}\nfn run() int! {{\n    let a = 8 |> half?\n    return a + 1\n}}\nfn main() void {{\n    println(1)\n}}\n"
    );
    assert!(matches!(check_source(&src).stage, Stage::Clean), "{:?}", messages(&src));
}

#[test]
fn a_try_stage_has_the_unwrapped_type() {
    let src = format!(
        "{FNS}\nfn run() int! {{\n    let a: string = 8 |> half?\n    return 1\n}}\nfn main() void {{\n    println(1)\n}}\n"
    );
    let m = one_sema(&src, "TYPE-101");
    assert!(m[0].contains("expected `string`, found `int`"), "{m:?}");
}

#[test]
fn a_try_stage_with_arguments_in_a_chain() {
    let src = format!(
        "{FNS}\nfn run() int! {{\n    let a = 8 |> half? |> add(1) |> half?\n    return a\n}}\nfn main() void {{\n    println(1)\n}}\n"
    );
    assert!(matches!(check_source(&src).stage, Stage::Clean), "{:?}", messages(&src));
}

#[test]
fn a_call_stage_with_try_is_the_call_then_the_try() {
    // `x |> f(a)?` is `(f(x, a))?`; the result is unwrapped once, not twice.
    let src = format!(
        "{FNS}\nfn halve_by(x: int, d: int) int! {{\n    return x / d\n}}\nfn run() int! {{\n    let a: string = 8 |> halve_by(2)?\n    return 1\n}}\nfn main() void {{\n    println(1)\n}}\n"
    );
    let m = one_sema(&src, "TYPE-101");
    assert!(m[0].contains("expected `string`, found `int`"), "{m:?}");
}

// ── A stage that is a method, a static function or a variant ─────────────────

const ACC: &str = r#"
trait Plus { fn plus(self, x: int) int }
struct Acc { n: int }
impl Plus for Acc { fn plus(self, x: int) int { return self.n + x } }
extend Acc {
    fn times(self, x: int) int { return self.n * x }
    fn mix(self, x: int, y: int) int { return self.n + x * y }
    fn make(x: int) Acc { return Acc { n = x } }
}
enum Slot { Full(int), Empty }
"#;

fn acc_main(body: &str) -> String {
    format!("{ACC}\nfn main() void {{\n    let a = Acc {{ n = 10 }}\n{body}\n}}\n")
}

#[test]
fn a_method_is_a_stage() {
    assert_eq!(out(&acc_main("    println(5 |> a.times)")), "50\n");
}

#[test]
fn a_trait_method_is_a_stage() {
    assert_eq!(out(&acc_main("    println(5 |> a.plus)")), "15\n");
}

#[test]
fn a_method_stage_gets_the_piped_value_before_its_other_arguments() {
    // a.mix(5, 2) = 10 + 5 * 2
    assert_eq!(out(&acc_main("    println(5 |> a.mix(2))")), "20\n");
}

#[test]
fn methods_chain() {
    assert_eq!(out(&acc_main("    println(5 |> a.plus |> a.times)")), "150\n");
}

#[test]
fn a_static_function_is_a_stage() {
    assert_eq!(out(&acc_main("    println((7 |> Acc.make).n)")), "7\n");
}

#[test]
fn an_enum_variant_is_a_stage() {
    let body = "    let s = 3 |> Slot.Full\n    match s {\n        Slot.Full(n) => { println(n) },\n        Slot.Empty => { println(0) },\n    }";
    assert_eq!(out(&acc_main(body)), "3\n");
}

#[test]
fn a_method_stage_is_called_like_the_same_method_called_directly() {
    // Same tier rule as `a.plus(5)`: a `LOW` function cannot call a `HIGH` method.
    let src = r#"
struct Acc { n: int }
extend Acc {
    @tier(high) fn plus(self, x: int) int { return self.n + x }
}
@tier(low)
fn go(a: Acc) int { return 5 |> a.plus }
fn main() void { println(1) }
"#;
    let m = one_sema(src, "TIER-005");
    assert!(m[0].contains("plus"), "{m:?}");
}

#[test]
fn a_dotted_name_inside_an_argument_is_still_a_path() {
    // Only the head of the stage is read as a callee; `a.n` in an argument is a field read.
    assert_eq!(out(&acc_main("    println(1 |> a.mix(a.n))")), "20\n");
}

#[test]
fn a_method_stage_may_end_at_a_closing_parenthesis() {
    // The stage head ends with its own segment; the `)` belongs to the group.
    assert_eq!(out(&acc_main("    println((5 |> a.times))")), "50\n");
}

#[test]
fn a_method_stage_may_be_followed_by_a_comparison() {
    assert_eq!(out(&acc_main("    println(5 |> a.times == 50)")), "true\n");
}

#[test]
fn an_unknown_method_stage_is_reported_at_the_stage_alone() {
    // `a.nope(5)` is TYPE-104; the stage form is the same error. The span is
    // the stage head, not the token after it.
    let src = acc_main("    println((5 |> a.nope))");
    let report = check_source(&src);
    assert_eq!(report.diagnostics.len(), 1, "{:?}", messages(&src));
    let d = &report.diagnostics[0];
    assert_eq!(d.code, "TYPE-104");
    let (start, end) = (d.primary_span.start as usize, d.primary_span.end as usize);
    assert_eq!(&src[start..end], "a.nope");
}

#[test]
fn an_unknown_field_stage_is_the_same_error_as_the_direct_call() {
    let staged = acc_main("    let r = 5 |> a.nope\n    println(r)");
    let direct = acc_main("    let r = a.nope(5)\n    println(r)");
    assert_eq!(codes(&staged).1, vec!["TYPE-104"]);
    assert_eq!(codes(&staged).1, codes(&direct).1);
}
