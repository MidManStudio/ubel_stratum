//! Supertraits, slice S3a: `trait B: A + C`.
//!
//! The `.ubl` fixtures show whole programs accepted or rejected. These tests
//! pin the finer contract: the parsed shape and spans, the exact code and
//! count of every diagnostic, the exact message text, what a bound implies
//! (inside a body and at a call), the order the chain is checked in, and the
//! runtime contract that a call made through a bound reaches the
//! supertrait's method.
//!
//! See docs/TRAITS_DESIGN.md, "What S3a settled".

use ubel_stratum::ast::arena::AstArena;
use ubel_stratum::ast::root::Item;
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

fn suggestions(source: &str) -> Vec<Option<String>> {
    check_source(source).diagnostics.iter().map(|d| d.suggestion.clone()).collect()
}

fn clean(source: &str) -> bool {
    codes(source) == (Stage::Clean, vec![])
}

/// Number of parse errors, or `Ok` when the source parses.
fn parse_errors(source: &str) -> Result<(), usize> {
    let tokens = ubel_stratum::lexer::tokenize(source).expect("source should lex");
    let arena = AstArena::new();
    match ubel_stratum_rd::parse(&arena, &tokens, source.to_string()) {
        Ok(_) => Ok(()),
        Err(mut errs) => Err(errs.take_parse_errors().len()),
    }
}

/// The supertrait list of the user trait `name`, as `(name, line, column)`
/// for each entry; `None` when the source does not parse or has no such
/// trait.
fn supertraits_of(source: &str, name: &str) -> Option<Vec<(String, usize, usize)>> {
    supertraits_in(source, name, false)
}

/// The same, for a prelude trait when `prelude` is true and for a trait of
/// the program otherwise.
fn supertraits_in(source: &str, name: &str, prelude: bool) -> Option<Vec<(String, usize, usize)>> {
    let tokens = ubel_stratum::lexer::tokenize(source).expect("source should lex");
    let arena = AstArena::new();
    let program = ubel_stratum_rd::parse(&arena, &tokens, source.to_string()).ok()?;
    for item in program.items {
        let Item::Trait(t) = item else { continue; };
        if t.name == name && t.span.is_prelude() == prelude {
            return Some(t.supertraits.iter()
                .map(|s| (s.name.to_string(), s.span.line as usize, s.span.column as usize))
                .collect());
        }
    }
    None
}

/// The names of every prelude trait's supertraits, as parsed from the
/// prelude's own source.
fn prelude_supertraits(trait_name: &str) -> Vec<String> {
    supertraits_in("fn main() void { }\n", trait_name, true)
        .expect("the prelude declares the trait")
        .into_iter().map(|(n, _, _)| n).collect()
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

/// `Named`, `Greeter: Named` and `Loud: Greeter`, the chain most tests use.
const CHAIN: &str = "trait Named {\n    fn name(self) string\n}\n\
                     trait Greeter: Named {\n    fn greeting(self) string { return $\"hi {self.name()}\" }\n}\n\
                     trait Loud: Greeter {\n    fn shout(self) string { return self.greeting() + \"!\" }\n}\n\
                     struct Dog { n: string }\n\
                     impl Named for Dog { fn name(self) string { return self.n } }\n\
                     impl Greeter for Dog { }\n\
                     impl Loud for Dog { }\n";

// ── The parsed shape ─────────────────────────────────────────────

#[test]
fn a_trait_with_no_colon_has_no_supertraits() {
    assert_eq!(supertraits_of("trait A { }\n", "A").unwrap(), vec![]);
}

#[test]
fn one_supertrait_is_recorded_by_name() {
    let got = supertraits_of("trait A { }\ntrait B: A { }\n", "B").unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].0, "A");
}

#[test]
fn several_supertraits_keep_the_written_order() {
    let got = supertraits_of("trait A { }\ntrait C { }\ntrait E { }\ntrait B: E + A + C { }\n", "B").unwrap();
    let names: Vec<&str> = got.iter().map(|(n, _, _)| n.as_str()).collect();
    assert_eq!(names, vec!["E", "A", "C"]);
}

