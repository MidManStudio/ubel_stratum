//! Array literals handed to a written `dyn` element type.
//!
//! S6 coerced the elements of an array literal one by one only on the right of
//! a `let` with a written type. Anywhere else the literal was typed by its own
//! first element, so `total([&a, &b])` with two different shapes failed with a
//! confusing `expected Circle, found Square`. The same hint is now armed at the
//! other typed sites that know the type before the literal is inferred:
//!
//! - an argument of a plain function (the callee's parameter types are known),
//! - a `return`,
//! - a field of a struct literal, and a field of an enum struct variant,
//! - an element of an outer array literal (`List<List<&dyn Shape>>`).
//!
//! Not covered: an argument of a method call (the method is resolved only after
//! its arguments are inferred), the tail expression of a function body, a tuple
//! variant payload, and a generic callee (its parameter types still hold
//! placeholders when the arguments are inferred).
//!
//! See docs/TRAITS_DESIGN.md, "What S6 settled", and docs/PARSER_RULES.md.

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

const SHAPES: &str = r#"
trait Shape { fn area(self) int }
struct Sq { s: int }
struct Ci { r: int }
struct Tri { h: int }
impl Shape for Sq { fn area(self) int { return self.s * self.s } }
impl Shape for Ci { fn area(self) int { return 3 * self.r * self.r } }
"#;

fn program(rest: &str) -> String {
    format!("{SHAPES}\n{rest}\n")
}

// ── The sites that now coerce a literal ──────────────────────────────────────

