// ============================================================================
// NOTICE: Full documentation, design decisions, and fix history for this file
// live in docs/ubel_stratum_lsp.md, section "features/mod.rs"
// ============================================================================
// crates/lsp/src/features/mod.rs
//! Feature handlers, one file each, added as they are built.
//!
//! Nothing lives here yet. The folder exists so the layout matches
//! `mdix-lsp`, whose `features/` holds `hover`, `completions`,
//! `goto_definition`, `references`, `rename`, `document_symbols`,
//! `workspace_symbols`, `semantic_tokens`, `folding`, `formatting`,
//! `inlay_hints`, `signature_help` and `code_actions`. The first ones worth
//! building for Ubel are hover (show a resolved type from sema), go to
//! definition (name resolution already records a `DefId` per identifier),
//! and semantic tokens (the lexer's token kinds map directly).