#[test]
fn each_supertrait_carries_the_span_of_its_own_name() {
    // `trait B: A + C {` -> A at column 10, C at column 14 (1-based).
    let got = supertraits_of("trait A { }\ntrait C { }\ntrait B: A + C { }\n", "B").unwrap();
    assert_eq!((got[0].1, got[0].2), (3, 10));
    assert_eq!((got[1].1, got[1].2), (3, 14));
}

#[test]
fn a_supertrait_list_may_follow_generic_parameters() {
    let got = supertraits_of("trait A { }\ntrait B<T>: A { }\n", "B").unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].0, "A");
}

#[test]
fn a_bodiless_trait_may_have_supertraits_and_the_next_item_still_parses() {
    let src = "trait A { }\ntrait B: A\nfn main() void { }\n";
    assert_eq!(parse_errors(src), Ok(()));
    assert_eq!(supertraits_of(src, "B").unwrap().len(), 1);
}

#[test]
fn a_public_trait_may_have_supertraits() {
    assert_eq!(parse_errors("trait A { }\npub trait B: A { }\n"), Ok(()));
}

#[test]
fn a_colon_with_no_name_is_one_parse_error() {
    assert_eq!(parse_errors("trait B: { }\nfn main() void { }\n"), Err(1));
}

#[test]
fn a_plus_with_no_name_after_it_is_one_parse_error() {
    assert_eq!(parse_errors("trait A { }\ntrait B: A + { }\nfn main() void { }\n"), Err(1));
}

#[test]
fn the_prelude_declares_what_each_trait_builds_on() {
    assert_eq!(prelude_supertraits("PartialEq"),  Vec::<String>::new());
    assert_eq!(prelude_supertraits("Eq"),         vec!["PartialEq"]);
    assert_eq!(prelude_supertraits("PartialOrd"), vec!["PartialEq"]);
    assert_eq!(prelude_supertraits("Ord"),        vec!["PartialOrd", "Eq"]);
    assert_eq!(prelude_supertraits("Clone"),      Vec::<String>::new());
    assert_eq!(prelude_supertraits("Hash"),       vec!["Eq"]);
}

// ── An impl needs the impls beneath it (TYPE-132) ────────────────

#[test]
fn an_impl_without_the_supertrait_impl_is_one_error_at_the_impl() {
    let src = "trait Named { fn name(self) string }\n\
               trait Greeter: Named { }\n\
               struct A { n: int }\n\
               impl Greeter for A { }\n\
               fn main() void { println(1) }\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["TYPE-132"]));
    assert_eq!(messages(src), vec!["`impl Greeter for A` needs `Named`, which `A` does not implement"]);
    let report = check_source(src);
    assert_eq!(report.diagnostics[0].primary_span.line, 4);
}

#[test]
fn the_whole_chain_present_is_clean() {
    assert!(clean(&format!("{CHAIN}fn main() void {{ println(1) }}\n")));
}

#[test]
fn the_first_missing_link_is_the_one_named() {
    // `Loud` needs `Greeter`, then `Named`. With `Named` present and
    // `Greeter` absent the report names `Greeter`.
    let src = "trait Named { fn name(self) string }\n\
               trait Greeter: Named { }\n\
               trait Loud: Greeter { }\n\
               struct B { n: int }\n\
               impl Named for B { fn name(self) string { return \"b\" } }\n\
               impl Loud for B { }\n\
               fn main() void { println(1) }\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["TYPE-132"]));
    assert_eq!(messages(src), vec!["`impl Loud for B` needs `Greeter`, which `B` does not implement"]);
}

#[test]
fn with_nothing_beneath_the_nearest_supertrait_is_named() {
    let src = "trait Named { fn name(self) string }\n\
               trait Greeter: Named { }\n\
               trait Loud: Greeter { }\n\
               struct C { n: int }\n\
               impl Loud for C { }\n\
               fn main() void { println(1) }\n";
    assert_eq!(messages(src), vec!["`impl Loud for C` needs `Greeter`, which `C` does not implement"]);
}

