//! Generic methods and bounded type aliases, slice S3c.
//!
//! A method can declare generic parameters of its own, with bounds, in a
//! struct, an `extend` block, an inherent `impl` and a trait, and a bound on a
//! type alias's parameter holds at every use of the alias. The `.ubl`
//! fixtures show whole programs accepted or rejected. These tests pin the
//! finer contract: the exact code, count, text and position of every
//! diagnostic, that each call chooses its own types, that an impl repeats a
//! trait's generic method exactly, and that the programs that are accepted
//! behave as written when run.
//!
//! See docs/TRAITS_DESIGN.md, "What S3c settled", and docs/GENERICS_RULES.md.

use ubel_stratum::ast::arena::AstArena;
use ubel_stratum::interpreter::Interpreter;
use ubel_stratum_rd::{check_source, Stage};

/// The stage a program stops at and the codes it reports, in order.
fn codes(source: &str) -> (Stage, Vec<&'static str>) {
    let report = check_source(source);
    let codes = report.diagnostics.iter().map(|d| d.code).collect();
    (report.stage, codes)
}

fn messages(source: &str) -> Vec<String> {
    check_source(source).diagnostics.iter().map(|d| d.message.clone()).collect()
}

/// The 1-based line and column of each diagnostic's primary span.
fn positions(source: &str) -> Vec<(usize, usize)> {
    check_source(source)
        .diagnostics
        .iter()
        .map(|d| (d.primary_span.line as usize, d.primary_span.column as usize))
        .collect()
}

/// The 1-based line and column where `needle` first appears in `source`.
fn pos_of(source: &str, needle: &str) -> (usize, usize) {
    let at = source.find(needle).unwrap_or_else(|| panic!("`{needle}` not in the source"));
    let before = &source[..at];
    let line = before.matches('\n').count() + 1;
    let column = at - before.rfind('\n').map_or(0, |i| i + 1) + 1;
    (line, column)
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
    interp.set_trait_call_sites(ctx.trait_call_sites);
    let result = interp.run_program(&program);
    let output = ubel_stratum::builtins::global::io::take_captured_output();
    result.map(|_| output)
}

/// Two traits (one generic) and three structs: `A` is `Show`, `Celsius`
/// is `Convert<int>`, `Label` is `Convert<string>`.
const PRE: &str = r#"trait Show {
    fn show(self) string
}
trait Convert<T> {
    fn convert(self) T
}
struct A { n: int }
struct Celsius { deg: int }
struct Label { text: string }
impl Show for A { fn show(self) string { return "A" } }
impl Convert<int> for Celsius { fn convert(self) int { return self.deg * 2 } }
impl Convert<string> for Label { fn convert(self) string { return self.text + "!" } }
"#;

fn prog(body: &str) -> String {
    format!("{PRE}{body}")
}

const CLEAN: (Stage, Vec<&str>) = (Stage::Clean, Vec::new());

// ── Bounds on a type alias ───────────────────────────────────────

const ALIASES: &str = r#"struct W<T> { x: T }
struct W2<P, Q> { p: P, q: Q }
type Shown<T: Show> = W<T>
type Converting<T: Convert<int>> = W<T>
"#;

fn with_aliases(body: &str) -> String {
    prog(&format!("{ALIASES}{body}"))
}

#[test]
fn an_alias_bound_is_declared_without_error() {
    let src = with_aliases("fn main() void { println(1) }\n");
    assert_eq!(codes(&src), CLEAN);
}

#[test]
fn an_alias_used_with_a_type_that_meets_the_bound_is_accepted_and_runs() {
    let src = with_aliases(r#"fn main() void {
    let w: Shown<A> = W { x = A { n = 1 } }
    println(w.x.show())
    let c: Converting<Celsius> = W { x = Celsius { deg = 4 } }
    println(c.x.convert())
}
"#);
    assert_eq!(run(&src).unwrap(), "A\n8\n");
}

#[test]
fn an_alias_used_with_a_type_that_misses_the_bound_is_one_type_126_at_the_use() {
    let src = with_aliases("fn plain(w: Shown<int>) void {\n}\nfn main() void { println(1) }\n");
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-126"]));
    assert_eq!(messages(&src), vec!["the trait `Show` is not implemented for `int`"]);
    assert_eq!(positions(&src), vec![pos_of(&src, "Shown<int>")]);
}

