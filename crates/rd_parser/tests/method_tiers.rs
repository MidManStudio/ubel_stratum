//! Tiers on methods, slice S4a.
//!
//! Two things are pinned here. First, a call to a method on a receiver obeys
//! the same cross-tier rule as a call to a function (`TIER-005`): a callee
//! must be at the caller's tier or lower. Second, the tier a method has is the
//! tier written for it: its own `@tier`, otherwise the `@tier` on the
//! `impl`, `extend` or struct around it, otherwise on a tier block around
//! that, otherwise `HIGH`. An unrelated attribute on a method never changes
//! the result.
//!
//! A trait's own tiers, and the calls through them, are in `trait_tiers.rs`.
//!
//! The `.ubl` fixtures show whole programs accepted or rejected. These tests
//! pin the exact diagnostics and the run-time output.
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

// ── A call on a receiver obeys the cross-tier rule ────────────────────────────

#[test]
fn low_calling_a_high_extend_method_is_rejected() {
    assert_tier_error(&prog(
        "extend S { fn hi(self) int { return self.x } }\n\
         @tier(low) fn l(s: S) int { return s.hi() }",
    ));
}

#[test]
fn low_calling_a_high_impl_method_is_rejected() {
    assert_tier_error(&prog(
        "impl S { fn hi(self) int { return self.x } }\n\
         @tier(low) fn l(s: S) int { return s.hi() }",
    ));
}

#[test]
fn low_calling_a_high_struct_member_method_is_rejected() {
    assert_tier_error(
        "struct S { x: int\n    fn hi(self) int { return self.x } }\n\
         @tier(low) fn l(s: S) int { return s.hi() }\n\
         fn main() void { }\n",
    );
}

#[test]
fn low_calling_a_high_trait_impl_method_is_rejected() {
    assert_tier_error(&prog(
        "trait T { fn f(self) int }\n\
         impl T for S { fn f(self) int { return self.x } }\n\
         @tier(low) fn l(s: S) int { return s.f() }",
    ));
}

#[test]
fn every_caller_and_callee_tier_pair_follows_the_rule() {
    // Index 0 is low, 1 is mid, 2 is high. A callee above the caller is an error.
    for caller in 0u8..3 {
        for callee in 0u8..3 {
            let src = prog(&format!(
                "@tier({callee_t}) extend S {{ fn f(self) int {{ return self.x }} }}\n\
                 @tier({caller_t}) fn c(s: S) int {{ return s.f() }}",
                callee_t = tier_word(callee),
                caller_t = tier_word(caller),
            ));
            if callee > caller {
                assert_tier_error(&src);
            } else {
                assert_clean(&src);
            }
        }
    }
}

#[test]
fn the_message_names_both_tiers_and_the_method() {
    let src = prog(
        "extend S { fn hi(self) int { return self.x } }\n\
         @tier(low) fn l(s: S) int { return s.hi() }",
    );
    assert_eq!(messages(&src), vec!["`@tier(low)` code cannot call `@tier(high)` function `hi`"]);
}

#[test]
fn the_error_points_at_the_call() {
    let src = prog(
        "extend S { fn hi(self) int { return self.x } }\n\
         @tier(low) fn l(s: S) int { return s.hi() }",
    );
    assert_eq!(positions(&src), vec![pos_of(&src, "s.hi()")]);
}

#[test]
fn each_offending_call_is_reported_once() {
    let src = prog(
        "extend S { fn hi(self) int { return self.x } }\n\
         @tier(low) fn l(s: S) int {\n    let a = s.hi()\n    let b = s.hi()\n    return a + b\n}",
    );
    assert_eq!(codes(&src), (Stage::Sema, vec!["TIER-005", "TIER-005"]));
}

#[test]
fn a_call_in_a_high_function_is_never_rejected() {
    assert_clean(&prog(
        "@tier(low) extend S { fn lo(self) int { return self.x } }\n\
         extend S { fn hi(self) int { return self.x } }\n\
         fn c(s: S) int { return s.lo() + s.hi() }",
    ));
}

