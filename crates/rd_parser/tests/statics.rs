//! `static` items: mutable module-level globals.
//!
//! The `.ubl` fixtures show whole programs being accepted or rejected.
//! These tests pin what they cannot see: the exact diagnostic code and
//! count for each rule, that a malformed declaration is reported once
//! rather than twice, and the runtime contract that a static is ONE value
//! shared by every function. That last one is the reason statics live in
//! their own table in the interpreter and not in its environment: each
//! call runs against a fresh copy of its closure's scope, so a global kept
//! there would be assigned to and then thrown away when the call returned.
//!
//! See docs/PARKED_IDEAS.md, "Mutable globals".

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

// ── The runtime contract: one value, shared ──────────────────────

#[test]
fn a_static_is_one_value_shared_by_every_function() {
    let src = "static N: int = 0\n\
               fn bump() void { N += 1 }\n\
               fn read() int { return N }\n\
               fn main() void {\n    bump()\n    bump()\n    bump()\n\
                   println(read())\n    println(N)\n}\n";
    assert_eq!(run(src).unwrap(), "3\n3\n");
}

#[test]
fn assignment_in_main_is_visible_to_a_function_and_back() {
    let src = "static N: int = 1\n\
               fn double() void { N = N * 2 }\n\
               fn main() void {\n    N = 10\n    double()\n    println(N)\n}\n";
    assert_eq!(run(src).unwrap(), "20\n");
}

#[test]
fn a_local_shadows_a_static_and_leaves_it_alone() {
    let src = "static N: int = 7\n\
               fn shadow() int {\n    let N = 100\n    N = N + 1\n    return N\n}\n\
               fn main() void {\n    println(shadow())\n    println(N)\n}\n";
    assert_eq!(run(src).unwrap(), "101\n7\n");
}

#[test]
fn a_closure_writes_the_shared_static() {
    let src = "static N: int = 0\n\
               fn main() void {\n    let add = fn() { N += 5 }\n    add()\n    add()\n    println(N)\n}\n";
    assert_eq!(run(src).unwrap(), "10\n");
}

#[test]
fn the_declared_type_types_the_initializer_literal() {
    // `250` is a u8 because of the declaration, so the sum wraps at 8 bits.
    let src = "static HP: u8 = 250\n\
               fn main() void {\n    HP += 10\n    println(HP)\n}\n";
    assert_eq!(run(src).unwrap(), "4\n");
}

// ── Initialization order ─────────────────────────────────────────

#[test]
fn initializers_may_refer_to_later_constants_and_statics() {
    let src = "static A: int = B + 1\nstatic B: int = C * 2\nconst C: int = 3\n\
               fn main() void {\n    println(A)\n    println(B)\n}\n";
    assert_eq!(run(src).unwrap(), "7\n6\n");
}

#[test]
fn a_cycle_between_statics_is_reported_at_startup() {
    let src = "static P: int = Q\nstatic Q: int = P\nfn main() void { println(P) }\n";
    let err = run(src).unwrap_err();
    assert!(err.contains("initializing static `P`"), "got: {err}");
}

// ── Tier rules (TIER-015) ────────────────────────────────────────

#[test]
fn reading_a_static_from_a_low_function_is_tier_015() {
    let src = "static T: int = 0\n@tier(low)\nfn f() int { return T }\nfn main() void { println(f()) }\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["TIER-015"]));
}

#[test]
fn writing_a_static_from_a_mid_function_is_tier_015() {
    let src = "static T: int = 0\n@tier(mid)\nfn f() void { T = 5 }\nfn main() void { f() }\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["TIER-015"]));
}

#[test]
fn a_static_read_inside_an_interpolation_hole_is_checked_too() {
    let src = "static T: int = 0\n@tier(low)\nfn f() string { return $\"{T}\" }\nfn main() void { println(f()) }\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["TIER-015"]));
}

#[test]
fn a_static_read_inside_a_lambda_in_a_mid_function_is_tier_015() {
    let src = "static T: int = 0\n@tier(mid)\nfn f() int {\n    let g = fn() T\n    return g()\n}\n\
               fn main() void { println(f()) }\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["TIER-015"]));
}