#[test]
fn a_missing_link_further_up_is_reported_at_each_impl_that_depends_on_it() {
    // `impl Y for S` lacks `X`, and `impl Z for S` has `Y` but, through it,
    // still lacks `X`. The chain is checked the same way for a prelude
    // trait (`impl Hash` with an `impl Eq` that has no `PartialEq`).
    let src = "trait X { }\ntrait Y: X { }\ntrait Z: Y { }\nstruct S { n: int }\n\
               impl Y for S { }\nimpl Z for S { }\nfn main() void { println(1) }\n";
    assert_eq!(messages(src), vec![
        "`impl Y for S` needs `X`, which `S` does not implement",
        "`impl Z for S` needs `X`, which `S` does not implement",
    ]);
}

#[test]
fn one_impl_missing_two_supertraits_is_one_diagnostic() {
    let src = "trait X { }\ntrait Y { }\ntrait Z: X + Y { }\nstruct S { n: int }\nimpl Z for S { }\nfn main() void { println(1) }\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["TYPE-132"]));
    assert_eq!(messages(src), vec!["`impl Z for S` needs `X`, which `S` does not implement"]);
}

#[test]
fn the_second_of_two_supertraits_is_named_when_only_it_is_missing() {
    let src = "trait X { }\ntrait Y { }\ntrait Z: X + Y { }\nstruct S { n: int }\nimpl X for S { }\nimpl Z for S { }\nfn main() void { println(1) }\n";
    assert_eq!(messages(src), vec!["`impl Z for S` needs `Y`, which `S` does not implement"]);
}

#[test]
fn the_impls_may_come_in_any_order() {
    let src = "trait Named { fn name(self) string }\n\
               trait Greeter: Named { }\n\
               struct D { n: int }\n\
               impl Greeter for D { }\n\
               impl Named for D { fn name(self) string { return \"d\" } }\n\
               fn main() void { println(1) }\n";
    assert!(clean(src));
}

#[test]
fn each_impl_is_checked_separately() {
    let src = "trait Named { fn name(self) string }\n\
               trait Greeter: Named { }\n\
               struct A { n: int }\nstruct B { n: int }\n\
               impl Greeter for A { }\nimpl Greeter for B { }\n\
               fn main() void { println(1) }\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["TYPE-132", "TYPE-132"]));
}

#[test]
fn a_user_trait_over_a_prelude_trait_is_satisfied_by_a_derive() {
    let src = "trait Ranked: PartialOrd { }\n\
               @derive(PartialEq, PartialOrd)\nstruct C { n: int }\n\
               impl Ranked for C { }\n\
               fn main() void { println(1) }\n";
    assert!(clean(src));
}

#[test]
fn a_user_trait_over_a_prelude_trait_is_satisfied_by_a_written_impl() {
    let src = "trait Ranked: PartialEq { }\n\
               struct C { n: int }\n\
               impl PartialEq for C { fn eq(self, other: C) bool { return self.n == other.n } }\n\
               impl Ranked for C { }\n\
               fn main() void { println(1) }\n";
    assert!(clean(src));
}

#[test]
fn a_missing_prelude_supertrait_suggests_a_derive() {
    let src = "trait Ranked: PartialOrd { }\n@derive(PartialEq)\nstruct C { n: int }\nimpl Ranked for C { }\nfn main() void { println(1) }\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["TYPE-132"]));
    assert_eq!(suggestions(src), vec![Some("add `@derive(PartialOrd)` or `impl PartialOrd for C { .. }`".to_string())]);
}

#[test]
fn a_missing_user_supertrait_suggests_only_an_impl() {
    let src = "trait Named { fn name(self) string }\ntrait Greeter: Named { }\nstruct A { n: int }\nimpl Greeter for A { }\nfn main() void { println(1) }\n";
    assert_eq!(suggestions(src), vec![Some("add `impl Named for A { .. }`".to_string())]);
}

#[test]
fn an_enum_can_implement_a_chain() {
    let src = "trait Named { fn name(self) string }\n\
               trait Greeter: Named { }\n\
               enum E { One, Two }\n\
               impl Named for E { fn name(self) string { return \"e\" } }\n\
               impl Greeter for E { }\n\
               fn main() void { println(1) }\n";
    assert!(clean(src));
}

