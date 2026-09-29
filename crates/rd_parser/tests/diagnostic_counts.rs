//! Exact diagnostic counts for the lex -> parse -> sema pipeline.
//!
//! The `.ubl` fixture sweep only records whether a program was accepted or
//! rejected, so it cannot tell one diagnostic from two for the same typo.
//! These tests run source text through the same three stages the
//! `pipeline` example does and assert how many diagnostics come back, plus
//! the parser-level behavior of the generic-closing `>` split. See
//! docs/ubel_stratum_rd.md.

use ubel_stratum::ast::arena::AstArena;

/// Lex and parse only. `Ok(())` when the source parses cleanly, otherwise
/// the number of parse errors.
fn parse_errors(source: &str) -> Result<(), usize> {
    let tokens = ubel_stratum::lexer::tokenize(source).expect("source should lex");
    let arena = AstArena::new();
    match ubel_stratum_rd::parse(&arena, &tokens, source.to_string()) {
        Ok(_) => Ok(()),
        Err(mut errs) => Err(errs.take_parse_errors().len()),
    }
}

/// Lex, parse and run sema. Returns `(name, type, move)` error counts, all
/// zero when sema accepts the program. Panics if lexing or parsing fails.
fn sema_errors(source: &str) -> (usize, usize, usize) {
    let tokens = ubel_stratum::lexer::tokenize(source).expect("source should lex");
    let arena = AstArena::new();
    let program = ubel_stratum_rd::parse(&arena, &tokens, source.to_string())
        .unwrap_or_else(|_| panic!("source should parse"));
    match ubel_stratum::sema::analyse(&program, &arena, source.to_string()) {
        Ok(_) => (0, 0, 0),
        Err(mut errs) => (
            errs.take_name_errors().len(),
            errs.take_type_errors().len(),
            errs.take_move_errors().len(),
        ),
    }
}

#[test]
fn unknown_method_on_a_methodless_struct_reports_once() {
    let src = "struct Article { title: string }\n\
               fn main() void {\n\
                   let a = Article { title = \"t\" }\n\
                   println(a.summarize())\n\
               }\n";
    assert_eq!(sema_errors(src), (0, 1, 0));
}

#[test]
fn unknown_method_on_a_struct_with_methods_reports_once() {
    let src = "struct P { n: int }\n\
               extend P { fn get(self) int { return self.n } }\n\
               fn main() void {\n\
                   let p = P { n = 1 }\n\
                   println($\"{p.gett()}\")\n\
               }\n";
    assert_eq!(sema_errors(src), (0, 1, 0));
}

#[test]
fn unknown_plain_field_access_still_reports_once() {
    let src = "struct P { n: int }\n\
               fn main() void {\n\
                   let p = P { n = 1 }\n\
                   println($\"{p.nope}\")\n\
               }\n";
    assert_eq!(sema_errors(src), (0, 1, 0));
}

#[test]
fn a_typo_in_the_target_of_a_call_still_reports() {
    // `a.b.c()`: the flag that silences NoSuchField applies to the outer
    // callee only, so the bad inner field access is still reported.
    let src = "struct Inner { n: int }\n\
               struct Outer { inner: Inner }\n\
               fn main() void {\n\
                   let o = Outer { inner = Inner { n = 1 } }\n\
                   println($\"{o.innr.n}\")\n\
               }\n";
    let (names, types, moves) = sema_errors(src);
    assert_eq!(names + moves, 0);
    assert!(types >= 1, "the mistyped field `innr` must still be reported");
}

#[test]
fn shadowing_local_is_assignable_but_global_const_is_not() {
    let ok = "const LIMIT: int = 5\n\
              fn main() void {\n\
                  let LIMIT = 1\n\
                  LIMIT = 2\n\
              }\n";
    assert_eq!(sema_errors(ok), (0, 0, 0));

    let bad = "const LIMIT: int = 5\n\
               fn main() void {\n\
                   LIMIT = 6\n\
               }\n";
    assert_eq!(sema_errors(bad), (1, 0, 0));
}

#[test]
fn nested_generic_close_parses_at_every_depth() {
    for ty in [
        "List<List<int>>",
        "List<List<List<int>>>",
        "Dictionary<string, List<int>>",
        "List<Dictionary<string, List<int>>>",
    ] {
        let src = format!("fn main() void {{ let a: {ty} = List.new() }}\n");
        assert_eq!(parse_errors(&src), Ok(()), "{ty} should parse");
    }
}

#[test]
fn unbalanced_generic_close_is_still_a_parse_error() {
    for ty in ["List<List<int>", "List<List<List<int>>", "Dictionary<string, List<int>"] {
        let src = format!("fn main() void {{ let a: {ty} = List.new() }}\n");
        assert!(parse_errors(&src).is_err(), "{ty} must not parse");
    }
}

#[test]
fn a_real_right_shift_is_not_split() {
    let src = "fn main() void { let a = 8 >> 1\n let b = 16 >> 2 >> 1 }\n";
    assert_eq!(parse_errors(src), Ok(()));
}

#[test]
fn lambda_return_type_reports_exactly_one_parse_error() {
    for ret in ["string", "List<int>", "List<List<int>>", "Dictionary<string, int>", "Point"] {
        let src = format!(
            "fn f(g: fn(int) int) int {{ return g(1) }}\n\
             fn main() void {{\n\
                 let n = f(fn(k: int) {ret} {{\n\
                     return k\n\
                 }})\n\
             }}\n"
        );
        assert_eq!(parse_errors(&src), Err(1), "return type `{ret}`");
    }
}

#[test]
fn lambda_with_a_struct_literal_body_is_not_mistaken_for_a_return_type() {
    let src = "struct Point { x: int, y: int }\n\
               fn mk(v: int, f: fn(int) Point) Point { return f(v) }\n\
               fn main() void {\n\
                   let p = mk(3, fn(n: int) Point { x = n, y = n })\n\
               }\n";
    assert_eq!(parse_errors(src), Ok(()));
}
