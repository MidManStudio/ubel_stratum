// ============================================================================
// NOTICE: Full documentation, design decisions, and fix history for this file
// live in docs/ubel_stratum_cli.md, section "output/json_output.rs"
// ============================================================================
// crates/cli/src/output/json_output.rs
//! JSON output for `--json`.

use serde::Serialize;

/// Pretty-print `value` as JSON on stdout.
pub fn print_result<T: Serialize>(value: T) {
    match serde_json::to_string_pretty(&value) {
        Ok(s)  => println!("{s}"),
        Err(e) => eprintln!("could not serialize output: {e}"),
    }
}