#[test]
fn passing_the_value_in_is_the_way_around_the_tier_rule() {
    let src = "static T: int = 3\n@tier(mid)\nfn twice(v: int) int { return v * 2 }\n\
               fn main() void { println(twice(T)) }\n";
    assert_eq!(codes(src), (Stage::Clean, vec![]));
}

#[test]
fn the_tier_message_names_the_static_and_the_functions_tier() {
    let src = "static T: int = 0\n@tier(low)\nfn f() int { return T }\nfn main() void { println(f()) }\n";
    let report = check_source(src);
    assert_eq!(report.diagnostics.len(), 1);
    assert_eq!(
        report.diagnostics[0].message,
        "the static `T` can only be used from `@tier(high)` code; this function is `@tier(low)`"
    );
}

// ── A const cannot read a static (NAME-008) ──────────────────────

#[test]
fn a_const_initializer_reading_a_static_is_name_008() {
    let src = "static B: int = 4\nconst D: int = B * 2\nfn main() void { println(D) }\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["NAME-008"]));
}

#[test]
fn a_const_reading_another_const_is_still_fine() {
    let src = "const A: int = 2\nconst B: int = A * 3\nfn main() void { println(B) }\n";
    assert_eq!(codes(src), (Stage::Clean, vec![]));
}

// ── Tier annotations on the item itself (PARSE-004) ──────────────

#[test]
fn a_low_tier_annotation_on_a_static_is_one_parse_004() {
    let src = "@tier(low)\nstatic A: int = 1\nfn main() void { println(A) }\n";
    assert_eq!(codes(src), (Stage::Parse, vec!["PARSE-004"]));
}

#[test]
fn a_high_tier_annotation_on_a_static_is_accepted() {
    let src = "@tier(high)\nstatic A: int = 1\nfn main() void { println(A) }\n";
    assert_eq!(codes(src), (Stage::Clean, vec![]));
}

#[test]
fn a_tier_annotation_on_a_const_is_one_parse_004() {
    let src = "@tier(mid)\nconst A: int = 1\nfn main() void { println(A) }\n";
    assert_eq!(codes(src), (Stage::Parse, vec!["PARSE-004"]));
}

#[test]
fn a_tier_block_around_a_const_and_a_static_is_not_an_annotation_on_them() {
    // The block's tier is for the functions in it; it never reaches a
    // const or a static, so neither is reported.
    let src = "@tier(low) {\n    const C: int = 4\n    static S: int = 5\n}\n\
               fn main() void { println(S) }\n";
    assert_eq!(codes(src), (Stage::Clean, vec![]));
}

// ── Malformed declarations report once ───────────────────────────

#[test]
fn a_missing_type_annotation_is_one_diagnostic() {
    // Without recovery the leftover `= 5` was parsed again as a stray
    // top-level item and reported a second time.
    let src = "static X = 5\nfn main() void { println(X) }\n";
    assert_eq!(codes(src), (Stage::Parse, vec!["PARSE-001"]));
}

#[test]
fn three_malformed_statics_are_three_diagnostics() {
    let src = "static X = 5\nstatic Y: int 6\nstatic Z: = 7\nfn main() void { println(\"x\") }\n";
    let (stage, found) = codes(src);
    assert_eq!(stage, Stage::Parse);
    assert_eq!(found.len(), 3, "one diagnostic per typo, got {found:?}");
}

#[test]
fn a_pub_static_is_accepted() {
    let src = "pub static A: int = 1\nfn main() void { println(A) }\n";
    assert_eq!(codes(src), (Stage::Clean, vec![]));
}

// ── Types ────────────────────────────────────────────────────────

#[test]
fn the_declared_type_is_enforced_at_the_initializer_and_every_assignment() {
    let src = "static C: int = \"text\"\nstatic N: string = \"a\"\n\
               fn main() void {\n    N = 5\n    C = \"no\"\n}\n";
    assert_eq!(codes(src), (Stage::Sema, vec!["TYPE-101", "TYPE-101", "TYPE-101"]));
}

#[test]
fn a_static_cannot_reuse_a_function_name() {
    let src = "fn thing() void { }\nstatic thing: int = 1\nfn main() void { thing() }\n";
    let (stage, found) = codes(src);
    assert_eq!(stage, Stage::Sema);
    assert!(found.contains(&"NAME-002"), "expected a duplicate-definition error, got {found:?}");
}
