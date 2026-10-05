//! Nominal traits, slice S1a: `impl Trait for Type` works and is checked.
//!
//! The `.ubl` fixtures show whole programs accepted or rejected. These tests
//! pin the finer contract: the exact code and count of every diagnostic, the
//! exact text of the signature-mismatch message, declaration-order
//! independence, the inherent-before-trait rule, and the runtime contract
//! that a call made THROUGH a trait (a default method calling `self.x()`)
//! runs the trait's method and never an inherent method of the same name.
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

/// Run a program that passes `check_source` and return what it printed.
/// `with_call_sites` is false to run without sema's table of calls resolved
/// through a trait, which is how an interpreter test that skips sema runs.
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

const SHAPE: &str = "trait Shape {\n    fn area(self) int\n    fn name(self) string { return \"shape\" }\n\
                     fn describe(self) string { return $\"{self.name()}:{self.area()}\" }\n}\n\
                     struct Sq { s: int }\nstruct Circle { r: int }\n\
                     impl Shape for Sq { fn area(self) int { return self.s * self.s } }\n\
                     impl Shape for Circle {\n    fn area(self) int { return 3 * self.r * self.r }\n\
                     fn name(self) string { return \"circle\" }\n}\n";

// ── An impl is usable ────────────────────────────────────────────

#[test]
fn an_impl_method_can_be_called() {
    let src = format!("{SHAPE}fn main() void {{ println(Sq {{ s = 3 }}.area()) }}\n");
    assert_eq!(run(&src).unwrap(), "9\n");
}

#[test]
fn two_types_implementing_one_trait_each_run_their_own_body() {
    let src = format!("{SHAPE}fn main() void {{\n    println(Sq {{ s = 3 }}.area())\n    println(Circle {{ r = 2 }}.area())\n}}\n");
    assert_eq!(run(&src).unwrap(), "9\n12\n");
}

#[test]
fn a_default_method_is_inherited_when_the_impl_omits_it() {
    let src = format!("{SHAPE}fn main() void {{ println(Sq {{ s = 3 }}.name()) }}\n");
    assert_eq!(run(&src).unwrap(), "shape\n");
}

#[test]
fn a_default_method_is_overridden_by_the_impl() {
    let src = format!("{SHAPE}fn main() void {{ println(Circle {{ r = 2 }}.name()) }}\n");
    assert_eq!(run(&src).unwrap(), "circle\n");
}

#[test]
fn a_default_that_calls_other_methods_dispatches_to_the_implementing_type() {
    let src = format!("{SHAPE}fn main() void {{\n    println(Sq {{ s = 3 }}.describe())\n    println(Circle {{ r = 2 }}.describe())\n}}\n");
    assert_eq!(run(&src).unwrap(), "shape:9\ncircle:12\n");
}

#[test]
fn an_associated_default_with_no_self_is_inherited_and_overridable() {
    let src = "trait Named { fn name() string { return \"default\" } }\n\
               struct A { v: int }\nstruct B { v: int }\nimpl Named for A { }\n\
               impl Named for B { fn name() string { return \"bee\" } }\n\
               fn main() void {\n    println(A.name())\n    println(B.name())\n}\n";
    assert_eq!(run(src).unwrap(), "default\nbee\n");
}

#[test]
fn an_empty_impl_inherits_every_default() {
    let src = "trait Marker { fn tag(self) string { return \"marked\" } }\nstruct A { v: int }\nimpl Marker for A { }\n\
               fn main() void { println(A { v = 1 }.tag()) }\n";
    assert_eq!(run(src).unwrap(), "marked\n");
}

#[test]
fn a_method_with_an_argument_dispatches_and_is_checked() {
    let src = "trait Scale { fn scale(self, k: int) int }\nstruct Sq { s: int }\n\
               impl Scale for Sq { fn scale(self, k: int) int { return self.s * k } }\n\
               fn main() void { println(Sq { s = 3 }.scale(4)) }\n";
    assert_eq!(run(src).unwrap(), "12\n");
}

#[test]
fn an_enum_implements_a_trait_on_unit_and_payload_variants() {
    let src = "enum Color { Red, Rgb(int, int, int) }\ntrait D {\n    fn label(self) string\n\
               fn shout(self) string { return $\"{self.label()}!\" }\n}\n\
               impl D for Color {\n    fn label(self) string {\n        match self {\n\
               Color.Red => { return \"red\" }\n            Color.Rgb(r, g, b) => { return $\"rgb{r + g + b}\" }\n        }\n    }\n}\n\
               fn main() void {\n    println(Color.Red.label())\n    println(Color.Rgb(1, 2, 3).shout())\n}\n";
    assert_eq!(run(src).unwrap(), "red\nrgb6!\n");
}

