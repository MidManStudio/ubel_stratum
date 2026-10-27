//! Tiers on traits, slice S4b.
//!
//! A trait method has a tier: its own `@tier`, otherwise the `@tier` on the
//! trait (or on a tier block around it), otherwise `HIGH`. A call through a
//! bound or a qualified call is checked against it, an impl method has to
//! equal it (`TIER-016`), and an impl method that writes no tier takes it.
//! The six prelude traits are `@tier(low)`.
//!
//! Calls on a concrete receiver and the tiers of methods in `impl`, `extend`
//! and struct blocks are in `method_tiers.rs`.
//!
//! See docs/TRAITS_TIERS_SPEC.md.

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

/// A `HIGH` function and a `MID` function, to detect which tier a method body
/// is checked at: calling `h()` is legal from `HIGH` only, calling `m()` from
/// `HIGH` and `MID`.
const FNS: &str = "fn h() int { return 1 }\n@tier(mid) fn m() int { return 1 }\n";
const S: &str = "struct S { x: int }\n";

/// `FNS`, `S`, the declarations under test and an empty `main`.
fn prog(decls: &str) -> String {
    format!("{FNS}{S}{decls}\nfn main() void {{ }}\n")
}

const SEMA_TIER: (Stage, [&str; 1]) = (Stage::Sema, ["TIER-005"]);

fn assert_tier_error(source: &str) {
    assert_eq!(codes(source), (SEMA_TIER.0, SEMA_TIER.1.to_vec()), "{source}");
}

fn assert_clean(source: &str) {
    assert_eq!(codes(source), (Stage::Clean, vec![]), "{source}");
}

fn tier_word(n: u8) -> &'static str {
    ["low", "mid", "high"][n as usize]
}

// ── Where a trait method's tier comes from ───────────────────────────────────

#[test]
fn a_trait_with_no_tier_written_is_high() {
    assert_tier_error(&prog(
        "trait Tick { fn f(self) int }\n\
         @tier(mid) fn run<T: Tick>(t: T) int { return t.f() }",
    ));
}

#[test]
fn a_tier_written_on_a_required_signature_is_applied() {
    assert_clean(&prog(
        "trait Tick { @tier(mid) fn f(self) int }\n\
         @tier(mid) fn run<T: Tick>(t: T) int { return t.f() }",
    ));
    assert_tier_error(&prog(
        "trait Tick { @tier(mid) fn f(self) int }\n\
         @tier(low) fn run<T: Tick>(t: T) int { return t.f() }",
    ));
}

#[test]
fn the_trait_tier_is_the_default_of_its_methods() {
    assert_clean(&prog(
        "@tier(mid) trait Tick { fn f(self) int }\n\
         @tier(mid) fn run<T: Tick>(t: T) int { return t.f() }",
    ));
}

#[test]
fn a_methods_own_tier_wins_over_the_trait() {
    // `f` is HIGH by its own tier, so a MID caller may not reach it; `g` takes
    // the trait's MID.
    let src = prog(
        "@tier(mid) trait Tick {\n\
             @tier(high) fn f(self) int\n\
             fn g(self) int\n\
         }\n\
         @tier(mid) fn run<T: Tick>(t: T) int { return t.f() + t.g() }",
    );
    assert_eq!(codes(&src), (Stage::Sema, vec!["TIER-005"]));
    assert_eq!(messages(&src), vec!["`@tier(mid)` code cannot call `@tier(high)` function `f`"]);
}

#[test]
fn an_unrelated_attribute_on_a_signature_changes_no_tier() {
    assert_clean(&prog(
        "@tier(mid) trait Tick { @inline fn f(self) int }\n\
         @tier(mid) fn run<T: Tick>(t: T) int { return t.f() }",
    ));
    assert_tier_error(&prog(
        "@tier(mid) trait Tick { @inline fn f(self) int }\n\
         @tier(low) fn run<T: Tick>(t: T) int { return t.f() }",
    ));
}

