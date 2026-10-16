//! Generic traits, slice S3b: `trait Convert<T>`, `impl Convert<int> for X`,
//! bounds with arguments (`T: Convert<int>`, `T: Convert<U>`) and generic
//! supertraits (`trait Wide<T>: Convert<T>`).
//!
//! The `.ubl` fixtures show whole programs accepted or rejected. These tests
//! pin the finer contract: the parsed shape (including the `>>` that closes a
//! bound's arguments and the parameter list at once), the exact code, count
//! and text of every diagnostic, what an impl's arguments mean for its
//! methods, what a bound's arguments imply (inside a body and at a call, and
//! how an impl decides an argument the call left open), how arguments are
//! carried through a supertrait, and the runtime contract that a type's one
//! impl of a trait is the one that runs.
//!
//! A type has one impl of a trait (docs/TRAITS_DESIGN.md, "What S3b
//! settled"), so every arrangement below has one answer to "which impl".

use ubel_stratum::ast::arena::AstArena;
use ubel_stratum::ast::root::Item;
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

/// How a written trait reference looks to the parser: name and the number of
/// type arguments.
type Written = (String, usize);

/// The bounds written on the generic parameters of the user function `name`,
/// one list per parameter.
fn fn_bounds(source: &str, name: &str) -> Option<Vec<Vec<Written>>> {
    let tokens = ubel_stratum::lexer::tokenize(source).expect("source should lex");
    let arena = AstArena::new();
    let program = ubel_stratum_rd::parse(&arena, &tokens, source.to_string()).ok()?;
    for item in program.items {
        let Item::Function(f) = item else { continue; };
        if f.name == name && !f.span.is_prelude() {
            return Some(f.generic_params.iter()
                .map(|g| g.bounds.iter().map(|b| (b.name.to_string(), b.args.len())).collect())
                .collect());
        }
    }
    None
}

/// The supertraits of the user trait `name`.
fn supertraits_of(source: &str, name: &str) -> Option<Vec<Written>> {
    let tokens = ubel_stratum::lexer::tokenize(source).expect("source should lex");
    let arena = AstArena::new();
    let program = ubel_stratum_rd::parse(&arena, &tokens, source.to_string()).ok()?;
    for item in program.items {
        let Item::Trait(t) = item else { continue; };
        if t.name == name && !t.span.is_prelude() {
            return Some(t.supertraits.iter().map(|b| (b.name.to_string(), b.args.len())).collect());
        }
    }
    None
}

/// The trait and arguments of the first impl of a trait in the source.
fn impl_header(source: &str) -> Option<(Option<String>, usize)> {
    let tokens = ubel_stratum::lexer::tokenize(source).expect("source should lex");
    let arena = AstArena::new();
    let program = ubel_stratum_rd::parse(&arena, &tokens, source.to_string()).ok()?;
    for item in program.items {
        let Item::Impl(i) = item else { continue; };
        return Some((i.trait_path.map(|p| p.join(".")), i.trait_args.len()));
    }
    None
}

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

/// `Convert<T>` with a default method, a struct per argument, and one impl
/// each; the base most tests build on.
const BASE: &str = "trait Convert<T> {\n    fn convert(self) T\n    fn again(self) T { return self.convert() }\n}\n\
                    struct Celsius { deg: int }\nstruct Label { text: string }\n\
                    impl Convert<int> for Celsius { fn convert(self) int { return self.deg * 2 } }\n\
                    impl Convert<string> for Label { fn convert(self) string { return self.text + \"!\" } }\n";

// ── The parsed shape ─────────────────────────────────────────────

#[test]
fn an_impl_records_the_arguments_written_after_its_trait() {
    assert_eq!(impl_header("trait C<T> { }\nstruct X { n: int }\nimpl C<int> for X { }\n"),
        Some((Some("C".to_string()), 1)));
}

#[test]
fn an_impl_of_a_trait_with_two_arguments_records_both() {
    assert_eq!(impl_header("trait P<A, B> { }\nstruct X { n: int }\nimpl P<int, string> for X { }\n"),
        Some((Some("P".to_string()), 2)));
}

#[test]
fn an_impl_of_a_trait_without_arguments_records_none() {
    assert_eq!(impl_header("trait C { }\nstruct X { n: int }\nimpl C for X { }\n"),
        Some((Some("C".to_string()), 0)));
}

#[test]
fn an_inherent_impl_of_a_generic_struct_is_not_a_trait_impl() {
    assert_eq!(impl_header("struct Box<T> { v: T }\nimpl Box<int> { }\n"), Some((None, 0)));
}

