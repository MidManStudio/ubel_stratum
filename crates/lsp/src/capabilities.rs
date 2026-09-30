// ============================================================================
// NOTICE: Full documentation, design decisions, and fix history for this file
// live in docs/ubel_stratum_lsp.md, section "capabilities.rs"
// ============================================================================
// crates/lsp/src/capabilities.rs
//! What the server tells the client it can do.
//!
//! Diagnostics only, for now: full-text document sync so every edit is
//! re-checked. The capabilities `mdix-lsp` advertises (semantic tokens,
//! hover, completion, definition, references, rename, folding, formatting,
//! inlay hints, code actions, workspace symbols) each get added here in the
//! same change that adds their handler under `features/`, never ahead of it,
//! since a client will call anything advertised.

use tower_lsp::lsp_types::{ServerCapabilities, TextDocumentSyncCapability, TextDocumentSyncKind};

pub fn server_capabilities() -> ServerCapabilities {
    ServerCapabilities {
        text_document_sync: Some(TextDocumentSyncCapability::Kind(TextDocumentSyncKind::FULL)),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_full_document_sync_is_advertised() {
        let caps = server_capabilities();
        assert_eq!(
            caps.text_document_sync,
            Some(TextDocumentSyncCapability::Kind(TextDocumentSyncKind::FULL))
        );
        assert!(caps.hover_provider.is_none());
        assert!(caps.completion_provider.is_none());
    }
}