#[test]
fn a_tier_written_on_a_default_method_is_applied_to_its_body() {
    // The body calls a HIGH function, which a LOW method may not.
    assert_tier_error(&prog("trait T { @tier(low) fn g(self) int { return h() } }"));
    // With no tier written the default method is HIGH and the call is fine.
    assert_clean(&prog("trait T { fn g(self) int { return h() } }"));
}

#[test]
fn the_trait_tier_is_applied_to_the_bodies_of_its_default_methods() {
    assert_tier_error(&prog("@tier(low) trait T { fn g(self) int { return h() } }"));
    assert_clean(&prog("@tier(mid) trait T { fn g(self) int { return m() } }"));
    assert_tier_error(&prog("@tier(low) trait T { fn g(self) int { return m() } }"));
}

#[test]
fn a_default_methods_own_tier_wins_over_the_trait() {
    assert_clean(&prog("@tier(low) trait T { @tier(high) fn g(self) int { return h() } }"));
}

#[test]
fn a_tier_block_reaches_the_methods_of_a_trait() {
    // Both a signature and a default method sit in the LOW block.
    assert_clean(&prog(
        "@tier(low) { trait T { fn f(self) int\n fn g(self) int { return 1 } } }\n\
         @tier(low) fn run<A: T>(a: A) int { return a.f() + a.g() }",
    ));
    // Without the block both are HIGH.
    assert_tier_error(&prog(
        "trait T { fn f(self) int }\n\
         @tier(low) fn run<A: T>(a: A) int { return a.f() }",
    ));
}

#[test]
fn a_tier_block_moves_only_the_trait_methods_that_write_no_tier() {
    let src = prog(
        "@tier(low) { trait T {\n\
             @tier(high) fn a(self) int\n\
             fn b(self) int\n\
         } }\n\
         @tier(low) fn run<A: T>(x: A) int { return x.a() + x.b() }",
    );
    assert_eq!(codes(&src), (Stage::Sema, vec!["TIER-005"]));
    assert_eq!(messages(&src), vec!["`@tier(low)` code cannot call `@tier(high)` function `a`"]);
}

#[test]
fn a_tier_block_moves_only_the_default_methods_that_write_no_tier() {
    // `a` writes HIGH and keeps it. `b` writes none and becomes LOW, so only its
    // call to the HIGH function is an error.
    let src = prog(
        "@tier(low) { trait T {\n\
             @tier(high) fn a(self) int { return h() }\n\
             fn b(self) int { return h() + 1 }\n\
         } }",
    );
    assert_eq!(codes(&src), (Stage::Sema, vec!["TIER-005"]));
    assert_eq!(positions(&src), vec![pos_of(&src, "h() + 1")]);
}

#[test]
fn a_trait_with_its_own_tier_keeps_it_inside_a_tier_block() {
    assert_clean(&prog(
        "@tier(low) { @tier(mid) trait T { fn f(self) int } }\n\
         @tier(mid) fn run<A: T>(a: A) int { return a.f() }",
    ));
}

#[test]
fn the_innermost_tier_block_wins_for_a_trait() {
    // The inner LOW wins over the outer MID: a LOW caller may call it.
    assert_clean(&prog(
        "@tier(mid) { @tier(low) { trait T { fn f(self) int } } }\n\
         @tier(low) fn run<A: T>(a: A) int { return a.f() }",
    ));
    // The inner MID wins over the outer LOW: a LOW caller may not.
    assert_tier_error(&prog(
        "@tier(low) { @tier(mid) { trait T { fn f(self) int } } }\n\
         @tier(low) fn run<A: T>(a: A) int { return a.f() }",
    ));
}

// ── A call through a trait obeys the cross-tier rule ─────────────────────────

