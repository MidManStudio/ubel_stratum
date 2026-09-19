// crates/core/src/sema/facts.rs
//! Phase C of the LOW-tier borrow checker: fact collection over the CFG
//! `cfg.rs` builds. Produces the raw, AST-driven inputs Phase D's
//! liveness/loan fixed point consumes (see `borrow_check.rs`). Like
//! `cfg.rs`, this module does no fixed-point computation itself — it's a
//! pure, cheap, per-function AST scan; the actual dataflow lives entirely
//! in `borrow_check.rs`, which wires both this and `cfg.rs` into
//! `sema::analyse`.
//!
//! ## Scope, stated plainly
//!
//! **Loans only — no move tracking.** Move/ownership checking
//! (`Unique<T>` semantics) is real, separate work — now real and landed
//! as its own module, `sema/move_facts.rs`, not folded into this one
//! (a genuinely different question: "was this consumed" vs. this
//! module's "is a live reference still watching this"). `Unique<T>`/
//! `Shared<T>`/`SyncShared<T>` have real type-level wiring, real
//! `Unique.new(...)` construction syntax, and real fact collection over
//! that construction syntax (see `MEMORY_MODEL.md` §9) — but no
//! reachability/violation *fixed point* reads those facts yet, so
//! there's still no enforcement rejecting a used-after-moved value.
//! Loans (`&`/`ref`, `&mut`/`ref mut`) have a fully settled story — real
//! syntax, real structural types (see §5.6), real checking
//! (`borrow_check.rs`) — so that's what this collects.
//!
//! **Places: local bindings only.** `&x` is tracked precisely
//! (`Place::Local`). `&x.field`, `&arr[i]` are still recorded as real
//! loans — nothing silently vanishes — but as `Place::Unknown`, with no
//! invalidation-conflict detection. Widening `Place` to real field/index
//! paths is valid, real follow-up work, not a soundness bug in what's
//! here: `Place::Unknown` is an *under*-approximation (fewer conflicts
//! found than a precise analysis would), never an over-approximation.
//! Phase D must treat `Place::Unknown` loans as "insufficient
//! information," not "no conflict."
//!
//! **Expression walking stops at the same boundary `cfg.rs` draws.**
//! Control-flow-as-*expression* (`if`/`match` used as a value, a `{ }`
//! block expression, a lambda body) is opaque — not descended into, for
//! the same reason `cfg.rs` doesn't decompose it: doing so needs a
//! statement walker layered on an expression walker, not just the
//! expression walker this module builds. A borrow hiding inside one of
//! these is simply never found — again, under-approximation, not unsound.
//!
//! **`loan_invalidated_at` is a pure syntactic scan, not CFG-reachability-
//! aware.** For each loan, every *other* statement in the whole function
//! gets checked for a conflicting access to the same place, regardless of
//! whether that statement is actually reachable from the loan's issue
//! point. Deliberately over-approximate — Phase D, which already needs
//! full CFG liveness for its own propagation, is where reachability
//! belongs. A candidate invalidation Phase D's liveness later shows is
//! unreachable from the loan just never gets combined into a real error;
//! over-approximating here costs a few discarded candidates, nothing more.
//!
//! ## Amendment for Phase D (`Loan::bound_place`, `Facts::place_defined_at`)
//!
//! Added once Phase D's actual needs became concrete — liveness-gating a
//! loan requires knowing which *reference-typed local* might still read
//! it, and this module previously only recorded what a loan borrows
//! *from*, never what it's bound *to*. Two small, additive facts:
//!
//! - `Loan::bound_place` — for a bare `let p = &x` / `p = &x`, the
//!   traceable local `p`. `Place::Unknown` when the borrow is consumed
//!   inline (a call argument, a return expression, nested in a larger
//!   expression) — there's no local Phase D can ask "is this still going
//!   to be used later" about, so such loans are only ever live at their
//!   own `issued_at` point (which `loan_invalidated_at` already excludes
//!   from candidates — see its own scan below). Under-approximation, not
//!   unsound, same spirit as `Place::Unknown` elsewhere in this module.
//! - `Facts::place_defined_at` — every point where a *simple local*
//!   place is freshly bound or reassigned. General-purpose, not
//!   loan-specific: Phase D uses it both as the "def" side of ordinary
//!   liveness for `bound_place`s, and to know when a loan's own
//!   `bound_place` stops referring to that loan (rebound to a new value,
//!   including a new loan) — a kill `loan_killed_at` alone can't express,
//!   since that only tracks reassignment of the *borrowed-from* place.
//!
//! ## Retrofit onto `ast::visitor::AstVisitor`
//!
//! This module's own hand-rolled `walk_expr` (deep recursion into one
//! expression tree) is now a thin adapter over the shared
//! `ast::visitor::walk_expr` instead of a second copy of the same
//! `ExprKind` match — see `ScopedExprWalker` below for exactly how its
//! opacity boundary (control-flow-as-*expression* stays undescended,
//! same one `cfg.rs` draws) survives the retrofit unchanged.
//! `for_each_top_expr` deliberately stays hand-written rather than
//! retrofitted onto the shared `walk_stmt` — see the comment directly
//! above it for the real, subtle reason (the shared walker's `if`/`match`
//! handling reaches into branch *bodies* this module was never meant to
//! see, since `cfg.rs` itself discards a `then expr`-shaped body without
//! ever recording it as a statement anywhere).

