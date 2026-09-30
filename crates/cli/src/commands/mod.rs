// ============================================================================
// NOTICE: Full documentation, design decisions, and fix history for this file
// live in docs/ubel_stratum_cli.md, section "commands/mod.rs"
// ============================================================================
// crates/cli/src/commands/mod.rs
//! Shared command types, exit codes, and the `CliError` enum.

pub mod check;
pub mod debug_tokens;
pub mod run;

use std::fmt;
use std::path::PathBuf;

use crate::output::printer;

/// The program ran, or the file checked clean.
pub const EXIT_OK: i32 = 0;
/// The file has diagnostics, or the program failed at runtime.
pub const EXIT_DIAGNOSTICS: i32 = 1;
/// The command could not be carried out at all (missing file, unreadable).
pub const EXIT_USAGE: i32 = 2;

/// Flags every subcommand receives.
#[derive(Debug, Clone)]
pub struct GlobalOpts {
    pub verbose: bool,
    pub quiet:   bool,
    pub json:    bool,
}

#[derive(Debug)]
pub enum CliError {
    FileNotFound(PathBuf),
    Io(std::io::Error),
    /// An internal stage failed after an earlier check had passed.
    Internal(String),
}

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CliError::FileNotFound(p) => write!(f, "file not found: {}", p.display()),
            CliError::Io(e)           => write!(f, "io error: {e}"),
            CliError::Internal(m)     => write!(f, "internal error: {m}"),
        }
    }
}

/// Print `err` and return the exit code for it.
pub fn handle_error(err: &CliError) -> i32 {
    printer::error(&err.to_string());
    EXIT_USAGE
}
