// ============================================================================
// NOTICE: Full documentation, design decisions, and fix history for this file
// live in docs/ubel_stratum_cli.md, section "output/printer.rs"
// ============================================================================
// crates/cli/src/output/printer.rs
//! Colored terminal output helpers. `--no-color` turns every call here into
//! plain text through `colored::control::set_override(false)`.

use colored::Colorize;
use std::time::Duration;

pub fn success(msg: &str) {
    println!("{} {}", "\u{2713}".green().bold(), msg);
}

pub fn error(msg: &str) {
    eprintln!("{} {}", "\u{2717}".red().bold(), msg);
}

/// Print a key and value with a padded key column.
pub fn kv(key: &str, value: &str) {
    println!("  {:<12} {}", key.bold(), value);
}

pub fn duration(d: Duration) {
    println!("  {:<12} {:.2} ms", "elapsed".bold(), d.as_secs_f64() * 1000.0);
}