use std::collections::HashMap;

use crate::ast::expressions::{Expr, ExprKind};
use crate::ast::statements::{Stmt, StmtKind};
use crate::lexer::Span;
use crate::sema::cfg::{BlockId, Cfg};

// ── Points and places ───────────────────────────────────────────────────

/// A point in the CFG — one specific statement within one specific block.
/// Matches `cfg.rs`'s own statement granularity; there's no finer
/// "start"/"mid" split the way Polonius uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Point {
    pub block: BlockId,
    pub stmt_index: usize,
}

/// What a loan borrows from. See the module doc's scope note — `Local`
/// is precise, everything else is `Unknown` for now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Place<'ast> {
    Local(&'ast str),
    Unknown,
}

/// `pub(crate)`, not private: `sema::outlives_check` (Phase E2,
/// `docs/OUTLIVES_RULES.md`) reuses this unchanged to resolve a call's
/// actual argument expression back to a `Place` — same rule, same
/// meaning of `Unknown` (a fresh inline expression, nothing this
/// module's own loans track).
pub(crate) fn expr_as_place<'ast>(expr: &'ast Expr<'ast>) -> Place<'ast> {
    match &expr.kind {
        ExprKind::Ident(name) => Place::Local(name),
        _ => Place::Unknown,
    }
}

// ── Loans ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LoanId(pub u32);

#[derive(Debug, Clone, Copy)]
pub struct Loan<'ast> {
    pub id: LoanId,
    pub place: Place<'ast>,
    /// The local this loan's resulting reference value is directly bound
    /// to — `let bound_place = &place` / `bound_place = &place`.
    /// `Place::Unknown` when the borrow is consumed inline (a call
    /// argument, a return expression, nested in a larger expression) —
    /// see the module doc's Phase D amendment note.
    pub bound_place: Place<'ast>,
    pub mutable: bool,
    pub issued_at: Point,
    pub span: Span,
}

#[derive(Debug, Default)]
pub struct Facts<'ast> {
    pub loans: Vec<Loan<'ast>>,
    /// Points where a loan's place is reassigned — its "natural" kill,
    /// before any liveness-based shrinking Phase D does.
    pub loan_killed_at: HashMap<LoanId, Vec<Point>>,
    /// Points where some access conflicts with a loan's terms. See the
    /// module doc's over-approximation note.
    pub loan_invalidated_at: HashMap<LoanId, Vec<Point>>,
    /// Every point where a *simple local* place is freshly bound
    /// (`let place = ...`) or reassigned (`place = ...`). General-
    /// purpose, not loan-specific — see the module doc's Phase D
    /// amendment note.
    pub place_defined_at: HashMap<Place<'ast>, Vec<Point>>,
}