#[test]
fn every_caller_and_trait_tier_pair_follows_the_rule_through_a_bound() {
    for caller in 0u8..3 {
        for declared in 0u8..3 {
            let src = prog(&format!(
                "@tier({d}) trait Tick {{ fn f(self) int }}\n\
                 @tier({c}) fn run<T: Tick>(t: T) int {{ return t.f() }}",
                d = tier_word(declared),
                c = tier_word(caller),
            ));
            if declared > caller { assert_tier_error(&src); } else { assert_clean(&src); }
        }
    }
}

#[test]
fn every_caller_and_trait_tier_pair_follows_the_rule_in_a_qualified_call() {
    for caller in 0u8..3 {
        for declared in 0u8..3 {
            let src = prog(&format!(
                "@tier({d}) trait Tick {{ fn f(self) int }}\n\
                 @tier({d}) impl Tick for S {{ fn f(self) int {{ return 1 }} }}\n\
                 @tier({c}) fn run(s: S) int {{ return Tick.f(s) }}",
                d = tier_word(declared),
                c = tier_word(caller),
            ));
            if declared > caller { assert_tier_error(&src); } else { assert_clean(&src); }
        }
    }
}

#[test]
fn the_message_for_a_call_through_a_bound_names_the_method_and_both_tiers() {
    let src = prog(
        "trait Tick { fn label(self) string }\n\
         @tier(mid) fn run<T: Tick>(t: T) string { return t.label() }",
    );
    assert_eq!(messages(&src), vec!["`@tier(mid)` code cannot call `@tier(high)` function `label`"]);
    assert_eq!(positions(&src), vec![pos_of(&src, "t.label()")]);
}

#[test]
fn a_call_through_the_abstract_self_in_a_default_body_is_checked() {
    // `g` is MID and calls `f`, which the trait declares HIGH.
    assert_tier_error(&prog(
        "trait T {\n\
             @tier(high) fn f(self) int\n\
             @tier(mid) fn g(self) int { return self.f() }\n\
         }",
    ));
    assert_clean(&prog(
        "trait T {\n\
             @tier(mid) fn f(self) int\n\
             @tier(mid) fn g(self) int { return self.f() }\n\
         }",
    ));
}

#[test]
fn a_method_reached_through_a_supertrait_bound_uses_that_traits_tier() {
    assert_tier_error(&prog(
        "trait A { fn a(self) int }\n\
         @tier(mid) trait B: A { fn b(self) int }\n\
         @tier(mid) fn run<T: B>(t: T) int { return t.a() }",
    ));
    assert_clean(&prog(
        "@tier(mid) trait A { fn a(self) int }\n\
         @tier(mid) trait B: A { fn b(self) int }\n\
         @tier(mid) fn run<T: B>(t: T) int { return t.a() + t.b() }",
    ));
}

#[test]
fn a_bounded_struct_method_calls_through_the_bound_at_its_own_tier() {
    assert_tier_error(&prog(
        "trait Tick { fn f(self) int }\n\
         @tier(low) struct Slot<T: Tick> { item: T\n    fn go(self) int { return self.item.f() } }",
    ));
    assert_clean(&prog(
        "@tier(low) trait Tick { fn f(self) int }\n\
         @tier(low) struct Slot<T: Tick> { item: T\n    fn go(self) int { return self.item.f() } }",
    ));
}

#[test]
fn a_default_method_called_on_a_concrete_receiver_has_the_trait_tier() {
    assert_tier_error(&prog(
        "trait T { fn g(self) int { return 1 } }\n\
         impl T for S { }\n\
         @tier(low) fn l(s: S) int { return s.g() }",
    ));
    assert_clean(&prog(
        "@tier(low) trait T { fn g(self) int { return 1 } }\n\
         impl T for S { }\n\
         @tier(low) fn l(s: S) int { return s.g() }",
    ));
}

// ── An impl method has to equal the trait's tier ─────────────────────────────

