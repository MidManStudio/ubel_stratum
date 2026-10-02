//! `type` aliases are transparent: `Score` from `type Score = int` IS `int`.
//!
//! The `.ubl` fixtures show whole programs accepted or rejected. These
//! tests pin the finer contract: exact codes and counts, that a cycle is
//! reported once per alias in it (and an unrelated alias is not), that an
//! alias works in expression and pattern position and not only in type
//! annotations, and that the VALUES built through an alias carry the real
//! type name, which is what lets a method written on the real struct find
//! them.
//!
//! See docs/PARKED_IDEAS.md, "Type aliases are transparent".

use ubel_stratum::ast::arena::AstArena;
use ubel_stratum::interpreter::Interpreter;
use ubel_stratum_rd::{check_source, Stage};

/// The stage a program stops at and the codes it reports, in order.
fn codes(source: &str) -> (Stage, Vec<&'static str>) {
    let report = check_source(source);
    let codes = report.diagnostics.iter().map(|d| d.code).collect();
    (report.stage, codes)
}

/// Run a program that passes `check_source` and return what it printed.
fn run(source: &str) -> Result<String, String> {
    assert!(matches!(check_source(source).stage, Stage::Clean), "program should check clean");
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

// ── Transparency in type position ────────────────────────────────

#[test]
fn an_alias_and_its_target_interchange_in_both_directions() {
    let src = "type Score = int\nfn main() void {\n    let a: Score = 5\n    let b: int = a\n    let c: Score = b\n    println(c)\n}\n";
    assert_eq!(codes(src), (Stage::Clean, vec![]));
}

#[test]
fn an_alias_to_a_collection_is_that_collection() {
    let src = "type Names = List<string>\nfn main() void {\n    let n: Names = List.new()\n    n.push(\"x\")\n    println(n.len())\n}\n";
    assert_eq!(run(src).unwrap(), "1\n");
}

#[test]
fn an_alias_of_an_alias_resolves_through_both() {
    let src = "type A = int\ntype B = A\ntype C = B\nfn main() void {\n    let x: C = 7\n    let y: int = x\n    println(y)\n}\n";
    assert_eq!(run(src).unwrap(), "7\n");
}

#[test]
fn an_alias_declared_after_its_use_still_resolves() {
    let src = "fn f(v: Late) Late { return v }\ntype Late = Early\ntype Early = int\n\
               fn main() void { println(f(3)) }\n";
    assert_eq!(run(src).unwrap(), "3\n");
}

#[test]
fn a_generic_alias_substitutes_its_arguments() {
    let src = "struct Box<T> { value: T }\ntype Wrapped<T> = Box<T>\n\
               fn main() void {\n    let w: Wrapped<string> = Box { value = \"s\" }\n    println(w.value)\n}\n";
    assert_eq!(run(src).unwrap(), "s\n");
}

#[test]
fn a_generic_alias_with_two_parameters_keeps_them_in_order() {
    let src = "type Pairs<K, V> = Dictionary<K, V>\n\
               fn main() void {\n    let p: Pairs<string, int> = Dictionary.new()\n    println(p.len())\n}\n";
    assert_eq!(run(src).unwrap(), "0\n");
}

#[test]
fn an_alias_to_a_function_type_is_callable() {
    let src = "type Handler = fn(int) int\nfn apply(h: Handler, n: int) int { return h(n) }\n\
               fn main() void { println(apply(fn(x: int) x * 3, 4)) }\n";
    assert_eq!(run(src).unwrap(), "12\n");
}

#[test]
fn an_aliased_integer_type_types_a_bare_literal() {
    // `Byte` is `u8`, so the literal is a u8 and the sum wraps at 8 bits.
    let src = "type Byte = u8\nfn main() void {\n    let b: Byte = 250\n    println(b + 10)\n}\n";
    assert_eq!(run(src).unwrap(), "4\n");
}

#[test]
fn a_mismatch_against_an_alias_names_the_target_type() {
    let src = "type Score = int\nfn main() void { let s: Score = \"text\" }\n";
    let report = check_source(src);
    assert_eq!(report.diagnostics.len(), 1);
    assert_eq!(report.diagnostics[0].code, "TYPE-101");
    assert!(report.diagnostics[0].message.contains("int"), "got: {}", report.diagnostics[0].message);
}

// ── Cycles (TYPE-121) and arity (TYPE-108) ───────────────────────

#[test]
fn a_self_referential_alias_is_one_type_121() {
    assert_eq!(codes("type A = A\nfn main() void { }\n"), (Stage::Sema, vec!["TYPE-121"]));
}

#[test]
fn a_cycle_through_two_aliases_reports_each_alias_once() {
    let src = "type B = C\ntype C = B\nfn main() void { }\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["TYPE-121", "TYPE-121"]));
}

#[test]
fn an_unrelated_alias_beside_a_cycle_is_not_reported() {
    let src = "type A = A\ntype D = int\nfn main() void { let x: D = 1 }\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["TYPE-121"]));
}

#[test]
fn a_cycle_does_not_cascade_into_errors_at_its_uses() {
    // `x: A` would be a mismatch against anything if A expanded to a real
    // type; it expands to unknown, so only the cycle itself is reported.
    let src = "type A = A\nfn main() void {\n    let x: A = 5\n    let y: A = \"s\"\n}\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["TYPE-121"]));
}

#[test]
fn too_many_too_few_and_a_wrong_count_inside_a_target_are_three_errors() {
    let src = "struct Box<T> { v: T }\ntype Pair<T> = List<T>\n\
               type TooMany = Pair<int, int>\ntype TooFew = Pair\ntype Worse = Box<int, int>\n\
               fn main() void { }\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["TYPE-108", "TYPE-108", "TYPE-108"]));
}

// ── Expression and pattern position ──────────────────────────────

#[test]
fn a_struct_literal_written_with_an_alias_builds_the_real_struct() {
    // The method is written on `Point`; it only finds a value built as
    // `P { .. }` if that value carries the real type name.
    let src = "struct Point { x: int, y: int }\ntype P = Point\n\
               extend Point { fn sum(self) int { return self.x + self.y } }\n\
               fn main() void {\n    let q = P { x = 3, y = 4 }\n    println(q.sum())\n}\n";
    assert_eq!(run(src).unwrap(), "7\n");
}

#[test]
fn an_associated_function_called_through_an_alias_works() {
    let src = "struct Point { x: int, y: int }\ntype P = Point\n\
               extend Point { fn origin() Point { return Point { x = 0, y = 0 } } }\n\
               fn main() void {\n    let o = P.origin()\n    println(o.x)\n}\n";
    assert_eq!(run(src).unwrap(), "0\n");
}

#[test]
fn an_alias_of_an_alias_works_in_a_struct_literal() {
    let src = "struct Point { x: int, y: int }\ntype P = Point\ntype Q = P\n\
               fn main() void {\n    let q = Q { x = 5, y = 6 }\n    println(q.x + q.y)\n}\n";
    assert_eq!(run(src).unwrap(), "11\n");
}

#[test]
fn enum_variants_of_every_shape_construct_through_an_alias() {
    let src = "enum Color { Red, Rgb(int, int, int), Named { label: string } }\ntype C = Color\n\
               fn name(c: Color) string {\n    match c {\n        Color.Red => { return \"red\" }\n\
               Color.Rgb(r, g, b) => { return \"rgb\" }\n        Color.Named { label } => { return label }\n    }\n}\n\
               fn main() void {\n    println(name(C.Red))\n    println(name(C.Rgb(1, 2, 3)))\n\
               println(name(C.Named { label = \"teal\" }))\n}\n";
    assert_eq!(run(src).unwrap(), "red\nrgb\nteal\n");
}

#[test]
fn an_enum_pattern_written_with_an_alias_matches() {
    let src = "enum Color { Red, Green }\ntype C = Color\n\
               fn name(c: C) string {\n    match c {\n        C.Red => { return \"red\" }\n        _ => { return \"other\" }\n    }\n}\n\
               fn main() void {\n    println(name(Color.Red))\n    println(name(C.Red))\n    println(name(C.Green))\n}\n";
    assert_eq!(run(src).unwrap(), "red\nred\nother\n");
}

#[test]
fn a_struct_pattern_written_with_an_alias_matches_the_real_struct() {
    let src = "struct Point { x: int, y: int }\ntype P = Point\n\
               fn corner(p: Point) string {\n    match p {\n        P { x, y } => { return $\"{x},{y}\" }\n    }\n}\n\
               fn main() void { println(corner(Point { x = 3, y = 4 })) }\n";
    assert_eq!(run(src).unwrap(), "3,4\n");
}
