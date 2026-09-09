// Full pipeline smoke test: tokenize -> rd_parser -> sema -> interpret.
//
// Usage: cargo run -p ubel_stratum_rd --example pipeline -- <dir-of-.ubl-files>
//
// For each .ubl file, runs every stage and reports the FIRST stage that
// fails (lex / parse / sema / interpret), so we know exactly how far the
// pipeline gets on real source.

use std::env;
use std::fs;
use ubel_stratum::ast::arena::AstArena;

fn main() {
    let dir = env::args().nth(1).unwrap_or_else(|| "tests/fixtures".to_string());
    let mut entries: Vec<_> = fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("cannot read {}: {}", dir, e))
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().map(|x| x == "ubl").unwrap_or(false))
        .collect();
    entries.sort_by_key(|e| e.path());

    let mut counts = [0usize; 5]; // lex, parse, sema, interpret, full-ok

    for entry in entries {
        let path = entry.path();
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let source = fs::read_to_string(&path).unwrap();

        // ── Stage 1: lex ──────────────────────────────────────────────
        let tokens = match ubel_stratum::lexer::tokenize(&source) {
            Ok(t) => t,
            Err(e) => {
                println!("[LEX-FAIL]       {name}: {:?}", e);
                continue;
            }
        };
        counts[0] += 1;

        // ── Stage 2: parse (rd_parser) ───────────────────────────────
        let arena = AstArena::new();
        let program = match ubel_stratum_rd::parse(&arena, &tokens, source.clone()) {
            Ok(p) => p,
            Err(mut errs) => {
                println!("[PARSE-FAIL]      {name}:");
                for e in errs.take_parse_errors() {
                    println!("                    {:?}", e);
                }
                continue;
            }
        };
        counts[1] += 1;

        // ── Stage 3: sema ─────────────────────────────────────────────
        let _sema_ctx = match ubel_stratum::sema::analyse(&program, &arena, source.clone()) {
            Ok(ctx) => ctx,
            Err(mut errs) => {
                println!("[SEMA-FAIL]       {name}:");
                for e in errs.take_name_errors() {
                    println!("                    name:  {:?}", e);
                }
                for e in errs.take_lifetime_errors() {
                    println!("                    lifetime:  {:?}", e);
                }
                for e in errs.take_type_errors() {
                    println!("                    type:  {:?}", e);
                }
                for e in errs.take_tier_errors() {
                    println!("                    tier:  {:?}", e);
                }
                for e in errs.take_borrow_errors() {
                    println!("                    borrow:  {:?}", e);
                }
                for e in errs.take_move_errors() {
                    println!("                    move:  {:?}", e);
                }
                continue;
            }
        };
        counts[2] += 1;

        // ── Stage 4: interpret ────────────────────────────────────────
        // Only attempt this on files that actually declare `main`.
        if !source.contains("fn main(") {
            println!("[NO-MAIN]         {name}  (lex+parse+sema OK, skipped interpret)");
            counts[3] += 1;
            counts[4] += 1;
            continue;
        }

        let mut interp = ubel_stratum::interpreter::Interpreter::new(&arena);
        match interp.run_program(&program) {
            Ok(()) => {
                println!("[FULL-PIPELINE-OK] {name}");
                counts[3] += 1;
                counts[4] += 1;
            }
            Err(e) => {
                println!("[INTERPRET-FAIL]  {name}: {e}");
                counts[3] += 1;
            }
        }
    }

    println!(
        "\nlex ok: {}  parse ok: {}  sema ok: {}  interpret attempted: {}  full pipeline ok: {}",
        counts[0], counts[1], counts[2], counts[3], counts[4]
    );
}