#[test]
fn declaration_order_does_not_matter() {
    // The impl comes first, then the struct, then the trait.
    let src = "impl Shape for Sq { fn area(self) int { return self.s } }\nstruct Sq { s: int }\n\
               trait Shape { fn area(self) int }\nfn main() void { println(Sq { s = 5 }.area()) }\n";
    assert_eq!(run(src).unwrap(), "5\n");
}

#[test]
fn self_in_a_trait_signature_is_the_implementing_type() {
    let src = "trait Maker {\n    fn make() Self\n    fn dup(self) Self\n    fn id(self) int\n}\nstruct Sq { s: int }\n\
               impl Maker for Sq {\n    fn make() Sq { return Sq { s = 1 } }\n\
               fn dup(self) Sq { return Sq { s = self.s + 1 } }\n    fn id(self) int { return self.s }\n}\n\
               fn main() void { println(Sq.make().dup().id()) }\n";
    assert_eq!(run(src).unwrap(), "2\n");
}

#[test]
fn an_impl_through_a_type_alias_implements_the_real_struct() {
    let src = "struct Point { x: int, y: int }\ntype P = Point\ntrait Sum { fn sum(self) int }\n\
               impl Sum for P { fn sum(self) int { return self.x + self.y } }\n\
               fn main() void { println(Point { x = 3, y = 4 }.sum()) }\n";
    assert_eq!(run(src).unwrap(), "7\n");
}

// ── Inherent first, trait through a trait ────────────────────────

const BOTH: &str = "trait Shape {\n    fn area(self) int\n    fn twice(self) int { return self.area() * 2 }\n}\n\
                    struct Sq { s: int }\n";

#[test]
fn an_inherent_method_beats_a_trait_method_when_the_extend_comes_first() {
    let src = format!("{BOTH}extend Sq {{ fn area(self) int {{ return 7 }} }}\n\
                       impl Shape for Sq {{ fn area(self) int {{ return self.s * self.s }} }}\n\
                       fn main() void {{ println(Sq {{ s = 3 }}.area()) }}\n");
    assert_eq!(run(&src).unwrap(), "7\n");
}

#[test]
fn an_inherent_method_beats_a_trait_method_when_the_impl_comes_first() {
    let src = format!("{BOTH}impl Shape for Sq {{ fn area(self) int {{ return self.s * self.s }} }}\n\
                       extend Sq {{ fn area(self) int {{ return 7 }} }}\n\
                       fn main() void {{ println(Sq {{ s = 3 }}.area()) }}\n");
    assert_eq!(run(&src).unwrap(), "7\n");
}

#[test]
fn the_trait_method_is_reachable_qualified_even_when_an_inherent_one_wins() {
    let src = format!("{BOTH}extend Sq {{ fn area(self) int {{ return 7 }} }}\n\
                       impl Shape for Sq {{ fn area(self) int {{ return self.s * self.s }} }}\n\
                       fn main() void {{ println(Shape.area(Sq {{ s = 3 }})) }}\n");
    assert_eq!(run(&src).unwrap(), "9\n");
}

#[test]
fn a_default_method_reaches_the_traits_method_not_the_inherent_one() {
    // `twice` calls `self.area()` through the trait: 9 * 2, never 7 * 2.
    let src = format!("{BOTH}extend Sq {{ fn area(self) int {{ return 7 }} }}\n\
                       impl Shape for Sq {{ fn area(self) int {{ return self.s * self.s }} }}\n\
                       fn main() void {{ println(Sq {{ s = 3 }}.twice()) }}\n");
    assert_eq!(run(&src).unwrap(), "18\n");
}

#[test]
fn without_the_call_site_table_a_bound_call_runs_the_flat_table_entry() {
    // Documents why `trait_call_sites` exists: sema resolved `self.area()`
    // in `twice` through the trait, and only that table tells the
    // interpreter. Without it the flat table's inherent `area` runs (14).
    let src = format!("{BOTH}extend Sq {{ fn area(self) int {{ return 7 }} }}\n\
                       impl Shape for Sq {{ fn area(self) int {{ return self.s * self.s }} }}\n\
                       fn main() void {{ println(Sq {{ s = 3 }}.twice()) }}\n");
    assert_eq!(run_with(&src, false).unwrap(), "14\n");
}

// ── Qualified calls and ambiguity ────────────────────────────────

const TWO: &str = "trait A { fn go(self) int }\ntrait B { fn go(self) int }\nstruct T { v: int }\n\
                   impl A for T { fn go(self) int { return 1 } }\nimpl B for T { fn go(self) int { return 2 } }\n";

