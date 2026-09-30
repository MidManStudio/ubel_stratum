// ============================================================================
// NOTICE: Full documentation, design decisions, and fix history for this file
// live in docs/ubel_stratum_lsp.md, section "lib.rs"
// ============================================================================
// crates/lsp/src/lib.rs
//! `ubel-lsp` as a library.
//!
//! The binary in `src/main.rs` is a thin wrapper around [`run`]. Everything
//! internal (the tower-lsp `Backend`, the analysis pipeline, the feature
//! implementations) stays private; the supported surface is [`Document`],
//! [`run`] and [`setup_logging`].

mod analyzer;
mod capabilities;
mod converters;
mod document;
mod features;
mod server;

pub use document::Document;

/// Re-exported so a downstream crate names `Position`, `Diagnostic` and the
/// rest from the exact `tower_lsp` this crate was built against, instead of
/// adding its own possibly mismatched `tower-lsp` dependency.
pub use tower_lsp;

use tower_lsp::{LspService, Server};

/// Serve the language protocol over stdin and stdout until the client exits.
pub async fn run() {
    let stdin  = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    let (service, socket) = LspService::new(server::Backend::new);
    Server::new(stdin, stdout, socket).serve(service).await;
}

/// Set up tracing to stderr, and to a file too when `UBEL_LSP_LOG` names
/// one. `RUST_LOG` picks the level. Exposed so a downstream binary gets the
/// same logging behavior without re-implementing it.
pub fn setup_logging() {
    use tracing_subscriber::EnvFilter;

    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("ubel_stratum_lsp=info,warn"));

    let log_file = std::env::var("UBEL_LSP_LOG").ok().filter(|s| !s.is_empty());

    if let Some(path) = log_file {
        match std::fs::OpenOptions::new().create(true).append(true).open(&path) {
            Ok(file) => {
                let (non_blocking, guard) = tracing_appender::non_blocking(file);
                // The guard flushes the writer on drop; the process owns the
                // logger for its whole life, so leak it deliberately.
                Box::leak(Box::new(guard));

                tracing_subscriber::fmt()
                    .with_env_filter(filter)
                    .with_writer(non_blocking)
                    .with_ansi(false)
                    .with_target(true)
                    .init();

                eprintln!("[ubel-lsp] logging to file: {path}");
            }
            Err(e) => {
                eprintln!("[ubel-lsp] could not open log file {path}: {e}");
                setup_stderr_logging(filter);
            }
        }
    } else {
        setup_stderr_logging(filter);
    }
}

fn setup_stderr_logging(filter: tracing_subscriber::EnvFilter) {
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .with_target(true)
        .init();
}