#[test]
fn an_alias_use_in_a_let_annotation_is_reported_once() {
    let src = with_aliases(r#"fn main() void {
    let w: Shown<int> = W { x = 1 }
}
"#);
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-126"]));
    assert_eq!(positions(&src), vec![pos_of(&src, "Shown<int> =")]);
}

#[test]
fn each_failing_use_of_an_alias_is_reported() {
    let src = with_aliases(r#"fn one(w: Shown<int>) void {
}
fn two(w: Shown<string>) void {
}
fn main() void { println(1) }
"#);
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-126", "TYPE-126"]));
    assert_eq!(messages(&src), vec![
        "the trait `Show` is not implemented for `int`",
        "the trait `Show` is not implemented for `string`",
    ]);
}

#[test]
fn an_alias_bound_with_a_trait_argument_needs_the_same_argument() {
    let src = with_aliases(r#"fn ok(w: Converting<Celsius>) void {
}
fn other(w: Converting<Label>) void {
}
fn main() void { println(1) }
"#);
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-126"]));
    assert_eq!(messages(&src), vec!["the trait `Convert<int>` is not implemented for `Label`"]);
    assert_eq!(positions(&src), vec![pos_of(&src, "Converting<Label>")]);
}

#[test]
fn an_alias_bound_that_is_not_a_trait_is_type_122_once() {
    let src = prog("struct W<T> { x: T }\ntype Missing<T: Nope> = W<T>\nfn main() void { println(1) }\n");
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-122"]));
    assert_eq!(messages(&src), vec!["unknown trait `Nope`"]);
}

#[test]
fn an_alias_bound_with_the_wrong_argument_count_is_type_108_once() {
    let src = prog("struct W<T> { x: T }\ntype Bare<T: Convert> = W<T>\nfn main() void { println(1) }\n");
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-108"]));
    assert_eq!(messages(&src), vec!["`Convert` expects 1 type argument(s), found 0"]);
}

#[test]
fn two_alias_bounds_on_one_trait_with_other_arguments_are_type_134() {
    let src = prog("struct W<T> { x: T }\ntype Both<T: Convert<int> + Convert<string>> = W<T>\nfn main() void { println(1) }\n");
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-134"]));
    assert_eq!(messages(&src), vec![
        "`T` is bounded by both `Convert<int>` and `Convert<string>`, and no type implements both",
    ]);
}