#[test]
fn every_trait_and_impl_tier_pair_follows_exact_match() {
    for declared in 0u8..3 {
        for written in 0u8..3 {
            let src = prog(&format!(
                "@tier({d}) trait Tick {{ fn f(self) int }}\n\
                 impl Tick for S {{ @tier({w}) fn f(self) int {{ return 1 }} }}",
                d = tier_word(declared),
                w = tier_word(written),
            ));
            if declared == written {
                assert_clean(&src);
            } else {
                assert_eq!(codes(&src), (Stage::Sema, vec!["TIER-016"]), "{src}");
            }
        }
    }
}

#[test]
fn the_mismatch_message_names_the_method_the_impl_and_both_tiers() {
    let src = prog(
        "trait Tick { @tier(mid) fn tick(self) void }\n\
         impl Tick for S { @tier(high) fn tick(self) void { } }",
    );
    assert_eq!(
        messages(&src),
        vec!["method `tick` in `impl Tick for S` is `@tier(high)`, but trait `Tick` declares it `@tier(mid)`"],
    );
}

#[test]
fn a_mismatched_impl_method_is_still_called_at_its_own_tier() {
    // The mismatch is reported once, and a MID caller of the HIGH impl method is
    // reported as well: the impl method really is HIGH.
    assert_eq!(
        codes(&prog(
            "@tier(mid) trait Tick { fn f(self) int }\n\
             impl Tick for S { @tier(high) fn f(self) int { return 1 } }\n\
             @tier(mid) fn l(s: S) int { return s.f() }",
        )),
        (Stage::Sema, vec!["TIER-016", "TIER-005"]),
    );
}

#[test]
fn the_mismatch_suggestion_names_the_trait_tier() {
    let report = check_source(&prog(
        "trait Tick { @tier(mid) fn tick(self) void }\n\
         impl Tick for S { @tier(high) fn tick(self) void { } }",
    ));
    assert_eq!(
        report.diagnostics[0].suggestion.as_deref(),
        Some("remove the tier written for this method (on it, on its `impl`, or on a tier block around it) to take the trait's tier, or write `@tier(mid)`"),
    );
}

#[test]
fn the_mismatch_points_at_the_impl_method() {
    let src = prog(
        "trait Tick { @tier(mid) fn tick(self) void }\n\
         impl Tick for S { @tier(high) fn tick(self) void { } }",
    );
    let at = positions(&src);
    assert_eq!(at.len(), 1);
    let impl_line = pos_of(&src, "impl Tick for S").0;
    assert_eq!(at[0].0, impl_line, "the error is on the line of the impl method");
}

#[test]
fn each_mismatched_method_is_reported_once() {
    assert_eq!(
        codes(&prog(
            "trait Tick { @tier(mid) fn a(self) int\n @tier(mid) fn b(self) int }\n\
             impl Tick for S {\n\
                 @tier(high) fn a(self) int { return 1 }\n\
                 @tier(low) fn b(self) int { return 1 }\n\
             }",
        )),
        (Stage::Sema, vec!["TIER-016", "TIER-016"]),
    );
}

#[test]
fn a_method_with_an_unrelated_attribute_is_not_a_written_tier() {
    assert_clean(&prog(
        "@tier(mid) trait Tick { fn f(self) int }\n\
         impl Tick for S { @inline fn f(self) int { return 1 } }",
    ));
}

#[test]
fn an_impl_method_that_writes_no_tier_takes_the_traits() {
    // The trait is MID, so the body is checked at MID: a MID call is fine and a
    // HIGH call is not.
    assert_clean(&prog(
        "@tier(mid) trait Tick { fn f(self) int }\n\
         impl Tick for S { fn f(self) int { return m() } }",
    ));
    assert_tier_error(&prog(
        "@tier(mid) trait Tick { fn f(self) int }\n\
         impl Tick for S { fn f(self) int { return h() } }",
    ));
    // A LOW trait: even the MID call is too much.
    assert_tier_error(&prog(
        "@tier(low) trait Tick { fn f(self) int }\n\
         impl Tick for S { fn f(self) int { return m() } }",
    ));
}