/// Collects every loan/kill/invalidation-candidate fact for one
/// function's CFG. Doesn't check tier — callers decide which functions
/// are worth running this on (LOW-tier ones; see `cfg::build`'s own
/// doc note, same division of responsibility).
pub fn collect<'ast>(cfg: &Cfg<'ast>) -> Facts<'ast> {
    let mut facts = Facts::default();
    let mut next_id = 0u32;

    for block in &cfg.blocks {
        for (stmt_index, stmt) in block.stmts.iter().enumerate() {
            let point = Point { block: block.id, stmt_index };

            if let Some(defined) = stmt_defines_place(stmt) {
                facts.place_defined_at.entry(defined).or_default().push(point);
            }

            // If this statement directly binds a *bare* borrow to a
            // simple local (`let p = &x` / `p = &x` — value is a Borrow
            // node itself, not one nested inside a call or another
            // expression), remember which place it binds to. `walk_expr`
            // visits a node before recursing into it, so when this shape
            // matches, the very first loan `collect_loans_in_expr` pushes
            // below is always this outer borrow, never a nested one.
            let bare_target = stmt_bare_borrow_target(stmt);
            let loans_before = facts.loans.len();

            for_each_top_expr(stmt, &mut |e| collect_loans_in_expr(e, point, &mut facts.loans, &mut next_id));

            if let Some(target) = bare_target {
                if let Some(loan) = facts.loans.get_mut(loans_before) {
                    loan.bound_place = target;
                }
            }
        }
    }

    for loan in &facts.loans {
        for block in &cfg.blocks {
            for (stmt_index, stmt) in block.stmts.iter().enumerate() {
                let point = Point { block: block.id, stmt_index };
                if point == loan.issued_at { continue; }
                match classify_access(stmt, loan.place) {
                    Access::None => {}
                    Access::Reassign => {
                        facts.loan_killed_at.entry(loan.id).or_default().push(point);
                    }
                    Access::Conflict => {
                        facts.loan_invalidated_at.entry(loan.id).or_default().push(point);
                    }
                }
            }
        }
    }

    facts
}

fn collect_loans_in_expr<'ast>(
    expr: &'ast Expr<'ast>, point: Point, loans: &mut Vec<Loan<'ast>>, next_id: &mut u32,
) {
    walk_expr(expr, &mut |e| {
        if let ExprKind::Borrow { mutable, place } = &e.kind {
            let id = LoanId(*next_id);
            *next_id += 1;
            loans.push(Loan {
                id, place: expr_as_place(place), bound_place: Place::Unknown,
                mutable: *mutable, issued_at: point, span: e.span,
            });
        }
    });
}

// ── Access classification ───────────────────────────────────────────────

enum Access {
    None,
    /// The place was reassigned (the loan's "natural" kill).
    Reassign,
    /// The place was read, or re-borrowed, conflicting with an
    /// outstanding loan's terms.
    Conflict,
}

/// Whether `stmt` reassigns or conflictingly-accesses `place`.
/// `Place::Unknown` never matches anything — see the module doc.
fn classify_access<'ast>(stmt: &'ast Stmt<'ast>, place: Place<'ast>) -> Access {
    if matches!(place, Place::Unknown) { return Access::None; }

    // Reassignment first: `place = value` kills any outstanding loan of
    // `place`, and doesn't ALSO count as a conflicting read of the old
    // value (the assignment's own RHS is checked separately, in case it
    // reads `place` on its way to overwriting it — e.g. `x = x + 1`,
    // which IS a real conflicting read of the old value if `x` is
    // mutably borrowed elsewhere).
    if let StmtKind::Expr(e) = &stmt.kind {
        if let ExprKind::Assign { target, value, .. } = &e.kind {
            if expr_as_place(target) == place {
                if expr_reads_place(value, place) {
                    return Access::Conflict;
                }
                return Access::Reassign;
            }
        }
    }

    if stmt_reads_place(stmt, place) { Access::Conflict } else { Access::None }
}

