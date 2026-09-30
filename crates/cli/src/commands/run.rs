// ============================================================================
// NOTICE: Full documentation, design decisions, and fix history for this file
// live in docs/ubel_stratum_cli.md, section "commands/run.rs"
// ============================================================================
// crates/cli/src/commands/run.rs
//! `ubel run <file>`.

use std::path::PathBuf;

use clap::Args;

use crate::commands::{check, handle_error, GlobalOpts, EXIT_DIAGNOSTICS, EXIT_OK};
use crate::output::printer;
use crate::services::pipeline::{self, RunOutcome};

#[derive(Args)]
pub struct RunArgs {
    /// Path to the .ubl file
    pub file: PathBuf,
}

pub fn run(args: RunArgs, global: &GlobalOpts) -> i32 {
    let file = args.file.display().to_string();
    match pipeline::run_file(&args.file) {
        Ok(RunOutcome::Finished)          => EXIT_OK,
        Ok(RunOutcome::Diagnostics(out))  => check::report(&file, &out, global),
        Ok(RunOutcome::RuntimeError(msg)) => {
            printer::error(&format!("runtime error: {msg}"));
            EXIT_DIAGNOSTICS
        }
        Err(e) => handle_error(&e),
    }
}
