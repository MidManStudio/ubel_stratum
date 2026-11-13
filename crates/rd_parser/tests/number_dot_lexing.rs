//! A float literal needs a digit after the dot.
//!
//! The lexer read `7.` as a float, so `7.double()` was the float `7.` followed
//! by a name, and `0..3` was `0.` then `.3`: a range needed spaces around `..`.
//! A `.` after an integer is now part of the number only when a digit follows
//! it, so `7.double()` is a method call on the integer `7` and `0..3` is a range.
//!
//! The float forms that have a digit after the dot (`1.5`, `1_000.5`, `2.5e3`,
//! `1.5f`) lex as before. `3.` and `3.f` no longer are floats: write `3.0` and
//! `3.0f`.

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

fn main_with(body: &str) -> String {
    format!("fn main() void {{\n{body}\n}}\n")
}

const DOUBLE: &str = "extend int {\n    fn double(self) int { return self * 2 }\n}\n";

// ── An integer followed by `.` and a name or another `.` ─────────────────────

#[test]
fn a_method_on_an_integer_literal() {
    let src = format!("{DOUBLE}{}", main_with("    println(7.double())"));
    assert_eq!(out(&src), "14\n");
}

#[test]
fn a_method_on_an_integer_literal_with_separators() {
    let src = format!("{DOUBLE}{}", main_with("    println(1_000.double())"));
    assert_eq!(out(&src), "2000\n");
}

#[test]
fn a_range_without_spaces() {
    let src = main_with("    let mut t = 0\n    for i in 0..4 { t += i }\n    println(t)");
    assert_eq!(out(&src), "6\n");
}

#[test]
fn an_inclusive_range_without_spaces() {
    let src = main_with("    let mut t = 0\n    for i in 1..=3 { t += i }\n    println(t)");
    assert_eq!(out(&src), "6\n");
}

#[test]
fn a_range_with_spaces_is_unchanged() {
    let src = main_with("    let mut t = 0\n    for i in 0 .. 4 { t += i }\n    println(t)");
    assert_eq!(out(&src), "6\n");
}

#[test]
fn a_range_between_variables_and_literals() {
    let src = main_with("    let n = 3\n    let mut t = 0\n    for i in 1..n { t += i }\n    for j in n..5 { t += j }\n    println(t)");
    assert_eq!(out(&src), "10\n");
}

// ── Floats keep working ──────────────────────────────────────────────────────

#[test]
fn floats_with_a_fraction_lex_as_before() {
    let src = main_with("    println(1.5)\n    println(1_000.5)\n    println(2.5e3)\n    println(0.25)");
    assert_eq!(out(&src), "1.5\n1000.5\n2500\n0.25\n");
}

#[test]
fn a_float32_literal_with_a_suffix() {
    let src = main_with("    let f: float = 1.5f\n    println(f + 1.0f)");
    assert_eq!(out(&src), "2.5\n");
}

#[test]
fn a_float_beside_a_range_dot() {
    // `1.5` then `..`: the first dot has a digit after it, the range dots do not.
    let src = main_with("    let x = 1.5\n    let mut t = 0\n    for i in 0..2 { t += i }\n    println(x)\n    println(t)");
    assert_eq!(out(&src), "1.5\n1\n");
}

#[test]
fn a_float_method_call_is_still_a_float() {
    let src = main_with("    let x = 2.5\n    println(x * 2.0)");
    assert_eq!(out(&src), "5\n");
}

// ── What no longer lexes as a float ──────────────────────────────────────────

#[test]
fn a_trailing_dot_is_not_a_float() {
    // `3.` is the integer 3 and a dot, which does not parse. Write `3.0`.
    let (stage, c) = codes(&main_with("    let x = 3."));
    assert!(matches!(stage, Stage::Parse), "{stage:?} {c:?}");
}

#[test]
fn a_trailing_dot_before_a_name_is_a_method_call() {
    // As in Rust: `3.` then a name on the next line is `3.println(x)`, so `x`
    // is not defined. It is an error, not a silent change of value.
    let (stage, c) = codes(&main_with("    let x = 3.\n    println(x)"));
    assert!(matches!(stage, Stage::Sema), "{stage:?} {c:?}");
    assert!(c.contains(&"NAME-001"), "{c:?}");
}

#[test]
fn the_integer_is_a_token_of_its_own() {
    let tokens = ubel_stratum::lexer::tokenize("7.double()").expect("lexes");
    let rendered: Vec<String> = tokens.iter().map(|t| format!("{:?}", t.kind)).collect();
    assert!(rendered[0].starts_with("IntLit"), "{rendered:?}");
    assert!(rendered[1].contains("Dot"), "{rendered:?}");
}

#[test]
fn a_range_is_three_tokens() {
    let tokens = ubel_stratum::lexer::tokenize("0..3").expect("lexes");
    let rendered: Vec<String> = tokens.iter().map(|t| format!("{:?}", t.kind)).collect();
    assert!(rendered[0].starts_with("IntLit"), "{rendered:?}");
    assert!(rendered[1].contains("DotDot"), "{rendered:?}");
    assert!(rendered[2].starts_with("IntLit"), "{rendered:?}");
}
