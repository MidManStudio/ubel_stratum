// ============================================================================
// NOTICE: Full documentation, design decisions, and fix history for this file
// live in docs/ubel_stratum_lsp.md, section "analyzer.rs"
// ============================================================================
// crates/lsp/src/analyzer.rs
//! Runs the compiler pipeline over one document.
//!
//! Goes through `ubel_stratum_rd::check_source`, the same entry the `ubel`
//! command line uses, so the editor and `ubel check` report the same
//! diagnostics for the same text. A compiler panic must not take the
//! server down, so the call is guarded, as `mdix-lsp`'s `run_pipeline` does.

use std::panic::{self, AssertUnwindSafe};

use ubel_stratum::error_management::Diagnostic;

/// Diagnostics for `source`, or none if the compiler panicked (logged).
pub fn analyze(source: &str) -> Vec<Diagnostic> {
    guard(|| ubel_stratum_rd::check_source(source).diagnostics)
}

fn guard<F: FnOnce() -> Vec<Diagnostic>>(f: F) -> Vec<Diagnostic> {
    match panic::catch_unwind(AssertUnwindSafe(f)) {
        Ok(diags)   => diags,
        Err(payload) => {
            let msg = payload
                .downcast_ref::<String>().cloned()
                .or_else(|| payload.downcast_ref::<&str>().map(|s| s.to_string()))
                .unwrap_or_else(|| "unknown panic".to_string());
            tracing::error!("analysis panicked: {msg}");
            Vec::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_clean_document_has_no_diagnostics() {
        assert!(analyze("fn main() void { println(\"hi\") }\n").is_empty());
    }

    #[test]
    fn a_type_error_is_reported_with_its_code() {
        let diags = analyze("fn main() void { let x: int = \"a\" }\n");
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].code, "TYPE-101");
    }

    #[test]
    fn a_panic_becomes_no_diagnostics_instead_of_unwinding() {
        let diags = guard(|| panic!("boom"));
        assert!(diags.is_empty());
    }
}
