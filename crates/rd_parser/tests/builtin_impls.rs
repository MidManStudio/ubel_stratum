//! Impls and `extend` blocks for built-in types, slice S3d.
//!
//! A primitive, a tuple or an array can implement a trait and take
//! inherent methods. Sema keeps every type apart (`int`, `long`, `i64` and
//! `isize` are four types). The interpreter cannot: they are one kind of
//! value, so a trait has one impl for all of them, a tuple is told by its
//! arity and an array is one key shared with `List<T>`. A prelude trait can
//! be implemented by hand only where the built-in type lacks it natively.
//!
//! See docs/TRAITS_DESIGN.md, "What S3d settled".

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

fn positions(source: &str) -> Vec<(usize, usize)> {
    check_source(source)
        .diagnostics
        .iter()
        .map(|d| (d.primary_span.line as usize, d.primary_span.column as usize))
        .collect()
}

fn pos_of(source: &str, needle: &str) -> (usize, usize) {
    let at = source.find(needle).unwrap_or_else(|| panic!("`{needle}` not in the source"));
    let before = &source[..at];
    let line = before.matches('\n').count() + 1;
    let column = at - before.rfind('\n').map_or(0, |i| i + 1) + 1;
    (line, column)
}

fn run(source: &str) -> Result<String, String> {
    assert!(matches!(check_source(source).stage, Stage::Clean), "program should check clean: {:?}", messages(source));
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

const CLEAN: (Stage, Vec<&str>) = (Stage::Clean, Vec::new());

const SHOW: &str = "trait Show {\n    fn show(self) string\n}\n";

fn with_show(rest: &str) -> String {
    format!("{SHOW}{rest}")
}

// ── Every primitive spelling ─────────────────────────────────────

/// Each spelling of a primitive type with a literal of that type.
const PRIMITIVES: &[(&str, &str)] = &[
    ("int", "1"), ("uint", "1"), ("long", "1"), ("ulong", "1"),
    ("short", "1"), ("ushort", "1"), ("byte", "1"), ("ubyte", "1"),
    ("i8", "1"), ("i16", "1"), ("i32", "1"), ("i64", "1"),
    ("u8", "1"), ("u16", "1"), ("u32", "1"), ("u64", "1"),
    ("isize", "1"), ("usize", "1"),
    ("float", "1.5f"), ("f32", "1.5 as f32"), ("double", "1.5"), ("f64", "1.5 as f64"),
    ("bool", "true"), ("char", "'a'"), ("string", "\"s\""),
];

#[test]
fn every_primitive_spelling_takes_an_impl_and_is_called_by_its_own_value() {
    for (ty, lit) in PRIMITIVES {
        let src = with_show(&format!(
            "impl Show for {ty} {{ fn show(self) string {{ return \"{ty}\" }} }}\n\
             fn via<T: Show>(x: T) string {{ return x.show() }}\n\
             fn main() void {{\n    let x: {ty} = {lit}\n    println(x.show())\n    println(via(x))\n    println(Show.show(x))\n}}\n"));
        assert_eq!(run(&src).unwrap(), format!("{ty}\n{ty}\n{ty}\n"), "for `{ty}`");
    }
}

#[test]
fn every_primitive_spelling_takes_an_extend_block() {
    for (ty, lit) in PRIMITIVES {
        let src = format!(
            "extend {ty} {{ fn tag(self) string {{ return \"{ty}\" }} }}\n\
             fn main() void {{\n    let x: {ty} = {lit}\n    println(x.tag())\n}}\n");
        assert_eq!(run(&src).unwrap(), format!("{ty}\n"), "for `{ty}`");
    }
}

#[test]
fn an_extend_method_takes_arguments_and_returns_a_value() {
    let src = "extend int { fn plus(self, other: int) int { return self + other } }\n\
               fn main() void {\n    let x: int = 4\n    println(x.plus(5))\n}\n";
    assert_eq!(run(src).unwrap(), "9\n");
}

#[test]
fn a_method_on_a_literal_in_parentheses_is_called() {
    let src = "extend int { fn double(self) int { return self * 2 } }\n\
               fn main() void {\n    let r = (21).double()\n    println(r)\n}\n";
    assert_eq!(run(src).unwrap(), "42\n");
}

#[test]
fn self_in_an_impl_for_a_primitive_is_that_primitive() {
    let src = "trait Same { fn same(self, other: Self) bool }\n\
               impl Same for int { fn same(self, other: Self) bool { return self == other } }\n\
               fn main() void {\n    let a: int = 1\n    let b: int = 1\n    println(a.same(b))\n}\n";
    assert_eq!(run(src).unwrap(), "true\n");
    let bad = "trait Same { fn same(self, other: Self) bool }\n\
               impl Same for int { fn same(self, other: Self) bool { return self == other } }\n\
               fn main() void {\n    let a: int = 1\n    println(a.same(\"s\"))\n}\n";
    assert_eq!(codes(bad), (Stage::Sema, vec!["TYPE-101"]));
    assert_eq!(messages(bad), vec!["type mismatch: expected `int`, found `string`"]);
}

#[test]
fn the_body_of_an_impl_for_a_primitive_is_checked_with_self_as_that_primitive() {
    let src = "trait Probe { fn probe(self) string }\nimpl Probe for int { fn probe(self) string { return self.nothing() } }\nfn main() void { println(1) }\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["TYPE-104"]));
    assert_eq!(messages(src), vec!["type `int` has no method `nothing`"]);
    let ext = "extend double { fn bad(self) int { return self } }\nfn main() void { println(1) }\n";
    assert_eq!(codes(ext), (Stage::Sema, vec!["TYPE-101"]));
    assert_eq!(messages(ext), vec!["type mismatch: expected `int`, found `double`"]);
}

#[test]
fn self_in_a_signature_of_an_extend_block_is_the_built_in_type() {
    let ok = "extend int { fn me(self) Self { return self } }\nfn main() void {\n    let a: int = 3\n    let b: int = a.me()\n    println(b)\n}\n";
    assert_eq!(run(ok).unwrap(), "3\n");
    let bad = "extend int { fn me(self) Self { return self } }\nfn main() void {\n    let a: int = 3\n    let s: string = a.me()\n}\n";
    assert_eq!(codes(bad), (Stage::Sema, vec!["TYPE-101"]));
    assert_eq!(messages(bad), vec!["type mismatch: expected `string`, found `int`"]);
}

#[test]
fn a_native_method_on_an_array_parameter_is_not_rejected() {
    let src = "fn f(a: [3]int) int { return a.len() }\nfn main() void { println(1) }\n";
    assert_eq!(codes(src), CLEAN);
}

#[test]
fn a_generic_arity_error_inside_a_built_in_target_is_reported_once() {
    let src = "struct Box<T> { v: T }\ntrait Show { fn show(self) string }\n\
               impl Show for (Box, int) { fn show(self) string { return \"x\" } }\nfn main() void { println(1) }\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["TYPE-108"]));
}

#[test]
fn a_type_error_inside_a_built_in_target_is_reported_once() {
    let src = with_show("impl Show for (Dictionary<double, int>, int) { fn show(self) string { return \"x\" } }\nfn main() void { println(1) }\n");
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-126"]));
    assert_eq!(messages(&src), vec!["the trait `Hash` is not implemented for `double`"]);
}

#[test]
fn the_qualified_form_reaches_a_hand_written_prelude_impl_on_a_built_in() {
    let src = "impl PartialOrd for bool {\n    fn partial_cmp(self, other: bool) Ordering? {\n        if self == other { return Ordering.Equal }\n        if self { return Ordering.Greater }\n        return Ordering.Less\n    }\n}\n\
               fn main() void {\n    println(PartialOrd.lt(false, true))\n    println(PartialOrd.lt(true, false))\n    println(PartialOrd.partial_cmp(false, true) == Ordering.Less)\n}\n";
    assert_eq!(run(src).unwrap(), "true\nfalse\ntrue\n");
}

#[test]
fn a_default_method_reaches_a_primitive_three_ways() {
    let src = "trait Describe {\n    fn name(self) string\n    fn tag(self, p: string) string { return p + self.name() }\n}\n\
               impl Describe for int { fn name(self) string { return \"int\" } }\n\
               fn via<T: Describe>(x: T) string { return x.tag(\"<\") }\n\
               fn main() void {\n    let n: int = 3\n    println(n.tag(\"#\"))\n    println(via(n))\n    println(Describe.tag(n, \"!\"))\n}\n";
    assert_eq!(run(src).unwrap(), "#int\n<int\n!int\n");
}

#[test]
fn a_generic_method_on_a_primitive_chooses_its_types_per_call() {
    let src = "extend int { fn pick<U>(self, u: U) U { return u } }\n\
               fn main() void {\n    let n: int = 1\n    let s: string = n.pick(\"a\")\n    let m: int = n.pick(2)\n    println(s)\n    println(m)\n}\n";
    assert_eq!(run(src).unwrap(), "a\n2\n");
}

#[test]
fn a_generic_trait_on_a_primitive_is_checked_with_its_argument() {
    let src = "trait Convert<T> { fn convert(self) T }\n\
               impl Convert<string> for int { fn convert(self) string { return \"x\" } }\n\
               fn via<T: Convert<string>>(x: T) string { return x.convert() }\n\
               fn bad<T: Convert<int>>(x: T) int { return x.convert() }\n\
               fn main() void {\n    let n: int = 1\n    println(via(n))\n    println(bad(n))\n}\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["TYPE-126"]));
    assert_eq!(messages(src), vec!["the trait `Convert<int>` is not implemented for `int`"]);
}

#[test]
fn an_impl_through_an_alias_of_a_primitive_is_for_that_primitive() {
    let src = with_show("type Score = int\nimpl Show for Score { fn show(self) string { return \"score\" } }\n\
                         fn main() void {\n    let s: Score = 3\n    let n: int = 4\n    println(s.show())\n    println(n.show())\n}\n");
    assert_eq!(run(&src).unwrap(), "score\nscore\n");
}

#[test]
fn an_extend_through_an_alias_of_a_primitive_is_for_that_primitive() {
    let src = "type Score = int\nextend Score { fn bump(self) int { return self + 1 } }\n\
               fn main() void {\n    let n: int = 4\n    println(n.bump())\n}\n";
    assert_eq!(run(src).unwrap(), "5\n");
}

// ── Sema keeps the types apart ───────────────────────────────────

#[test]
fn an_impl_for_int_does_not_make_long_implement_the_trait() {
    let src = with_show("impl Show for int { fn show(self) string { return \"i\" } }\n\
                         fn via<T: Show>(x: T) string { return x.show() }\n\
                         fn main() void {\n    let l: long = 5\n    println(via(l))\n}\n");
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-126"]));
    assert_eq!(messages(&src), vec!["the trait `Show` is not implemented for `long`"]);
}

#[test]
fn a_type_without_the_impl_has_no_such_method() {
    let src = with_show("impl Show for int { fn show(self) string { return \"i\" } }\n\
                         fn main() void {\n    let s: string = \"s\"\n    println(s.show())\n}\n");
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-104"]));
}

#[test]
fn types_of_different_kinds_each_have_their_own_impl() {
    let src = with_show("impl Show for int { fn show(self) string { return \"int\" } }\n\
                         impl Show for i32 { fn show(self) string { return \"i32\" } }\n\
                         impl Show for uint { fn show(self) string { return \"uint\" } }\n\
                         impl Show for u8 { fn show(self) string { return \"u8\" } }\n\
                         impl Show for float { fn show(self) string { return \"float\" } }\n\
                         impl Show for double { fn show(self) string { return \"double\" } }\n\
                         fn main() void {\n    let a: int = 1\n    let b: i32 = 1\n    let c: uint = 1\n    let d: u8 = 1\n    let e: float = 1.5f\n    let f: double = 1.5\n    println(a.show())\n    println(b.show())\n    println(c.show())\n    println(d.show())\n    println(e.show())\n    println(f.show())\n}\n");
    assert_eq!(run(&src).unwrap(), "int\ni32\nuint\nu8\nfloat\ndouble\n");
}

// ── One kind of value, one impl ──────────────────────────────────

#[test]
fn a_second_impl_for_a_type_of_the_same_kind_is_type_128() {
    let src = with_show("impl Show for int { fn show(self) string { return \"i\" } }\n\
                         impl Show for long { fn show(self) string { return \"l\" } }\n\
                         fn main() void { println(1) }\n");
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-128"]));
    assert_eq!(messages(&src), vec![
        "`Show` is implemented for `long` more than once (`int` and `long` are one kind of value at run time, and a call cannot tell types of the same kind apart)",
    ]);
    assert_eq!(positions(&src), vec![pos_of(&src, "impl Show for long")]);
}

#[test]
fn each_group_of_types_that_share_a_kind_overlaps_within_itself() {
    let groups: &[&[&str]] = &[
        &["int", "long", "i64", "isize", "short"],
        &["uint", "u32", "ushort"],
        &["ulong", "u64", "usize"],
        &["i8", "byte"],
        &["u8", "ubyte"],
        &["float", "f32"],
        &["double", "f64"],
    ];
    for group in groups {
        for other in &group[1..] {
            let src = with_show(&format!(
                "impl Show for {} {{ fn show(self) string {{ return \"a\" }} }}\n\
                 impl Show for {other} {{ fn show(self) string {{ return \"b\" }} }}\n\
                 fn main() void {{ println(1) }}\n", group[0]));
            assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-128"]), "`{}` and `{other}`", group[0]);
        }
    }
}

#[test]
fn the_same_type_twice_is_the_plain_overlap() {
    let src = with_show("impl Show for int { fn show(self) string { return \"a\" } }\n\
                         impl Show for int { fn show(self) string { return \"b\" } }\n\
                         fn main() void { println(1) }\n");
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-128"]));
    assert_eq!(messages(&src), vec!["`Show` is implemented for `int` more than once"]);
}

#[test]
fn two_traits_may_each_have_an_impl_for_types_of_one_kind() {
    let src = "trait A { fn a(self) string }\ntrait B { fn b(self) string }\n\
               impl A for int { fn a(self) string { return \"a\" } }\n\
               impl B for long { fn b(self) string { return \"b\" } }\n\
               fn main() void {\n    let x: int = 1\n    let y: long = 1\n    println(x.a())\n    println(y.b())\n}\n";
    assert_eq!(run(src).unwrap(), "a\nb\n");
}

#[test]
fn a_method_that_does_not_exist_on_a_primitive_is_type_104() {
    // It used to pass sema and panic at run time.
    let src = "fn main() void {\n    let x: int = 1\n    let d: double = 1.5\n    println(x.nothing())\n    println(d.nothing())\n}\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["TYPE-104", "TYPE-104"]));
    assert_eq!(messages(src), vec![
        "type `int` has no method `nothing`",
        "type `double` has no method `nothing`",
    ]);
}

#[test]
fn a_function_without_self_is_not_called_as_a_method_on_a_primitive() {
    let src = "extend int { fn zero() int { return 0 } }\nfn main() void {\n    let x: int = 1\n    println(x.zero())\n}\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["TYPE-104"]));
}

#[test]
fn an_impl_for_one_trait_does_not_satisfy_a_bound_on_another() {
    let src = "trait A { fn a(self) string }\ntrait B { fn b(self) string }\n\
               impl A for int { fn a(self) string { return \"a\" } }\n\
               fn f<T: B>(x: T) string { return x.b() }\n\
               fn main() void {\n    let n: int = 1\n    println(f(n))\n}\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["TYPE-126"]));
    assert_eq!(messages(src), vec!["the trait `B` is not implemented for `int`"]);
}

#[test]
fn an_impl_written_after_the_signature_that_needs_it_is_found() {
    let src = "trait Show { fn show(self) string }\nstruct W<T: Show> { x: T }\n\
               fn f(w: W<int>) void { }\n\
               impl Show for int { fn show(self) string { return \"i\" } }\n\
               fn main() void { println(1) }\n";
    assert_eq!(codes(src), CLEAN);
    let prelude = "fn f(d: Dictionary<double, int>) void { }\n\
                   impl Eq for double { }\n\
                   impl Hash for double { fn hash(self, state: Hasher) void { } }\n\
                   fn main() void { println(1) }\n";
    assert_eq!(codes(prelude), CLEAN);
}

#[test]
fn the_qualified_form_of_a_generic_trait_on_a_primitive_learns_the_argument() {
    let src = "trait Convert<T> { fn convert(self) T }\n\
               impl Convert<string> for int { fn convert(self) string { return \"c\" } }\n\
               fn main() void {\n    let n: int = 1\n    let s: string = Convert.convert(n)\n    println(s)\n}\n";
    assert_eq!(run(src).unwrap(), "c\n");
    let bad = "trait Convert<T> { fn convert(self) T }\n\
               impl Convert<string> for int { fn convert(self) string { return \"c\" } }\n\
               fn main() void {\n    let n: int = 1\n    let t: int = Convert.convert(n)\n}\n";
    assert_eq!(codes(bad), (Stage::Sema, vec!["TYPE-101"]));
    assert_eq!(messages(bad), vec!["type mismatch: expected `int`, found `string`"]);
}

// ── Native methods, extend and trait methods ─────────────────────

#[test]
fn a_native_method_wins_over_an_extend_method_of_the_same_name() {
    let src = "extend string { fn len(self) int { return 99 } fn twice(self) string { return self + self } }\n\
               fn main() void {\n    println(\"ab\".len())\n    println(\"ab\".twice())\n}\n";
    assert_eq!(run(src).unwrap(), "2\nabab\n");
}

#[test]
fn an_extend_method_beats_a_trait_method_and_the_qualified_form_reaches_the_trait() {
    let src = "trait Name { fn name(self) string }\n\
               extend int { fn name(self) string { return \"ext\" } }\n\
               impl Name for int { fn name(self) string { return \"trait\" } }\n\
               fn main() void {\n    let x: int = 1\n    println(x.name())\n    println(Name.name(x))\n}\n";
    assert_eq!(run(src).unwrap(), "ext\ntrait\n");
}

#[test]
fn two_traits_with_one_method_name_are_ambiguous_on_a_primitive() {
    let src = "trait A { fn name(self) string }\ntrait B { fn name(self) string }\n\
               impl A for int { fn name(self) string { return \"a\" } }\n\
               impl B for int { fn name(self) string { return \"b\" } }\n\
               fn main() void {\n    let x: int = 1\n    println(x.name())\n}\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["TYPE-127"]));
    assert_eq!(messages(src), vec!["method `name` on `int` is supplied by more than one trait (`A`, `B`)"]);
}

#[test]
fn the_qualified_form_picks_one_of_two_traits_on_a_primitive() {
    let src = "trait A { fn name(self) string }\ntrait B { fn name(self) string }\n\
               impl A for int { fn name(self) string { return \"a\" } }\n\
               impl B for int { fn name(self) string { return \"b\" } }\n\
               fn main() void {\n    let x: int = 1\n    println(A.name(x))\n    println(B.name(x))\n}\n";
    assert_eq!(run(src).unwrap(), "a\nb\n");
}

#[test]
fn supertraits_are_required_of_a_primitive_impl() {
    let missing = with_show("trait Loud: Show { fn loud(self) string }\n\
                             impl Loud for int { fn loud(self) string { return \"!\" } }\n\
                             fn main() void { println(1) }\n");
    assert_eq!(codes(&missing), (Stage::Sema, vec!["TYPE-132"]));
    let ok = with_show("trait Loud: Show { fn loud(self) string }\n\
                        impl Show for int { fn show(self) string { return \"s\" } }\n\
                        impl Loud for int { fn loud(self) string { return \"!\" } }\n\
                        fn both<T: Loud>(x: T) string { return x.show() + x.loud() }\n\
                        fn main() void {\n    let n: int = 1\n    println(both(n))\n}\n");
    assert_eq!(run(&ok).unwrap(), "s!\n");
}

#[test]
fn an_impl_that_misses_a_method_or_has_the_wrong_signature_is_reported_for_a_primitive() {
    let missing = with_show("impl Show for int { }\nfn main() void { println(1) }\n");
    assert_eq!(codes(&missing), (Stage::Sema, vec!["TYPE-123"]));
    let wrong = with_show("impl Show for int { fn show(self) int { return 1 } }\nfn main() void { println(1) }\n");
    assert_eq!(codes(&wrong), (Stage::Sema, vec!["TYPE-125"]));
}

// ── Tuples ───────────────────────────────────────────────────────

#[test]
fn a_tuple_type_takes_an_impl_and_a_tuple_value_finds_it() {
    let src = with_show("impl Show for (int, string) { fn show(self) string { return \"pair\" } }\n\
                         fn via<T: Show>(x: T) string { return x.show() }\n\
                         fn main() void {\n    let p = (1, \"a\")\n    println(p.show())\n    println(via(p))\n    println(Show.show(p))\n}\n");
    assert_eq!(run(&src).unwrap(), "pair\npair\npair\n");
}

#[test]
fn another_tuple_type_does_not_satisfy_the_bound() {
    let src = with_show("impl Show for (int, string) { fn show(self) string { return \"pair\" } }\n\
                         fn via<T: Show>(x: T) string { return x.show() }\n\
                         fn main() void {\n    let q = (\"a\", \"b\")\n    println(via(q))\n}\n");
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-126"]));
    assert_eq!(messages(&src), vec!["the trait `Show` is not implemented for `(string, string)`"]);
}

#[test]
fn tuples_of_different_arity_each_have_an_impl() {
    let src = with_show("impl Show for (int, int) { fn show(self) string { return \"two\" } }\n\
                         impl Show for (int, int, int) { fn show(self) string { return \"three\" } }\n\
                         fn main() void {\n    let a = (1, 2)\n    let b = (1, 2, 3)\n    println(a.show())\n    println(b.show())\n}\n");
    assert_eq!(run(&src).unwrap(), "two\nthree\n");
}

#[test]
fn two_tuple_types_of_one_arity_overlap() {
    let src = with_show("impl Show for (int, int) { fn show(self) string { return \"a\" } }\n\
                         impl Show for (string, string) { fn show(self) string { return \"b\" } }\n\
                         fn main() void { println(1) }\n");
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-128"]));
    assert_eq!(messages(&src), vec![
        "`Show` is implemented for `(string, string)` more than once (`(int, int)` and `(string, string)` are one kind of value at run time, and a call cannot tell tuples with the same number of elements apart)",
    ]);
}

#[test]
fn a_tuple_extend_block_and_an_alias_of_a_tuple_work() {
    let src = "type Pair = (int, int)\n\
               extend Pair { fn tag(self) string { return \"pair\" } }\n\
               fn main() void {\n    let p = (1, 2)\n    println(p.tag())\n}\n";
    assert_eq!(run(src).unwrap(), "pair\n");
}

#[test]
fn a_native_tuple_method_wins_over_an_extend_method() {
    let src = "extend (int, int) { fn len(self) int { return 99 } fn tag(self) string { return \"t\" } }\n\
               fn main() void {\n    let p = (1, 2)\n    println(p.tag())\n}\n";
    assert_eq!(run(src).unwrap(), "t\n");
}

// ── Arrays ───────────────────────────────────────────────────────

#[test]
fn an_array_type_takes_an_impl_and_a_parameter_of_it_calls_the_method() {
    // No expression makes a `[3]int` yet (an array literal is a `List<T>`),
    // so a value of the type can only arrive as a parameter.
    let src = with_show("impl Show for [3]int { fn show(self) string { return \"arr\" } }\n\
                         fn via<T: Show>(x: T) string { return x.show() }\n\
                         fn direct(a: [3]int) string { return a.show() }\n\
                         fn bound(a: [3]int) string { return via(a) }\n\
                         fn main() void { println(1) }\n");
    assert_eq!(codes(&src), CLEAN);
}

#[test]
fn another_array_type_does_not_satisfy_the_bound() {
    let src = with_show("impl Show for [3]int { fn show(self) string { return \"arr\" } }\n\
                         fn via<T: Show>(x: T) string { return x.show() }\n\
                         fn other(a: [4]int) string { return via(a) }\n\
                         fn main() void { println(1) }\n");
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-126"]));
    assert_eq!(messages(&src), vec!["the trait `Show` is not implemented for `[4]int`"]);
}

#[test]
fn two_array_types_overlap_whatever_their_length_or_element() {
    let src = with_show("impl Show for [3]int { fn show(self) string { return \"a\" } }\n\
                         impl Show for [4]string { fn show(self) string { return \"b\" } }\n\
                         fn main() void { println(1) }\n");
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-128"]));
    assert_eq!(messages(&src), vec![
        "`Show` is implemented for `[4]string` more than once (`[3]int` and `[4]string` are one kind of value at run time, and a call cannot tell arrays of every length and element type apart)",
    ]);
}

// ── Prelude traits ───────────────────────────────────────────────

#[test]
fn a_prelude_trait_a_built_in_type_has_natively_cannot_be_implemented_again() {
    let src = "impl Eq for int { }\n\
               impl Hash for string { fn hash(self, state: Hasher) void { } }\n\
               impl Clone for bool { fn clone(self) bool { return self } }\n\
               impl PartialEq for (int, int) { fn eq(self, other: (int, int)) bool { return true } }\n\
               fn main() void { println(1) }\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["TYPE-128"; 4]));
    assert_eq!(messages(src), vec![
        "`Eq` is implemented for `int` more than once (the built-in type already implements it)",
        "`Hash` is implemented for `string` more than once (the built-in type already implements it)",
        "`Clone` is implemented for `bool` more than once (the built-in type already implements it)",
        "`PartialEq` is implemented for `(int, int)` more than once (the built-in type already implements it)",
    ]);
}

#[test]
fn hash_for_a_float_is_written_by_hand_and_makes_it_a_dictionary_key() {
    let without = "fn main() void {\n    let d: Dictionary<double, int> = Dictionary.new()\n    d.set(1.5, 2)\n}\n";
    assert_eq!(codes(without), (Stage::Sema, vec!["TYPE-126"]));
    assert_eq!(messages(without), vec!["the trait `Hash` is not implemented for `double`"]);
    let with = "impl Eq for double { }\n\
                impl Hash for double { fn hash(self, state: Hasher) void { } }\n\
                fn main() void {\n    let d: Dictionary<double, int> = Dictionary.new()\n    d.set(1.5, 2)\n    println(d.contains_key(1.5))\n}\n";
    assert_eq!(run(with).unwrap(), "true\n");
}

#[test]
fn a_hand_written_prelude_impl_still_needs_the_traits_it_builds_on() {
    let src = "impl Hash for double { fn hash(self, state: Hasher) void { } }\nfn main() void { println(1) }\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["TYPE-132"]));
    assert_eq!(messages(src), vec!["`impl Hash for double` needs `Eq`, which `double` does not implement"]);
    let ord = "impl Ord for char { fn cmp(self, other: char) Ordering { return Ordering.Equal } }\nfn main() void { println(1) }\n";
    assert_eq!(codes(ord), (Stage::Sema, vec!["TYPE-132"]));
}

#[test]
fn ordering_on_bool_needs_a_hand_written_partial_ord_and_then_runs() {
    let without = "fn main() void { println(true < false) }\n";
    assert_eq!(codes(without), (Stage::Sema, vec!["TYPE-118"]));
    let with = "impl PartialOrd for bool {\n    fn partial_cmp(self, other: bool) Ordering? {\n        if self == other { return Ordering.Equal }\n        if self { return Ordering.Greater }\n        return Ordering.Less\n    }\n}\n\
                fn main() void {\n    println(true < false)\n    println(false < true)\n    println(true.lt(false))\n    println(false.lt(true))\n}\n";
    assert_eq!(run(with).unwrap(), "false\ntrue\nfalse\ntrue\n");
}

#[test]
fn ord_for_char_is_written_by_hand_beside_partial_ord() {
    let src = "impl PartialOrd for char { fn partial_cmp(self, other: char) Ordering? { return Ordering.Less } }\n\
               impl Ord for char { fn cmp(self, other: char) Ordering { return Ordering.Less } }\n\
               fn main() void {\n    let a: char = 'a'\n    let b: char = 'b'\n    println(a < b)\n    println(a.cmp(b) == Ordering.Less)\n}\n";
    assert_eq!(run(src).unwrap(), "true\ntrue\n");
}

#[test]
fn a_prelude_impl_on_a_built_in_is_used_through_a_bound() {
    let src = "impl PartialOrd for bool {\n    fn partial_cmp(self, other: bool) Ordering? {\n        if self == other { return Ordering.Equal }\n        if self { return Ordering.Greater }\n        return Ordering.Less\n    }\n}\n\
               fn bigger<T: PartialOrd>(a: T, b: T) bool { return a < b }\n\
               fn main() void {\n    println(bigger(false, true))\n    println(bigger(true, false))\n}\n";
    assert_eq!(run(src).unwrap(), "true\nfalse\n");
}

// ── Targets that stay unsupported ────────────────────────────────

#[test]
fn an_impl_or_extend_for_a_set_is_type_129() {
    // List, Dictionary, Queue, Stack and InlineList take impls since S3e
    // (tests/generic_impls.rs); a Set has no value of its own at run time.
    let imp = with_show("impl Show for Set<int> { fn show(self) string { return \"l\" } }\nfn main() void { println(1) }\n");
    assert_eq!(codes(&imp), (Stage::Sema, vec!["TYPE-129"]));
    assert_eq!(messages(&imp), vec![
        "implementing a trait for a type that is not a struct, an enum, a primitive, a tuple, an array, a List, a Dictionary, a Queue, a Stack or an InlineList is not supported yet",
    ]);
    let ext = "extend Set<int> { fn first2(self) int { return 1 } }\nfn main() void { println(1) }\n";
    assert_eq!(codes(ext), (Stage::Sema, vec!["TYPE-129"]));
    assert_eq!(messages(ext), vec![
        "extending a type that is not a struct, an enum, a primitive, a tuple, an array, a List, a Dictionary, a Queue, a Stack or an InlineList is not supported yet",
    ]);
}

#[test]
fn structs_and_enums_keep_working_beside_built_in_impls() {
    let src = with_show("struct P { n: int }\nenum E { One, Two }\n\
                         impl Show for P { fn show(self) string { return \"P\" } }\n\
                         impl Show for E { fn show(self) string { return \"E\" } }\n\
                         impl Show for int { fn show(self) string { return \"int\" } }\n\
                         fn via<T: Show>(x: T) string { return x.show() }\n\
                         fn main() void {\n    println(via(P { n = 1 }))\n    println(via(E.One))\n    println(via(3))\n}\n");
    assert_eq!(run(&src).unwrap(), "P\nE\nint\n");
}