/// Does `stmt` (anywhere among its own top-level expressions — see
/// `for_each_top_expr`'s scope) read `place`? Shared by `classify_access`
/// above and, since Phase D's per-`bound_place` liveness needs the exact
/// same "does this statement use X" question for an arbitrary local, by
/// `borrow_check.rs` too.
pub(crate) fn stmt_reads_place<'ast>(stmt: &'ast Stmt<'ast>, place: Place<'ast>) -> bool {
    let mut found = false;
    for_each_top_expr(stmt, &mut |e| {
        if expr_reads_place(e, place) { found = true; }
    });
    found
}

/// Does `expr` (or anything it directly contains, per `walk_expr`'s scope)
/// read `place` — as a plain use, or by borrowing it again?
fn expr_reads_place<'ast>(expr: &'ast Expr<'ast>, place: Place<'ast>) -> bool {
    let mut found = false;
    walk_expr(expr, &mut |e| {
        match &e.kind {
            ExprKind::Ident(name) if Place::Local(name) == place => found = true,
            ExprKind::Borrow { place: inner, .. } if expr_as_place(inner) == place => found = true,
            _ => {}
        }
    });
    found
}

/// If `stmt` freshly binds or reassigns a *simple local* place — `let
/// name = ...` or `name = ...` — returns that place. `None` for
/// destructuring bindings, field/index assignment targets, and any
/// statement shape that isn't a direct binding at all; see the module
/// doc's `Place::Unknown`-precision-only-for-simple-locals note. Used
/// both to populate `Facts::place_defined_at` and, by `borrow_check.rs`,
/// as the "def" side of `bound_place` liveness.
pub(crate) fn stmt_defines_place<'ast>(stmt: &'ast Stmt<'ast>) -> Option<Place<'ast>> {
    match &stmt.kind {
        StmtKind::Let { binding: crate::ast::statements::BindingTarget::Ident(name), .. } => {
            Some(Place::Local(name))
        }
        StmtKind::Expr(e) => {
            if let ExprKind::Assign { target, .. } = &e.kind {
                match expr_as_place(target) {
                    p @ Place::Local(_) => Some(p),
                    Place::Unknown => None,
                }
            } else {
                None
            }
        }
        _ => None,
    }
}

/// If `stmt` directly binds a *bare* borrow expression to a simple local
/// (`let p = &x` / `p = &x` — the value is a `Borrow` node itself, not
/// nested inside a call or another expression), returns that local's
/// place. This is how `collect` distinguishes "traceable reference,
/// worth Phase D liveness-tracking" loans from "consumed inline" ones —
/// see `Loan::bound_place`'s doc.
fn stmt_bare_borrow_target<'ast>(stmt: &'ast Stmt<'ast>) -> Option<Place<'ast>> {
    let (target, value) = match &stmt.kind {
        StmtKind::Let { binding: crate::ast::statements::BindingTarget::Ident(name), value, .. } => {
            (Place::Local(name), *value)
        }
        StmtKind::Expr(e) => {
            if let ExprKind::Assign { op: crate::ast::common::AssignOp::Assign, target, value } = &e.kind {
                (expr_as_place(target), *value)
            } else {
                return None;
            }
        }
        _ => return None,
    };
    if matches!(value.kind, ExprKind::Borrow { .. }) { Some(target) } else { None }
}

// ── Expression walking ──────────────────────────────────────────────────
//
// `for_each_top_expr` stays hand-written, not retrofitted onto the
// shared `ast::visitor::walk_stmt` — deliberately. That shared walker's
// `if`/`match` handling also visits `then expr`/bare-match-arm-expr
// branch *bodies* (not just conditions/scrutinees), because passes that
// want full depth need that. This module's scope boundary is narrower:
// `cfg.rs` never even records a `then expr`-shaped branch body as a
// statement anywhere (`build_branch_body`'s `BranchBody::Expr(_) =>
// (entry, Some(entry))` — the expression itself is discarded, the block
// stays empty), so this module has no CFG point to attach such a finding
// to even if it looked. Making `for_each_top_expr` reach past what
// `cfg.rs` itself tracks would be a real scope *widening*, not a refactor
// — so it keeps its own explicit, narrower dispatch below.
//
// `walk_expr` (deep recursion into a single expression tree) is the part
// that safely retrofits: its opacity boundary (Lambda/Block/If/Match-as-
// *expression* are opaque, matching `cfg.rs`'s identical one) can be
// enforced as a single check in one place, with everything else handed
// off to the shared walker instead of hand-rolling every `ExprKind` arm
// a second time.