#[test]
fn an_argument_that_is_itself_generic_closes_with_a_double_angle() {
    assert_eq!(parse_errors("trait C<T> { }\nstruct X { n: int }\nimpl C<List<int>> for X { }\n"), Ok(()));
    assert_eq!(impl_header("trait C<T> { }\nstruct X { n: int }\nimpl C<List<int>> for X { }\n"),
        Some((Some("C".to_string()), 1)));
}

#[test]
fn a_bound_records_its_arguments() {
    let got = fn_bounds("trait C<T> { }\nfn f<T: C<int>>(x: T) void { }\n", "f").unwrap();
    assert_eq!(got, vec![vec![("C".to_string(), 1)]]);
}

#[test]
fn a_bound_with_arguments_as_the_last_thing_in_the_list_parses() {
    // `T: C<int>>` ends in one `>>` token that closes the arguments and the
    // parameter list.
    assert_eq!(parse_errors("trait C<T> { }\nfn f<T: C<int>>(x: T) void { }\n"), Ok(()));
}

#[test]
fn a_bound_with_nested_arguments_parses() {
    let src = "trait C<T> { }\nfn f<T: C<List<int>>>(x: T) void { }\n";
    assert_eq!(parse_errors(src), Ok(()));
    assert_eq!(fn_bounds(src, "f").unwrap(), vec![vec![("C".to_string(), 1)]]);
}

#[test]
fn several_bounds_and_several_parameters_keep_their_order() {
    let got = fn_bounds("trait A { }\ntrait C<T> { }\nfn f<T: A + C<int>, U: C<T>, V>(x: T) void { }\n", "f").unwrap();
    assert_eq!(got, vec![
        vec![("A".to_string(), 0), ("C".to_string(), 1)],
        vec![("C".to_string(), 1)],
        vec![],
    ]);
}

#[test]
fn a_bound_may_name_another_parameter_as_its_argument() {
    assert_eq!(parse_errors("trait C<T> { }\nfn f<T: C<U>, U>(x: T) void { }\n"), Ok(()));
}

#[test]
fn a_supertrait_records_its_arguments() {
    let got = supertraits_of("trait A<T> { }\ntrait C { }\ntrait B<T>: A<T> + C { }\n", "B").unwrap();
    assert_eq!(got, vec![("A".to_string(), 1), ("C".to_string(), 0)]);
}

#[test]
fn a_supertrait_with_arguments_as_the_last_thing_before_the_body_parses() {
    assert_eq!(parse_errors("trait A<T> { }\ntrait B: A<int> { }\n"), Ok(()));
}

#[test]
fn a_bound_name_with_nothing_after_the_plus_is_still_one_parse_error() {
    assert_eq!(parse_errors("trait C<T> { }\nfn f<T: C<int> + >(x: T) void { }\n"), Err(1));
}

// ── Declaring and implementing a generic trait ───────────────────

#[test]
fn a_generic_trait_with_impls_for_different_arguments_is_clean() {
    assert!(clean(&format!("{BASE}fn main() void {{ println(1) }}\n")));
}

#[test]
fn a_generic_trait_may_take_two_parameters() {
    let src = "trait Pair<A, B> { fn first(self) A\n fn second(self) B }\n\
               struct X { n: int }\n\
               impl Pair<int, string> for X { fn first(self) int { return self.n } fn second(self) string { return \"s\" } }\n\
               fn main() void { println(1) }\n";
    assert!(clean(src));
}

#[test]
fn an_impl_with_too_few_arguments_is_reported_at_the_impl() {
    let src = "trait C<T> { fn c(self) T }\nstruct X { n: int }\nimpl C for X { fn c(self) int { return 1 } }\nfn main() void { println(1) }\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["TYPE-108"]));
    assert_eq!(messages(src), vec!["`C` expects 1 type argument(s), found 0"]);
    assert_eq!(check_source(src).diagnostics[0].primary_span.line, 3);
}

#[test]
fn an_impl_with_too_many_arguments_is_reported() {
    let src = "trait C<T> { fn c(self) T }\nstruct X { n: int }\nimpl C<int, int> for X { fn c(self) int { return 1 } }\nfn main() void { println(1) }\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["TYPE-108"]));
    assert_eq!(messages(src), vec!["`C` expects 1 type argument(s), found 2"]);
}

#[test]
fn arguments_on_a_trait_that_takes_none_are_reported() {
    let src = "trait C { fn c(self) int }\nstruct X { n: int }\nimpl C<int> for X { fn c(self) int { return 1 } }\nfn main() void { println(1) }\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["TYPE-108"]));
    assert_eq!(messages(src), vec!["`C` expects 0 type argument(s), found 1"]);
}