#[test]
fn the_same_alias_bound_twice_is_one_bound() {
    let src = with_aliases(r#"type Twice<T: Show + Show> = W<T>
fn f(w: Twice<A>) void {
}
fn main() void { println(1) }
"#);
    assert_eq!(codes(&src), CLEAN);
}

#[test]
fn a_use_through_another_alias_is_checked_at_the_outer_use() {
    let src = with_aliases(r#"type Outer<U: Show> = Shown<U>
fn ok(w: Outer<A>) void {
}
fn bad(w: Outer<int>) void {
}
fn main() void { println(1) }
"#);
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-126"]));
    assert_eq!(positions(&src), vec![pos_of(&src, "Outer<int>")]);
}

#[test]
fn an_alias_in_a_generic_function_needs_the_bound_on_the_parameter() {
    let src = with_aliases(r#"fn has<T: Show>(w: Shown<T>) string { return w.x.show() }
fn lacks<T>(w: Shown<T>) void {
}
fn main() void { println(1) }
"#);
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-126"]));
    assert_eq!(positions(&src), vec![pos_of(&src, "Shown<T>) void")]);
}

#[test]
fn only_the_bounded_parameter_of_a_two_parameter_alias_is_checked() {
    let src = prog(r#"struct W2<P, Q> { p: P, q: Q }
type OnlyFirst<P: Show, Q> = W2<P, Q>
fn ok(w: OnlyFirst<A, int>) void {
}
fn bad(w: OnlyFirst<int, A>) void {
}
fn main() void { println(1) }
"#);
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-126"]));
    assert_eq!(positions(&src), vec![pos_of(&src, "OnlyFirst<int, A>")]);
    assert_eq!(messages(&src), vec!["the trait `Show` is not implemented for `int`"]);
}

#[test]
fn an_unbounded_alias_of_a_bounded_struct_is_still_reported() {
    // The struct's own bound is asked of the alias's parameter: the control
    // for the alias bound, which is a separate requirement.
    let src = prog("struct W<T: Show> { x: T }\ntype Loose<T> = W<T>\nfn main() void { println(1) }\n");
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-126"]));
}

#[test]
fn an_alias_bound_does_not_change_the_aliased_value() {
    let src = with_aliases(r#"fn main() void {
    let w: Shown<A> = W { x = A { n = 3 } }
    let same: W<A> = w
    println(same.x.n)
}
"#);
    assert_eq!(run(&src).unwrap(), "3\n");
}

// ── A method's own generic parameters ────────────────────────────

const BOX: &str = r#"struct Box {
    v: int,
    fn pick<U>(self, x: U) U { return x }
    fn put<U: Show>(self, x: U) string { return x.show() }
    fn make<U: Show>(x: U) string { return x.show() }
    fn via<U: Convert<int>>(self, x: U) int { return x.convert() + self.v }
    fn conv<U: Convert<V>, V>(self, x: U) V { return x.convert() }
    fn pair<A2, B2: Convert<A2>>(self, a: A2, b: B2) A2 { return b.convert() }
}
"#;

fn with_box(main_body: &str) -> String {
    prog(&format!("{BOX}fn main() void {{\n    let b = Box {{ v = 1 }}\n{main_body}}}\n"))
}

#[test]
fn every_call_chooses_the_type_of_an_own_parameter() {
    let src = with_box(r#"    let n: int = b.pick(5)
    let s: string = b.pick("five")
    println(n)
    println(s)
"#);
    assert_eq!(run(&src).unwrap(), "5\nfive\n");
}

#[test]
fn a_result_used_as_the_wrong_type_is_type_101() {
    let src = with_box("    let r: string = b.pick(5)\n");
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-101"]));
    assert_eq!(messages(&src), vec!["type mismatch: expected `string`, found `int`"]);
}

#[test]
fn a_wrong_argument_count_to_a_generic_method_is_reported_once() {
    let src = with_box("    println(b.pick(1, 2))\n");
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-102"]));
}

#[test]
fn a_bounded_own_parameter_lets_the_body_call_the_bound_method() {
    let src = with_box(r#"    println(b.put(A { n = 1 }))
    println(Box.make(A { n = 2 }))
"#);
    assert_eq!(run(&src).unwrap(), "A\nA\n");
}

#[test]
fn an_argument_that_misses_a_bound_is_type_126_at_that_argument() {
    let src = with_box("    println(b.put(5))\n    println(b.put(\"s\"))\n");
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-126", "TYPE-126"]));
    assert_eq!(messages(&src), vec![
        "the trait `Show` is not implemented for `int`",
        "the trait `Show` is not implemented for `string`",
    ]);
    assert_eq!(positions(&src), vec![pos_of(&src, "5))"), pos_of(&src, "\"s\"))")]);
}

#[test]
fn a_bound_met_by_a_later_argument_type_is_one_obligation_not_two() {
    let src = prog(r#"struct Box {
    v: int,
    fn both<U: Show>(self, a: U, b: U) string { return a.show() }
}
fn main() void {
    let b = Box { v = 1 }
    println(b.both(1, 2))
}
"#);
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-126"]));
    assert_eq!(positions(&src), vec![pos_of(&src, "1, 2))")]);
}

#[test]
fn a_bound_with_a_trait_argument_is_checked_with_that_argument() {
    let src = with_box("    println(b.via(Celsius { deg = 2 }))\n    println(b.via(Label { text = \"x\" }))\n");
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-126"]));
    assert_eq!(messages(&src), vec!["the trait `Convert<int>` is not implemented for `Label`"]);
}

#[test]
fn a_bound_argument_the_impl_decides_is_learned_from_the_call() {
    let src = with_box(r#"    let i: int = b.conv(Celsius { deg = 3 })
    println(i)
"#);
    assert_eq!(run(&src).unwrap(), "6\n");
    let bad = with_box("    let s: string = b.conv(Celsius { deg = 3 })\n");
    assert_eq!(codes(&bad), (Stage::Sema, vec!["TYPE-101"]));
    assert_eq!(messages(&bad), vec!["type mismatch: expected `string`, found `int`"]);
}

#[test]
fn a_bound_may_name_an_earlier_own_parameter() {
    let ok = with_box("    let n: int = b.pair(1, Celsius { deg = 1 })\n    println(n)\n");
    assert_eq!(run(&ok).unwrap(), "2\n");
    let bad = with_box("    let s: string = b.pair(\"x\", Celsius { deg = 1 })\n");
    assert_eq!(codes(&bad), (Stage::Sema, vec!["TYPE-126"]));
    assert_eq!(messages(&bad), vec!["the trait `Convert<string>` is not implemented for `Celsius`"]);
    // Reported at the argument for the bounded parameter, the second one.
    assert_eq!(positions(&bad), vec![pos_of(&bad, "Celsius { deg = 1 })")]);
}

#[test]
fn an_own_bound_brings_in_the_traits_it_builds_on() {
    let src = prog(r#"trait Loud: Show { fn loud(self) string }
struct B { n: int }
impl Show for B { fn show(self) string { return "b" } }
impl Loud for B { fn loud(self) string { return "B!" } }
struct Box {
    v: int,
    fn both<U: Loud>(self, x: U) string { return x.show() + x.loud() }
}
fn main() void {
    let b = Box { v = 1 }
    println(b.both(B { n = 1 }))
}
"#);
    assert_eq!(run(&src).unwrap(), "bB!\n");
    let bad = prog(r#"trait Loud: Show { fn loud(self) string }
struct Box {
    v: int,
    fn both<U: Loud>(self, x: U) string { return x.show() }
}
fn main() void {
    let b = Box { v = 1 }
    println(b.both(A { n = 1 }))
}
"#);
    assert_eq!(codes(&bad), (Stage::Sema, vec!["TYPE-126"]));
    assert_eq!(messages(&bad), vec!["the trait `Loud` is not implemented for `A`"]);
}

#[test]
fn an_associated_function_checks_its_own_bounds() {
    let src = with_box("    println(Box.make(1))\n");
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-126"]));
    assert_eq!(positions(&src), vec![pos_of(&src, "1))")]);
}

#[test]
fn an_extend_block_method_checks_its_own_bounds() {
    let src = prog(r#"struct Box { v: int }
extend Box {
    fn put<U: Show>(self, x: U) string { return x.show() }
}
fn main() void {
    let b = Box { v = 1 }
    println(b.put(A { n = 1 }))
    println(b.put(1))
}
"#);
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-126"]));
    assert_eq!(positions(&src), vec![pos_of(&src, "1))\n}")]);
}

#[test]
fn an_inherent_impl_method_checks_its_own_bounds() {
    let src = prog(r#"struct Box { v: int }
impl Box {
    fn put<U: Show>(self, x: U) string { return x.show() }
    fn twice<U>(self, x: U) U { return x }
}
fn main() void {
    let b = Box { v = 1 }
    let s: string = b.twice("x")
    println(b.put(A { n = 1 }))
    println(b.put(1))
}
"#);
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-126"]));
}

#[test]
fn an_own_parameter_comes_after_the_structs_parameters() {
    let src = prog(r#"struct Pair<T> {
    first: T,
    fn both<U>(self, u: U, t: T) U { return u }
}
fn main() void {
    let p = Pair { first = 7 }
    let ok: string = p.both("u", 3)
    println(ok)
    let bad: int = p.both("u", 3)
}
"#);
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-101"]));
    assert_eq!(messages(&src), vec!["type mismatch: expected `int`, found `string`"]);
}

#[test]
fn a_bound_on_an_own_parameter_of_a_generic_struct_method_is_checked_at_its_own_index() {
    let src = prog(r#"struct Pair<T> {
    first: T,
    fn keep<U: Show>(self, u: U) T { return self.first }
}
fn main() void {
    let p = Pair { first = 7 }
    println(p.keep(A { n = 1 }))
    println(p.keep(2))
}
"#);
    // `T` is `int`, which is not `Show`; only the argument for `U` is asked.
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-126"]));
    assert_eq!(messages(&src), vec!["the trait `Show` is not implemented for `int`"]);
    assert_eq!(positions(&src), vec![pos_of(&src, "2))\n}")]);
}

#[test]
fn a_receiver_with_too_few_type_arguments_does_not_cascade() {
    let src = prog(r#"struct Box<T> { v: T, fn both<U>(self, u: U, t: T) U { return u } }
fn f(b: Box) void {
    let r: string = b.both("a", 1)
}
fn main() void { println(1) }
"#);
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-108"]));
}

#[test]
fn a_receiver_with_too_many_type_arguments_does_not_cascade() {
    // The extra argument must not be taken for the method's own parameter.
    let src = prog(r#"struct Box<T> { v: T, fn both<U>(self, u: U, t: T) U { return u } }
fn f(b: Box<int, string>) void {
    let r: int = b.both(5, 1)
}
fn main() void { println(1) }
"#);
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-108"]));
}

#[test]
fn an_own_parameter_of_a_method_on_a_generic_struct_runs() {
    let src = prog(r#"struct Box<T> {
    v: T,
    fn get(self) T { return self.v }
    fn map<U>(self, f: fn(T) U) U { return f(self.v) }
}
fn dbl(x: int) int { return x * 2 }
fn main() void {
    let b = Box { v = 4 }
    let r: int = b.map(dbl)
    println(r)
}
"#);
    assert_eq!(run(&src).unwrap(), "8\n");
}

#[test]
fn a_method_on_an_extend_of_a_generic_struct_has_its_own_parameter() {
    let src = prog(r#"struct Box<T> { v: T }
extend Box {
    fn tag<U>(self, u: U) U { return u }
}
fn main() void {
    let b = Box { v = 1 }
    let s: string = b.tag("t")
    println(s)
}
"#);
    assert_eq!(run(&src).unwrap(), "t\n");
}

// ── Inside the body ──────────────────────────────────────────────

#[test]
fn a_result_of_the_wrong_type_names_the_parameter() {
    let src = prog("struct Box { v: int, fn bad<U>(self, x: U) int { return x } }\nfn main() void { println(1) }\n");
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-101"]));
    assert_eq!(messages(&src), vec!["type mismatch: expected `int`, found `U`"]);
}

#[test]
fn a_trait_default_body_names_its_abstract_self() {
    let src = prog("trait Named {\n    fn name(self) string\n    fn bad(self) int { return self }\n}\nfn main() void { println(1) }\n");
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-101"]));
    assert_eq!(messages(&src), vec!["type mismatch: expected `int`, found `Self`"]);
}

#[test]
fn a_method_called_on_an_unbounded_own_parameter_is_type_130() {
    let src = prog("struct Box { v: int, fn bad<U>(self, x: U) string { return x.show() } }\nfn main() void { println(1) }\n");
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-130"]));
    assert_eq!(messages(&src), vec![
        "cannot call `show` on a value of type parameter `U`: it has no trait bound",
    ]);
}

#[test]
fn an_operator_on_an_unbounded_own_parameter_is_type_131() {
    let src = prog("struct Box { v: int, fn eq<U>(self, a: U, b: U) bool { return a == b } }\nfn main() void { println(1) }\n");
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-131"]));
    assert_eq!(messages(&src), vec!["`==` on a value of type parameter `U` needs a `PartialEq` bound"]);
}

#[test]
fn an_operator_bound_on_an_own_parameter_is_enough() {
    let src = prog(r#"struct Box {
    v: int,
    fn same<U: PartialEq>(self, a: U, b: U) bool { return a == b }
}
fn main() void {
    let b = Box { v = 1 }
    println(b.same(2, 2))
    println(b.same("a", "b"))
}
"#);
    assert_eq!(run(&src).unwrap(), "true\nfalse\n");
}

#[test]
fn a_bound_on_an_own_parameter_that_is_not_a_trait_is_type_122_once() {
    let src = prog("struct Box { v: int, fn m<U: Nope>(self, x: U) int { return 1 } }\nfn main() void { println(1) }\n");
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-122"]));
    assert_eq!(messages(&src), vec!["unknown trait `Nope`"]);
    assert_eq!(positions(&src), vec![pos_of(&src, "U: Nope")]);
}

#[test]
fn a_name_that_is_not_a_trait_beside_a_real_bound_is_reported_once_per_declaration() {
    // The bound that is a trait is kept, so the declaration's bounds are
    // converted a second time; the bad name must not be reported again.
    let src = prog(r#"fn f<T: Show + Nope>(x: T) void {
}
struct S<T: Show + Nope> { v: T }
type M<T: Show + Nope> = List<T>
struct Box { v: int, fn m<U: Show + Nope>(self, x: U) int { return 1 } }
fn main() void { println(1) }
"#);
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-122"; 4]));
    assert_eq!(positions(&src), vec![
        pos_of(&src, "T: Show + Nope>(x"),
        pos_of(&src, "T: Show + Nope> { v"),
        pos_of(&src, "T: Show + Nope> = List"),
        pos_of(&src, "U: Show + Nope>(self"),
    ]);
}

#[test]
fn an_own_bound_with_the_wrong_argument_count_is_type_108_once() {
    let src = prog("struct Box { v: int, fn m<U: Convert>(self, x: U) int { return 1 } }\nfn main() void { println(1) }\n");
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-108"]));
    assert_eq!(messages(&src), vec!["`Convert` expects 1 type argument(s), found 0"]);
}

#[test]
fn two_own_bounds_on_one_trait_with_other_arguments_are_type_134() {
    let src = prog("struct Box { v: int, fn m<U: Convert<int> + Convert<string>>(self, x: U) int { return 1 } }\nfn main() void { println(1) }\n");
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-134"]));
    assert_eq!(messages(&src), vec![
        "`U` is bounded by both `Convert<int>` and `Convert<string>`, and no type implements both",
    ]);
}

#[test]
fn a_repeated_own_bound_is_one_bound() {
    let src = prog(r#"struct Box {
    v: int,
    fn put<U: Show + Show>(self, x: U) string { return x.show() }
}
fn main() void {
    let b = Box { v = 1 }
    println(b.put(A { n = 1 }))
    println(b.put(2))
}
"#);
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-126"]));
}

// ── Generic methods in traits ────────────────────────────────────

const MAPPER: &str = r#"trait Mapper {
    fn apply<U>(self, x: U) U
    fn render<U: Show>(self, x: U) string
}
struct Id { k: int }
"#;

fn with_mapper(rest: &str) -> String {
    prog(&format!("{MAPPER}{rest}"))
}

const MAPPER_IMPL: &str = r#"impl Mapper for Id {
    fn apply<V>(self, x: V) V { return x }
    fn render<W: Show>(self, x: W) string { return x.show() }
}
"#;

#[test]
fn an_impl_that_repeats_a_generic_method_is_accepted_and_runs() {
    let src = with_mapper(&format!(r#"{MAPPER_IMPL}fn main() void {{
    let m = Id {{ k = 1 }}
    println(m.apply(5))
    println(m.apply("a"))
    println(m.render(A {{ n = 1 }}))
}}
"#));
    assert_eq!(run(&src).unwrap(), "5\na\nA\n");
}

#[test]
fn a_signature_that_names_self_and_an_own_parameter_keeps_them_apart() {
    let src = prog(r#"trait Joiner { fn join<U>(self, other: Self, u: U) U }
struct Id { k: int }
impl Joiner for Id { fn join<V>(self, other: Id, v: V) V { return v } }
fn main() void {
    let a = Id { k = 1 }
    let b = Id { k = 2 }
    println(a.join(b, 5))
}
"#);
    assert_eq!(run(&src).unwrap(), "5\n");
}

#[test]
fn a_bound_on_a_trait_methods_parameter_can_name_the_traits_parameter() {
    let src = prog(r#"trait Lifting<T> { fn lift<U: Convert<T>>(self, u: U) T }
struct S { k: int }
impl Lifting<int> for S { fn lift<V: Convert<int>>(self, v: V) int { return v.convert() } }
fn main() void {
    let s = S { k = 1 }
    println(s.lift(Celsius { deg = 2 }))
    println(s.lift(Label { text = "x" }))
}
"#);
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-126"]));
    assert_eq!(messages(&src), vec!["the trait `Convert<int>` is not implemented for `Label`"]);
}

#[test]
fn an_impl_may_rename_the_own_parameters() {
    // `U` in the trait, `V` and `W` in the impl (the first test above).
    let src = with_mapper(&format!("{MAPPER_IMPL}fn main() void {{ println(1) }}\n"));
    assert_eq!(codes(&src), CLEAN);
}

#[test]
fn an_impl_that_drops_the_generic_parameter_is_type_125() {
    let src = with_mapper(r#"impl Mapper for Id {
    fn apply(self, x: int) int { return x }
    fn render<W: Show>(self, x: W) string { return x.show() }
}
fn main() void { println(1) }
"#);
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-125"]));
    assert_eq!(messages(&src), vec![
        "method `apply` does not match trait `Mapper`: expected `fn<U>(self, U) U`, found `fn(self, int) int`",
    ]);
}

#[test]
fn an_impl_that_drops_a_bound_is_type_125() {
    let src = with_mapper(r#"impl Mapper for Id {
    fn apply<V>(self, x: V) V { return x }
    fn render<W>(self, x: W) string { return "" }
}
fn main() void { println(1) }
"#);
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-125"]));
    assert_eq!(messages(&src), vec![
        "method `render` does not match trait `Mapper`: expected `fn<U: Show>(self, U) string`, found `fn<W>(self, W) string`",
    ]);
}

#[test]
fn an_impl_that_adds_a_bound_is_type_125() {
    let src = with_mapper(r#"impl Mapper for Id {
    fn apply<V: Show>(self, x: V) V { return x }
    fn render<W: Show>(self, x: W) string { return x.show() }
}
fn main() void { println(1) }
"#);
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-125"]));
    assert_eq!(messages(&src), vec![
        "method `apply` does not match trait `Mapper`: expected `fn<U>(self, U) U`, found `fn<V: Show>(self, V) V`",
    ]);
}

#[test]
fn an_impl_with_an_extra_own_parameter_is_type_125() {
    let src = with_mapper(r#"impl Mapper for Id {
    fn apply<V, X>(self, x: V) V { return x }
    fn render<W: Show>(self, x: W) string { return x.show() }
}
fn main() void { println(1) }
"#);
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-125"]));
    assert_eq!(messages(&src), vec![
        "method `apply` does not match trait `Mapper`: expected `fn<U>(self, U) U`, found `fn<V, X>(self, V) V`",
    ]);
}

#[test]
fn the_order_of_the_bounds_on_an_own_parameter_does_not_matter() {
    let src = prog(r#"trait Two { fn go<U: Show + Convert<int>>(self, x: U) int }
struct S { k: int }
impl Two for S { fn go<V: Convert<int> + Show>(self, x: V) int { return 1 } }
fn main() void { println(1) }
"#);
    assert_eq!(codes(&src), CLEAN);
}

#[test]
fn bounds_with_different_trait_arguments_do_not_match() {
    let src = prog(r#"trait Two { fn go<U: Convert<int>>(self, x: U) int }
struct S { k: int }
impl Two for S { fn go<V: Convert<string>>(self, x: V) int { return 1 } }
fn main() void { println(1) }
"#);
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-125"]));
    assert_eq!(messages(&src), vec![
        "method `go` does not match trait `Two`: expected `fn<U: Convert<int>>(self, U) int`, found `fn<V: Convert<string>>(self, V) int`",
    ]);
}

#[test]
fn a_bound_on_a_trait_method_that_is_not_a_trait_is_type_122_once() {
    let src = prog("trait Bad { fn go<U: Nope>(self, u: U) int }\nfn main() void { println(1) }\n");
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-122"]));
    assert_eq!(positions(&src), vec![pos_of(&src, "U: Nope")]);
}

#[test]
fn a_generic_trait_can_have_a_generic_method() {
    let src = prog(r#"trait Lifting<T> { fn lift<U: Show>(self, u: U) T }
struct S { k: int }
impl Lifting<int> for S { fn lift<V: Show>(self, v: V) int { return self.k } }
fn main() void {
    let s = S { k = 3 }
    println(s.lift(A { n = 1 }))
}
"#);
    assert_eq!(run(&src).unwrap(), "3\n");
}

#[test]
fn the_traits_argument_and_the_methods_bound_are_both_checked() {
    let src = prog(r#"trait Lifting<T> { fn lift<U: Show>(self, u: U) T }
struct S { k: int }
impl Lifting<int> for S { fn lift<V: Show>(self, v: V) int { return self.k } }
fn main() void {
    let s = S { k = 3 }
    println(s.lift(1))
    let t: string = s.lift(A { n = 2 })
}
"#);
    // A bound is checked once the call's arguments are settled, so its
    // diagnostic can follow a mismatch on a later line.
    let mut found = messages(&src);
    found.sort();
    assert_eq!(codes(&src).0, Stage::Sema);
    assert_eq!(found, vec![
        "the trait `Show` is not implemented for `int`",
        "type mismatch: expected `string`, found `int`",
    ]);
}

const NAMED: &str = r#"trait Named {
    fn name(self) string
    fn tag<U: Show>(self, u: U) string { return self.name() + u.show() }
}
struct S { k: int }
impl Named for S { fn name(self) string { return "s" } }
"#;

#[test]
fn a_default_body_can_have_generic_parameters_with_bounds() {
    let src = prog(&format!("{NAMED}fn main() void {{\n    let s = S {{ k = 1 }}\n    println(s.tag(A {{ n = 1 }}))\n}}\n"));
    assert_eq!(run(&src).unwrap(), "sA\n");
}

#[test]
fn a_call_to_a_default_generic_method_checks_the_bound() {
    let src = prog(&format!("{NAMED}fn main() void {{\n    let s = S {{ k = 1 }}\n    println(s.tag(1))\n}}\n"));
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-126"]));
    assert_eq!(positions(&src), vec![pos_of(&src, "1))\n}")]);
}

#[test]
fn a_default_body_cannot_call_a_method_its_own_parameter_lacks() {
    let src = prog("trait Named {\n    fn name(self) string\n    fn tag<U>(self, u: U) string { return u.show() }\n}\nfn main() void { println(1) }\n");
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-130"]));
}

#[test]
fn a_call_through_a_bounded_parameter_chooses_the_types_and_checks_the_bound() {
    let src = with_mapper(&format!(r#"{MAPPER_IMPL}fn use_it<T: Mapper>(m: T) void {{
    let a: int = m.apply(5)
    let b: string = m.apply(5)
    println(m.render(A {{ n = 1 }}))
    println(m.render(7))
}}
fn main() void {{ println(1) }}
"#));
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-101", "TYPE-126"]));
    assert_eq!(messages(&src), vec![
        "type mismatch: expected `string`, found `int`",
        "the trait `Show` is not implemented for `int`",
    ]);
}

#[test]
fn a_call_through_a_bound_runs_the_impls_generic_method() {
    let src = with_mapper(&format!(r#"{MAPPER_IMPL}fn use_it<T: Mapper>(m: T) string {{
    let n: int = m.apply(5)
    return m.render(A {{ n = n }})
}}
fn main() void {{ println(use_it(Id {{ k = 1 }})) }}
"#));
    assert_eq!(run(&src).unwrap(), "A\n");
}

#[test]
fn the_qualified_form_chooses_the_types_and_checks_the_bound() {
    let src = with_mapper(&format!(r#"{MAPPER_IMPL}fn main() void {{
    let m = Id {{ k = 1 }}
    let a: int = Mapper.apply(m, 5)
    let b: string = Mapper.apply(m, 5)
    println(Mapper.render(m, A {{ n = 1 }}))
    println(Mapper.render(m, 7))
}}
"#));
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-101", "TYPE-126"]));
    assert_eq!(positions(&src).last().copied(), Some(pos_of(&src, "7))\n}")));
}

#[test]
fn the_qualified_form_runs_a_generic_method() {
    let src = with_mapper(&format!(r#"{MAPPER_IMPL}fn main() void {{
    let m = Id {{ k = 1 }}
    println(Mapper.apply(m, 9))
}}
"#));
    assert_eq!(run(&src).unwrap(), "9\n");
}

#[test]
fn an_enum_can_implement_a_trait_with_a_generic_method() {
    let src = prog(r#"trait Mapper { fn apply<U>(self, x: U) U }
enum E { One, Two }
impl Mapper for E { fn apply<V>(self, x: V) V { return x } }
fn main() void {
    let e = E.One
    let s: string = e.apply("x")
    println(s)
}
"#);
    assert_eq!(run(&src).unwrap(), "x\n");
}

#[test]
fn an_impl_body_is_checked_against_its_own_parameters() {
    let src = prog("trait Mapper { fn apply<U>(self, x: U) U }\nstruct Id { k: int }\nimpl Mapper for Id { fn apply<V>(self, x: V) V { return 1 } }\nfn main() void { println(1) }\n");
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-101"]));
    assert_eq!(messages(&src), vec!["type mismatch: expected `V`, found `int`"]);
}