#[test]
fn a_qualified_call_picks_the_named_traits_method() {
    let src = format!("{TWO}fn main() void {{\n    let t = T {{ v = 0 }}\n    println(A.go(t))\n    println(B.go(t))\n}}\n");
    assert_eq!(run(&src).unwrap(), "1\n2\n");
}

#[test]
fn an_unqualified_call_two_traits_both_supply_is_one_type_127() {
    let src = format!("{TWO}fn main() void {{\n    let t = T {{ v = 0 }}\n    println(t.go())\n}}\n");
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-127"]));
}

#[test]
fn the_ambiguity_message_lists_both_traits_and_suggests_the_qualified_form() {
    let src = format!("{TWO}fn main() void {{\n    let t = T {{ v = 0 }}\n    println(t.go())\n}}\n");
    let report = check_source(&src);
    assert_eq!(report.diagnostics.len(), 1);
    assert_eq!(report.diagnostics[0].message,
               "method `go` on `T` is supplied by more than one trait (`A`, `B`)");
}

#[test]
fn an_inherent_method_removes_the_ambiguity() {
    // A let-bound receiver; the next test covers the other declaration order.
    let src = format!("{TWO}extend T {{ fn go(self) int {{ return 100 }} }}\n\
                       fn main() void {{\n    let t = T {{ v = 0 }}\n    println(t.go())\n}}\n");
    assert_eq!(codes(&src), (Stage::Clean, vec![]));
    assert_eq!(run(&src).unwrap(), "100\n");
}

#[test]
fn an_inherent_method_removes_the_ambiguity_whichever_block_comes_first() {
    let src = "trait A { fn go(self) int }\ntrait B { fn go(self) int }\nstruct T { v: int }\n\
               extend T { fn go(self) int { return 100 } }\n\
               impl A for T { fn go(self) int { return 1 } }\nimpl B for T { fn go(self) int { return 2 } }\n\
               fn main() void {\n    let t = T { v = 0 }\n    println(t.go())\n}\n";
    assert_eq!(codes(src), (Stage::Clean, vec![]));
    assert_eq!(run(src).unwrap(), "100\n");
}

#[test]
fn a_qualified_call_on_a_type_without_the_impl_is_type_126() {
    let src = "trait Shape { fn area(self) int }\nstruct Sq { s: int }\nstruct O { x: int }\n\
               impl Shape for Sq { fn area(self) int { return 1 } }\n\
               fn main() void { let a = Shape.area(O { x = 1 }) }\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["TYPE-126"]));
}

#[test]
fn a_qualified_call_of_a_method_the_trait_lacks_is_type_104() {
    let src = "trait Shape { fn area(self) int }\nstruct Sq { s: int }\nimpl Shape for Sq { fn area(self) int { return 1 } }\n\
               fn main() void { let a = Shape.nothing(Sq { s = 1 }) }\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["TYPE-104"]));
}

#[test]
fn a_qualified_call_with_no_receiver_is_type_102() {
    let src = "trait Shape { fn area(self) int }\nstruct Sq { s: int }\nimpl Shape for Sq { fn area(self) int { return 1 } }\n\
               fn main() void { let a = Shape.area() }\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["TYPE-102"]));
}

// ── Conformance ──────────────────────────────────────────────────

const REQUIRED: &str = "trait Shape {\n    fn area(self) int\n    fn perimeter(self) int\n}\nstruct Sq { s: int }\n";

#[test]
fn a_missing_required_method_is_one_type_123() {
    let src = format!("{REQUIRED}impl Shape for Sq {{ fn area(self) int {{ return 1 }} }}\nfn main() void {{ }}\n");
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-123"]));
}

#[test]
fn a_missing_method_with_a_default_is_not_reported() {
    let src = "trait Shape { fn area(self) int\n    fn extra(self) int { return 0 } }\nstruct Sq { s: int }\n\
               impl Shape for Sq { fn area(self) int { return 1 } }\nfn main() void { }\n";
    assert_eq!(codes(src), (Stage::Clean, vec![]));
}

#[test]
fn a_method_the_trait_does_not_declare_is_one_type_124() {
    let src = format!("{REQUIRED}impl Shape for Sq {{\n    fn area(self) int {{ return 1 }}\n    fn perimeter(self) int {{ return 1 }}\n\
                       fn extra(self) int {{ return 2 }}\n}}\nfn main() void {{ }}\n");
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-124"]));
}

#[test]
fn a_different_return_type_is_one_type_125_with_both_signatures() {
    let src = "trait Shape { fn area(self) int }\nstruct Sq { s: int }\n\
               impl Shape for Sq { fn area(self) string { return \"x\" } }\nfn main() void { }\n";
    assert_eq!(messages(src),
               vec!["method `area` does not match trait `Shape`: expected `fn(self) int`, found `fn(self) string`".to_string()]);
}