#[test]
fn an_associated_function_is_checked() {
    assert_tier_error(&prog(
        "extend S { fn make() S { return S { x = 1 } } }\n\
         @tier(low) fn l() int { let s = S.make()\n    return 1 }",
    ));
}

#[test]
fn a_low_associated_function_is_callable_from_low() {
    assert_clean(&prog(
        "@tier(low) extend S { fn make() S { return S { x = 1 } } }\n\
         @tier(low) fn l() int { let s = S.make()\n    return 1 }",
    ));
}

#[test]
fn a_method_of_a_built_in_type_is_checked() {
    assert_tier_error(
        "extend int { fn double(self) int { return self * 2 } }\n\
         @tier(low) fn l(n: int) int { return n.double() }\n\
         fn main() void { }\n",
    );
}

#[test]
fn a_method_of_a_generic_struct_is_checked() {
    assert_tier_error(
        "struct Box<T> { v: T }\n\
         impl<T> Box<T> { fn get(self) T { return self.v } }\n\
         @tier(low) fn l(b: Box<int>) int { return b.get() }\n\
         fn main() void { }\n",
    );
}

#[test]
fn a_method_of_a_generic_impl_with_a_concrete_argument_is_checked() {
    assert_tier_error(
        "struct Box<T> { v: T }\n\
         impl Box<int> { fn get(self) int { return self.v } }\n\
         @tier(low) fn l(b: Box<int>) int { return b.get() }\n\
         fn main() void { }\n",
    );
}

#[test]
fn a_call_to_a_sibling_method_is_checked_at_the_callers_tier() {
    // `a` is LOW and calls `b`, which is HIGH.
    assert_tier_error(&prog(
        "extend S { fn b(self) int { return self.x } }\n\
         @tier(low) extend S { fn a(self) int { return self.b() } }",
    ));
    // The same call with `b` LOW is fine.
    assert_clean(&prog(
        "@tier(low) extend S { fn b(self) int { return self.x } }\n\
         @tier(low) extend S { fn a(self) int { return self.b() } }",
    ));
}

#[test]
fn built_in_methods_carry_no_tier() {
    assert_clean(
        "@tier(low) fn l(s: string) string { return s.to_upper() }\n\
         fn main() void { }\n",
    );
}

#[test]
fn a_trait_impl_method_is_called_at_the_tier_the_impl_wrote() {
    // The impl method is LOW, equal to the trait's, so a LOW caller may call it.
    assert_clean(&prog(
        "@tier(low) trait T { fn f(self) int }\n\
         impl T for S { @tier(low) fn f(self) int { return self.x } }\n\
         @tier(low) fn l(s: S) int { return s.f() }",
    ));
}

#[test]
fn a_tier_block_around_the_trait_impl_sets_its_methods() {
    assert_clean(&prog(
        "@tier(low) trait T { fn f(self) int }\n\
         @tier(low) { impl T for S { fn f(self) int { return self.x } } }\n\
         @tier(low) fn l(s: S) int { return s.f() }",
    ));
}

// ── A tier written around a method reaches it ────────────────────────────────

#[test]
fn an_impl_method_with_an_unrelated_attribute_keeps_the_block_tier() {
    // The body calls a HIGH function, which a LOW method may not.
    assert_tier_error(&prog("@tier(low) impl S { @inline fn f(self) int { return h() } }"));
}

#[test]
fn an_impl_method_with_no_attribute_keeps_the_block_tier() {
    assert_tier_error(&prog("@tier(low) impl S { fn f(self) int { return h() } }"));
}

#[test]
fn a_methods_own_tier_wins_over_the_impl_block() {
    assert_clean(&prog("@tier(low) impl S { @tier(high) fn f(self) int { return h() } }"));
    assert_clean(&prog("@tier(low) impl S { @inline @tier(high) fn f(self) int { return h() } }"));
}