/// Extracts the direct, non-block-body expression(s) a statement carries
/// — e.g. an `if`'s condition, not its branch bodies. Branch/loop bodies
/// are already separate blocks in `cfg.rs`'s output, visited by
/// `collect`'s own outer loop over every block — this only needs the
/// expressions living directly ON the statement itself.
///
/// `pub(crate)`, not private: `sema::move_facts` reuses this unchanged
/// for the exact same reason — same statement-to-expression mapping,
/// same scope boundary, no reason to duplicate it a second time.
pub(crate) fn for_each_top_expr<'ast>(stmt: &'ast Stmt<'ast>, f: &mut impl FnMut(&'ast Expr<'ast>)) {
    match &stmt.kind {
        StmtKind::Let { value, .. } => f(value),
        StmtKind::Expr(e) => f(e),
        StmtKind::Return(e) | StmtKind::Break(e) => { if let Some(e) = e { f(e); } }
        StmtKind::Fail(e) | StmtKind::Defer(e) => f(e),
        StmtKind::If(if_expr) => {
            f(if_expr.condition);
            for elif in if_expr.elif_branches { f(elif.condition); }
        }
        StmtKind::Match { scrutinee, .. } => f(scrutinee),
        StmtKind::For { iter, .. } => f(iter),
        StmtKind::While { condition, .. } => f(condition),
        StmtKind::Using { bindings, .. } => { for b in *bindings { f(b.value); } }
        StmtKind::Extract { value, .. } => f(value),
        // Loop/Continue carry no expression of their own. With's
        // allocator size, Try's catch binding, and Unsafe's body are
        // deliberately not walked here — none plausibly carries a loan
        // worth tracking in v1, and body statements are separate blocks
        // already, same as If/Match/loop bodies.
        StmtKind::Loop(_) | StmtKind::Continue
        | StmtKind::With { .. } | StmtKind::Try { .. } | StmtKind::Unsafe(_) => {}
    }
}

/// Thin adapter over the shared `ast::visitor::walk_expr` — same public
/// shape (`expr` plus a `FnMut` callback) this module's `walk_expr` had
/// before the retrofit, so `collect_loans_in_expr`/`expr_reads_place`
/// below needed zero changes. The one thing it has to do itself: enforce
/// this module's opacity boundary (control-flow-as-*expression* isn't
/// descended into — see the module doc and the comment above
/// `for_each_top_expr`), since the shared walker is built to go all the
/// way down for passes that want that.
struct ScopedExprWalker<'w, 'ast, F: FnMut(&'ast Expr<'ast>)> {
    visit: &'w mut F,
    _marker: std::marker::PhantomData<&'ast ()>,
}

impl<'w, 'ast, F: FnMut(&'ast Expr<'ast>)> crate::ast::visitor::AstVisitor<'ast>
    for ScopedExprWalker<'w, 'ast, F>
{
    fn visit_expr(&mut self, e: &'ast Expr<'ast>) {
        (self.visit)(e);
        // Opacity boundary — matches cfg.rs's identical one exactly.
        if matches!(e.kind, ExprKind::Lambda(_) | ExprKind::Block(_) | ExprKind::If(_) | ExprKind::Match(_)) {
            return;
        }
        crate::ast::visitor::walk_expr(self, e);
    }
}

/// Calls `visit` on `expr` and recursively on every sub-expression it
/// directly contains, respecting this module's opacity boundary.
fn walk_expr<'ast>(expr: &'ast Expr<'ast>, visit: &mut impl FnMut(&'ast Expr<'ast>)) {
    use crate::ast::visitor::AstVisitor;
    ScopedExprWalker { visit, _marker: std::marker::PhantomData }.visit_expr(expr);
}