#[test]
fn an_unwritten_impl_method_is_called_at_the_traits_tier() {
    assert_clean(&prog(
        "@tier(mid) trait Tick { fn f(self) int }\n\
         impl Tick for S { fn f(self) int { return 1 } }\n\
         @tier(mid) fn l(s: S) int { return s.f() }",
    ));
    assert_tier_error(&prog(
        "@tier(mid) trait Tick { fn f(self) int }\n\
         impl Tick for S { fn f(self) int { return 1 } }\n\
         @tier(low) fn l(s: S) int { return s.f() }",
    ));
}

#[test]
fn a_tier_written_on_the_impl_block_is_a_written_tier() {
    // Equal to the trait: fine. Different: a mismatch for every method.
    assert_clean(&prog(
        "@tier(mid) trait Tick { fn f(self) int }\n\
         @tier(mid) impl Tick for S { fn f(self) int { return 1 } }",
    ));
    assert_eq!(
        codes(&prog(
            "@tier(mid) trait Tick { fn f(self) int }\n\
             @tier(low) impl Tick for S { fn f(self) int { return 1 } }",
        )),
        (Stage::Sema, vec!["TIER-016"]),
    );
}

#[test]
fn an_explicit_high_on_the_impl_block_is_a_written_tier() {
    // `@tier(high)` is the default tier but writing it is still writing it.
    assert_eq!(
        codes(&prog(
            "@tier(mid) trait Tick { fn f(self) int }\n\
             @tier(high) impl Tick for S { fn f(self) int { return 1 } }",
        )),
        (Stage::Sema, vec!["TIER-016"]),
    );
    assert_eq!(
        codes(&prog(
            "@tier(mid) trait Tick { fn f(self) int }\n\
             impl Tick for S { @tier(high) fn f(self) int { return 1 } }",
        )),
        (Stage::Sema, vec!["TIER-016"]),
    );
}

#[test]
fn a_tier_block_around_an_impl_is_a_written_tier() {
    assert_clean(&prog(
        "@tier(mid) trait Tick { fn f(self) int }\n\
         @tier(mid) { impl Tick for S { fn f(self) int { return 1 } } }",
    ));
    assert_eq!(
        codes(&prog(
            "@tier(mid) trait Tick { fn f(self) int }\n\
             @tier(high) { impl Tick for S { fn f(self) int { return 1 } } }",
        )),
        (Stage::Sema, vec!["TIER-016"]),
    );
}

#[test]
fn a_block_around_the_trait_and_its_impl_gives_both_the_same_tier() {
    assert_clean(&prog(
        "@tier(low) { trait Tick { fn f(self) int }\n\
                      impl Tick for S { fn f(self) int { return 1 } } }\n\
         @tier(low) fn l(s: S) int { return s.f() }",
    ));
}

#[test]
fn an_override_of_a_default_method_has_to_match_the_default() {
    assert_eq!(
        codes(&prog(
            "@tier(mid) trait T { fn g(self) int { return 1 } }\n\
             impl T for S { @tier(high) fn g(self) int { return 2 } }",
        )),
        (Stage::Sema, vec!["TIER-016"]),
    );
    // Unwritten takes the default's tier, so its body is checked at MID.
    assert_tier_error(&prog(
        "@tier(mid) trait T { fn g(self) int { return 1 } }\n\
         impl T for S { fn g(self) int { return h() } }",
    ));
}

#[test]
fn an_impl_for_a_built_in_type_follows_the_same_rule() {
    let src = "@tier(mid) trait Tick { fn f(self) int }\n\
               impl Tick for int { @tier(high) fn f(self) int { return 1 } }\n\
               fn main() void { }\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["TIER-016"]));
    assert_eq!(
        messages(src),
        vec!["method `f` in `impl Tick for int` is `@tier(high)`, but trait `Tick` declares it `@tier(mid)`"],
    );
}

#[test]
fn a_generic_impl_follows_the_same_rule() {
    let src = "@tier(mid) trait Show { fn show(self) string }\n\
               struct Box<T> { v: T }\n\
               impl<T> Show for Box<T> { @tier(high) fn show(self) string { return \"b\" } }\n\
               fn main() void { }\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["TIER-016"]));
}

