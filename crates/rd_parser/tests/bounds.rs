//! Trait bounds, slice S1b: `fn f<T: Shape>(x: T)` is enforced.
//!
//! The `.ubl` fixtures show whole programs accepted or rejected. These tests
//! pin the finer contract: the exact code, count and position of every
//! diagnostic, that a call made through a bound runs the TRAIT's method and
//! never an inherent method of the same name, and that a bound is enforced
//! everywhere a bounded parameter is instantiated (a call, a struct or enum
//! construction, a type annotation, a function held in a variable).
//!
//! See docs/TRAITS_DESIGN.md and docs/PARKED_IDEAS.md, "Traits".

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
fn lines(source: &str) -> Vec<(usize, usize)> {
    check_source(source)
        .diagnostics
        .iter()
        .map(|d| (d.primary_span.line as usize, d.primary_span.column as usize))
        .collect()
}

/// Run a program that passes `check_source` and return what it printed.
/// `with_call_sites` is false to run without sema's table of calls resolved
/// through a trait.
fn run_with(source: &str, with_call_sites: bool) -> Result<String, String> {
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
    if with_call_sites {
        interp.set_trait_call_sites(ctx.trait_call_sites);
    }
    let result = interp.run_program(&program);
    let output = ubel_stratum::builtins::global::io::take_captured_output();
    result.map(|_| output)
}

fn run(source: &str) -> Result<String, String> {
    run_with(source, true)
}

/// Two traits, three structs, three impls. `Sq` implements both traits,
/// `Circle` only `Shape`, `Plain` neither.
const PRELUDE: &str = r#"trait Shape {
    fn area(self) int
}
trait Named {
    fn name(self) string
}
struct Sq { s: int }
struct Circle { r: int }
struct Plain { p: int }
impl Shape for Sq { fn area(self) int { return self.s * self.s } }
impl Shape for Circle { fn area(self) int { return 3 * self.r * self.r } }
impl Named for Sq { fn name(self) string { return "sq" } }
"#;

fn prog(body: &str) -> String {
    format!("{PRELUDE}{body}")
}

/// The line (1-based) the first line of `body` lands on after `PRELUDE`.
fn body_line() -> usize {
    PRELUDE.lines().count() + 1
}

// ── A call through a bound runs the trait's method ───────────────