#[test]
fn a_method_must_fit_the_arguments_the_impl_gives() {
    let src = "trait C<T> { fn c(self) T }\nstruct X { n: int }\nimpl C<int> for X { fn c(self) string { return \"x\" } }\nfn main() void { println(1) }\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["TYPE-125"]));
    assert_eq!(messages(src), vec![
        "method `c` does not match trait `C`: expected `fn(self) int`, found `fn(self) string`"]);
}

#[test]
fn a_method_taking_the_trait_parameter_must_take_the_argument() {
    let src = "trait Add<T> { fn plus(self, other: T) T }\nstruct X { n: int }\n\
               impl Add<int> for X { fn plus(self, other: string) int { return 1 } }\nfn main() void { println(1) }\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["TYPE-125"]));
    assert_eq!(messages(src), vec![
        "method `plus` does not match trait `Add`: expected `fn(self, int) int`, found `fn(self, string) int`"]);
}

#[test]
fn a_second_impl_of_the_trait_for_one_type_is_an_overlap_whatever_the_arguments() {
    let src = "trait C<T> { fn c(self) T }\nstruct X { n: int }\n\
               impl C<int> for X { fn c(self) int { return 1 } }\n\
               impl C<string> for X { fn c(self) string { return \"x\" } }\nfn main() void { println(1) }\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["TYPE-128"]));
    assert_eq!(messages(src), vec!["`C` is implemented for `X` more than once"]);
    assert_eq!(check_source(src).diagnostics[0].primary_span.line, 4);
}

#[test]
fn two_impls_with_the_same_arguments_are_an_overlap_too() {
    let src = "trait C<T> { fn c(self) T }\nstruct X { n: int }\n\
               impl C<int> for X { fn c(self) int { return 1 } }\n\
               impl C<int> for X { fn c(self) int { return 2 } }\nfn main() void { println(1) }\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["TYPE-128"]));
}

#[test]
fn different_types_may_give_the_trait_different_arguments() {
    let src = "trait C<T> { fn c(self) T }\nstruct X { n: int }\nstruct Y { n: int }\n\
               impl C<int> for X { fn c(self) int { return 1 } }\n\
               impl C<string> for Y { fn c(self) string { return \"y\" } }\nfn main() void { println(1) }\n";
    assert!(clean(src));
}

#[test]
fn a_trait_parameters_own_bound_is_checked_at_the_impl() {
    let src = "trait Holder<T: Eq> { fn held(self) T }\nstruct Plain { n: int }\n\
               impl Holder<Plain> for Plain { fn held(self) Plain { return self } }\nfn main() void { println(1) }\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["TYPE-126"]));
    assert_eq!(messages(src), vec!["the trait `Eq` is not implemented for `Plain`"]);
}

#[test]
fn a_trait_parameters_own_bound_is_met_by_a_derive() {
    let src = "trait Holder<T: Eq> { fn held(self) T }\n@derive(PartialEq, Eq)\nstruct Plain { n: int }\n\
               impl Holder<Plain> for Plain { fn held(self) Plain { return self } }\nfn main() void { println(1) }\n";
    assert!(clean(src));
}

#[test]
fn an_associated_type_is_still_not_built() {
    let src = "trait Holder<T> {\n    type Item\n    fn get(self) int\n}\nfn main() void { }\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["TYPE-129"]));
}

// ── Calling through a concrete type ──────────────────────────────

#[test]
fn a_method_of_a_generic_trait_has_the_impls_types() {
    let src = format!("{BASE}fn main() void {{\nlet c = Celsius {{ deg = 1 }}\nlet n: int = c.convert()\nlet l = Label {{ text = \"a\" }}\nlet s: string = l.convert()\n}}\n");
    assert!(clean(&src));
}

#[test]
fn the_result_of_a_method_is_not_another_arguments_type() {
    let src = format!("{BASE}fn main() void {{\nlet c = Celsius {{ deg = 1 }}\nlet s: string = c.convert()\n}}\n");
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-101"]));
    assert_eq!(messages(&src), vec!["type mismatch: expected `string`, found `int`"]);
}

#[test]
fn a_default_method_returns_the_trait_parameter_as_the_impl_gave_it() {
    let src = format!("{BASE}fn main() void {{\nlet l = Label {{ text = \"a\" }}\nlet s: string = l.again()\n}}\n");
    assert!(clean(&src));
    let bad = format!("{BASE}fn main() void {{\nlet l = Label {{ text = \"a\" }}\nlet n: int = l.again()\n}}\n");
    assert_eq!(codes(&bad), (Stage::Sema, vec!["TYPE-101"]));
}