#[test]
fn a_tier_block_reaches_the_methods_of_an_impl() {
    assert_tier_error(&prog("@tier(low) { impl S { fn f(self) int { return h() } } }"));
}

#[test]
fn an_impl_with_its_own_tier_keeps_it_inside_a_tier_block() {
    assert_clean(&prog("@tier(low) { @tier(high) impl S { fn f(self) int { return h() } } }"));
}

#[test]
fn an_extend_block_tier_reaches_its_methods() {
    assert_tier_error(&prog("@tier(low) extend S { fn f(self) int { return h() } }"));
}

#[test]
fn an_extend_methods_own_tier_wins_over_the_block() {
    assert_clean(&prog("@tier(low) extend S { @tier(high) fn f(self) int { return h() } }"));
}

#[test]
fn a_tier_block_reaches_the_methods_of_an_extend() {
    assert_tier_error(&prog("@tier(low) { extend S { fn f(self) int { return h() } } }"));
}

#[test]
fn an_extend_with_its_own_tier_keeps_it_inside_a_tier_block() {
    assert_clean(&prog("@tier(low) { @tier(high) extend S { fn f(self) int { return h() } } }"));
}

#[test]
fn a_struct_tier_is_the_default_of_its_methods() {
    assert_tier_error(
        "fn h() int { return 1 }\n\
         @tier(low) struct S { x: int\n    fn f(self) int { return h() } }\n\
         fn main() void { }\n",
    );
}

#[test]
fn a_struct_methods_own_tier_wins_over_the_struct() {
    assert_clean(
        "fn h() int { return 1 }\n\
         @tier(low) struct S { x: int\n    @tier(high) fn f(self) int { return h() } }\n\
         fn main() void { }\n",
    );
}

#[test]
fn a_struct_method_with_an_unrelated_attribute_keeps_the_struct_tier() {
    assert_tier_error(
        "fn h() int { return 1 }\n\
         @tier(low) struct S { x: int\n    @inline fn f(self) int { return h() } }\n\
         fn main() void { }\n",
    );
}

#[test]
fn a_tier_block_reaches_the_methods_of_a_struct() {
    assert_tier_error(
        "fn h() int { return 1 }\n\
         @tier(low) { struct S { x: int\n    fn f(self) int { return h() } } }\n\
         fn main() void { }\n",
    );
}

#[test]
fn a_struct_with_its_own_tier_keeps_it_inside_a_tier_block() {
    assert_clean(
        "fn h() int { return 1 }\n\
         @tier(low) { @tier(high) struct S { x: int\n    fn f(self) int { return h() } } }\n\
         fn main() void { }\n",
    );
}

#[test]
fn a_tier_block_leaves_a_methods_own_tier_alone() {
    assert_clean(
        "fn h() int { return 1 }\n\
         @tier(low) { struct S { x: int\n    @tier(high) fn f(self) int { return h() } } }\n\
         fn main() void { }\n",
    );
}

#[test]
fn a_tier_block_moves_only_the_impl_methods_that_write_no_tier() {
    // `a` writes HIGH and keeps it. `b` writes none and becomes LOW, so only
    // its call to the HIGH function is an error.
    let src = prog(
        "@tier(low) { impl S {\n\
             @tier(high) fn a(self) int { return h() }\n\
             fn b(self) int { return h() + 1 }\n\
         } }",
    );
    assert_eq!(codes(&src), (Stage::Sema, vec!["TIER-005"]));
    assert_eq!(positions(&src), vec![pos_of(&src, "h() + 1")]);
}

#[test]
fn a_tier_block_moves_only_the_extend_methods_that_write_no_tier() {
    let src = prog(
        "@tier(low) { extend S {\n\
             @tier(high) fn a(self) int { return h() }\n\
             fn b(self) int { return h() + 1 }\n\
         } }",
    );
    assert_eq!(codes(&src), (Stage::Sema, vec!["TIER-005"]));
    assert_eq!(positions(&src), vec![pos_of(&src, "h() + 1")]);
}

