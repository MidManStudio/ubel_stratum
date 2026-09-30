// ============================================================================
// NOTICE: Full documentation, design decisions, and fix history for this file
// live in docs/ubel_stratum_lsp.md, section "document.rs"
// ============================================================================
// crates/lsp/src/document.rs
//! Per-document state.
//!
//! Today a document is its text and the editor's version number. The
//! mdix-lsp `Document` also caches the token stream, AST and semantic
//! result so features can read them without re-running the pipeline; that
//! arrives with the first feature that needs it, since the Ubel AST borrows
//! an arena and cannot simply be stored beside the source.

use tower_lsp::lsp_types::Url;

#[derive(Debug, Clone)]
pub struct Document {
    pub uri:     Url,
    pub source:  String,
    /// The editor's version counter, used to drop analysis results that
    /// finish after a newer edit arrived.
    pub version: i32,
}

impl Document {
    pub fn new(uri: Url, source: String, version: i32) -> Self {
        Document { uri, source, version }
    }
}