#[test]
fn a_method_taking_the_trait_parameter_checks_its_argument() {
    let head = "trait Add<T> { fn plus(self, other: T) T }\nstruct X { n: int }\n\
                impl Add<int> for X { fn plus(self, other: int) int { return self.n + other } }\n";
    assert!(clean(&format!("{head}fn main() void {{ let x = X {{ n = 1 }}\nprintln(x.plus(2)) }}\n")));
    let bad = format!("{head}fn main() void {{ let x = X {{ n = 1 }}\nprintln(x.plus(\"a\")) }}\n");
    assert_eq!(codes(&bad), (Stage::Sema, vec!["TYPE-101"]));
}

#[test]
fn the_qualified_form_has_the_impls_arguments() {
    let src = format!("{BASE}fn main() void {{\nlet c = Celsius {{ deg = 1 }}\nlet n: int = Convert.convert(c)\n}}\n");
    assert!(clean(&src));
    let bad = format!("{BASE}fn main() void {{\nlet c = Celsius {{ deg = 1 }}\nlet s: string = Convert.convert(c)\n}}\n");
    assert_eq!(codes(&bad), (Stage::Sema, vec!["TYPE-101"]));
}

// ── Bounds with arguments ────────────────────────────────────────

#[test]
fn a_bound_with_a_concrete_argument_gives_the_methods_those_types() {
    let src = format!("{BASE}fn f<T: Convert<int>>(x: T) int {{ return x.convert() }}\nfn main() void {{ println(f(Celsius {{ deg = 1 }})) }}\n");
    assert!(clean(&src));
}

#[test]
fn inside_a_body_the_bounds_argument_is_the_methods_type() {
    let src = format!("{BASE}fn f<T: Convert<int>>(x: T) string {{ return x.convert() }}\nfn main() void {{ println(1) }}\n");
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-101"]));
}

#[test]
fn a_type_whose_impl_has_other_arguments_does_not_meet_the_bound() {
    let src = format!("{BASE}fn f<T: Convert<string>>(x: T) string {{ return x.convert() }}\nfn main() void {{ println(f(Celsius {{ deg = 1 }})) }}\n");
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-126"]));
    assert_eq!(messages(&src), vec!["the trait `Convert<string>` is not implemented for `Celsius`"]);
}

#[test]
fn a_type_with_no_impl_does_not_meet_the_bound() {
    let src = format!("{BASE}struct Plain {{ n: int }}\nfn f<T: Convert<int>>(x: T) int {{ return x.convert() }}\nfn main() void {{ println(f(Plain {{ n = 1 }})) }}\n");
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-126"]));
    assert_eq!(messages(&src), vec!["the trait `Convert<int>` is not implemented for `Plain`"]);
}

#[test]
fn a_bounds_argument_may_be_another_parameter() {
    let src = format!("{BASE}fn widen<T: Convert<U>, U>(x: T) U {{ return x.convert() }}\nfn main() void {{ println(widen(Celsius {{ deg = 1 }})) }}\n");
    assert!(clean(&src));
}

#[test]
fn the_impl_decides_an_argument_the_call_left_open() {
    let src = format!("{BASE}fn widen<T: Convert<U>, U>(x: T) U {{ return x.convert() }}\n\
                       fn main() void {{\nlet c = Celsius {{ deg = 1 }}\nlet n: int = widen(c)\nlet l = Label {{ text = \"a\" }}\nlet s: string = widen(l)\n}}\n");
    assert!(clean(&src));
}

#[test]
fn a_result_used_as_the_wrong_type_after_the_impl_decided_is_a_mismatch() {
    let src = format!("{BASE}fn widen<T: Convert<U>, U>(x: T) U {{ return x.convert() }}\n\
                       fn main() void {{\nlet c = Celsius {{ deg = 1 }}\nlet s: string = widen(c)\n}}\n");
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-101"]));
    assert_eq!(messages(&src), vec!["type mismatch: expected `string`, found `int`"]);
}

#[test]
fn an_argument_the_impl_decides_flows_into_later_arithmetic() {
    let src = format!("{BASE}fn widen<T: Convert<U>, U>(x: T) U {{ return x.convert() }}\n\
                       fn main() void {{\nlet c = Celsius {{ deg = 1 }}\nlet n = widen(c) + 1\nprintln(n)\n}}\n");
    assert!(clean(&src));
}

#[test]
fn a_method_taking_the_bounds_argument_is_checked_in_the_body() {
    let head = "trait Add<T> { fn plus(self, other: T) T }\n";
    let ok = format!("{head}fn f<A: Add<int>>(a: A) int {{ return a.plus(1) }}\nfn main() void {{ println(1) }}\n");
    assert!(clean(&ok));
    let bad = format!("{head}fn f<A: Add<int>>(a: A) int {{ return a.plus(\"x\") }}\nfn main() void {{ println(1) }}\n");
    assert_eq!(codes(&bad), (Stage::Sema, vec!["TYPE-101"]));
}