#[test]
fn a_program_with_no_tier_written_on_a_trait_is_unchanged() {
    assert_clean(&prog(
        "trait Tick { fn f(self) int }\n\
         impl Tick for S { fn f(self) int { return h() } }\n\
         fn use_it(s: S) int { return s.f() }",
    ));
}

// ── The prelude traits are LOW ───────────────────────────────────────────────

#[test]
fn code_at_any_tier_can_call_the_prelude_methods_through_a_bound() {
    for tier in ["low", "mid", "high"] {
        let src = format!(
            "@tier({tier}) fn a<T: Clone>(x: T) T {{ return x.clone() }}\n\
             @tier({tier}) fn b<T: PartialEq>(x: T, y: T) bool {{ return x.eq(y) }}\n\
             @tier({tier}) fn c<T: PartialEq>(x: T, y: T) bool {{ return x.ne(y) }}\n\
             @tier({tier}) fn d<T: PartialOrd>(x: T, y: T) bool {{ return x.lt(y) }}\n\
             @tier({tier}) fn e<T: Ord>(x: T, y: T) Ordering {{ return x.cmp(y) }}\n\
             @tier({tier}) fn f<T: Hash>(x: T, h: Hasher) void {{ x.hash(h) }}\n\
             fn main() void {{ }}\n"
        );
        assert_clean(&src);
    }
}

#[test]
fn a_hand_written_prelude_impl_has_to_be_low() {
    let src = "struct P { n: int }\n\
               impl PartialEq for P { @tier(high) fn eq(self, other: Self) bool { return self.n == other.n } }\n\
               fn main() void { }\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["TIER-016"]));
    assert_eq!(
        messages(src),
        vec!["method `eq` in `impl PartialEq for P` is `@tier(high)`, but trait `PartialEq` declares it `@tier(low)`"],
    );
    // Mid is a mismatch as well, and LOW written out is fine.
    assert_eq!(
        codes("struct P { n: int }\n\
               impl PartialEq for P { @tier(mid) fn eq(self, other: Self) bool { return true } }\n\
               fn main() void { }\n"),
        (Stage::Sema, vec!["TIER-016"]),
    );
    assert_clean(
        "struct P { n: int }\n\
         impl PartialEq for P { @tier(low) fn eq(self, other: Self) bool { return true } }\n\
         fn main() void { }\n",
    );
}

#[test]
fn a_hand_written_prelude_impl_that_writes_no_tier_is_low() {
    assert_clean(
        "struct P { n: int }\n\
         impl PartialEq for P { fn eq(self, other: Self) bool { return self.n == other.n } }\n\
         @tier(low) fn same(a: P, b: P) bool { return a.eq(b) }\n\
         fn main() void { }\n",
    );
}

#[test]
fn the_body_of_a_hand_written_prelude_impl_is_checked_at_low() {
    // A HIGH function.
    assert_tier_error(
        "fn log_it() int { return 1 }\n\
         struct P { n: int }\n\
         impl PartialEq for P { fn eq(self, other: Self) bool { return log_it() == 1 } }\n\
         fn main() void { }\n",
    );
    // A static.
    assert_eq!(
        codes("static CALLS: int = 0\n\
               struct P { n: int }\n\
               impl PartialEq for P { fn eq(self, other: Self) bool { CALLS += 1\n return true } }\n\
               fn main() void { }\n"),
        (Stage::Sema, vec!["TIER-015"]),
    );
    // A collection.
    assert_eq!(
        codes("struct P { n: int }\n\
               impl PartialEq for P { fn eq(self, other: Self) bool { let l = List.new()\n return true } }\n\
               fn main() void { }\n"),
        (Stage::Sema, vec!["TIER-009"]),
    );
    // A LOW helper is fine.
    assert_clean(
        "@tier(low) fn key(p: P) int { return p.n % 10 }\n\
         struct P { n: int }\n\
         impl PartialEq for P { fn eq(self, other: Self) bool { return key(self) == key(other) } }\n\
         fn main() void { }\n",
    );
}

