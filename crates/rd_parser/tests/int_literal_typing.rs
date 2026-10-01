//! Context-driven typing of unsuffixed integer literals.
//!
//! The `.ubl` fixtures check that whole programs are accepted or rejected
//! and that their runtime values come out right. These tests pin the
//! finer contract underneath them, using real source text through the
//! lex -> parse -> sema -> interpret pipeline:
//!
//! * exact diagnostic counts and messages for the range check (`TYPE-120`),
//! * exactly which literals sema records in `int_literal_types`,
//! * that the interpreter needs that table (a run without it panics on a
//!   sized/plain mix instead of guessing a width),
//! * the two places a literal is settled as plain `int` instead of being
//!   left open: the end of a body, and a type-dependent format spec.
//!
//! See docs/PARKED_IDEAS.md, "Unsuffixed integer literals".

use ubel_stratum::ast::arena::AstArena;
use ubel_stratum::ast::literals::IntSuffix;
use ubel_stratum::error_management::Diagnosable;
use ubel_stratum::interpreter::Interpreter;

/// Type-error messages for a program that lexes and parses but fails
/// sema. Empty when sema accepts it.
fn type_error_messages(source: &str) -> Vec<String> {
    let tokens = ubel_stratum::lexer::tokenize(source).expect("source should lex");
    let arena = AstArena::new();
    let program = ubel_stratum_rd::parse(&arena, &tokens, source.to_string())
        .unwrap_or_else(|_| panic!("source should parse"));
    match ubel_stratum::sema::analyse(&program, &arena, source.to_string()) {
        Ok(_) => Vec::new(),
        Err(mut errs) => errs.take_type_errors().iter().map(|e| e.message()).collect(),
    }
}

/// The widths sema recorded for a program that passes sema, sorted so a
/// test can compare them without depending on hash-map order.
fn recorded_widths(source: &str) -> Vec<&'static str> {
    let tokens = ubel_stratum::lexer::tokenize(source).expect("source should lex");
    let arena = AstArena::new();
    let program = ubel_stratum_rd::parse(&arena, &tokens, source.to_string())
        .unwrap_or_else(|_| panic!("source should parse"));
    let ctx = ubel_stratum::sema::analyse(&program, &arena, source.to_string())
        .unwrap_or_else(|_| panic!("source should pass sema"));
    let mut widths: Vec<&'static str> =
        ctx.int_literal_types.values().map(|s: &IntSuffix| s.as_str()).collect();
    widths.sort_unstable();
    widths
}

/// Run a program that passes sema and return what it printed. When
/// `with_table` is false the interpreter is not given sema's literal
/// widths, which is how an interpreter unit test that skips sema runs.
fn run(source: &str, with_table: bool) -> Result<String, String> {
    let tokens = ubel_stratum::lexer::tokenize(source).expect("source should lex");
    let arena = AstArena::new();
    let program = ubel_stratum_rd::parse(&arena, &tokens, source.to_string())
        .unwrap_or_else(|_| panic!("source should parse"));
    let ctx = ubel_stratum::sema::analyse(&program, &arena, source.to_string())
        .unwrap_or_else(|_| panic!("source should pass sema"));

    ubel_stratum::builtins::global::io::start_output_capture();
    let mut interp = Interpreter::new(&arena);
    if with_table {
        interp.set_int_literal_types(ctx.int_literal_types);
    }
    let result = interp.run_program(&program);
    let output = ubel_stratum::builtins::global::io::take_captured_output();
    result.map(|_| output)
}

// ── Range check (TYPE-120) ───────────────────────────────────────

#[test]
fn a_literal_too_wide_for_its_annotation_is_one_type_120() {
    let msgs = type_error_messages("fn main() void {\n    let bad: u8 = 300\n}\n");
    assert_eq!(msgs, vec!["literal `300` is out of range for `u8`".to_string()]);
}

#[test]
fn a_negative_literal_for_an_unsigned_type_shows_its_sign() {
    let msgs = type_error_messages("fn main() void {\n    let bad: u32 = -1\n}\n");
    assert_eq!(msgs, vec!["literal `-1` is out of range for `u32`".to_string()]);
}