#[test]
fn a_method_taking_a_parameter_argument_is_checked_in_the_body() {
    let head = "trait Add<T> { fn plus(self, other: T) T }\n";
    let ok = format!("{head}fn f<A: Add<B>, B>(a: A, b: B) B {{ return a.plus(b) }}\nfn main() void {{ println(1) }}\n");
    assert!(clean(&ok));
    let bad = format!("{head}fn f<A: Add<B>, B>(a: A, b: B) B {{ return a.plus(1) }}\nfn main() void {{ println(1) }}\n");
    assert_eq!(codes(&bad).0, Stage::Sema);
}

#[test]
fn a_structs_bound_may_name_another_parameter_of_the_struct() {
    let head = "trait C<T> { fn c(self) T }\nstruct X { n: int }\nimpl C<int> for X { fn c(self) int { return 1 } }\n\
                struct Pair<T, U: C<T>> { a: T, b: U }\n";
    let ok = format!("{head}fn main() void {{ let p = Pair {{ a = 1, b = X {{ n = 1 }} }}\nprintln(1) }}\n");
    assert!(clean(&ok));
    let bad = format!("{head}fn main() void {{ let p = Pair {{ a = \"s\", b = X {{ n = 1 }} }}\nprintln(1) }}\n");
    assert_eq!(codes(&bad), (Stage::Sema, vec!["TYPE-126"]));
    assert_eq!(messages(&bad), vec!["the trait `C<string>` is not implemented for `X`"]);
}

#[test]
fn a_bound_with_the_wrong_number_of_arguments_is_reported_once() {
    let src = "trait C<T> { fn c(self) T }\nfn f<T: C<int, int>>(x: T) void { }\nfn g<T: C<int, int>>(x: T) void { }\nfn main() void { println(1) }\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["TYPE-108", "TYPE-108"]));
    assert_eq!(messages(src)[0], "`C` expects 1 type argument(s), found 2");
}

#[test]
fn a_bound_with_too_few_arguments_does_not_cascade_into_the_body() {
    // One diagnostic, for the count. The missing argument is unknown, so a
    // use of the method's result is not a second error.
    let src = "trait C<T> { fn c(self) T }\nfn f<T: C>(x: T) void { let n: int = x.c() }\nfn main() void { println(1) }\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["TYPE-108"]));
}

#[test]
fn a_bound_with_too_many_arguments_does_not_move_self() {
    // `Self` follows the trait's own parameters; extra written arguments are
    // dropped, so `me` still returns the bounded type.
    let src = "trait C<T> { fn c(self) T\n fn me(self) Self }\nfn f<T: C<int, int>>(x: T) T { return x.me() }\nfn main() void { println(1) }\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["TYPE-108"]));
}

#[test]
fn a_trait_parameters_bound_with_too_few_arguments_does_not_cascade_into_a_default() {
    // A trait's default body is checked in a scope built from the trait's
    // own parameters; a bound there with a missing argument is one error,
    // and what the bound's method returns is unknown, not the bounded type.
    let src = "trait C<X> { fn c(self) X }\ntrait Holder<T: C> { fn go(self, t: T) void { let n: int = t.c() } }\nfn main() void { println(1) }\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["TYPE-108"]));
}

#[test]
fn a_trait_parameters_bound_with_too_many_arguments_does_not_move_self() {
    let src = "trait C<X> { fn c(self) X\n fn me(self) Self }\ntrait Holder<T: C<int, int>> { fn go(self, t: T) T { return t.me() } }\nfn main() void { println(1) }\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["TYPE-108"]));
}

#[test]
fn a_bound_with_no_arguments_on_a_generic_trait_is_reported() {
    let src = "trait C<T> { fn c(self) T }\nfn f<T: C>(x: T) void { }\nfn main() void { println(1) }\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["TYPE-108"]));
    assert_eq!(messages(src), vec!["`C` expects 1 type argument(s), found 0"]);
    assert_eq!(check_source(src).diagnostics[0].primary_span.column, 9);
}

#[test]
fn two_bounds_with_different_arguments_conflict() {
    let src = "trait C<T> { fn c(self) T }\nfn f<T: C<int> + C<string>>(x: T) void { }\nfn main() void { println(1) }\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["TYPE-134"]));
    assert_eq!(messages(src), vec!["`T` is bounded by both `C<int>` and `C<string>`, and no type implements both"]);
    assert_eq!(suggestions(src), vec![Some(
        "a type has one impl of `C`, so a parameter can be bounded by it once; keep one of the two".to_string())]);
}