#[test]
fn a_call_through_a_bound_runs_the_trait_method_for_each_type() {
    let src = prog(r#"fn total<T: Shape>(x: T) int { return x.area() }
fn main() void {
    println(total(Sq { s = 3 }))
    println(total(Circle { r = 2 }))
}
"#);
    assert_eq!(run(&src).unwrap(), "9\n12\n");
}

#[test]
fn a_bound_call_runs_the_trait_method_and_never_an_inherent_one() {
    // `Sq` has an inherent `area` returning 1 beside the trait's. Called
    // directly the inherent one wins; called through the bound the
    // trait's runs.
    let src = prog(r#"extend Sq { fn area(self) int { return 1 } }
fn total<T: Shape>(x: T) int { return x.area() }
fn main() void {
    let q = Sq { s = 3 }
    println(total(q))
    println(q.area())
}
"#);
    assert_eq!(run(&src).unwrap(), "9\n1\n");
}

#[test]
fn without_the_call_site_table_a_bound_call_runs_the_flat_table_entry() {
    // Documents why `trait_call_sites` exists: sema resolved `x.area()`
    // through the bound, and only that table tells the interpreter.
    let src = prog(r#"extend Sq { fn area(self) int { return 1 } }
fn total<T: Shape>(x: T) int { return x.area() }
fn main() void { println(total(Sq { s = 3 })) }
"#);
    assert_eq!(run_with(&src, true).unwrap(), "9\n");
    assert_eq!(run_with(&src, false).unwrap(), "1\n");
}

#[test]
fn two_bounds_both_supply_methods() {
    let src = prog(r#"fn both<T: Shape + Named>(x: T) string { return x.name() }
fn sum<T: Shape + Named>(x: T) int { return x.area() }
fn main() void {
    println(both(Sq { s = 2 }))
    println(sum(Sq { s = 2 }))
}
"#);
    assert_eq!(run(&src).unwrap(), "sq\n4\n");
}

#[test]
fn a_bound_may_name_a_trait_declared_later() {
    let src = r#"fn total<T: Shape>(x: T) int { return x.area() }
fn main() void { println(total(Sq { s = 3 })) }
struct Sq { s: int }
impl Shape for Sq { fn area(self) int { return self.s * self.s } }
trait Shape { fn area(self) int }
"#;
    assert_eq!(run(src).unwrap(), "9\n");
}

#[test]
fn a_bounded_generic_function_may_call_itself_and_another_bounded_one() {
    let src = prog(r#"fn total<T: Shape>(x: T) int { return x.area() }
fn depth<T: Shape>(x: T, n: int) int {
    if n == 0 { return total(x) }
    return depth(x, n - 1)
}
fn main() void { println(depth(Circle { r = 1 }, 3)) }
"#);
    assert_eq!(run(&src).unwrap(), "3\n");
}

#[test]
fn a_default_method_may_be_called_through_a_bound() {
    let src = r#"trait Shape {
    fn area(self) int
    fn twice(self) int { return self.area() * 2 }
}
struct Sq { s: int }
impl Shape for Sq { fn area(self) int { return self.s * self.s } }
fn dbl<T: Shape>(x: T) int { return x.twice() }
fn main() void { println(dbl(Sq { s = 3 })) }
"#;
    assert_eq!(run(src).unwrap(), "18\n");
}

#[test]
fn a_value_reached_through_an_arena_still_satisfies_the_bound() {
    let src = prog(r#"@tier(mid)
fn total<T: Shape>(x: T) int { return x.area() }
@tier(mid)
fn go() void {
    with arena(64KB) {
        let q = Sq { s = 3 }
        println(total(q))
    }
}
fn main() void { go() }
"#);
    assert_eq!(run(&src).unwrap(), "9\n");
}

#[test]
fn a_type_alias_to_a_struct_satisfies_the_bound_of_the_struct() {
    let src = prog(r#"type Square = Sq
fn total<T: Shape>(x: T) int { return x.area() }
fn main() void {
    let q: Square = Sq { s = 3 }
    println(total(q))
}
"#);
    assert_eq!(run(&src).unwrap(), "9\n");
}

// ── Generic structs and enums with bounds ────────────────────────

#[test]
fn a_bounded_struct_method_calls_through_the_bound() {
    let src = prog(r#"extend Sq { fn area(self) int { return 1 } }
struct Holder<T: Shape> {
    v: T,
    fn get_area(self) int { return self.v.area() }
}
fn main() void {
    let h = Holder { v = Sq { s = 3 } }
    println(h.get_area())
}
"#);
    assert_eq!(run(&src).unwrap(), "9\n");
}

#[test]
fn an_extend_block_on_a_bounded_struct_sees_the_bound() {
    let src = prog(r#"struct Holder<T: Shape> { v: T }
extend Holder {
    fn twice(self) int { return self.v.area() * 2 }
}
fn main() void {
    let h = Holder { v = Circle { r = 2 } }
    println(h.twice())
}
"#);
    assert_eq!(run(&src).unwrap(), "24\n");
}

#[test]
fn a_bounded_generic_function_may_build_a_bounded_struct_from_its_own_parameter() {
    let src = prog(r#"struct Holder<T: Shape> {
    v: T,
    fn get_area(self) int { return self.v.area() }
}
fn wrap<T: Shape>(x: T) int {
    let h = Holder { v = x }
    return h.get_area()
}
fn main() void { println(wrap(Sq { s = 4 })) }
"#);
    assert_eq!(run(&src).unwrap(), "16\n");
}

#[test]
fn a_bounded_enum_accepts_an_implementing_payload() {
    let src = prog(r#"enum Slot<T: Shape> {
    Full(T),
    Empty,
}
fn main() void {
    let a = Slot.Full(Sq { s = 5 })
    let b: Slot<Sq> = Slot.Empty
    println(1)
}
"#);
    assert_eq!(run(&src).unwrap(), "1\n");
}

#[test]
fn an_enum_variant_left_unconstrained_is_not_an_error() {
    // Nothing says what `T` is, so nothing says the bound fails.
    let src = prog(r#"enum Slot<T: Shape> {
    Full(T),
    Empty,
}
fn main() void {
    let e = Slot.Empty
    println(1)
}
"#);
    assert_eq!(codes(&src), (Stage::Clean, vec![]));
}

// ── The bound is enforced at the call ────────────────────────────

#[test]
fn a_primitive_argument_fails_the_bound_once_and_at_the_argument() {
    let src = prog(r#"fn total<T: Shape>(x: T) int { return x.area() }
fn main() void {
    println(total(5))
}
"#);
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-126"]));
    // Line 3 of the body, at the literal `5`.
    assert_eq!(lines(&src), vec![(body_line() + 2, 19)]);
}

#[test]
fn an_integer_literal_is_reported_as_its_settled_type() {
    let src = prog(r#"fn total<T: Shape>(x: T) int { return x.area() }
fn main() void { println(total(5)) }
"#);
    let msgs = messages(&src);
    assert_eq!(msgs.len(), 1);
    assert!(msgs[0].contains("`int`"), "should name the settled type, got {msgs:?}");
    assert!(!msgs[0].contains("{integer}"), "must not leak the literal variable, got {msgs:?}");
}

#[test]
fn a_struct_without_the_impl_fails_the_bound() {
    let src = prog(r#"fn total<T: Shape>(x: T) int { return x.area() }
fn main() void { println(total(Plain { p = 1 })) }
"#);
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-126"]));
    let msgs = messages(&src);
    assert!(msgs[0].contains("Plain") && msgs[0].contains("Shape"), "got {msgs:?}");
}

#[test]
fn a_type_parameter_without_the_bound_cannot_be_passed_to_a_bounded_function() {
    let src = prog(r#"fn total<T: Shape>(x: T) int { return x.area() }
fn relay<T>(x: T) int { return total(x) }
fn main() void { println(relay(Sq { s = 1 })) }
"#);
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-126"]));
    assert!(messages(&src)[0].contains("`T`"), "should name the parameter, got {:?}", messages(&src));
}

#[test]
fn a_type_parameter_with_the_bound_can_be_passed_on() {
    let src = prog(r#"fn total<T: Shape>(x: T) int { return x.area() }
fn relay<T: Shape>(x: T) int { return total(x) }
fn main() void { println(relay(Sq { s = 2 })) }
"#);
    assert_eq!(run(&src).unwrap(), "4\n");
}

#[test]
fn the_bound_is_checked_through_a_list_argument() {
    let src = prog(r#"fn count<T: Shape>(xs: List<T>) int { return 0 }
fn main() void {
    println(count([Sq { s = 1 }]))
    println(count([1, 2]))
}
"#);
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-126"]));
    assert_eq!(lines(&src)[0].0, body_line() + 3);
}

#[test]
fn a_bounded_function_held_in_a_variable_keeps_its_bound() {
    let src = prog(r#"fn total<T: Shape>(x: T) int { return x.area() }
fn main() void {
    let f = total
    println(f(Sq { s = 2 }))
    println(f(7))
}
"#);
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-126"]));
    assert_eq!(lines(&src)[0].0, body_line() + 4);
}

#[test]
fn two_mistakes_are_two_diagnostics_and_one_mistake_is_one() {
    let src = prog(r#"fn total<T: Shape>(x: T) int { return x.area() }
fn main() void {
    println(total(1))
    println(total("s"))
    println(total(Plain { p = 1 }))
}
"#);
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-126", "TYPE-126", "TYPE-126"]));
}

// ── The bound is enforced on structs and enums ───────────────────

#[test]
fn constructing_a_bounded_struct_with_a_bad_argument_fails_the_bound() {
    let src = prog(r#"struct Holder<T: Shape> { v: T }
fn main() void {
    let h = Holder { v = Plain { p = 1 } }
    println(1)
}
"#);
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-126"]));
    assert_eq!(lines(&src)[0].0, body_line() + 2);
}

#[test]
fn an_annotation_naming_a_bad_argument_fails_the_bound_at_the_annotation() {
    let src = prog(r#"struct Holder<T: Shape> { v: T }
fn show(h: Holder<Plain>) void { println(1) }
fn main() void { println(1) }
"#);
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-126"]));
    assert_eq!(lines(&src)[0].0, body_line() + 1);
}

#[test]
fn an_annotation_and_the_construction_it_annotates_are_each_reported() {
    // Two sites, two diagnostics, each at its own position.
    let src = prog(r#"struct Holder<T: Shape> { v: T }
fn main() void {
    let h: Holder<Plain> = Holder { v = Plain { p = 1 } }
    println(1)
}
"#);
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-126", "TYPE-126"]));
    let found = lines(&src);
    assert_eq!(found[0].0, body_line() + 2);
    assert_eq!(found[1].0, body_line() + 2);
    assert_ne!(found[0].1, found[1].1, "the two sites must differ in column, got {found:?}");
}

#[test]
fn a_bounded_enum_variant_with_a_bad_payload_fails_the_bound() {
    let src = prog(r#"enum Slot<T: Shape> {
    Full(T),
    Empty,
}
fn main() void {
    let s = Slot.Full(Plain { p = 2 })
    println(1)
}
"#);
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-126"]));
}

#[test]
fn a_struct_field_cannot_use_a_bounded_struct_with_an_unbounded_parameter() {
    let src = prog(r#"struct Holder<T: Shape> { v: T }
struct Wrap<T> { h: Holder<T> }
fn main() void { println(1) }
"#);
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-126"]));
    assert_eq!(lines(&src)[0].0, body_line() + 1);
}

#[test]
fn a_struct_field_may_use_a_bounded_struct_with_a_bounded_parameter() {
    let src = prog(r#"struct Holder<T: Shape> { v: T }
struct Wrap<T: Shape> { h: Holder<T> }
fn main() void { println(1) }
"#);
    assert_eq!(codes(&src), (Stage::Clean, vec![]));
}

// ── A method call on a type parameter ────────────────────────────

#[test]
fn a_method_call_on_an_unbounded_parameter_is_type_130() {
    let src = prog(r#"fn total<T>(x: T) int { return x.area() }
fn main() void { println(total(Sq { s = 3 })) }
"#);
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-130"]));
    // At the callee `x.area`, not the whole call.
    assert_eq!(lines(&src), vec![(body_line(), 32)]);
    assert!(messages(&src)[0].contains("`area`") && messages(&src)[0].contains("`T`"),
            "should name method and parameter, got {:?}", messages(&src));
}

#[test]
fn each_call_on_an_unbounded_parameter_is_its_own_diagnostic() {
    let src = prog(r#"fn both<T>(x: T) int { return x.area() + x.area() }
fn main() void { println(1) }
"#);
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-130", "TYPE-130"]));
}

#[test]
fn a_bound_that_lacks_the_method_is_no_such_method_not_type_130() {
    let src = prog(r#"fn total<T: Named>(x: T) int { return x.area() }
fn main() void { println(1) }
"#);
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-104"]));
}

#[test]
fn a_method_call_on_an_unbounded_struct_parameter_is_type_130() {
    let src = r#"struct Box<T> {
    v: T,
    fn show(self) int { return self.v.area() }
}
fn main() void { println(1) }
"#;
    assert_eq!(codes(src), (Stage::Sema, vec!["TYPE-130"]));
}

#[test]
fn an_extend_on_an_unbounded_generic_struct_names_the_parameter() {
    let src = r#"struct Box<T> { v: T }
extend Box {
    fn show(self) int { return self.v.area() }
}
fn main() void { println(1) }
"#;
    assert_eq!(codes(src), (Stage::Sema, vec!["TYPE-130"]));
    assert!(messages(src)[0].contains("`T`"), "should name `T`, got {:?}", messages(src));
}

#[test]
fn two_bounds_with_the_same_method_are_ambiguous_at_the_call() {
    let src = r#"trait A { fn go(self) int }
trait B { fn go(self) int }
fn run<T: A + B>(x: T) int { return x.go() }
fn main() void { println(1) }
"#;
    assert_eq!(codes(src), (Stage::Sema, vec!["TYPE-127"]));
}

#[test]
fn a_fieldless_generic_function_with_no_method_calls_is_unaffected() {
    let src = r#"fn ident<T>(x: T) T { return x }
fn first<T>(xs: List<T>) T { return xs[0] }
fn main() void {
    println(ident(3))
    println(first([4, 5]))
}
"#;
    assert_eq!(run(src).unwrap(), "3\n4\n");
}

// ── Bound declarations are validated ─────────────────────────────

#[test]
fn a_bound_naming_nothing_is_type_122_once() {
    let src = prog(r#"fn a<T: Missing>(x: T) int { return 1 }
fn main() void { println(a(1)) }
"#);
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-122"]));
    assert_eq!(lines(&src), vec![(body_line(), 6)]);
}

#[test]
fn a_bound_naming_a_struct_is_type_122() {
    let src = prog(r#"fn a<T: Plain>(x: T) int { return 1 }
fn main() void { println(1) }
"#);
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-122"]));
    assert!(messages(&src)[0].contains("struct"), "should say what it is, got {:?}", messages(&src));
}

#[test]
fn a_bound_on_a_struct_or_enum_parameter_is_validated_too() {
    let src = prog(r#"struct S<T: Missing> { v: T }
enum E<T: Plain> { A(T) }
fn main() void { println(1) }
"#);
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-122", "TYPE-122"]));
}

#[test]
fn a_bound_naming_hash_is_type_129_until_the_hasher_exists() {
    // The other five derive names are prelude traits now (see prelude.rs).
    let src = "fn a<T: Hash>(x: T) int { return 1 }\nfn main() void { println(1) }\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["TYPE-129"]));
}

#[test]
fn a_user_trait_named_like_a_derive_is_a_real_bound() {
    let src = r#"trait Ord { fn rank(self) int }
struct A { v: int }
impl Ord for A { fn rank(self) int { return self.v } }
fn top<T: Ord>(x: T) int { return x.rank() }
fn main() void { println(top(A { v = 7 })) }
"#;
    assert_eq!(run(src).unwrap(), "7\n");
}

#[test]
fn a_bound_on_a_type_alias_parameter_is_type_129() {
    let src = prog(r#"type Pair<T: Shape> = List<T>
fn main() void { println(1) }
"#);
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-129"]));
}

#[test]
fn a_bound_on_a_methods_own_generic_parameter_is_type_129() {
    let src = prog(r#"struct H {
    v: int,
    fn m<U: Shape>(self, u: U) int { return 1 }
}
fn main() void { println(1) }
"#);
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-129"]));
}

#[test]
fn a_repeated_bound_is_one_bound() {
    let src = prog(r#"fn total<T: Shape + Shape>(x: T) int { return x.area() }
fn main() void { println(total(Sq { s = 3 })) }
"#);
    assert_eq!(run(&src).unwrap(), "9\n");
}

// ── Order ────────────────────────────────────────────────────────

#[test]
fn diagnostics_come_out_in_source_order_even_for_a_literal_settled_late() {
    // The literal `5` is only settled at the end of the body, after the
    // string and the struct beside it were already known to fail.
    let src = prog(r#"fn total<T: Shape>(x: T) int { return x.area() }
fn main() void {
    println(total(5))
    println(total("s"))
    println(total(Plain { p = 1 }))
}
"#);
    let found = lines(&src);
    let rows: Vec<usize> = found.iter().map(|(l, _)| *l).collect();
    assert_eq!(rows, vec![body_line() + 2, body_line() + 3, body_line() + 4], "got {found:?}");
}

#[test]
fn a_satisfied_program_with_many_instantiations_is_clean() {
    let src = prog(r#"struct Holder<T: Shape> { v: T }
enum Slot<T: Shape> { Full(T), Empty }
fn total<T: Shape>(x: T) int { return x.area() }
fn main() void {
    let a = Holder { v = Sq { s = 1 } }
    let b = Holder { v = Circle { r = 1 } }
    let c: Holder<Sq> = Holder { v = Sq { s = 2 } }
    let d = Slot.Full(Circle { r = 2 })
    println(total(Sq { s = 1 }) + total(Circle { r = 1 }))
}
"#);
    assert_eq!(codes(&src), (Stage::Clean, vec![]));
}

#[test]
fn a_nested_call_reports_the_outer_obligation_before_the_inner_one() {
    // The inner call is inferred first, so its obligation is raised first,
    // but the outer one starts earlier in the source. Reported by position.
    let src = prog(r#"fn make<T: Shape>(x: T) T { return x }
fn total<T: Shape>(x: T) int { return x.area() }
fn main() void {
    println(total(make(Plain { p = 1 })))
}
"#);
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-126", "TYPE-126"]));
    let found = lines(&src);
    assert_eq!(found[0].0, found[1].0, "both on one line, got {found:?}");
    assert!(found[0].1 < found[1].1, "outer (first column) must come first, got {found:?}");
}