#[test]
fn an_enum_missing_a_user_supertrait_is_reported() {
    let src = "trait Named { fn name(self) string }\n\
               trait Greeter: Named { }\n\
               enum E { One, Two }\n\
               impl Greeter for E { }\n\
               fn main() void { println(1) }\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["TYPE-132"]));
}

#[test]
fn a_hand_written_impl_of_a_prelude_trait_still_needs_what_it_builds_on() {
    let src = "struct A { n: int }\nimpl Ord for A { fn cmp(self, other: A) Ordering { return Ordering.Equal } }\nfn main() void { println(1) }\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["TYPE-132"]));
    assert_eq!(messages(src), vec!["`impl Ord for A` needs `PartialOrd`, which `A` does not implement"]);
}

#[test]
fn ord_with_partial_ord_but_not_eq_names_eq() {
    let src = "@derive(PartialEq, PartialOrd)\nstruct A { n: int }\n\
               impl Ord for A { fn cmp(self, other: A) Ordering { return Ordering.Equal } }\nfn main() void { println(1) }\n";
    assert_eq!(messages(src), vec!["`impl Ord for A` needs `Eq`, which `A` does not implement"]);
}

// ── What a bound implies ─────────────────────────────────────────

#[test]
fn a_bound_on_a_sub_trait_reaches_the_supertrait_method() {
    let src = format!("{CHAIN}fn f<T: Greeter>(x: T) string {{ return x.name() }}\nfn main() void {{ println(f(Dog {{ n = \"a\" }})) }}\n");
    assert!(clean(&src));
}

#[test]
fn the_implication_is_transitive() {
    let src = format!("{CHAIN}fn f<T: Loud>(x: T) string {{ return x.name() + x.greeting() }}\nfn main() void {{ println(f(Dog {{ n = \"a\" }})) }}\n");
    assert!(clean(&src));
}

#[test]
fn a_bound_on_a_supertrait_does_not_reach_the_sub_trait_method() {
    let src = format!("{CHAIN}fn f<T: Named>(x: T) string {{ return x.greeting() }}\nfn main() void {{ println(f(Dog {{ n = \"a\" }})) }}\n");
    let (stage, got) = codes(&src);
    assert_eq!(stage, Stage::Sema);
    assert_eq!(got.len(), 1, "{got:?}");
}

#[test]
fn a_sub_trait_bound_satisfies_a_supertrait_requirement() {
    let src = format!("{CHAIN}fn need<T: Named>(x: T) string {{ return x.name() }}\n\
                       fn pass<T: Loud>(x: T) string {{ return need(x) }}\nfn main() void {{ println(pass(Dog {{ n = \"a\" }})) }}\n");
    assert!(clean(&src));
}

#[test]
fn a_supertrait_bound_does_not_satisfy_a_sub_trait_requirement() {
    let src = format!("{CHAIN}fn need<T: Loud>(x: T) string {{ return x.shout() }}\n\
                       fn pass<T: Named>(x: T) string {{ return need(x) }}\nfn main() void {{ println(pass(Dog {{ n = \"a\" }})) }}\n");
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-126"]));
    assert_eq!(messages(&src), vec!["the trait `Loud` is not implemented for `T`"]);
}

#[test]
fn a_call_with_a_type_that_has_the_whole_chain_satisfies_the_sub_trait_bound() {
    let src = format!("{CHAIN}fn need<T: Loud>(x: T) string {{ return x.shout() }}\nfn main() void {{ println(need(Dog {{ n = \"a\" }})) }}\n");
    assert!(clean(&src));
}

#[test]
fn a_default_body_may_call_a_supertrait_method_and_not_an_unrelated_one() {
    let ok = "trait Named { fn name(self) string }\ntrait G: Named { fn g(self) string { return self.name() } }\nfn main() void { println(1) }\n";
    assert!(clean(ok));
    let bad = "trait Named { fn name(self) string }\ntrait Other { fn other(self) string }\n\
               trait G: Named { fn g(self) string { return self.other() } }\nfn main() void { println(1) }\n";
    let (stage, got) = codes(bad);
    assert_eq!(stage, Stage::Sema);
    assert_eq!(got.len(), 1, "{got:?}");
}