#[test]
fn after_a_conflict_the_first_bound_stays_in_force() {
    // One diagnostic, for the conflict; the body then uses the first bound
    // and does not also report the call as ambiguous.
    let src = "trait C<T> { fn c(self) T }
fn f<T: C<int> + C<string>>(x: T) int { return x.c() }
fn main() void { println(1) }\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["TYPE-134"]));
}

#[test]
fn a_conflicting_bound_does_not_also_fail_every_call() {
    // The first bound is the one a call must meet; the second is not
    // demanded of the argument as well.
    let src = "trait C<T> { fn c(self) T }\nstruct X { n: int }\nimpl C<int> for X { fn c(self) int { return 1 } }\n\
               fn f<T: C<int> + C<string>>(x: T) int { return x.c() }\n\
               fn main() void { println(f(X { n = 1 })) }\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["TYPE-134"]));
}

#[test]
fn two_bounds_with_the_same_arguments_are_one_bound() {
    let src = "trait C<T> { fn c(self) T }\nfn f<T: C<int> + C<int>>(x: T) int { return x.c() }\nfn main() void { println(1) }\n";
    assert!(clean(src));
}

#[test]
fn a_struct_parameter_bounded_by_a_generic_trait_is_checked_where_it_is_built() {
    let ok = format!("{BASE}struct Crate<T: Convert<int>> {{ item: T }}\nfn main() void {{ let k = Crate {{ item = Celsius {{ deg = 1 }} }}\nprintln(1) }}\n");
    assert!(clean(&ok));
    let bad = format!("{BASE}struct Crate<T: Convert<string>> {{ item: T }}\nfn main() void {{ let k = Crate {{ item = Celsius {{ deg = 1 }} }}\nprintln(1) }}\n");
    assert_eq!(codes(&bad), (Stage::Sema, vec!["TYPE-126"]));
    assert_eq!(messages(&bad), vec!["the trait `Convert<string>` is not implemented for `Celsius`"]);
}

#[test]
fn a_function_declared_before_the_trait_still_sees_the_bounds_arguments() {
    let src = "fn f<T: Convert<int>>(x: T) int { return x.convert() }\n\
               trait Convert<T> { fn convert(self) T }\n\
               struct X { n: int }\nimpl Convert<int> for X { fn convert(self) int { return 1 } }\n\
               fn main() void { println(f(X { n = 1 })) }\n";
    assert!(clean(src));
}

#[test]
fn an_alias_in_a_bounds_argument_is_expanded() {
    let src = "type Score = int\ntrait C<T> { fn c(self) T }\nstruct X { n: int }\nimpl C<int> for X { fn c(self) int { return 1 } }\n\
               fn f<T: C<Score>>(x: T) int { return x.c() }\nfn main() void { println(f(X { n = 1 })) }\n";
    assert!(clean(src));
}

#[test]
fn a_bound_on_the_trait_applied_to_the_wrong_alias_target_is_reported() {
    let src = "type Name = string\ntrait C<T> { fn c(self) T }\nstruct X { n: int }\nimpl C<int> for X { fn c(self) int { return 1 } }\n\
               fn f<T: C<Name>>(x: T) void { }\nfn main() void { f(X { n = 1 }) }\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["TYPE-126"]));
}

// ── Generic supertraits ──────────────────────────────────────────

const WIDE: &str = "trait Convert<T> { fn convert(self) T }\n\
                    trait Wide<T>: Convert<T> { fn wide(self) T { return self.convert() } }\n\
                    struct Meters { n: int }\n\
                    impl Convert<int> for Meters { fn convert(self) int { return self.n } }\n\
                    impl Wide<int> for Meters { }\n";

#[test]
fn a_generic_sub_trait_with_its_supertrait_impl_is_clean() {
    assert!(clean(&format!("{WIDE}fn main() void {{ println(1) }}\n")));
}

#[test]
fn a_default_of_a_generic_sub_trait_calls_the_supertrait_with_the_same_argument() {
    let src = format!("{WIDE}fn main() void {{ let m = Meters {{ n = 2 }}\nlet n: int = m.wide()\n}}\n");
    assert!(clean(&src));
}

#[test]
fn a_default_returning_the_supertraits_result_as_the_wrong_type_is_reported() {
    let src = "trait Convert<T> { fn convert(self) T }\n\
               trait Wide<T>: Convert<T> { fn wide(self) string { return self.convert() } }\nfn main() void { println(1) }\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["TYPE-101"]));
}

