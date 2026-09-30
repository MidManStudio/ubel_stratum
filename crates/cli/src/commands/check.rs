// ============================================================================
// NOTICE: Full documentation, design decisions, and fix history for this file
// live in docs/ubel_stratum_cli.md, section "commands/check.rs"
// ============================================================================
// crates/cli/src/commands/check.rs
//! `ubel check <file>`.

use std::path::PathBuf;

use clap::Args;
use serde::Serialize;
use ubel_stratum::error_management::render_all;

use crate::commands::{handle_error, GlobalOpts, EXIT_DIAGNOSTICS, EXIT_OK};
use crate::output::{json_output, printer};
use crate::services::pipeline;

#[derive(Args)]
pub struct CheckArgs {
    /// Path to the .ubl file
    pub file: PathBuf,
}

/// One diagnostic in `--json` output.
#[derive(Serialize)]
pub struct DiagnosticJson {
    pub code:       &'static str,
    pub message:    String,
    pub line:       usize,
    pub column:     usize,
    pub suggestion: Option<String>,
}

#[derive(Serialize)]
struct CheckOutput {
    file:        String,
    ok:          bool,
    stage:       &'static str,
    diagnostics: Vec<DiagnosticJson>,
    elapsed_ms:  f64,
}

pub fn run(args: CheckArgs, global: &GlobalOpts) -> i32 {
    let outcome = match pipeline::check_file(&args.file) {
        Ok(o)  => o,
        Err(e) => return handle_error(&e),
    };
    report(&args.file.display().to_string(), &outcome, global)
}

/// Print a check outcome in the requested format and return its exit code.
/// Shared with `run`, which reports a failed check the same way.
pub fn report(file: &str, outcome: &pipeline::CheckOutcome, global: &GlobalOpts) -> i32 {
    let ok = outcome.report.is_clean();

    if global.json {
        json_output::print_result(CheckOutput {
            file:        file.to_string(),
            ok,
            stage:       outcome.report.stage.as_str(),
            diagnostics: pipeline::to_json(&outcome.report.diagnostics),
            elapsed_ms:  outcome.elapsed.as_secs_f64() * 1000.0,
        });
        return if ok { EXIT_OK } else { EXIT_DIAGNOSTICS };
    }

    if ok {
        if !global.quiet {
            printer::success(&format!("{file} is valid"));
            if global.verbose {
                printer::kv("stage", outcome.report.stage.as_str());
                printer::duration(outcome.elapsed);
            }
        }
        return EXIT_OK;
    }

    eprint!("{}", render_all(&outcome.report.diagnostics, &outcome.source));
    printer::error(&format!(
        "{} diagnostic(s) in {file} (stopped at {})",
        outcome.report.diagnostics.len(),
        outcome.report.stage.as_str(),
    ));
    EXIT_DIAGNOSTICS
}
