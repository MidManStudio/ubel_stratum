// src/sema/mod.rs
//! Semantic analysis, six passes over the arena AST.
//!
//! Pass order:
//!   1. name_resolution  → SymbolTable, ResolutionMap, top_level map
//!   2. lifetime_check   → well-formedness of `[lifetime L]`/`[lifetime
//!      L where L outlives M]` declarations on functions and structs,
//!      and of the lifetime names their own signatures/fields use.
//!      Purely structural, no CFG or type information needed, which is
//!      why it runs this early rather than alongside borrow_check. Does
//!      NOT check that real usage respects a declared outlives bound,
//!      that fixed point is still borrow_check's unbuilt job (see
//!      lifetime_check.rs's own module doc)
//!   3. type_infer       → TypeTable, expr_types, def_types, arena coloring
//!   4. tier_check       → enforces HIGH/MID/LOW cross-tier rules
//!   5. borrow_check     → LOW-tier liveness-gated borrow checking
//!      (Phase D: cfg.rs builds the graph, facts.rs collects loan/kill/
//!      invalidation facts over it, borrow_check.rs runs the actual
//!      liveness/reaching fixed point and turns real violations into
//!      diagnostics; see borrow_check.rs's module doc for exactly what
//!      this pass does and doesn't catch yet)
//!   6. move_check       → LOW-tier use-after-move checking for
//!      `Unique<T>` locals (move_facts.rs collects move candidates over
//!      the same cfg.rs graph, move_check.rs runs the reachability fixed
//!      point and turns real violations into diagnostics; see
//!      move_check.rs's module doc for exactly what "reaches" means,
//!      including the loop-back-edge case)
//!
//! Each pass appends errors to a shared ErrorManager and the orchestrator
//! stops after any phase that produced errors.

pub mod symbol_table;
pub mod sema_context;
pub mod type_table;
pub mod name_resolution;
pub mod lifetime_check;
pub mod type_infer;
pub mod tier_check;
pub mod borrow_check;
pub mod move_check;

#[cfg(test)]
mod tests;
mod cfg;
mod facts;
mod move_facts;

pub use symbol_table::{DefId, DefKind, Def, SymbolTable, ResolutionMap, Scope, ScopeStack};
pub use sema_context::SemaContext;
pub use type_table::{TypeId, TypeTable, SemaType, ArenaId};

use crate::ast::arena::AstArena;
use crate::ast::root::Program;
use crate::error_management::{ErrorManager, errors::{BorrowError, MoveError}};

/// Run all semantic analysis passes on `program`.
/// Returns a populated `SemaContext` on success, `Err(ErrorManager)` on failure.
pub fn analyse<'ast>(
    program: &Program<'ast>,
    _arena:  &'ast AstArena,
    source:  String,
) -> Result<SemaContext, ErrorManager> {
    let mut errors = ErrorManager::new(source);
    let mut ctx    = SemaContext::new();

    // ── Pass 1: Name resolution ──────────────────────────────────
    name_resolution::resolve(program, &mut ctx, &mut errors);
    if errors.has_errors() {
        return Err(errors);
    }

    // ── Pass 2: Lifetime declaration well-formedness ─────────────
    lifetime_check::check(program, &mut errors);
    if errors.has_errors() {
        return Err(errors);
    }

    // ── Pass 3: Type inference + arena coloring ───────────────────
    type_infer::infer(program, &mut ctx, &mut errors);
    if errors.has_errors() {
        return Err(errors);
    }

    // ── Pass 4: Tier rule enforcement ─────────────────────────────
    tier_check::check(program, &ctx, &mut errors);
    if errors.has_errors() {
        return Err(errors);
    }

    // ── Pass 5: LOW-tier borrow checking (Phase D) ────────────────
    for violation in borrow_check::check_program(program) {
        errors.add_borrow_error(BorrowError::ConflictingAccessWhileBorrowed {
            place: violation.place,
            loan_span: violation.loan_span,
            conflict_span: violation.conflict_span,
        });
    }
    if errors.has_errors() {
        return Err(errors);
    }

    // ── Pass 6: LOW-tier move checking ────────────────────────────
    for violation in move_check::check_program(program) {
        errors.add_move_error(MoveError::UseAfterMove {
            place: violation.place,
            moved_span: violation.moved_span,
            used_span: violation.used_span,
        });
    }
    if errors.has_errors() {
        return Err(errors);
    }

    Ok(ctx)
}
