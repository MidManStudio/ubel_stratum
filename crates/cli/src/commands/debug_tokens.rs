// ============================================================================
// NOTICE: Full documentation, design decisions, and fix history for this file
// live in docs/ubel_stratum_cli.md, section "commands/debug_tokens.rs"
// ============================================================================
// crates/cli/src/commands/debug_tokens.rs
//! `ubel debug-tokens <file>`.

use std::path::PathBuf;

use clap::Args;
use serde::Serialize;
use ubel_stratum::error_management::{render_all, Diagnosable};

use crate::commands::{handle_error, GlobalOpts, EXIT_DIAGNOSTICS, EXIT_OK};
use crate::output::{json_output, printer};
use crate::services::file_io;

#[derive(Args)]
pub struct DebugTokensArgs {
    /// Path to the .ubl file
    pub file: PathBuf,
}

#[derive(Serialize)]
struct TokenJson {
    line:   usize,
    column: usize,
    kind:   String,
    lexeme: String,
}

pub fn run(args: DebugTokensArgs, global: &GlobalOpts) -> i32 {
    let source = match file_io::read_source(&args.file) {
        Ok(s)  => s,
        Err(e) => return handle_error(&e),
    };

    let tokens = match ubel_stratum::lexer::tokenize(&source) {
        Ok(t) => t,
        Err(mut errs) => {
            let diags: Vec<_> = errs.take_lexical_errors().iter().map(|e| e.to_diagnostic()).collect();
            eprint!("{}", render_all(&diags, &source));
            printer::error(&format!("{} lexical error(s)", diags.len()));
            return EXIT_DIAGNOSTICS;
        }
    };

    if global.json {
        json_output::print_result(
            tokens.iter().map(|t| TokenJson {
                line:   t.span.line,
                column: t.span.column,
                kind:   format!("{:?}", t.kind),
                lexeme: t.lexeme.clone(),
            }).collect::<Vec<_>>(),
        );
        return EXIT_OK;
    }

    for t in &tokens {
        println!("[{}:{}] {:?}  {:?}", t.span.line, t.span.column, t.kind, t.lexeme);
    }
    if global.verbose {
        printer::kv("tokens", &tokens.len().to_string());
    }
    EXIT_OK
}
