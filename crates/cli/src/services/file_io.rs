// ============================================================================
// NOTICE: Full documentation, design decisions, and fix history for this file
// live in docs/ubel_stratum_cli.md, section "services/file_io.rs"
// ============================================================================
// crates/cli/src/services/file_io.rs
//! Reading source files.

use std::fs;
use std::io::ErrorKind;
use std::path::Path;

use crate::commands::CliError;

pub fn read_source(path: &Path) -> Result<String, CliError> {
    fs::read_to_string(path).map_err(|e| match e.kind() {
        ErrorKind::NotFound => CliError::FileNotFound(path.to_path_buf()),
        _                   => CliError::Io(e),
    })
}
