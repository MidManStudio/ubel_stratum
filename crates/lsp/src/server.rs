// ============================================================================
// NOTICE: Full documentation, design decisions, and fix history for this file
// live in docs/ubel_stratum_lsp.md, section "server.rs"
// ============================================================================
// crates/lsp/src/server.rs
//! The tower-lsp `Backend`.

use std::sync::Arc;

use dashmap::DashMap;
use tower_lsp::jsonrpc::Result as LspResult;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer};

use crate::analyzer;
use crate::capabilities::server_capabilities;
use crate::converters::to_diagnostics;
use crate::document::Document;

pub struct Backend {
    client:    Client,
    documents: Arc<DashMap<Url, Document>>,
}

impl Backend {
    pub fn new(client: Client) -> Self {
        Backend { client, documents: Arc::new(DashMap::new()) }
    }

    /// Check the stored text of `uri` and publish the result, unless a newer
    /// edit arrived while the check ran, in which case the newer edit's own
    /// check publishes instead.
    async fn analyze_and_publish(&self, uri: Url, version: i32) {
        let source = match self.documents.get(&uri).map(|d| d.source.clone()) {
            Some(s) => s,
            None    => return,
        };

        let for_check = source.clone();
        let diags = match tokio::task::spawn_blocking(move || analyzer::analyze(&for_check)).await {
            Ok(d)  => d,
            Err(e) => {
                tracing::error!("analysis task failed for {uri}: {e}");
                return;
            }
        };

        let current = self.documents.get(&uri).map(|d| d.version);
        if current != Some(version) {
            tracing::debug!("dropping stale analysis for {uri} (v{version})");
            return;
        }

        let lsp_diags = to_diagnostics(&diags, &source, &uri);
        self.client.publish_diagnostics(uri, lsp_diags, Some(version)).await;
    }
}

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, _: InitializeParams) -> LspResult<InitializeResult> {
        Ok(InitializeResult {
            capabilities: server_capabilities(),
            server_info: Some(ServerInfo {
                name:    "ubel-lsp".to_string(),
                version: Some(env!("CARGO_PKG_VERSION").to_string()),
            }),
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        self.client.log_message(MessageType::INFO, "ubel-lsp ready").await;
    }

    async fn shutdown(&self) -> LspResult<()> {
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let doc = params.text_document;
        self.documents.insert(doc.uri.clone(), Document::new(doc.uri.clone(), doc.text, doc.version));
        self.analyze_and_publish(doc.uri, doc.version).await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        // Full-document sync: the last change event carries the whole text.
        let Some(change) = params.content_changes.into_iter().last() else { return };
        let uri     = params.text_document.uri;
        let version = params.text_document.version;
        self.documents.insert(uri.clone(), Document::new(uri.clone(), change.text, version));
        self.analyze_and_publish(uri, version).await;
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let uri = params.text_document.uri;
        self.documents.remove(&uri);
        // Clear what this server published for the closed file.
        self.client.publish_diagnostics(uri, Vec::new(), None).await;
    }
}
