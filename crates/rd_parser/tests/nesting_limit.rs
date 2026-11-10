//! The nesting limit and the large stack.
//!
//! Every pass walks the tree recursively, and type inference alone needs
//! several KB of stack per level (about 15 KB in a debug build), so a long
//! chain such as `a + a + ... + a` (one level per operator, though the parser
//! builds it without recursion) overflowed the stack and aborted the process:
//! at about 68 operators on a 1 MB stack, about 526 on 8 MB. In a browser the
//! overflow runs off the start of linear memory and shows as "index out of
//! bounds".
//!
//! Two things fix it together. The parser rejects nesting deeper than
//! `MAX_NESTING` (1000) with `PARSE-006`, one diagnostic, instead of letting a
//! later pass fall over. The entry points run the pipeline on a thread with a
//! large stack (`ubel_stratum::stack`), so everything up to the limit fits.
//!
//! See docs/PARSER_RULES.md, section 5.14, and docs/DIAGNOSTICS_RULES.md.

use ubel_stratum::ast::arena::AstArena;
use ubel_stratum::interpreter::Interpreter;
use ubel_stratum_rd::{check_source, Stage, MAX_NESTING};

fn codes(source: &str) -> (Stage, Vec<&'static str>) {
    let report = check_source(source);
    let codes = report.diagnostics.iter().map(|d| d.code).collect();
    (report.stage, codes)
}

/// Runs a program that has to check clean, on the large stack, like the CLI.
fn run(source: &str) -> Result<String, String> {
    ubel_stratum::stack::with_big_stack(|| {
        let report = check_source(source);
        assert!(matches!(report.stage, Stage::Clean), "{:?}", report.diagnostics.iter().map(|d| d.message.clone()).collect::<Vec<_>>());
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
    })
}

fn chain(op: &str, n: usize) -> String {
    let terms = vec!["a"; n].join(op);
    format!("fn main() void {{\n    let a = 1\n    let s = {terms}\n    println(s)\n}}\n")
}

fn string_chain(n: usize) -> String {
    let terms = vec!["a"; n].join(" + ");
    format!("fn main() void {{\n    let a = \"x\"\n    let s = {terms}\n    println(s.len())\n}}\n")
}

fn parens(n: usize) -> String {
    format!("fn main() void {{\n    let s = {}1{}\n    println(s)\n}}\n", "(".repeat(n), ")".repeat(n))
}

fn calls(n: usize) -> String {
    format!(
        "fn id(x: int) int {{ return x }}\nfn main() void {{\n    let s = {}1{}\n    println(s)\n}}\n",
        "id(".repeat(n), ")".repeat(n)
    )
}

fn blocks(n: usize) -> String {
    format!("fn main() void {{\n{}    println(1)\n{}}}\n", "    if true {\n".repeat(n), "    }\n".repeat(n))
}

fn one_nesting_error(source: &str) {
    let (stage, c) = codes(source);
    assert!(matches!(stage, Stage::Parse), "{stage:?} {c:?}");
    assert_eq!(c, vec!["PARSE-006"], "one diagnostic, no cascade");
    let report = check_source(source);
    let d = &report.diagnostics[0];
    assert!(d.message.contains(&MAX_NESTING.to_string()), "{}", d.message);
    assert!(d.suggestion.is_some());
}

// ── Within the limit: checks and runs ────────────────────────────────────────

#[test]
fn the_limit_is_a_thousand() {
    assert_eq!(MAX_NESTING, 1000);
}

#[test]
fn a_long_string_chain_runs() {
    // The shape of the original report: hundreds of operands in one expression.
    assert_eq!(run(&string_chain(900)).unwrap(), "900\n");
}

#[test]
fn the_longest_chain_that_fits_runs() {
    // The function body and the `let` value take two levels, so 998 operands fit.
    assert_eq!(run(&chain(" + ", 998)).unwrap(), "998\n");
}

#[test]
fn one_operand_more_is_the_limit_error() {
    one_nesting_error(&chain(" + ", 999));
}

#[test]
fn a_long_integer_chain_runs() {
    assert_eq!(run(&chain(" + ", 900)).unwrap(), "900\n");
}

#[test]
fn a_long_chain_of_another_operator_runs() {
    assert_eq!(run(&chain(" * ", 900)).unwrap(), "1\n");
}

#[test]
fn deep_parentheses_run() {
    assert_eq!(run(&parens(900)).unwrap(), "1\n");
}

#[test]
fn deeply_nested_calls_run() {
    assert_eq!(run(&calls(900)).unwrap(), "1\n");
}

#[test]
fn deeply_nested_blocks_run() {
    assert_eq!(run(&blocks(900)).unwrap(), "1\n");
}

// ── Past the limit: one clean diagnostic ─────────────────────────────────────

#[test]
fn a_chain_past_the_limit_is_one_diagnostic() {
    one_nesting_error(&chain(" + ", 1100));
}

#[test]
fn a_string_chain_past_the_limit_is_one_diagnostic() {
    one_nesting_error(&string_chain(1100));
}

#[test]
fn a_very_long_chain_does_not_crash() {
    // Far past the limit: reported, not walked.
    one_nesting_error(&chain(" + ", 20_000));
}

#[test]
fn parentheses_past_the_limit_are_one_diagnostic() {
    one_nesting_error(&parens(1100));
}

#[test]
fn calls_past_the_limit_are_one_diagnostic() {
    one_nesting_error(&calls(1100));
}

#[test]
fn blocks_past_the_limit_are_one_diagnostic() {
    one_nesting_error(&blocks(1100));
}

#[test]
fn a_pipe_chain_past_the_limit_is_one_diagnostic() {
    let stages = vec!["id"; 1100].join(" |> ");
    let src = format!("fn id(x: int) int {{ return x }}\nfn main() void {{\n    let s = 1 |> {stages}\n    println(s)\n}}\n");
    one_nesting_error(&src);
}

#[test]
fn the_nesting_error_ends_the_parse() {
    // Like rustc's recursion limit, the error is fatal: what follows an
    // over-deep construct is unreliable, so a later syntax error in the same
    // file is not reported on top of it.
    let mut src = chain(" + ", 1100);
    src.push_str("fn broken( {\n");
    let (stage, c) = codes(&src);
    assert!(matches!(stage, Stage::Parse), "{stage:?} {c:?}");
    assert_eq!(c, vec!["PARSE-006"], "{c:?}");
}

#[test]
fn an_error_before_the_limit_is_still_reported_with_it() {
    // Errors found before the over-deep construct are kept.
    let mut src = String::from("fn broken( {\n}\n");
    src.push_str(&chain(" + ", 1100));
    let (stage, c) = codes(&src);
    assert!(matches!(stage, Stage::Parse), "{stage:?} {c:?}");
    assert!(c.contains(&"PARSE-006"), "{c:?}");
    assert!(c.len() >= 2, "{c:?}");
}

#[test]
fn the_nesting_diagnostic_is_a_parse_error_with_its_own_code() {
    let report = check_source(&chain(" + ", 1100));
    assert_eq!(report.diagnostics.len(), 1);
    assert_eq!(report.diagnostics[0].code, "PARSE-006");
}
