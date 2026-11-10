//! Two parser fixes.
//!
//! 1. A call whose first argument starts with `(`: `f((a + b))`, `f((1, 2))`,
//!    `v.push((a))`. The Pratt loop consumes the call's `(`; `parse_call` must
//!    not consume another, which was the argument's own.
//! 2. `return`, `break` and `or return` read a value only when the next token
//!    can start an expression (`Parser::can_start_expr`). A token missing from
//!    that list made `return 3.0 * r` a bare `return` (void) followed by a dead
//!    statement. The table below has one row per kind of expression start that
//!    `parse_prefix` accepts, so a new prefix arm without a row here shows up
//!    in review.
//!
//! See docs/PARSER_RULES.md, section 5.15.

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

// ── 1. A parenthesis that starts the first argument ──────────────────────────

#[test]
fn grouped_expression_as_the_only_argument() {
    let src = "fn main() void {\n    let a = 2\n    let b = 3\n    println((a + b))\n}\n";
    assert_eq!(out(src), "5\n");
}

#[test]
fn grouped_name_as_the_first_argument() {
    let src = "fn take(x: int) int { return x }\nfn main() void {\n    let a = 4\n    println(take((a)))\n}\n";
    assert_eq!(out(src), "4\n");
}

#[test]
fn tuple_as_the_first_argument() {
    let src = "fn first(t: (int, int)) int { return t[0] }\nfn main() void {\n    println(first((7, 8)))\n}\n";
    assert_eq!(out(src), "7\n");
}

#[test]
fn several_parentheses_deep() {
    let src = "fn take(x: int) int { return x }\nfn main() void {\n    println(take(((1 + 2))))\n}\n";
    assert_eq!(out(src), "3\n");
}

#[test]
fn grouped_first_argument_followed_by_more_arguments() {
    let src = "fn add(x: int, y: int) int { return x + y }\nfn main() void {\n    println(add((1 + 2), (3)))\n}\n";
    assert_eq!(out(src), "6\n");
}

#[test]
fn grouped_first_argument_of_a_method_call() {
    let src = "fn main() void {\n    let l: List<int> = List.new()\n    l.push((1 + 2))\n    println(l.len())\n}\n";
    assert_eq!(out(src), "1\n");
}

#[test]
fn grouped_first_argument_of_a_chained_call() {
    let src = "fn main() void {\n    let l: List<int> = List.new()\n    l.push((1))\n    l.push((2))\n    println(l.len())\n}\n";
    assert_eq!(out(src), "2\n");
}

#[test]
fn a_call_with_no_arguments() {
    let src = "fn zero() int { return 0 }\nfn main() void {\n    println(zero())\n}\n";
    assert_eq!(out(src), "0\n");
}

#[test]
fn an_unclosed_call_is_still_one_diagnostic() {
    let (stage, c) = codes("fn take(x: int) int { return x }\nfn main() void {\n    println(take((1))\n}\n");
    assert!(matches!(stage, Stage::Parse), "{stage:?} {c:?}");
    assert!(!c.is_empty() && c.iter().all(|x| x.starts_with("PARSE-")), "{c:?}");
}

// ── 2. Which tokens start a value after `return` ─────────────────────────────

/// One row: the return type, the expression written after `return`, the
/// expression `main` prints, and what it prints. `{}` in the print column is
/// the call `f()`.
struct Row {
    ty:    &'static str,
    value: &'static str,
    print: &'static str,
    out:   &'static str,
}

const ROWS: &[Row] = &[
    // literals
    Row { ty: "int",    value: "7",                      print: "{}",         out: "7" },
    Row { ty: "u32",    value: "5u32",                   print: "{}",         out: "5" },
    Row { ty: "double", value: "1.5",                    print: "{}",         out: "1.5" },
    Row { ty: "string", value: "\"hi\"",                 print: "{}",         out: "hi" },
    Row { ty: "char",   value: "'z'",                    print: "{}",         out: "z" },
    Row { ty: "bool",   value: "true",                   print: "{}",         out: "true" },
    Row { ty: "bool",   value: "false",                  print: "{}",         out: "false" },
    // a literal followed by an operator: the original bug
    Row { ty: "double", value: "3.0 * 2.0 * 2.0",        print: "{}",         out: "12" },
    Row { ty: "u32",    value: "5u32 + 1u32",            print: "{}",         out: "6" },
    // prefix operators
    Row { ty: "int",    value: "-3",                     print: "{}",         out: "-3" },
    Row { ty: "bool",   value: "!false",                 print: "{}",         out: "true" },
    Row { ty: "bool",   value: "not false",              print: "{}",         out: "true" },
    Row { ty: "int",    value: "~0",                     print: "{}",         out: "-1" },
    // grouping, arrays, blocks of control flow
    Row { ty: "int",    value: "(1 + 2)",                print: "{}",         out: "3" },
    Row { ty: "int",    value: "[1, 2, 3].len()",        print: "{}",         out: "3" },
    Row { ty: "int",    value: "if true { 1 } else { 2 }", print: "{}",       out: "1" },
    // the built-in collection names, which are keywords of their own
    Row { ty: "List<int>",               value: "List.new()",       print: "{}.len()", out: "0" },
    Row { ty: "Dictionary<string, int>", value: "Dictionary.new()", print: "{}.len()", out: "0" },
    Row { ty: "Queue<int>",              value: "Queue.new()",      print: "{}.len()", out: "0" },
    Row { ty: "Stack<int>",              value: "Stack.new()",      print: "{}.len()", out: "0" },
    Row { ty: "InlineList<int>",         value: "InlineList.new(2)", print: "{}.len()", out: "0" },
];

#[test]
fn return_reads_a_value_for_every_expression_start() {
    for row in ROWS {
        let call = "f()";
        let print = row.print.replace("{}", call);
        let src = format!(
            "fn f() {} {{ return {} }}\nfn main() void {{\n    println({})\n}}\n",
            row.ty, row.value, print
        );
        assert_eq!(out(&src), format!("{}\n", row.out), "return {}\n{src}", row.value);
    }
}

#[test]
fn a_bare_return_still_returns_nothing() {
    let src = "fn f() void {\n    return\n}\nfn main() void {\n    f()\n    println(1)\n}\n";
    assert_eq!(out(src), "1\n");
}

#[test]
fn return_self_in_a_method() {
    let src = "struct P { x: int }\nextend P {\n    fn me(self) P { return self }\n}\nfn main() void {\n    let p = P { x = 4 }\n    println(p.me().x)\n}\n";
    assert_eq!(out(src), "4\n");
}

#[test]
fn a_float_return_is_no_longer_void() {
    // Before the fix this was TYPE-101, expected `double`, found `void`.
    let src = "struct C { r: double }\nextend C {\n    fn area(self) double { return 3.0 * self.r * self.r }\n}\nfn main() void {\n    let c = C { r = 2.0 }\n    println(c.area())\n}\n";
    assert_eq!(out(src), "12\n");
}