#[test]
fn a_bound_on_the_sub_trait_gives_the_supertraits_methods_with_the_carried_argument() {
    let src = format!("{WIDE}fn f<T: Wide<int>>(x: T) int {{ return x.convert() }}\nfn main() void {{ println(f(Meters {{ n = 1 }})) }}\n");
    assert!(clean(&src));
    let bad = format!("{WIDE}fn f<T: Wide<int>>(x: T) string {{ return x.convert() }}\nfn main() void {{ println(1) }}\n");
    assert_eq!(codes(&bad), (Stage::Sema, vec!["TYPE-101"]));
}

#[test]
fn a_bound_on_the_sub_trait_satisfies_a_requirement_on_the_supertrait_with_that_argument() {
    let src = format!("{WIDE}fn need<T: Convert<int>>(x: T) int {{ return x.convert() }}\n\
                       fn pass<T: Wide<int>>(x: T) int {{ return need(x) }}\nfn main() void {{ println(pass(Meters {{ n = 1 }})) }}\n");
    assert!(clean(&src));
}

#[test]
fn a_bound_on_the_sub_trait_does_not_satisfy_the_supertrait_with_another_argument() {
    let src = format!("{WIDE}fn need<T: Convert<string>>(x: T) string {{ return x.convert() }}\n\
                       fn pass<T: Wide<int>>(x: T) string {{ return need(x) }}\nfn main() void {{ println(1) }}\n");
    assert_eq!(codes(&src), (Stage::Sema, vec!["TYPE-126"]));
    assert_eq!(messages(&src), vec!["the trait `Convert<string>` is not implemented for `T`"]);
}

#[test]
fn an_impl_of_a_generic_sub_trait_without_the_supertrait_impl_is_reported_with_the_argument() {
    let src = "trait Convert<T> { fn convert(self) T }\ntrait Wide<T>: Convert<T> { }\nstruct E { n: int }\n\
               impl Wide<int> for E { }\nfn main() void { println(1) }\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["TYPE-132"]));
    assert_eq!(messages(src), vec!["`impl Wide for E` needs `Convert<int>`, which `E` does not implement"]);
}

#[test]
fn an_impl_whose_supertrait_impl_has_other_arguments_is_reported() {
    let src = "trait Convert<T> { fn convert(self) T }\ntrait Wide<T>: Convert<T> { }\nstruct F { n: int }\n\
               impl Convert<string> for F { fn convert(self) string { return \"f\" } }\n\
               impl Wide<int> for F { }\nfn main() void { println(1) }\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["TYPE-132"]));
    assert_eq!(messages(src), vec!["`impl Wide for F` needs `Convert<int>`, which `F` does not implement"]);
}

#[test]
fn the_supertrait_may_have_a_fixed_argument() {
    let src = "trait Convert<T> { fn convert(self) T }\ntrait Narrow: Convert<int> { fn narrow(self) int { return self.convert() + 1 } }\n\
               struct X { n: int }\nimpl Convert<int> for X { fn convert(self) int { return 1 } }\nimpl Narrow for X { }\n\
               fn f<T: Narrow>(x: T) int { return x.convert() }\nfn main() void { println(f(X { n = 1 })) }\n";
    assert!(clean(src));
}

#[test]
fn a_fixed_argument_supertrait_is_not_met_by_other_arguments() {
    let src = "trait Convert<T> { fn convert(self) T }\ntrait Narrow: Convert<int> { }\n\
               struct X { n: int }\nimpl Convert<string> for X { fn convert(self) string { return \"x\" } }\nimpl Narrow for X { }\n\
               fn main() void { println(1) }\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["TYPE-132"]));
    assert_eq!(messages(src), vec!["`impl Narrow for X` needs `Convert<int>`, which `X` does not implement"]);
}

#[test]
fn a_supertrait_with_the_wrong_number_of_arguments_is_reported() {
    let src = "trait Convert<T> { fn convert(self) T }\ntrait Bad: Convert { }\nfn main() void { println(1) }\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["TYPE-108"]));
    assert_eq!(messages(src), vec!["`Convert` expects 1 type argument(s), found 0"]);
}

#[test]
fn a_cycle_through_generic_supertraits_is_still_one_cycle() {
    let src = "trait A<T>: B<T> { }\ntrait B<T>: A<T> { }\nfn main() void { println(1) }\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["TYPE-133"]));
}

#[test]
fn a_supertrait_chain_carries_an_argument_two_levels() {
    let src = "trait A<T> { fn a(self) T }\ntrait B<T>: A<T> { }\ntrait C<T>: B<T> { fn c(self) T { return self.a() } }\n\
               struct X { n: int }\nimpl A<int> for X { fn a(self) int { return 1 } }\nimpl B<int> for X { }\nimpl C<int> for X { }\n\
               fn f<Z: C<int>>(z: Z) int { return z.a() }\nfn main() void { println(f(X { n = 1 })) }\n";
    assert!(clean(src));
}

