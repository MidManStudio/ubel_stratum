// src/error_management/error_manager.rs

use crate::error_management::errors::{
    LexicalError, ParseError, NameError, TypeError, TierError, BorrowError, MoveError, LifetimeError,
};
use crate::error_management::logger::Logger;
use crate::error_management::Diagnosable;

/// Central error accumulator for the entire compiler pipeline.
///
/// Each phase appends its errors here.  The manager never stops the
/// pipeline immediately, callers check `has_errors()` at phase
/// boundaries and decide whether to continue.
///
/// Phase order:
///   1. Lex    → `add_lexical_error`
///   2. Parse  → `add_parse_error`
///   3. Resolve → `add_name_error`
///   4. LifetimeCheck (declaration well-formedness only) → `add_lifetime_error`
///   5. TypeCheck → `add_type_error`
///   6. TierCheck → `add_tier_error`
///   7. BorrowCheck (LOW tier only) → `add_borrow_error`
///   8. MoveCheck (LOW tier only) → `add_move_error`
#[derive(Debug)]
pub struct ErrorManager {
    lexical_errors:   Vec<LexicalError>,
    parse_errors:     Vec<ParseError>,
    name_errors:      Vec<NameError>,
    lifetime_errors:  Vec<LifetimeError>,
    type_errors:      Vec<TypeError>,
    tier_errors:      Vec<TierError>,
    borrow_errors:    Vec<BorrowError>,
    move_errors:      Vec<MoveError>,
    source:           String,
    max_errors:       usize,
}

impl ErrorManager {
    pub fn new(source: String) -> Self {
        ErrorManager {
            lexical_errors:  Vec::new(),
            parse_errors:    Vec::new(),
            name_errors:     Vec::new(),
            lifetime_errors: Vec::new(),
            type_errors:     Vec::new(),
            tier_errors:     Vec::new(),
            borrow_errors:   Vec::new(),
            move_errors:     Vec::new(),
            source,
            max_errors: 100,
        }
    }

    // ── Lexical ──────────────────────────────────────────────────

    pub fn add_lexical_error(&mut self, error: LexicalError) {
        if self.total_errors() < self.max_errors {
            self.lexical_errors.push(error);
        }
    }

    pub fn lexical_error_count(&self) -> usize { self.lexical_errors.len() }

    pub fn take_lexical_errors(&mut self) -> Vec<LexicalError> {
        std::mem::take(&mut self.lexical_errors)
    }

    // ── Parse ────────────────────────────────────────────────────

    pub fn add_parse_error(&mut self, error: ParseError) {
        if self.total_errors() < self.max_errors {
            self.parse_errors.push(error);
        }
    }

    pub fn parse_error_count(&self) -> usize { self.parse_errors.len() }

    pub fn take_parse_errors(&mut self) -> Vec<ParseError> {
        std::mem::take(&mut self.parse_errors)
    }

    // ── Name resolution ──────────────────────────────────────────

    pub fn add_name_error(&mut self, error: NameError) {
        if self.total_errors() < self.max_errors {
            self.name_errors.push(error);
        }
    }

    pub fn name_error_count(&self) -> usize { self.name_errors.len() }

    pub fn take_name_errors(&mut self) -> Vec<NameError> {
        std::mem::take(&mut self.name_errors)
    }

    // ── Type / tier checking ──────────────────────────────────────

    pub fn add_type_error(&mut self, error: TypeError) {
        if self.total_errors() < self.max_errors {
            self.type_errors.push(error);
        }
    }

    pub fn type_error_count(&self) -> usize { self.type_errors.len() }

    pub fn take_type_errors(&mut self) -> Vec<TypeError> {
        std::mem::take(&mut self.type_errors)
    }

    // ── Lifetime well-formedness ──────────────────────────────────

    pub fn add_lifetime_error(&mut self, error: LifetimeError) {
        if self.total_errors() < self.max_errors {
            self.lifetime_errors.push(error);
        }
    }

    pub fn lifetime_error_count(&self) -> usize { self.lifetime_errors.len() }

    pub fn take_lifetime_errors(&mut self) -> Vec<LifetimeError> {
        std::mem::take(&mut self.lifetime_errors)
    }

    // ── Tier / arena-boundary checking ──────────────────────────────

    pub fn add_tier_error(&mut self, error: TierError) {
        if self.total_errors() < self.max_errors {
            self.tier_errors.push(error);
        }
    }

    pub fn tier_error_count(&self) -> usize { self.tier_errors.len() }

    pub fn take_tier_errors(&mut self) -> Vec<TierError> {
        std::mem::take(&mut self.tier_errors)
    }

    // ── Borrow checking (LOW tier, Phase D) ─────────────────────────

    pub fn add_borrow_error(&mut self, error: BorrowError) {
        if self.total_errors() < self.max_errors {
            self.borrow_errors.push(error);
        }
    }

    pub fn borrow_error_count(&self) -> usize { self.borrow_errors.len() }

    pub fn take_borrow_errors(&mut self) -> Vec<BorrowError> {
        std::mem::take(&mut self.borrow_errors)
    }

    // ── Move checking (LOW tier) ─────────────────────────────────

    pub fn add_move_error(&mut self, error: MoveError) {
        if self.total_errors() < self.max_errors {
            self.move_errors.push(error);
        }
    }

    pub fn move_error_count(&self) -> usize { self.move_errors.len() }

    pub fn take_move_errors(&mut self) -> Vec<MoveError> {
        std::mem::take(&mut self.move_errors)
    }

    // ── Combined ─────────────────────────────────────────────────

    pub fn has_errors(&self) -> bool {
        !self.lexical_errors.is_empty()
            || !self.parse_errors.is_empty()
            || !self.name_errors.is_empty()
            || !self.lifetime_errors.is_empty()
            || !self.type_errors.is_empty()
            || !self.tier_errors.is_empty()
            || !self.borrow_errors.is_empty()
            || !self.move_errors.is_empty()
    }

    pub fn total_errors(&self) -> usize {
        self.lexical_errors.len()
            + self.parse_errors.len()
            + self.name_errors.len()
            + self.lifetime_errors.len()
            + self.type_errors.len()
            + self.tier_errors.len()
            + self.borrow_errors.len()
            + self.move_errors.len()
    }

    /// Backwards-compatible alias used by older call sites.
    pub fn error_count(&self) -> usize { self.total_errors() }

    /// Print every accumulated error grouped by phase.
    pub fn report_all(&self) {
        self.report_section("lexical", &self.lexical_errors);
        self.report_section("parse", &self.parse_errors);
        self.report_section("name resolution", &self.name_errors);
        self.report_section("lifetime", &self.lifetime_errors);
        self.report_section("type", &self.type_errors);
        self.report_section("tier", &self.tier_errors);
        self.report_section("borrow", &self.borrow_errors);
        self.report_section("move", &self.move_errors);
    }

    fn report_section<E: Diagnosable>(
        &self,
        phase: &str,
        errors: &[E],
    ) {
        if errors.is_empty() { return; }

        Logger::error(&format!("\n{} {} error(s):", errors.len(), phase));

        for (i, error) in errors.iter().enumerate() {
            let diag = error.to_diagnostic();
            eprint!("{}", crate::error_management::render(&diag, &self.source));
            if i < errors.len() - 1 { eprintln!(); }
        }
    }

    // Legacy helpers used by existing call sites in parser.
    pub fn take_errors(&mut self) -> Vec<LexicalError> {
        std::mem::take(&mut self.lexical_errors)
    }
}