// ── Tests ────────────────────────────────────────────────────────────
//
// Same self-contained, hand-built-AST approach as cfg.rs's own tests —
// builds real function bodies via the arena, runs them through
// cfg::build then facts::collect, and asserts on the real output.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::arena::AstArena;
    use crate::ast::common::{AssignOp, Span, TierAnnotation, Visibility};
    use crate::ast::declarations::FunctionDecl;
    use crate::ast::expressions::{Arg, ArgKind};
    use crate::ast::literals::Literal;
    use crate::sema::cfg;

    const Z: Span = Span { start: 0, end: 0, line: 0, column: 0 };

    fn ident<'a>(arena: &'a AstArena, name: &'a str) -> &'a Expr<'a> {
        arena.alloc(Expr { kind: ExprKind::Ident(arena.alloc_str(name)), span: Z })
    }

    fn lit_int<'a>(arena: &'a AstArena, n: i64) -> &'a Expr<'a> {
        arena.alloc(Expr { kind: ExprKind::Lit(Literal::Int(n)), span: Z })
    }

    fn borrow<'a>(arena: &'a AstArena, mutable: bool, place: &'a Expr<'a>) -> &'a Expr<'a> {
        arena.alloc(Expr { kind: ExprKind::Borrow { mutable, place }, span: Z })
    }

    fn let_stmt<'a>(arena: &'a AstArena, name: &'a str, value: &'a Expr<'a>) -> Stmt<'a> {
        Stmt {
            kind: StmtKind::Let {
                mutable: false,
                binding: crate::ast::statements::BindingTarget::Ident(arena.alloc_str(name)),
                ty: None,
                value,
            },
            span: Z,
        }
    }

    fn reassign_stmt<'a>(arena: &'a AstArena, name: &'a str, value: &'a Expr<'a>) -> Stmt<'a> {
        let target = ident(arena, name);
        let assign = arena.alloc(Expr {
            kind: ExprKind::Assign { op: AssignOp::Assign, target, value },
            span: Z,
        });
        Stmt { kind: StmtKind::Expr(assign), span: Z }
    }

    fn func<'a>(arena: &'a AstArena, stmts: &[Stmt<'a>]) -> FunctionDecl<'a> {
        FunctionDecl {
            tier: TierAnnotation::default(), attributes: &[], visibility: Visibility::default(),
            is_async: false, name: arena.alloc_str("f"), lifetime_params: &[], generic_params: &[],
            params: &[], return_type: None,
            body: crate::ast::statements::Block { stmts: arena.alloc_slice_copy(stmts), span: Z },
            span: Z,
        }
    }

    fn facts_for<'a>(decl: &'a FunctionDecl<'a>) -> Facts<'a> {
        let graph = cfg::build(decl);
        collect(&graph)
    }

    #[test]
    fn shared_borrow_is_one_loan() {
        let arena = AstArena::new();
        let x = ident(&arena, "x");
        let stmts = [let_stmt(&arena, "p", borrow(&arena, false, x))];
        let decl = arena.alloc(func(&arena, &stmts));
        let facts = facts_for(decl);

        assert_eq!(facts.loans.len(), 1);
        assert_eq!(facts.loans[0].place, Place::Local("x"));
        assert!(!facts.loans[0].mutable);
    }

    #[test]
    fn mutable_borrow_is_flagged_mutable() {
        let arena = AstArena::new();
        let x = ident(&arena, "x");
        let stmts = [let_stmt(&arena, "p", borrow(&arena, true, x))];
        let decl = arena.alloc(func(&arena, &stmts));
        let facts = facts_for(decl);

        assert_eq!(facts.loans.len(), 1);
        assert!(facts.loans[0].mutable);
    }

    #[test]
    fn reassignment_kills_the_loan() {
        let arena = AstArena::new();
        let x1 = ident(&arena, "x");
        let stmts = [
            let_stmt(&arena, "p", borrow(&arena, false, x1)),
            reassign_stmt(&arena, "x", lit_int(&arena, 5)),
        ];
        let decl = arena.alloc(func(&arena, &stmts));
        let facts = facts_for(decl);

        assert_eq!(facts.loans.len(), 1);
        let killed = facts.loan_killed_at.get(&facts.loans[0].id);
        assert_eq!(killed.map(|v| v.len()), Some(1), "x = 5 should kill the loan on x");
        assert!(facts.loan_invalidated_at.get(&facts.loans[0].id).is_none(),
            "a pure reassignment is a kill, not a conflicting read");
    }

    #[test]
    fn read_while_mutably_borrowed_is_invalidation() {
        let arena = AstArena::new();
        let x1 = ident(&arena, "x");
        let x2 = ident(&arena, "x");
        let stmts = [
            let_stmt(&arena, "p", borrow(&arena, true, x1)),
            let_stmt(&arena, "y", x2), // reads x while &mut x is outstanding
        ];
        let decl = arena.alloc(func(&arena, &stmts));
        let facts = facts_for(decl);

        assert_eq!(facts.loans.len(), 1);
        let invalidated = facts.loan_invalidated_at.get(&facts.loans[0].id);
        assert_eq!(invalidated.map(|v| v.len()), Some(1), "reading x should invalidate the outstanding &mut x");
    }

    #[test]
    fn unrelated_statement_produces_no_facts() {
        let arena = AstArena::new();
        let x = ident(&arena, "x");
        let stmts = [
            let_stmt(&arena, "p", borrow(&arena, false, x)),
            let_stmt(&arena, "y", lit_int(&arena, 10)),
        ];
        let decl = arena.alloc(func(&arena, &stmts));
        let facts = facts_for(decl);

        assert_eq!(facts.loans.len(), 1);
        assert!(facts.loan_killed_at.is_empty());
        assert!(facts.loan_invalidated_at.is_empty());
    }

    #[test]
    fn two_borrows_in_one_call_are_two_loans() {
        let arena = AstArena::new();
        let a = ident(&arena, "a");
        let b = ident(&arena, "b");
        let callee = ident(&arena, "combine");
        let args: &[Arg] = arena.alloc_slice_copy(&[
            Arg { kind: ArgKind::Positional(borrow(&arena, false, a)), span: Z },
            Arg { kind: ArgKind::Positional(borrow(&arena, true, b)), span: Z },
        ]);
        let call = arena.alloc(Expr { kind: ExprKind::Call { callee, args }, span: Z });
        let stmts = [let_stmt(&arena, "z", call)];
        let decl = arena.alloc(func(&arena, &stmts));
        let facts = facts_for(decl);

        // Proves the expression walker correctly descends into Call args
        // — a real gap class this session already learned to check for
        // (can_start_expr, the where/.query() collision).
        assert_eq!(facts.loans.len(), 2, "one borrow per call argument");
    }

    #[test]
    fn assigning_through_a_read_of_the_old_value_is_a_conflict_not_a_kill() {
        // x = x + 1 — this DOES read x's old value on its way to
        // overwriting it, and if x is borrowed elsewhere, that read is a
        // real conflict, not just a clean reassignment.
        let arena = AstArena::new();
        let x1 = ident(&arena, "x");
        let x2 = ident(&arena, "x");
        let one = lit_int(&arena, 1);
        let sum = arena.alloc(Expr {
            kind: ExprKind::BinOp { op: crate::ast::common::BinOp::Add, lhs: x2, rhs: one },
            span: Z,
        });
        let stmts = [
            let_stmt(&arena, "p", borrow(&arena, true, x1)),
            reassign_stmt(&arena, "x", sum),
        ];
        let decl = arena.alloc(func(&arena, &stmts));
        let facts = facts_for(decl);

        assert_eq!(facts.loans.len(), 1);
        assert!(facts.loan_invalidated_at.get(&facts.loans[0].id).is_some(),
            "x = x + 1 reads the old x — a real conflict against &mut x");
        assert!(facts.loan_killed_at.get(&facts.loans[0].id).is_none(),
            "the conflict path is exclusive of the plain-kill path — see classify_access");
    }

    // ── Phase D amendment: bound_place / place_defined_at ──────────────

    #[test]
    fn bare_borrow_bound_to_a_let_records_bound_place() {
        let arena = AstArena::new();
        let x = ident(&arena, "x");
        let stmts = [let_stmt(&arena, "p", borrow(&arena, true, x))];
        let decl = arena.alloc(func(&arena, &stmts));
        let facts = facts_for(decl);

        assert_eq!(facts.loans.len(), 1);
        assert_eq!(facts.loans[0].bound_place, Place::Local("p"),
            "let p = &x should record p as the loan's traceable carrier");
    }

    #[test]
    fn borrow_nested_in_a_call_has_unknown_bound_place() {
        let arena = AstArena::new();
        let x = ident(&arena, "x");
        let callee = ident(&arena, "consume");
        let args: &[Arg] = arena.alloc_slice_copy(&[
            Arg { kind: ArgKind::Positional(borrow(&arena, false, x)), span: Z },
        ]);
        let call = arena.alloc(Expr { kind: ExprKind::Call { callee, args }, span: Z });
        let stmts = [let_stmt(&arena, "z", call)];
        let decl = arena.alloc(func(&arena, &stmts));
        let facts = facts_for(decl);

        assert_eq!(facts.loans.len(), 1);
        assert_eq!(facts.loans[0].bound_place, Place::Unknown,
            "a borrow consumed inline as a call argument has no traceable carrier");
    }

    #[test]
    fn bare_borrow_bound_via_plain_assignment_records_bound_place() {
        let arena = AstArena::new();
        let x = ident(&arena, "x");
        let p_decl = ident(&arena, "p"); // pre-declared elsewhere; only the assignment matters here
        let _ = p_decl;
        let assign = arena.alloc(Expr {
            kind: ExprKind::Assign {
                op: AssignOp::Assign,
                target: ident(&arena, "p"),
                value: borrow(&arena, true, x),
            },
            span: Z,
        });
        let stmts = [Stmt { kind: StmtKind::Expr(assign), span: Z }];
        let decl = arena.alloc(func(&arena, &stmts));
        let facts = facts_for(decl);

        assert_eq!(facts.loans.len(), 1);
        assert_eq!(facts.loans[0].bound_place, Place::Local("p"),
            "p = &x (plain assignment, not a let) should also record p as the carrier");
    }

    #[test]
    fn place_defined_at_records_let_and_assignment_targets() {
        let arena = AstArena::new();
        let stmts = [
            let_stmt(&arena, "n", lit_int(&arena, 1)),
            reassign_stmt(&arena, "n", lit_int(&arena, 2)),
        ];
        let decl = arena.alloc(func(&arena, &stmts));
        let facts = facts_for(decl);

        let defs = facts.place_defined_at.get(&Place::Local("n"));
        assert_eq!(defs.map(|v| v.len()), Some(2),
            "both the initial let and the later reassignment define n");
    }

    #[test]
    fn rebinding_a_carrier_is_recorded_independently_of_the_original_loan() {
        // `let p = &x; let p = &y;` — the second statement rebinds p to a
        // brand new loan. Phase D needs place_defined_at to see this as a
        // def of p (ending the FIRST loan's association with p), even
        // though it's simultaneously the SECOND loan's own issued_at.
        let arena = AstArena::new();
        let x = ident(&arena, "x");
        let y = ident(&arena, "y");
        let stmts = [
            let_stmt(&arena, "p", borrow(&arena, false, x)),
            let_stmt(&arena, "p", borrow(&arena, false, y)),
        ];
        let decl = arena.alloc(func(&arena, &stmts));
        let facts = facts_for(decl);

        assert_eq!(facts.loans.len(), 2);
        assert_eq!(facts.loans[0].bound_place, Place::Local("p"));
        assert_eq!(facts.loans[1].bound_place, Place::Local("p"));
        let defs = facts.place_defined_at.get(&Place::Local("p"));
        assert_eq!(defs.map(|v| v.len()), Some(2),
            "p is defined twice — once per let — even though both loans share the same bound_place");
    }
}