#[test]
fn a_supertrait_may_mention_self_in_its_arguments() {
    let src = "trait Cmp<T> { fn cmp_with(self, other: T) int }\ntrait Ordered: Cmp<Self> { }\n\
               struct X { n: int }\nimpl Cmp<X> for X { fn cmp_with(self, other: X) int { return 0 } }\nimpl Ordered for X { }\n\
               fn main() void { println(1) }\n";
    assert!(clean(src));
}

// ── Prelude traits are untouched ─────────────────────────────────

#[test]
fn the_prelude_traits_take_no_arguments_and_nothing_about_them_changed() {
    let src = "@derive(PartialEq, Eq, PartialOrd, Ord, Hash, Clone)\nstruct P { n: int }\n\
               fn f<T: Ord + Hash + Clone>(a: T, b: T) bool { return a < b }\nfn main() void { println(f(P { n = 1 }, P { n = 2 })) }\n";
    assert!(clean(src));
}

#[test]
fn arguments_on_a_prelude_trait_are_reported() {
    let src = "fn f<T: Eq<int>>(a: T) void { }\nfn main() void { println(1) }\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["TYPE-108"]));
    assert_eq!(messages(src), vec!["`Eq` expects 0 type argument(s), found 1"]);
}

// ── At run time ──────────────────────────────────────────────────

#[test]
fn each_type_runs_its_own_impl_of_the_generic_trait() {
    let src = format!("{BASE}fn main() void {{\nprintln(Celsius {{ deg = 3 }}.convert())\nprintln(Label {{ text = \"a\" }}.convert())\n}}\n");
    assert_eq!(run(&src).unwrap(), "6\na!\n");
}

#[test]
fn a_default_method_runs_the_impls_method() {
    let src = format!("{BASE}fn main() void {{\nprintln(Celsius {{ deg = 3 }}.again())\nprintln(Label {{ text = \"a\" }}.again())\n}}\n");
    assert_eq!(run(&src).unwrap(), "6\na!\n");
}

#[test]
fn a_call_through_a_bound_with_a_parameter_argument_runs_the_types_impl() {
    let src = format!("{BASE}fn widen<T: Convert<U>, U>(x: T) U {{ return x.convert() }}\n\
                       fn main() void {{\nprintln(widen(Celsius {{ deg = 3 }}))\nprintln(widen(Label {{ text = \"a\" }}))\n}}\n");
    assert_eq!(run(&src).unwrap(), "6\na!\n");
}

#[test]
fn a_call_through_a_bound_runs_the_trait_not_an_inherent_method_of_the_same_name() {
    let src = format!("{BASE}extend Celsius {{ fn convert(self) int {{ return 0 - 1 }} }}\n\
                       fn f<T: Convert<int>>(x: T) int {{ return x.convert() }}\n\
                       fn main() void {{ let c = Celsius {{ deg = 3 }}\nprintln(f(c))\nprintln(c.convert()) }}\n");
    assert_eq!(run(&src).unwrap(), "6\n-1\n");
}

#[test]
fn a_method_taking_the_trait_parameter_runs_with_the_argument() {
    let src = "trait Add<T> { fn plus(self, other: T) T }\nstruct X { n: int }\nstruct S { s: string }\n\
               impl Add<int> for X { fn plus(self, other: int) int { return self.n + other } }\n\
               impl Add<string> for S { fn plus(self, other: string) string { return self.s + other } }\n\
               fn add_any<A: Add<B>, B>(a: A, b: B) B { return a.plus(b) }\n\
               fn main() void { println(add_any(X { n = 1 }, 2))\nprintln(add_any(S { s = \"a\" }, \"b\")) }\n";
    assert_eq!(run(src).unwrap(), "3\nab\n");
}

#[test]
fn a_generic_supertrait_chain_runs_each_level() {
    let src = format!("{WIDE}fn f<T: Wide<int>>(x: T) int {{ return x.convert() + x.wide() }}\n\
                       fn main() void {{ println(f(Meters {{ n = 4 }}))\nprintln(Meters {{ n = 5 }}.wide()) }}\n");
    assert_eq!(run(&src).unwrap(), "8\n5\n");
}

#[test]
fn the_qualified_form_runs_the_impl() {
    let src = format!("{BASE}fn main() void {{ println(Convert.convert(Celsius {{ deg = 4 }}))\nprintln(Convert.convert(Label {{ text = \"q\" }})) }}\n");
    assert_eq!(run(&src).unwrap(), "8\nq!\n");
}