#[test]
fn a_default_body_sees_the_supertraits_of_a_supertrait() {
    let src = "trait A { fn a(self) int }\ntrait B: A { }\ntrait C: B { fn c(self) int { return self.a() } }\nfn main() void { println(1) }\n";
    assert!(clean(src));
}

#[test]
fn a_prelude_supertrait_gives_the_default_body_its_operators() {
    let ok = "trait Scored: PartialOrd { fn better(self, other: Self) bool { return self > other } }\nfn main() void { println(1) }\n";
    assert!(clean(ok));
    let bad = "trait Scored { fn better(self, other: Self) bool { return self > other } }\nfn main() void { println(1) }\n";
    assert_eq!(codes(bad), (Stage::Sema, vec!["TYPE-131"]));
}

#[test]
fn a_prelude_supertrait_two_levels_up_gives_equality() {
    let src = "trait Scored: Ord { fn same(self, other: Self) bool { return self == other } }\nfn main() void { println(1) }\n";
    assert!(clean(src));
}

#[test]
fn a_bound_on_a_user_trait_over_a_prelude_trait_gives_the_operators() {
    let src = "trait Scored: Ord { }\nfn f<T: Scored>(a: T, b: T) bool { return a < b or a == b }\nfn main() void { println(1) }\n";
    assert!(clean(src));
}

#[test]
fn a_diamond_is_one_trait_to_the_body() {
    let src = "trait Base { fn id(self) int }\n\
               trait L: Base { }\ntrait R: Base { }\n\
               trait Both: L + R { fn both(self) int { return self.id() } }\n\
               fn f<T: Both>(x: T) int { return x.id() }\nfn main() void { println(1) }\n";
    assert!(clean(src));
}

#[test]
fn a_repeated_supertrait_is_not_an_error() {
    let src = "trait A { fn a(self) int }\ntrait B: A + A { fn b(self) int { return self.a() } }\nfn main() void { println(1) }\n";
    assert!(clean(src));
}

#[test]
fn a_sub_trait_may_be_declared_before_its_supertrait() {
    let src = "trait B: A { fn b(self) int { return self.a() } }\ntrait A { fn a(self) int }\n\
               struct S { n: int }\nimpl B for S { }\nimpl A for S { fn a(self) int { return 1 } }\nfn main() void { println(1) }\n";
    assert!(clean(src));
}

// ── Names that are not traits (TYPE-122) ─────────────────────────

#[test]
fn an_unknown_supertrait_is_reported_at_its_name() {
    let src = "trait B: Nope { }\nfn main() void { println(1) }\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["TYPE-122"]));
    assert_eq!(messages(src), vec!["unknown trait `Nope`"]);
    let report = check_source(src);
    assert_eq!((report.diagnostics[0].primary_span.line, report.diagnostics[0].primary_span.column), (1, 10));
}

#[test]
fn a_struct_as_a_supertrait_is_reported_as_a_struct() {
    let src = "struct S { n: int }\ntrait B: S { }\nfn main() void { println(1) }\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["TYPE-122"]));
    assert_eq!(messages(src), vec!["`S` is a struct, not a trait"]);
}

#[test]
fn a_bad_supertrait_does_not_hide_a_good_one() {
    let src = "trait A { fn a(self) int }\ntrait B: Nope + A { fn b(self) int { return self.a() } }\nfn main() void { println(1) }\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["TYPE-122"]));
}

// ── Cycles (TYPE-133) ────────────────────────────────────────────

#[test]
fn a_trait_that_is_its_own_supertrait_is_one_cycle() {
    let src = "trait A: A { }\nfn main() void { println(1) }\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["TYPE-133"]));
    assert_eq!(messages(src), vec!["trait `A` is its own supertrait: A -> A"]);
}

#[test]
fn two_traits_that_build_on_each_other_are_one_cycle() {
    let src = "trait A: B { }\ntrait B: A { }\nfn main() void { println(1) }\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["TYPE-133"]));
    assert_eq!(messages(src), vec!["trait `A` is its own supertrait: A -> B -> A"]);
}