#[test]
fn a_tier_block_around_a_prelude_impl_has_to_be_low() {
    assert_eq!(
        codes("struct P { n: int }\n\
               @tier(high) { impl PartialEq for P { fn eq(self, other: Self) bool { return true } } }\n\
               fn main() void { }\n"),
        (Stage::Sema, vec!["TIER-016"]),
    );
    assert_clean(
        "struct P { n: int }\n\
         @tier(low) { impl PartialEq for P { fn eq(self, other: Self) bool { return true } } }\n\
         fn main() void { }\n",
    );
}

#[test]
fn derived_and_native_prelude_methods_carry_no_tier() {
    assert_clean(
        "@derive(PartialEq, Clone)\n\
         struct P { n: int }\n\
         @tier(low) fn same(a: P, b: P) bool { return a.eq(b) }\n\
         @tier(low) fn copy(a: P) P { return a.clone() }\n\
         @tier(low) fn ints(a: int, b: int) bool { return a.eq(b) }\n\
         fn main() void { }\n",
    );
}

#[test]
fn an_impl_of_a_prelude_trait_with_supertraits_is_low_throughout() {
    assert_clean(
        "struct P { n: int }\n\
         impl PartialEq for P { fn eq(self, other: Self) bool { return self.n == other.n } }\n\
         impl Eq for P { }\n\
         impl PartialOrd for P {\n\
             fn partial_cmp(self, other: Self) Ordering? {\n\
                 if self.n < other.n { return Ordering.Less }\n\
                 if self.n > other.n { return Ordering.Greater }\n\
                 return Ordering.Equal\n\
             }\n\
         }\n\
         impl Ord for P {\n\
             fn cmp(self, other: Self) Ordering {\n\
                 if self.n < other.n { return Ordering.Less }\n\
                 if self.n > other.n { return Ordering.Greater }\n\
                 return Ordering.Equal\n\
             }\n\
         }\n\
         @tier(low) fn order<T: Ord>(a: T, b: T) Ordering { return a.cmp(b) }\n\
         fn main() void { }\n",
    );
}

// ── async keeps its own rule ─────────────────────────────────────────────────

#[test]
fn an_async_default_method_that_inherits_low_is_an_async_tier_error() {
    assert_eq!(
        codes(&prog("@tier(low) trait T { async fn f(self) int { return 1 } }")),
        (Stage::Sema, vec!["TIER-003"]),
    );
    assert_clean(&prog("@tier(low) trait T { @tier(high) async fn f(self) int { return 1 } }"));
}

// ── Run time is unchanged ────────────────────────────────────────────────────

#[test]
fn a_mid_trait_with_a_bound_call_and_a_default_method_runs() {
    let src = "@tier(mid) trait Shape {\n\
                   fn area(self) int\n\
                   fn doubled(self) int { return self.area() * 2 }\n\
               }\n\
               struct Sq { s: int }\n\
               impl Shape for Sq { fn area(self) int { return self.s * self.s } }\n\
               @tier(mid) fn total<T: Shape>(x: T) int { return x.area() + x.doubled() }\n\
               @tier(mid) fn go() int { return total(Sq { s = 3 }) }\n\
               fn main() void { println(go()) }\n";
    assert_eq!(run(src).as_deref(), Ok("27\n"));
}

#[test]
fn a_hand_written_prelude_impl_still_drives_the_operators() {
    let src = "struct Id { n: int }\n\
               impl PartialEq for Id { fn eq(self, other: Id) bool { return self.n % 10 == other.n % 10 } }\n\
               fn main() void {\n\
                   println(Id { n = 13 } == Id { n = 23 })\n\
                   println(Id { n = 13 } == Id { n = 24 })\n\
               }\n";
    assert_eq!(run(src).as_deref(), Ok("true\nfalse\n"));
}
