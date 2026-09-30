// ============================================================================
// NOTICE: Full documentation, design decisions, and fix history for this file
// live in docs/ubel_stratum_rd.md, section "check.rs"
// ============================================================================
// crates/rd_parser/src/check.rs
//! One-call diagnostics for tooling.
//!
//! The command line and the language server both need the same thing: run
//! lex, parse and sema over a source string and get back every diagnostic
//! as data. Each of them used to have to repeat the stage sequencing and the
//! per-phase error draining that `examples/pipeline.rs` does by hand, so it
//! lives here once. Nothing in this module prints or renders; callers pick
//! their own presentation (`render_all`, JSON, LSP diagnostics).

use ubel_stratum::{
    ast::arena::AstArena,
    error_management::{Diagnosable, Diagnostic, ErrorManager},
};

/// The stage a check stopped at. `Clean` means every stage passed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Lex,
    Parse,
    Sema,
    Clean,
}

impl Stage {
    /// Stable lowercase name, used in machine-readable output.
    pub fn as_str(self) -> &'static str {
        match self {
            Stage::Lex   => "lex",
            Stage::Parse => "parse",
            Stage::Sema  => "sema",
            Stage::Clean => "clean",
        }
    }
}

/// Everything one check produced.
pub struct CheckReport {
    /// The first stage that reported errors, or `Clean`.
    pub stage:       Stage,
    /// Every diagnostic from that stage, in phase order.
    pub diagnostics: Vec<Diagnostic>,
}

impl CheckReport {
    pub fn is_clean(&self) -> bool {
        self.diagnostics.is_empty()
    }
}

/// Lex, parse and run semantic analysis over `source`. Stops at the first
/// stage that reports errors, since later stages have nothing sound to work
/// on once an earlier one failed.
pub fn check_source(source: &str) -> CheckReport {
    let tokens = match ubel_stratum::lexer::tokenize(source) {
        Ok(t)        => t,
        Err(mut errs) => return failed(Stage::Lex, &mut errs),
    };

    let arena = AstArena::new();
    let program = match crate::parse(&arena, &tokens, source.to_string()) {
        Ok(p)        => p,
        Err(mut errs) => return failed(Stage::Parse, &mut errs),
    };

    match ubel_stratum::sema::analyse(&program, &arena, source.to_string()) {
        Ok(_)        => CheckReport { stage: Stage::Clean, diagnostics: Vec::new() },
        Err(mut errs) => failed(Stage::Sema, &mut errs),
    }
}

fn failed(stage: Stage, errs: &mut ErrorManager) -> CheckReport {
    CheckReport { stage, diagnostics: drain(errs) }
}

fn push_all<E: Diagnosable>(out: &mut Vec<Diagnostic>, errs: Vec<E>) {
    out.extend(errs.iter().map(|e| e.to_diagnostic()));
}

/// Drain every per-phase error list into one flat, phase-ordered list.
fn drain(errs: &mut ErrorManager) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    push_all(&mut out, errs.take_lexical_errors());
    push_all(&mut out, errs.take_parse_errors());
    push_all(&mut out, errs.take_name_errors());
    push_all(&mut out, errs.take_lifetime_errors());
    push_all(&mut out, errs.take_type_errors());
    push_all(&mut out, errs.take_tier_errors());
    push_all(&mut out, errs.take_borrow_errors());
    push_all(&mut out, errs.take_move_errors());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_program_is_clean() {
        let r = check_source("fn main() void { println(\"hi\") }\n");
        assert_eq!(r.stage, Stage::Clean);
        assert!(r.is_clean());
    }

    #[test]
    fn type_error_stops_at_sema_with_its_code() {
        let r = check_source("fn main() void { let x: int = \"a\" }\n");
        assert_eq!(r.stage, Stage::Sema);
        assert_eq!(r.diagnostics.len(), 1);
        assert_eq!(r.diagnostics[0].code, "TYPE-101");
    }

    #[test]
    fn parse_error_stops_at_parse() {
        let r = check_source("fn main( void {\n");
        assert_eq!(r.stage, Stage::Parse);
        assert!(!r.diagnostics.is_empty());
    }

    #[test]
    fn stage_names_are_stable() {
        assert_eq!(Stage::Lex.as_str(), "lex");
        assert_eq!(Stage::Parse.as_str(), "parse");
        assert_eq!(Stage::Sema.as_str(), "sema");
        assert_eq!(Stage::Clean.as_str(), "clean");
    }
}