#[test]
fn an_argument_of_a_plain_function() {
    let src = program(r#"
fn total(xs: List<&dyn Shape>) int {
    let mut t = 0
    for x in xs { t += x.area() }
    return t
}
fn main() void {
    let a = Sq { s = 2 }
    let b = Ci { r = 1 }
    println(total([&a, &b]))
}"#);
    assert_eq!(out(&src), "7\n");
}

#[test]
fn a_named_argument() {
    let src = program(r#"
fn total(xs: List<&dyn Shape>) int { return xs.len() }
fn main() void {
    let a = Sq { s = 2 }
    let b = Ci { r = 1 }
    println(total(xs = [&a, &b]))
}"#);
    assert_eq!(out(&src), "2\n");
}

#[test]
fn the_second_argument() {
    let src = program(r#"
fn count(n: int, xs: List<&dyn Shape>) int { return n + xs.len() }
fn main() void {
    let a = Sq { s = 2 }
    let b = Ci { r = 1 }
    println(count(10, [&a, &b]))
}"#);
    assert_eq!(out(&src), "12\n");
}

#[test]
fn a_return() {
    let src = program(r#"
fn make(a: &Sq, b: &Ci) List<&dyn Shape> { return [a, b] }
fn main() void {
    let a = Sq { s = 2 }
    let b = Ci { r = 1 }
    let xs = make(&a, &b)
    let mut t = 0
    for x in xs { t += x.area() }
    println(t)
}"#);
    assert_eq!(out(&src), "7\n");
}

#[test]
fn a_return_inside_a_lambda_keeps_its_own_inference() {
    // The lambda's return type is still a variable; no hint applies.
    let src = program(r#"
fn main() void {
    let f = fn(x: int) x + 1
    println(f(1))
}"#);
    assert_eq!(out(&src), "2\n");
}

#[test]
fn a_struct_field() {
    let src = program(r#"
struct Holder { items: List<&dyn Shape> }
fn main() void {
    let a = Sq { s = 2 }
    let b = Ci { r = 1 }
    let h = Holder { items = [&a, &b] }
    let mut t = 0
    for x in h.items { t += x.area() }
    println(t)
}"#);
    assert_eq!(out(&src), "7\n");
}

#[test]
fn a_field_of_an_enum_struct_variant() {
    let src = program(r#"
enum Bag { Full { items: List<&dyn Shape> }, Empty }
fn main() void {
    let a = Sq { s = 2 }
    let b = Ci { r = 1 }
    let g = Bag.Full { items = [&a, &b] }
    println(1)
}"#);
    assert_eq!(out(&src), "1\n");
}

#[test]
fn a_nested_literal() {
    let src = program(r#"
fn main() void {
    let a = Sq { s = 2 }
    let b = Ci { r = 1 }
    let rows: List<List<&dyn Shape>> = [[&a, &b], [&b]]
    let mut t = 0
    for row in rows { for x in row { t += x.area() } }
    println(t)
}"#);
    assert_eq!(out(&src), "10\n");
}

// ── What it must not change ──────────────────────────────────────────────────

#[test]
fn a_missing_impl_in_the_literal_is_one_clear_error() {
    // Before: `expected Sq, found Tri`. The element is coerced, so the missing
    // impl is what is reported.
    let src = program(r#"
fn total(xs: List<&dyn Shape>) int { return xs.len() }
fn main() void {
    let a = Sq { s = 2 }
    let t = Tri { h = 1 }
    println(total([&a, &t]))
}"#);
    let (stage, c) = codes(&src);
    assert!(matches!(stage, Stage::Sema), "{stage:?} {c:?}");
    assert_eq!(c, vec!["TYPE-126"], "{:?}", messages(&src));
}

#[test]
fn a_literal_of_a_plain_type_is_inferred_as_before() {
    let src = r#"
fn total(xs: List<int>) int {
    let mut t = 0
    for x in xs { t += x }
    return t
}
fn main() void {
    println(total([1, 2, 3]))
}"#;
    assert_eq!(out(src), "6\n");
}

#[test]
fn mixed_element_types_in_a_plain_list_are_still_an_error() {
    let src = "fn total(xs: List<int>) int { return xs.len() }\nfn main() void {\n    println(total([1, \"a\"]))\n}\n";
    let (stage, c) = codes(src);
    assert!(matches!(stage, Stage::Sema), "{stage:?} {c:?}");
    assert_eq!(c, vec!["TYPE-101"], "{:?}", messages(src));
}

#[test]
fn a_generic_callee_is_inferred_as_before() {
    let src = r#"
fn first<T>(xs: List<T>) T { return xs[0] }
fn main() void {
    println(first([4, 5]))
}"#;
    assert_eq!(out(src), "4\n");
}

#[test]
fn a_generic_callee_whose_parameter_has_no_placeholder_is_hinted() {
    // `xs` is a plain `List<&dyn Shape>`; only `t` is generic.
    let src = program(r#"
fn tagged<T>(t: T, xs: List<&dyn Shape>) int { return xs.len() }
fn main() void {
    let a = Sq { s = 2 }
    let b = Ci { r = 1 }
    println(tagged(1, [&a, &b]))
}"#);
    assert_eq!(out(&src), "2\n");
}

#[test]
fn an_argument_that_is_not_a_literal_is_untouched() {
    let src = program(r#"
fn total(xs: List<&dyn Shape>) int { return xs.len() }
fn main() void {
    let a = Sq { s = 2 }
    let xs: List<&dyn Shape> = [&a]
    println(total(xs))
}"#);
    assert_eq!(out(&src), "1\n");
}

#[test]
fn a_hint_armed_for_a_non_literal_does_not_reach_a_later_literal() {
    // `total(held)` is not a literal. If a hint were armed for it anyway, the
    // next literal in the program, `[1, 2, 3]`, would take it.
    let src = program(r#"
fn total(xs: List<&dyn Shape>) int { return xs.len() }
fn main() void {
    let a = Sq { s = 2 }
    let held: List<&dyn Shape> = [&a]
    println(total(held))
    let ints = [1, 2, 3]
    println(ints.len())
}"#);
    assert_eq!(out(&src), "1\n3\n");
}

#[test]
fn an_empty_literal_takes_the_written_type() {
    let src = program(r#"
fn total(xs: List<&dyn Shape>) int { return xs.len() }
fn main() void {
    println(total([]))
}"#);
    assert_eq!(out(&src), "0\n");
}