#[test]
fn a_cycle_through_three_traits_is_one_cycle() {
    let src = "trait A: B { }\ntrait B: C { }\ntrait C: A { }\nfn main() void { println(1) }\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["TYPE-133"]));
    assert_eq!(messages(src), vec!["trait `A` is its own supertrait: A -> B -> C -> A"]);
}

#[test]
fn two_separate_cycles_are_two_diagnostics() {
    let src = "trait A: B { }\ntrait B: A { }\ntrait C: D { }\ntrait D: C { }\nfn main() void { println(1) }\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["TYPE-133", "TYPE-133"]));
}

#[test]
fn a_trait_that_only_leads_into_a_cycle_is_not_reported() {
    let src = "trait A: B { }\ntrait B: A { }\ntrait C: A { }\nfn main() void { println(1) }\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["TYPE-133"]));
}

#[test]
fn a_repeated_supertrait_inside_a_cycle_is_still_one_report() {
    let src = "trait A: A + A { }\nfn main() void { println(1) }\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["TYPE-133"]));
    let src = "trait A: B + B { }\ntrait B: A + A { }\nfn main() void { println(1) }\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["TYPE-133"]));
}

#[test]
fn a_cycle_is_named_from_where_it_closes_not_from_where_the_walk_began() {
    // The walk starts at `C`, which only leads into the cycle; the path
    // printed is the cycle itself.
    let src = "trait C: A { }\ntrait A: B { }\ntrait B: A { }\nfn main() void { println(1) }\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["TYPE-133"]));
    assert_eq!(messages(src), vec!["trait `A` is its own supertrait: A -> B -> A"]);
}

#[test]
fn a_cycle_is_reported_at_the_name_that_closes_it() {
    let src = "trait A: B { }\ntrait B: A { }\nfn main() void { println(1) }\n";
    let report = check_source(src);
    let span = report.diagnostics[0].primary_span;
    assert_eq!((span.line, span.column), (2, 10));
}

#[test]
fn a_cycle_suggestion_says_to_remove_a_link() {
    let src = "trait A: A { }\nfn main() void { println(1) }\n";
    assert_eq!(suggestions(src), vec![Some(
        "a supertrait is required by the trait that builds on it, so the chain of `: ...` links must end; remove one of them".to_string()
    )]);
}

#[test]
fn impls_missing_a_supertrait_are_reported_in_source_order() {
    let src = "trait Named { fn name(self) string }\ntrait G: Named { }\n\
               struct A { n: int }\nstruct B { n: int }\nstruct C { n: int }\nstruct D { n: int }\n\
               impl G for A { }\nimpl G for B { }\nimpl G for C { }\nimpl G for D { }\n\
               fn main() void { println(1) }\n";
    assert_eq!(messages(src), vec![
        "`impl G for A` needs `Named`, which `A` does not implement",
        "`impl G for B` needs `Named`, which `B` does not implement",
        "`impl G for C` needs `Named`, which `C` does not implement",
        "`impl G for D` needs `Named`, which `D` does not implement",
    ]);
}

#[test]
fn a_diamond_is_not_a_cycle() {
    let src = "trait Base { }\ntrait L: Base { }\ntrait R: Base { }\ntrait Both: L + R { }\nfn main() void { println(1) }\n";
    assert!(clean(src));
}

#[test]
fn a_cycle_does_not_hang_a_bound_that_uses_it() {
    let src = "trait A: B { fn a(self) int }\ntrait B: A { }\nfn f<T: A>(x: T) int { return x.a() }\nfn main() void { println(1) }\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["TYPE-133"]));
}

// ── Shadowing the prelude ────────────────────────────────────────

#[test]
fn a_program_may_declare_its_own_partial_eq_and_build_on_it() {
    let src = "trait PartialEq { fn same(self, other: Self) bool }\ntrait Mine: PartialEq { }\n\
               struct S { n: int }\nimpl PartialEq for S { fn same(self, other: S) bool { return true } }\nimpl Mine for S { }\n\
               fn main() void { println(1) }\n";
    assert!(clean(src));
}

