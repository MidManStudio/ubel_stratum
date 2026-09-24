// Stage-by-stage pipeline diagnostic tool.
//
// Runs ONE .ubl file through tokenize -> rd_parser -> sema -> interpret and
// prints a clearly section-delimited report of what happened at each stage.
// Designed to be both directly readable (as Action log output) and easy for
// a downstream script to parse (split on the "=== NAME ===" markers).
//
// Usage: cargo run -p ubel_stratum_rd --example diagnose -- <path/to/file.ubl>
//
// Exit code is always 0 — this tool reports, it never fails the build.
// (ci-check.yml's own `cargo test` step is what should fail the build on
// a real regression; this tool is purely diagnostic.)

use std::env;
use std::fs;
use std::process;
use ubel_stratum::ast::arena::AstArena;

fn section(name: &str) {
    println!("\n=== {name} ===");
}

fn main() {
    let path = match env::args().nth(1) {
        Some(p) => p,
        None => {
            eprintln!("usage: diagnose <path/to/file.ubl>");
            process::exit(2);
        }
    };

    let source = match fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("cannot read {path}: {e}");
            process::exit(2);
        }
    };

    println!("=== FILE: {path} ===");

    section("SOURCE");
    println!("{source}");

    // ── Stage 1: lex ────────────────────────────────────────────────────
    section("TOKENS");
    let tokens = match ubel_stratum::lexer::tokenize(&source) {
        Ok(t) => {
            println!("status: OK ({} tokens)", t.len());
            for tok in &t {
                println!(
                    "  [{}:{}] {:?}  {:?}",
                    tok.span.line, tok.span.column, tok.kind, tok.lexeme
                );
            }
            t
        }
        Err(mut errs) => {
            println!("status: LEX ERROR");
            use ubel_stratum::error_management::Diagnosable;
            let diags: Vec<_> = errs.take_lexical_errors()
                .iter()
                .map(|e| e.to_diagnostic())
                .collect();
            print!("{}", ubel_stratum::error_management::render_all(&diags, &source));
            print_summary(true, false, None, None, None);
            return;
        }
    };

    // ── Stage 2: parse (rd_parser) ────────────────────────────────────────
    section("PARSE");
    let arena = AstArena::new();
    let program = match ubel_stratum_rd::parse(&arena, &tokens, source.clone()) {
        Ok(p) => {
            println!("status: OK ({} top-level items)", p.items.len());
            println!("{:#?}", p);
            p
        }
        Err(mut errs) => {
            println!("status: PARSE ERROR");
            use ubel_stratum::error_management::Diagnosable;
            let diags: Vec<_> = errs.take_parse_errors()
                .iter()
                .map(|e| e.to_diagnostic())
                .collect();
            print!("{}", ubel_stratum::error_management::render_all(&diags, &source));
            print_summary(false, true, None, None, None);
            return;
        }
    };

    // ── Stage 3: sema ───────────────────────────────────────────────────
    section("SEMA");
    let sema_result = ubel_stratum::sema::analyse(&program, &arena, source.clone());
    let sema_ok = match &sema_result {
        Ok(ctx) => {
            println!("status: OK");
            println!("{:#?}", ctx);
            true
        }
        Err(_) => false,
    };
    if let Err(mut errs) = sema_result {
        println!("status: SEMA ERROR");
        use ubel_stratum::error_management::Diagnosable;
        let mut diags: Vec<_> = errs.take_name_errors()
            .iter()
            .map(|e| e.to_diagnostic())
            .collect();
        diags.extend(errs.take_type_errors().iter().map(|e| e.to_diagnostic()));
        diags.extend(errs.take_tier_errors().iter().map(|e| e.to_diagnostic()));
        // BorrowError (Phase D, BORROW-0xx) was added to sema::analyse as
        // its own error category without a matching update here — this
        // stage silently printed nothing beyond "status: SEMA ERROR" for
        // any borrow-checker failure (e.g. ConflictingAccessWhileBorrowed)
        // even though BorrowError has always had a complete Diagnosable
        // impl (code/message/suggestion/secondary_spans) ready to render;
        // it just was never collected here. Same recurring pattern as
        // every other "new error family needs wiring into every consumer"
        // gap this project has hit before.
        diags.extend(errs.take_borrow_errors().iter().map(|e| e.to_diagnostic()));
        // MoveError (move_check.rs, MOVE-0xx) added alongside BorrowError
        // in the same delivery that wires this stage up correctly from
        // the start -- BorrowError's own comment just above documents
        // the exact "new error family, forgot to collect it here" gap
        // this project has already hit once; not repeating it this time.
        diags.extend(errs.take_move_errors().iter().map(|e| e.to_diagnostic()));
        // LifetimeError (lifetime_check.rs's LIFETIME-001/002/003, and
        // outlives_check.rs's LIFETIME-004/005/006 added on top of those
        // later) hit the exact gap the two comments just above already
        // warned about, for real: found by testing `diagnose.rs` against
        // a real err_outlives_*.ubl fixture and getting nothing but the
        // bare "status: SEMA ERROR" line, no rendered diagnostic at all
        // -- LifetimeError has had a complete Diagnosable impl this
        // whole time, same as BorrowError did; it just was never
        // collected here, for every one of its error codes, not only
        // the newest ones. `pipeline.rs`'s own debug-only `{:?}` dump
        // (a different, lower-fidelity example) did show *something*
        // for lifetime errors throughout, which is exactly why this sat
        // unnoticed -- checking only one of the two example tools
        // hid it.
        diags.extend(errs.take_lifetime_errors().iter().map(|e| e.to_diagnostic()));
        print!("{}", ubel_stratum::error_management::render_all(&diags, &source));
    }
    if !sema_ok {
        print_summary(false, false, Some(false), None, None);
        return;
    }

    // ── Stage 4: interpret ────────────────────────────────────────────────
    section("INTERPRET");
    if !source.contains("fn main(") {
        println!("status: SKIPPED (no `fn main`)");
        print_summary(false, false, Some(true), None, None);
        return;
    }

    println!("--- program stdout ---");
    let mut interp = ubel_stratum::interpreter::Interpreter::new(&arena);
    let run_result = interp.run_program(&program);
    println!("--- end program stdout ---");

    let interp_ok = run_result.is_ok();
    match run_result {
        Ok(()) => println!("status: OK"),
        Err(e) => println!("status: RUNTIME ERROR: {e}"),
    }

    print_summary(false, false, Some(true), Some(interp_ok), None);
}

fn print_summary(
    lex_failed: bool,
    parse_failed: bool,
    sema_ok: Option<bool>,
    interp_ok: Option<bool>,
    _reserved: Option<()>,
) {
    section("SUMMARY");
    println!("lex:       {}", if lex_failed { "FAIL" } else { "ok" });
    println!(
        "parse:     {}",
        if lex_failed {
            "skipped"
        } else if parse_failed {
            "FAIL"
        } else {
            "ok"
        }
    );
    println!(
        "sema:      {}",
        match sema_ok {
            None => "skipped",
            Some(true) => "ok",
            Some(false) => "FAIL",
        }
    );
    println!(
        "interpret: {}",
        match interp_ok {
            None => "skipped",
            Some(true) => "ok",
            Some(false) => "FAIL",
        }
    );
  }
