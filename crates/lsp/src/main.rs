// ============================================================================
// NOTICE: Full documentation, design decisions, and fix history for this file
// live in docs/ubel_stratum_lsp.md, section "main.rs"
// ============================================================================
// crates/lsp/src/main.rs
//! ubel-lsp: language server for Ubel Stratum (.ubl files).
//!
//! A thin wrapper over the `ubel_stratum_lsp` library, the same split
//! `mdix-lsp` uses, so a downstream tool can embed the server without
//! forking this binary.
//!
//! # Logging
//!
//! All tracing goes to stderr, never stdout: stdout is the LSP stdio channel.
//!
//! - `RUST_LOG=ubel_stratum_lsp=debug ubel-lsp` sets the level.
//! - `UBEL_LSP_LOG=/tmp/ubel-lsp.log ubel-lsp` also writes to a file.

#[tokio::main]
async fn main() {
    ubel_stratum_lsp::setup_logging();
    ubel_stratum_lsp::run().await;
}