#[test]
fn a_program_declaring_partial_eq_loses_the_prelude_traits_built_on_it() {
    // The prelude's `Eq` named `PartialEq`; with the program's own
    // `PartialEq` it would have bound to the wrong trait, so it is gone and
    // `Eq` is an unknown name here.
    let src = "trait PartialEq { fn same(self, other: Self) bool }\nfn f<T: Eq>(x: T) int { return 1 }\nfn main() void { println(1) }\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["TYPE-122"]));
    assert_eq!(messages(src), vec!["unknown trait `Eq`"]);
}

// ── At run time ──────────────────────────────────────────────────

#[test]
fn a_default_runs_what_the_supertrait_impl_provides() {
    let src = format!("{CHAIN}fn main() void {{ let d = Dog {{ n = \"rex\" }}\nprintln(d.shout()) }}\n");
    assert_eq!(run(&src).unwrap(), "hi rex!\n");
}

#[test]
fn a_call_through_a_sub_trait_bound_runs_the_supertrait_method() {
    let src = format!("{CHAIN}fn f<T: Loud>(x: T) string {{ return x.name() }}\nfn main() void {{ println(f(Dog {{ n = \"rex\" }})) }}\n");
    assert_eq!(run(&src).unwrap(), "rex\n");
}

#[test]
fn a_call_through_a_bound_runs_the_trait_not_an_inherent_method_of_the_same_name() {
    let src = format!("{CHAIN}extend Dog {{ fn name(self) string {{ return \"inherent\" }} }}\n\
                       fn f<T: Loud>(x: T) string {{ return x.name() }}\n\
                       fn main() void {{ let d = Dog {{ n = \"rex\" }}\nprintln(f(d))\nprintln(d.name()) }}\n");
    assert_eq!(run(&src).unwrap(), "rex\ninherent\n");
}

#[test]
fn an_overridden_supertrait_default_is_what_the_sub_trait_default_calls() {
    let src = "trait A { fn a(self) int { return 1 } }\n\
               trait B: A { fn b(self) int { return self.a() + 10 } }\n\
               struct X { n: int }\nstruct Y { n: int }\n\
               impl A for X { }\nimpl B for X { }\n\
               impl A for Y { fn a(self) int { return 5 } }\nimpl B for Y { }\n\
               fn main() void { println(X { n = 0 }.b())\nprintln(Y { n = 0 }.b()) }\n";
    assert_eq!(run(src).unwrap(), "11\n15\n");
}

#[test]
fn the_diamond_default_runs_each_level_once() {
    let src = "trait Base { fn id(self) int }\n\
               trait L: Base { fn l(self) int { return self.id() * 10 } }\n\
               trait R: Base { fn r(self) int { return self.id() * 100 } }\n\
               trait Both: L + R { fn both(self) int { return self.l() + self.r() + self.id() } }\n\
               struct N { n: int }\n\
               impl Base for N { fn id(self) int { return self.n } }\nimpl L for N { }\nimpl R for N { }\nimpl Both for N { }\n\
               fn main() void { println(N { n = 2 }.both()) }\n";
    assert_eq!(run(src).unwrap(), "222\n");
}

#[test]
fn a_prelude_supertrait_in_a_default_body_orders_derived_values() {
    let src = "trait Scored: Ord { fn better(self, other: Self) bool { return self > other } }\n\
               @derive(PartialEq, Eq, PartialOrd, Ord)\nstruct Card { rank: int }\n\
               impl Scored for Card { }\n\
               fn main() void { println(Card { rank = 5 }.better(Card { rank = 3 }))\nprintln(Card { rank = 3 }.better(Card { rank = 5 })) }\n";
    assert_eq!(run(src).unwrap(), "true\nfalse\n");
}

#[test]
fn a_qualified_call_names_the_supertrait() {
    let src = format!("{CHAIN}fn main() void {{ println(Named.name(Dog {{ n = \"rex\" }})) }}\n");
    assert_eq!(run(&src).unwrap(), "rex\n");
}