#[test]
fn every_part_of_a_signature_is_compared() {
    let src = "trait Api {\n    fn a(self) int\n    fn b(self, n: int) int\n    fn c(self, n: int) int\n    fn d(self) int\n    fn ok(self) int\n}\n\
               struct S { v: int }\nimpl Api for S {\n    fn a(self) string { return \"x\" }\n    fn b(self, n: string) int { return 1 }\n\
               fn c(self) int { return 1 }\n    fn d() int { return 1 }\n    fn ok(self) int { return 1 }\n}\nfn main() void { }\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["TYPE-125", "TYPE-125", "TYPE-125", "TYPE-125"]));
}

#[test]
fn a_signature_mismatch_is_not_also_reported_as_missing() {
    let src = "trait Shape { fn area(self) int }\nstruct Sq { s: int }\n\
               impl Shape for Sq { fn area(self) string { return \"x\" } }\nfn main() void { }\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["TYPE-125"]));
}

#[test]
fn a_matching_self_return_is_accepted() {
    let src = "trait Maker { fn dup(self) Self }\nstruct Sq { s: int }\n\
               impl Maker for Sq { fn dup(self) Sq { return Sq { s = 1 } } }\nfn main() void { }\n";
    assert_eq!(codes(src), (Stage::Clean, vec![]));
}

// ── Names and overlap ────────────────────────────────────────────

#[test]
fn a_second_impl_of_one_trait_for_one_type_is_one_type_128() {
    let src = "trait Shape { fn area(self) int }\nstruct Sq { s: int }\n\
               impl Shape for Sq { fn area(self) int { return 1 } }\nimpl Shape for Sq { fn area(self) int { return 2 } }\nfn main() void { }\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["TYPE-128"]));
}

#[test]
fn an_unknown_trait_name_is_one_type_122() {
    let src = "struct P { x: int }\nimpl Missing for P { fn m(self) int { return 1 } }\nfn main() void { }\n";
    assert_eq!(messages(src), vec!["unknown trait `Missing`".to_string()]);
}

#[test]
fn a_struct_named_where_a_trait_belongs_is_one_type_122() {
    let src = "struct P { x: int }\nstruct Sq { s: int }\nimpl P for Sq { fn m(self) int { return 1 } }\nfn main() void { }\n";
    assert_eq!(messages(src), vec!["`P` is a struct, not a trait".to_string()]);
}

#[test]
fn an_unknown_trait_does_not_cascade_into_errors_about_its_methods() {
    let src = "struct P { x: int }\nimpl Missing for P {\n    fn a(self) int { return 1 }\n    fn b(self) int { return 2 }\n}\nfn main() void { }\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["TYPE-122"]));
}

// ── Self inside a default body ───────────────────────────────────

#[test]
fn an_unknown_method_on_self_in_a_default_names_self() {
    let src = "trait Shape {\n    fn area(self) int\n    fn bad(self) int { return self.nothing() }\n}\n\
               struct Sq { s: int }\nimpl Shape for Sq { fn area(self) int { return 1 } }\nfn main() void { }\n";
    assert_eq!(messages(src), vec!["type `Self` has no method `nothing`".to_string()]);
}

#[test]
fn a_default_may_call_the_traits_other_methods_on_self() {
    let src = "trait Shape {\n    fn area(self) int\n    fn doubled(self) int { return self.area() + self.area() }\n}\n\
               struct Sq { s: int }\nimpl Shape for Sq { fn area(self) int { return 4 } }\n\
               fn main() void { println(Sq { s = 0 }.doubled()) }\n";
    assert_eq!(run(src).unwrap(), "8\n");
}

// ── Features that are not built yet are reported (TYPE-129) ──────

#[test]
fn an_impl_for_a_builtin_type_is_reported_not_ignored() {
    let src = "trait Shape { fn area(self) int }\nimpl Shape for int { fn area(self) int { return 1 } }\nfn main() void { }\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["TYPE-129"]));
}

#[test]
fn a_generic_trait_is_reported_not_ignored() {
    let src = "trait Conv<T> { fn conv(self) T }\nfn main() void { }\n";
    assert_eq!(messages(src), vec!["a generic trait is not supported yet".to_string()]);
}

#[test]
fn an_associated_type_is_reported_not_ignored() {
    let src = "trait Holder {\n    type Item\n    fn get(self) int\n}\nfn main() void { }\n";
    assert_eq!(messages(src), vec!["an associated type is not supported yet".to_string()]);
}

#[test]
fn an_impl_for_a_generic_struct_is_reported_not_ignored() {
    let src = "trait Shape { fn area(self) int }\nstruct Box<T> { v: T }\n\
               impl Shape for Box { fn area(self) int { return 1 } }\nfn main() void { }\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["TYPE-129"]));
}