#[test]
fn the_most_negative_value_of_a_signed_type_is_allowed() {
    let src = "fn main() void {\n    let lowest: i8 = -128\n    let narrow: i16 = -32768\n}\n";
    assert!(type_error_messages(src).is_empty());
}

#[test]
fn one_past_the_most_negative_value_is_rejected_once() {
    let msgs = type_error_messages("fn main() void {\n    let bad: i8 = -129\n}\n");
    assert_eq!(msgs.len(), 1);
}

#[test]
fn a_call_argument_is_range_checked_against_the_parameter() {
    let src = "fn takes(b: u8) u8 { return b }\n\
               fn main() void {\n    let a = takes(256)\n}\n";
    assert_eq!(type_error_messages(src).len(), 1);
}

#[test]
fn a_return_value_is_range_checked_against_the_return_type() {
    let src = "fn fixed() u16 { return 70000 }\nfn main() void {\n    let a = fixed()\n}\n";
    assert_eq!(type_error_messages(src).len(), 1);
}

#[test]
fn a_struct_field_is_range_checked_against_the_declared_type() {
    let src = "struct Cell { v: i8 }\n\
               fn main() void {\n    let c = Cell { v = 200 }\n}\n";
    assert_eq!(type_error_messages(src).len(), 1);
}

#[test]
fn a_match_pattern_is_range_checked_against_the_scrutinee() {
    let src = "fn band(v: u8) string {\n    match v {\n        300 => { return \"x\" }\n\
               _ => { return \"y\" }\n    }\n}\nfn main() void {\n    println(band(3))\n}\n";
    assert_eq!(type_error_messages(src).len(), 1);
}

#[test]
fn two_literals_sharing_one_variable_are_each_checked() {
    // `1 + 300` is one merged variable holding two literals; binding it to
    // u8 must report the one that does not fit, and only that one.
    let src = "fn main() void {\n    let v: u8 = 1 + 300\n}\n";
    assert_eq!(type_error_messages(src).len(), 1);
}

#[test]
fn one_bad_element_in_an_annotated_list_is_one_diagnostic() {
    // The three element literals share one variable once the annotation
    // binds it; only the 300 is out of range for u8.
    let src = "fn main() void {\n    let l: List<u8> = [1, 2, 300]\n    println($\"{l.len()}\")\n}\n";
    let msgs = type_error_messages(src);
    assert_eq!(msgs, vec!["literal `300` is out of range for `u8`".to_string()]);
}

// ── Mismatches ───────────────────────────────────────────────────

#[test]
fn an_integer_literal_never_becomes_a_float() {
    let msgs = type_error_messages("fn main() void {\n    let f: float = 5\n}\n");
    assert_eq!(msgs.len(), 1);
    assert!(msgs[0].contains("float"), "message should name the expected type: {}", msgs[0]);
}

#[test]
fn one_binding_cannot_be_two_integer_types() {
    let src = "fn a(v: u8) void { println($\"{v}\") }\n\
               fn b(v: i16) void { println($\"{v}\") }\n\
               fn main() void {\n    let n = 7\n    a(n)\n    b(n)\n}\n";
    assert_eq!(type_error_messages(src).len(), 1);
}

#[test]
fn a_literal_against_a_string_is_a_single_mismatch() {
    let msgs = type_error_messages("fn main() void {\n    let s: string = 5\n}\n");
    assert_eq!(msgs.len(), 1);
}

// ── Ordering on sized integers ───────────────────────────────────

#[test]
fn ordering_comparisons_on_sized_integers_are_accepted() {
    let src = "fn main() void {\n    let a: u32 = 1\n    let b: u32 = 2\n\
               let c: i64 = 3\n    let d: i64 = 4\n    let x = a < b\n    let y = c >= d\n}\n";
    assert!(type_error_messages(src).is_empty());
}

#[test]
fn ordering_two_bare_literals_is_accepted() {
    assert!(type_error_messages("fn main() void {\n    let x = 1 < 2\n}\n").is_empty());
}

// ── What sema records for the interpreter ────────────────────────