#[test]
fn a_tier_block_moves_only_the_struct_methods_that_write_no_tier() {
    let src = "fn h() int { return 1 }\n\
               @tier(low) { struct S { x: int\n\
                   @tier(high) fn a(self) int { return h() }\n\
                   fn b(self) int { return h() + 1 }\n\
               } }\n\
               fn main() void { }\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["TIER-005"]));
    assert_eq!(positions(src), vec![pos_of(src, "h() + 1")]);
}

#[test]
fn the_innermost_tier_block_wins() {
    // LOW code may not call the MID function `m`; MID code may.
    assert_tier_error(&prog("@tier(mid) { @tier(low) { impl S { fn f(self) int { return m() } } } }"));
    assert_clean(&prog("@tier(low) { @tier(mid) { impl S { fn f(self) int { return m() } } } }"));
}

#[test]
fn an_unrelated_attribute_on_an_item_in_a_block_changes_no_tier() {
    assert_tier_error(&prog("@tier(low) { @inline impl S { fn f(self) int { return h() } } }"));
}

#[test]
fn a_tier_reaches_the_call_sites_of_methods_it_covers() {
    // The block makes `f` LOW, so a LOW caller may now call it.
    assert_clean(&prog(
        "@tier(low) { impl S { fn f(self) int { return self.x } } }\n\
         @tier(low) fn l(s: S) int { return s.f() }",
    ));
    // Without the block `f` is HIGH.
    assert_tier_error(&prog(
        "impl S { fn f(self) int { return self.x } }\n\
         @tier(low) fn l(s: S) int { return s.f() }",
    ));
}

// ── async keeps its own rule ─────────────────────────────────────────────────

#[test]
fn an_async_method_that_inherits_low_is_an_async_tier_error() {
    // The same rule as an `async fn` inside a tier block.
    let in_block = "@tier(low) { async fn g() int { return 1 } }\nfn main() void { }\n";
    assert_eq!(codes(in_block), (Stage::Sema, vec!["TIER-003"]));
    assert_eq!(
        codes(&prog("@tier(low) { impl S { async fn f(self) int { return 1 } } }")),
        (Stage::Sema, vec!["TIER-003"]),
    );
    assert_eq!(
        codes("@tier(low) struct S { x: int\n    async fn f(self) int { return 1 } }\nfn main() void { }\n"),
        (Stage::Sema, vec!["TIER-003"]),
    );
}

#[test]
fn an_async_method_with_its_own_high_tier_is_fine_in_a_low_impl() {
    assert_clean(&prog("@tier(low) impl S { @tier(high) async fn f(self) int { return 1 } }"));
}

// ── Run time is unchanged ────────────────────────────────────────────────────

#[test]
fn low_methods_called_from_low_code_run() {
    let src = "struct Counter { value: int }\n\
               @tier(low) extend Counter {\n\
                   fn bump(self) void { self.value = self.value + 1 }\n\
                   fn read(self) int { return self.value }\n\
               }\n\
               @tier(low) fn twice(c: Counter) int {\n\
                   c.bump()\n\
                   c.bump()\n\
                   return c.read()\n\
               }\n\
               fn main() void {\n\
                   let c = Counter { value = 0 }\n\
                   println(twice(c))\n\
               }\n";
    assert_eq!(run(src).as_deref(), Ok("2\n"));
}

#[test]
fn a_struct_tier_does_not_change_what_a_high_caller_can_do() {
    let src = "@tier(low) struct Counter { value: int\n    fn read(self) int { return self.value } }\n\
               fn main() void {\n\
                   let c = Counter { value = 7 }\n\
                   println(c.read())\n\
               }\n";
    assert_eq!(run(src).as_deref(), Ok("7\n"));
}
