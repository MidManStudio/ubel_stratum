//! An expression whose error was already reported, used anywhere.
//!
//! `TypeId::ERROR` marks a type that has been reported (`*5` is `TYPE-114`,
//! `await 5` is `TYPE-106`) so that nothing built on it reports again. It is not
//! an index into the type table, and `TypeTable::get` indexed with it, so every
//! consumer that forgot to test for it panicked with "index out of bounds": as
//! the receiver of a method call, the callee, the indexed value, the scrutinee
//! of a `match`, the value of an assignment. `get` now reads the sentinel as an
//! unknown type, which every consumer already handles without a diagnostic.
//!
//! Each position below holds one such expression and must produce exactly one
//! diagnostic, the root cause, and no panic.
//!
//! See docs/PARKED_IDEAS.md (the entry this closes) and `TypeTable::get`.

use ubel_stratum_rd::{check_source, Stage};

/// Positions in `main`. `{e}` is the failed expression.
const POSITIONS: &[(&str, &str)] = &[
    ("a typed let",            "let x: int = {e}"),
    ("the left of +",          "let x = {e} + 1"),
    ("the right of +",         "let x = 1 + {e}"),
    ("a comparison",           "let x = {e} < 2"),
    ("an and",                 "let x = {e} and true"),
    ("a negation",             "let x = -{e}"),
    ("a not",                  "let x = !{e}"),
    ("an if condition",        "if {e} { println(1) }"),
    ("a while condition",      "while {e} { println(1) }"),
    ("a for iterable",         "for i in {e} { println(i) }"),
    ("a lambda body",          "let f = fn(a: int) {e}"),
    ("a call argument",        "println({e})"),
    ("a method argument",      "let l: List<int> = List.new()\n    l.push({e})"),
    ("an index",               "let l = [1]\n    println(l[{e}])"),
    ("an index assignment",    "let mut l = [1]\n    l[{e}] = 1"),
    ("a field access",         "let x = ({e}).field"),
    ("a struct field value",   "let p = P { x = {e} }"),
    ("a tuple element",        "let t = ({e}, 1)\n    println(t[0])"),
    ("an array element",       "let a = [{e}, 1]"),
    ("a compound assignment",  "let mut x = 1\n    x += {e}"),
    ("a cast",                 "let x = {e} as int"),
    ("a method receiver",      "let x = ({e}).len()"),
    ("a callee",               "let x = ({e})(1)"),
    ("a match scrutinee",      "match {e} { _ => { println(1) }, }"),
    ("a dictionary value",     "let d: Dictionary<string, int> = Dictionary.new()\n    d.set(\"a\", {e})"),
    ("a pipe",                 "let x = {e} |> id"),
    ("a try",                  "let x = ({e})?"),
    ("a borrow",               "let x = &({e})"),
    ("an assignment value",    "let mut x = 1\n    x = {e}"),
];

/// The failed expressions and the code each one reports.
const FAILED: &[(&str, &str)] = &[
    ("*5",       "TYPE-114"),
    ("await 5",  "TYPE-106"),
];

fn program(position: &str, expr: &str) -> String {
    let body = position.replace("{e}", expr);
    format!("struct P {{ x: int }}\nfn id(a: int) int {{ return a }}\nfn main() void {{\n    {body}\n}}\n")
}

#[test]
fn every_position_reports_the_root_cause_once() {
    for (expr, code) in FAILED {
        for (name, position) in POSITIONS {
            let src = program(position, expr);
            let report = check_source(&src);
            let codes: Vec<&str> = report.diagnostics.iter().map(|d| d.code).collect();
            assert!(matches!(report.stage, Stage::Sema), "{name} / {expr}: {:?} {codes:?}\n{src}", report.stage);
            assert_eq!(codes, vec![*code], "{name} / {expr}\n{src}");
        }
    }
}

#[test]
fn the_receiver_of_a_method_call() {
    // The case the docs named first.
    let report = check_source("fn main() void {\n    let t = (*5).go()\n}\n");
    assert_eq!(report.diagnostics.len(), 1);
    assert_eq!(report.diagnostics[0].code, "TYPE-114");
}

#[test]
fn a_task_receiver_after_await() {
    let report = check_source("fn main() void {\n    let t = await 5\n    t.go()\n}\n");
    assert_eq!(report.diagnostics.len(), 1);
    assert_eq!(report.diagnostics[0].code, "TYPE-106");
}

#[test]
fn a_failed_operand_does_not_hide_a_real_error_beside_it() {
    // One unknown type does not stop the checker: the unrelated error is found.
    let report = check_source("fn main() void {\n    let a = *5\n    let b: int = \"x\"\n}\n");
    let codes: Vec<&str> = report.diagnostics.iter().map(|d| d.code).collect();
    assert!(codes.contains(&"TYPE-114") && codes.contains(&"TYPE-101"), "{codes:?}");
}

#[test]
fn a_function_that_returns_a_failed_expression() {
    let report = check_source("fn f() int { return *5 }\nfn main() void {\n    println(f())\n}\n");
    let codes: Vec<&str> = report.diagnostics.iter().map(|d| d.code).collect();
    assert_eq!(codes, vec!["TYPE-114"], "{codes:?}");
}