#[test]
fn only_literals_with_a_non_default_width_are_recorded() {
    let src = "fn main() void {\n    let a: u8 = 5\n    let b: i8 = -3\n    let c: u32 = 7\n\
               let d: i64 = 9\n    let e = 11\n    let f: int = 13\n}\n";
    // u8, i8, u32 are recorded. i64, the default int, and the
    // unannotated literal are plain 64-bit values and need no entry.
    assert_eq!(recorded_widths(src), vec!["i8", "u32", "u8"]);
}

#[test]
fn an_open_literal_is_recorded_with_the_type_a_later_use_gave_it() {
    let src = "fn main() void {\n    let n = 65535\n    let w: u16 = n\n}\n";
    assert_eq!(recorded_widths(src), vec!["u16"]);
}

#[test]
fn a_global_const_initializer_is_recorded() {
    let src = "const LIMIT: u16 = 60000\nfn main() void {\n    let a = LIMIT\n}\n";
    assert_eq!(recorded_widths(src), vec!["u16"]);
}

#[test]
fn an_unconstrained_literal_is_settled_as_plain_int() {
    assert!(recorded_widths("fn main() void {\n    let n = 5\n    let m = n + 2\n}\n").is_empty());
}

// ── The interpreter needs the table ──────────────────────────────

#[test]
fn sized_literals_wrap_at_their_declared_width() {
    let src = "fn main() void {\n    let a: u8 = 250\n    let w = a + 10\n    println(w)\n}\n";
    assert_eq!(run(src, true).unwrap(), "4\n");
}

#[test]
fn without_the_table_a_sized_and_a_plain_value_do_not_mix() {
    // The interpreter has no static types, so without sema's widths the
    // `250` is a plain int and `a + 10` is a mix of a real u8 and ints.
    // That must be a clear runtime panic, never a guessed width.
    let src = "fn main() void {\n    let a: u8 = 250u8\n    let w = a + 10\n    println(w)\n}\n";
    let err = run(src, false).unwrap_err();
    assert!(err.contains("type mismatch in binary op"), "got: {err}");
}

#[test]
fn a_negative_literal_keeps_its_sign_through_the_table() {
    let src = "fn main() void {\n    let lowest: i8 = -128\n    let a: i16 = -5\n    println(lowest)\n    println(a)\n}\n";
    assert_eq!(run(src, true).unwrap(), "-128\n-5\n");
}

#[test]
fn a_literal_pattern_matches_a_sized_scrutinee() {
    let src = "fn classify(b: u8) string {\n    match b {\n        0 => { return \"zero\" }\n\
               255 => { return \"max\" }\n        _ => { return \"other\" }\n    }\n}\n\
               fn main() void {\n    println(classify(255))\n    println(classify(7))\n}\n";
    assert_eq!(run(src, true).unwrap(), "max\nother\n");
}

#[test]
fn an_unconstrained_literal_keeps_its_full_64_bit_range() {
    let src = "fn main() void {\n    let big = 1_000_000 * 4000\n    println(big)\n}\n";
    assert_eq!(run(src, true).unwrap(), "4000000000\n");
}

// ── Format specs ─────────────────────────────────────────────────

#[test]
fn a_type_dependent_format_spec_settles_an_open_literal_as_int() {
    // `{n:+}` only exists for plain int at runtime, so it fixes `n` as int
    // and the later `let b: u8 = n` is an honest mismatch rather than a
    // silently ignored spec.
    let src = "fn main() void {\n    let n = 255\n    println($\"{n:+}\")\n    let b: u8 = n\n}\n";
    assert_eq!(type_error_messages(src).len(), 1);
}

#[test]
fn a_width_only_format_spec_leaves_an_open_literal_open() {
    let src = "fn main() void {\n    let n = 255\n    println($\"{n:>6}\")\n    let b: u8 = n\n}\n";
    assert!(type_error_messages(src).is_empty());
}

#[test]
fn a_bare_literal_with_a_numeric_base_is_an_int() {
    let src = "fn main() void {\n    println($\"{255:x}\")\n}\n";
    assert_eq!(run(src, true).unwrap(), "ff\n");
}
