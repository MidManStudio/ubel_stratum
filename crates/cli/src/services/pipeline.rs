// ============================================================================
// NOTICE: Full documentation, design decisions, and fix history for this file
// live in docs/ubel_stratum_cli.md, section "services/pipeline.rs"
// ============================================================================
// crates/cli/src/services/pipeline.rs
//! Check and run a file. Both go through `ubel_stratum_rd::check_source`, the
//! same entry the language server uses, so the two tools report identical
//! diagnostics for the same source.

use std::path::Path;
use std::time::{Duration, Instant};

use ubel_stratum::{ast::arena::AstArena, error_management::Diagnostic, interpreter::Interpreter};
use ubel_stratum_rd::{check_source, CheckReport};

use crate::commands::check::DiagnosticJson;
use crate::commands::CliError;
use crate::services::file_io;

pub struct CheckOutcome {
    pub source:  String,
    pub report:  CheckReport,
    pub elapsed: Duration,
}

pub enum RunOutcome {
    /// The program ran to completion.
    Finished,
    /// The check failed, so the program was not run.
    Diagnostics(CheckOutcome),
    /// The program started and failed at runtime.
    RuntimeError(String),
}

pub fn check_file(path: &Path) -> Result<CheckOutcome, CliError> {
    let source  = file_io::read_source(path)?;
    let started = Instant::now();
    let report  = check_source(&source);
    Ok(CheckOutcome { source, report, elapsed: started.elapsed() })
}

pub fn run_file(path: &Path) -> Result<RunOutcome, CliError> {
    let outcome = check_file(path)?;
    if !outcome.report.is_clean() {
        return Ok(RunOutcome::Diagnostics(outcome));
    }

    // The check already proved this source lexes and parses, so the two
    // calls below cannot fail on it; they run again only because the AST
    // borrows an arena that `check_source` owned and dropped.
    let tokens = ubel_stratum::lexer::tokenize(&outcome.source)
        .map_err(|_| CliError::Internal("re-lex failed after a clean check".into()))?;
    let arena = AstArena::new();
    let program = ubel_stratum_rd::parse(&arena, &tokens, outcome.source.clone())
        .map_err(|_| CliError::Internal("re-parse failed after a clean check".into()))?;

    // Sema resolved the width of every unsuffixed integer literal; the
    // interpreter has no static types and needs that table to run
    // `let x: u8 = 5` with a real `u8`. Spans are byte offsets into the
    // same source, so the table from this pass matches the tree parsed
    // just above.
    let sema_ctx = ubel_stratum::sema::analyse(&program, &arena, outcome.source.clone())
        .map_err(|_| CliError::Internal("re-analysis failed after a clean check".into()))?;

    let mut interp = Interpreter::new(&arena);
    interp.set_int_literal_types(sema_ctx.int_literal_types);
    match interp.run_program(&program) {
        Ok(())  => Ok(RunOutcome::Finished),
        Err(e)  => Ok(RunOutcome::RuntimeError(format!("{e}"))),
    }
}

pub fn to_json(diags: &[Diagnostic]) -> Vec<DiagnosticJson> {
    diags.iter().map(|d| DiagnosticJson {
        code:       d.code,
        message:    d.message.clone(),
        line:       d.primary_span.line,
        column:     d.primary_span.column,
        suggestion: d.suggestion.clone(),
    }).collect()
}
