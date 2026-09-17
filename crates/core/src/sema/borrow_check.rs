// crates/core/src/sema/borrow_check.rs
//! Phase D of the LOW-tier borrow checker: the liveness/loan fixed point.
//! Consumes `cfg.rs`'s graph and `facts.rs`'s `Loan`/`loan_killed_at`/
//! `loan_invalidated_at`/`place_defined_at` to decide which invalidation
//! *candidates* are real violations, and wires the whole pipeline
//! (`cfg::build` → `facts::collect` → this module's `check`) into
//! `sema::analyse` for every `@tier(low)` function.
//!
//! ## The core idea, stated plainly
//!
//! A candidate conflict from `facts::loan_invalidated_at` is only a real
//! error if the loan is still *live* at that point — this is the
//! "liveness-gated" part `MEMORY_MODEL.md` §9 already committed to (full
//! NLL-style, not lexical-scope). Liveness here means two things have to
//! both hold at the conflicting point:
//!
//! 1. **Reaching** — the loan hasn't been killed yet on the path that led
//!    here (forward propagation from `issued_at`, stopping at
//!    `loan_killed_at` points and at any point where the loan's own
//!    `bound_place` gets rebound to something else — see
//!    `compute_reaches_before`).
//! 2. **Carrier liveness** — the reference-typed local the loan is bound
//!    to (`Loan::bound_place`) will actually be read again later on some
//!    path (backward liveness, standard live-variable dataflow — see
//!    `compute_liveness`).
//!
//! This is the literal reason `let p = &mut n; let last = *p; n = 5;`
//! (where `p`'s last use is `*p`, strictly before `n = 5`) is accepted:
//! liveness gating means the loan is already dead by the time `n` gets
//! touched again, exactly the non-lexical-scope behavior real NLL is
//! named for. A naive "any candidate is an error" checker would wrongly
//! reject this — see the `ok_borrow_dead_after_last_use` fixture, which
//! exists specifically to prove this isn't happening.
//!
//! ## Scope, stated plainly (read this before extending the checker)
//!
//! **Only `Loan::bound_place`-carried loans are checked at all.** A loan
//! consumed inline (a call argument, a return expression — anything
//! `facts.rs` couldn't trace to a simple local) has no reference anyone
//! could still be holding by the time a later statement runs, so it's
//! only ever "live" at its own `issued_at` point — which
//! `loan_invalidated_at` already excludes from candidates. These loans
//! never produce a diagnostic here. Real, valid follow-up (same spirit as
//! `Place::Unknown` throughout this checker): under-approximation, not
//! unsoundness.
//!
//! **Only mutable loans are checked.** `facts::classify_access` doesn't
//! distinguish "plain read" from "new borrow" among conflicting accesses,
//! and two *shared* loans of the same place never actually conflict with
//! each other — flagging every invalidation candidate regardless of
//! mutability would produce real false positives on valid shared-borrow
//! code (rejecting good code is a correctness bug, not just an
//! imprecision, unlike the under-approximations elsewhere in this
//! checker). The real gap this leaves: a *shared* loan that's still live
//! when a *later* `&mut` of the same place appears elsewhere is not yet
//! caught. Documented, real, separate follow-up — not silently dropped.
//!
//! **Intra-statement conflicts are still out of scope.** Two loans issued
//! at the very same point (e.g. `f(&n, &mut n)`, or the four-way alias in
//! `ok_reference_dual_spelling.ubl`'s `mixed_signature` call) are excluded
//! from `loan_invalidated_at` by `facts::collect` itself
//! (`if point == loan.issued_at { continue; }`) — this module never even
//! sees them as candidates. A same-point multi-loan check is a distinct,
//! separate piece of work `facts.rs`'s own module doc already flagged as
//! not yet collected.
//!
//! **Free functions only, for now.** `check_program` walks `Item::Function`
//! entries. `@tier(low)` methods (`Item::Impl` / `Item::Extend`) aren't
//! walked yet — `cfg::build`/`facts::collect` only accept `&FunctionDecl`
//! today, and methods are a different AST shape. Real, separate follow-up.

use std::collections::{HashMap, HashSet, VecDeque};

use crate::ast::common::TierAnnotation;
use crate::ast::declarations::FunctionDecl;
use crate::ast::root::{Item, Program};
use crate::lexer::Span;
use crate::sema::cfg::{self, BasicBlock, BlockId, Cfg, Terminator};
use crate::sema::facts::{self, Facts, Loan, LoanId, Place, Point};

/// One real, liveness-confirmed borrow violation, ready to become a
/// `BorrowError`.
#[derive(Debug, Clone)]
pub struct Violation {
    pub loan_span: Span,
    pub conflict_span: Span,
    pub place: String,
    pub mutable: bool,
}

/// Runs Phase D for every `@tier(low)` free function in `program`,
/// returning every real violation found. Doesn't touch `errors` itself —
/// callers (see `sema::analyse`) decide how to turn a `Violation` into a
/// diagnostic; keeping this pure makes it trivial to unit-test without
/// standing up the whole error-management machinery.
pub fn check_program<'ast>(program: &Program<'ast>) -> Vec<Violation> {
    let mut violations = Vec::new();
    for item in program.items {
        if let Item::Function(f) = item {
            if f.tier == TierAnnotation::Low {
                violations.extend(check_function(f));
            }
        }
    }
    violations
}

/// Runs the full `cfg::build` → `facts::collect` → liveness/reaching
/// pipeline for one function.
pub fn check_function<'ast>(decl: &'ast FunctionDecl<'ast>) -> Vec<Violation> {
    let cfg = cfg::build(decl);
    let facts = facts::collect(&cfg);
    check(&cfg, &facts)
}

/// The actual fixed point + diagnostic combination, given an
/// already-built CFG and already-collected facts. Split out from
/// `check_function` so tests (and any future caller with its own CFG/
/// facts, e.g. an incremental checker) can drive it directly.
pub fn check<'ast>(cfg: &Cfg<'ast>, facts: &Facts<'ast>) -> Vec<Violation> {
    let mut violations = Vec::new();
    let mut liveness_cache: HashMap<Place<'ast>, HashMap<Point, bool>> = HashMap::new();

    for loan in &facts.loans {
        if !loan.mutable { continue; }
        if matches!(loan.bound_place, Place::Unknown) { continue; }

        let Some(candidates) = facts.loan_invalidated_at.get(&loan.id) else { continue };
        if candidates.is_empty() { continue; }

        let reaches = compute_reaches_before(cfg, loan, facts);
        if reaches.is_empty() { continue; }

        let live_after = liveness_cache
            .entry(loan.bound_place)
            .or_insert_with(|| compute_liveness(cfg, loan.bound_place).1);

        for &point in candidates {
            let is_live = reaches.contains(&point) && live_after.get(&point).copied().unwrap_or(false);
            if !is_live { continue; }

            let conflict_span = cfg.block(point.block).stmts[point.stmt_index].span;
            let place = match loan.place {
                Place::Local(name) => name.to_string(),
                Place::Unknown => "<unknown>".to_string(),
            };
            violations.push(Violation {
                loan_span: loan.span,
                conflict_span,
                place,
                mutable: loan.mutable,
            });
        }
    }

    violations
}

/// **Phase E1** (`docs/OUTLIVES_RULES.md`) — every loan's own natural
/// region: the CFG points where it is both forward-reachable from its
/// issue point (`compute_reaches_before`) and its bound place is live
/// (`compute_liveness`, the *before* side — see that function's own
/// doc comment for why not the *after* side `check` itself uses).
/// Same two functions `check` already calls (one of them under a new
/// name, same body), used the same way; the only difference is this
/// keeps the
/// intersection as an explicit `HashSet` per loan instead of testing
/// points one at a time against a specific candidate list and
/// discarding the rest.
///
/// Deliberately **not** restricted to `check`'s own subset (mutable,
/// bound, with at least one invalidation candidate) — `check` narrows
/// to that subset because only a mutable loan can conflict with
/// anything, so there's no reason to compute reaching/liveness for a
/// shared loan that can never produce a `Violation` here. Phase E2
/// needs a region for *any* loan that might flow across a function or
/// struct boundary, and the large majority of `&L T` parameters/fields
/// in real code are shared, not `&mut` — restricting this function the
/// same way `check` does would make it useless for its actual purpose.
/// So this has its own loop over every loan in `facts.loans`, not a
/// refactor of `check`'s loop: same building blocks, a different
/// (broader) subset of loans, and no candidate list to test against
/// since a signature boundary isn't a specific invalidating statement,
/// it's "does this remain valid for as long as the declared lifetime
/// requires" — the whole region matters, not particular points in it.
///
/// A loan with `bound_place == Place::Unknown` (consumed inline — a
/// call argument, a return expression) is skipped, same as `check`:
/// nothing downstream can be holding a reference to it by name, so it
/// has no meaningful region beyond its own issue point. Real,
/// documented under-approximation, not unsoundness — same stance as
/// everywhere else in this checker (see this module's own doc comment).
/// A loan whose computed region comes out empty (dead before its
/// carrier is ever live — e.g. the carrier is immediately overwritten
/// without being read) is left out of the returned map entirely rather
/// than inserted as an empty set, so `.get(loan_id).is_some()` alone
/// tells a caller whether the loan has any region at all.
pub fn compute_loan_regions<'ast>(
    cfg: &Cfg<'ast>,
    facts: &Facts<'ast>,
) -> HashMap<LoanId, HashSet<Point>> {
    let mut regions: HashMap<LoanId, HashSet<Point>> = HashMap::new();
    let mut liveness_cache: HashMap<Place<'ast>, HashMap<Point, bool>> = HashMap::new();

    for loan in &facts.loans {
        if matches!(loan.bound_place, Place::Unknown) { continue; }

        let reaches = compute_reaches_before(cfg, loan, facts);
        if reaches.is_empty() { continue; }

        let live_before = liveness_cache
            .entry(loan.bound_place)
            .or_insert_with(|| compute_liveness(cfg, loan.bound_place).0);

        let region: HashSet<Point> = reaches
            .into_iter()
            .filter(|p| live_before.get(p).copied().unwrap_or(false))
            .collect();

        if !region.is_empty() {
            regions.insert(loan.id, region);
        }
    }

    regions
}

// ── Point-level CFG graph helpers ───────────────────────────────────────
//
// Both fixed points below need "what point(s) immediately follow this
// one" — the next statement in the same block, or the first *real*
// (non-empty) statement of each successor block. `cfg.rs` creates
// genuinely empty blocks on purpose (a `while`/`for` loop's header block
// carries no statements of its own — see cfg.rs's own comment on why),
// so "first point of a block" has to skip transparently through chains
// of those. `seen` guards the pathological case of an all-empty loop
// (`loop { }`) whose header targets itself — see `check_program`'s
// module doc note: that's a genuinely correct "no point ever follows
// this" answer, not a bug being worked around.

fn block_successors(b: &BasicBlock<'_>) -> Vec<BlockId> {
    match &b.terminator {
        Terminator::Goto(t) => vec![*t],
        Terminator::Branch(ts) => ts.clone(),
        Terminator::Return | Terminator::Unreachable => vec![],
    }
}

fn first_points_of(cfg: &Cfg<'_>, block: BlockId, seen: &mut HashSet<BlockId>) -> Vec<Point> {
    if !seen.insert(block) { return Vec::new(); }
    let b = cfg.block(block);
    if !b.stmts.is_empty() {
        return vec![Point { block, stmt_index: 0 }];
    }
    let mut out = Vec::new();
    for succ in block_successors(b) {
        out.extend(first_points_of(cfg, succ, seen));
    }
    out
}

/// Point-level successor(s) of `p` -- the next statement in the same
/// block, or the first point(s) of each successor block if `p` is a
/// block's last statement. `pub(crate)`: `sema::move_check` reuses this
/// unchanged for its own forward reachability propagation — same CFG,
/// same point granularity, no reason to duplicate the block/empty-block
/// skipping logic a second time.
pub(crate) fn succ_points(cfg: &Cfg<'_>, p: Point) -> Vec<Point> {
    let b = cfg.block(p.block);
    if p.stmt_index + 1 < b.stmts.len() {
        return vec![Point { block: p.block, stmt_index: p.stmt_index + 1 }];
    }
    let mut out = Vec::new();
    for succ in block_successors(b) {
        let mut seen = HashSet::new();
        out.extend(first_points_of(cfg, succ, &mut seen));
    }
    out
}

fn all_points(cfg: &Cfg<'_>) -> Vec<Point> {
    let mut pts = Vec::new();
    for block in &cfg.blocks {
        for i in 0..block.stmts.len() {
            pts.push(Point { block: block.id, stmt_index: i });
        }
    }
    pts
}

// ── Reaching loans (forward, per loan) ──────────────────────────────────

/// "May still be unkilled" forward propagation for one loan: a point is
/// in the result if there's *some* path from `loan.issued_at` to it with
/// no kill in between. Union merge (may-analysis), matching
/// `loan_invalidated_at`'s own over-approximation philosophy — see the
/// module doc.
///
/// A kill is either `facts.loan_killed_at[loan.id]` (the *borrowed-from*
/// place got reassigned) or a point in `facts.place_defined_at[bound_place]`
/// other than `issued_at` itself (the loan's own *carrier* got rebound to
/// something else, including a fresh loan — `loan_killed_at` alone can't
/// express this, since it only ever tracks the borrowed-from side).
fn compute_reaches_before<'ast>(
    cfg: &Cfg<'ast>, loan: &Loan<'ast>, facts: &Facts<'ast>,
) -> HashSet<Point> {
    let is_kill: HashSet<Point> = {
        let mut s: HashSet<Point> = facts.loan_killed_at
            .get(&loan.id).into_iter().flatten().copied().collect();
        if let Some(pts) = facts.place_defined_at.get(&loan.bound_place) {
            s.extend(pts.iter().copied().filter(|p| *p != loan.issued_at));
        }
        s
    };

    let mut reaches_before: HashSet<Point> = HashSet::new();
    let mut worklist: VecDeque<Point> = VecDeque::new();

    for s in succ_points(cfg, loan.issued_at) {
        if reaches_before.insert(s) { worklist.push_back(s); }
    }

    while let Some(p) = worklist.pop_front() {
        if is_kill.contains(&p) { continue; } // the loan dies going through p — don't propagate past it
        for s in succ_points(cfg, p) {
            if reaches_before.insert(s) { worklist.push_back(s); }
        }
    }

    reaches_before
}

// ── Liveness (backward, per tracked place) ──────────────────────────────

/// Standard backward live-variable dataflow for one place, computed at
/// CFG-point granularity via `succ_points`. Returns `(live_before,
/// live_after)`. `live_after[p]` is true iff `place` may be read again
/// at or after whatever immediately follows `p` — i.e. strictly after
/// `p`'s own statement runs, which is what a *conflict* check wants
/// (`check`, below): "will the carrier still be read after this
/// invalidating point". `live_before[p]` is true iff `place` is read at
/// `p` itself or is live-after `p` — which is what a loan's own region
/// wants (`compute_loan_regions`): "is this point still within the
/// window where the carrier is genuinely going to be used, counting its
/// own read". The two are genuinely different questions, not the same
/// value under two names — conflating them was an actual bug caught
/// while building `compute_loan_regions` (see that function's own
/// commit history / `docs/OUTLIVES_RULES.md`): intersecting `reaches`
/// with `live_after` instead of `live_before` silently produced an
/// empty region for every loan whose own last use was the final read in
/// its live range, since `live_after` is false at exactly that point by
/// definition.
///
/// Plain fixed-point iteration over every point until nothing changes —
/// not a smarter worklist — deliberately: LOW-tier function bodies are
/// small, and this is far easier to read and trust than an optimized
/// version would be. Revisit only if profiling ever says otherwise.
fn compute_liveness<'ast>(cfg: &Cfg<'ast>, place: Place<'ast>) -> (HashMap<Point, bool>, HashMap<Point, bool>) {
    let points = all_points(cfg);
    let mut live_before: HashMap<Point, bool> = points.iter().map(|p| (*p, false)).collect();
    let mut live_after: HashMap<Point, bool> = points.iter().map(|p| (*p, false)).collect();

    let mut changed = true;
    while changed {
        changed = false;
        for &p in points.iter().rev() {
            let stmt = cfg.block(p.block).stmts[p.stmt_index];
            let uses = facts::stmt_reads_place(stmt, place);
            let defines = facts::stmt_defines_place(stmt) == Some(place);

            let new_after = succ_points(cfg, p).iter().any(|q| live_before[q]);
            let new_before = uses || (new_after && !defines);

            if new_after != live_after[&p] { live_after.insert(p, new_after); changed = true; }
            if new_before != live_before[&p] { live_before.insert(p, new_before); changed = true; }
        }
    }

    (live_before, live_after)
}

// ── Tests ────────────────────────────────────────────────────────────
//
// Same self-contained, hand-built-AST approach cfg.rs/facts.rs already
// use — real function bodies via the arena, run through the real
// cfg::build → facts::collect → check pipeline, asserted on real output.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::arena::AstArena;
    use crate::ast::common::{AssignOp, BinOp, Span, Visibility};
    use crate::ast::expressions::{Expr, ExprKind, IfExpr};
    use crate::ast::literals::Literal;
    use crate::ast::statements::{BindingTarget, Block, Stmt, StmtKind};

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

    fn deref<'a>(arena: &'a AstArena, inner: &'a Expr<'a>) -> &'a Expr<'a> {
        arena.alloc(Expr { kind: ExprKind::Deref(inner), span: Z })
    }

    fn let_stmt<'a>(arena: &'a AstArena, name: &'a str, value: &'a Expr<'a>) -> Stmt<'a> {
        Stmt {
            kind: StmtKind::Let {
                mutable: false, binding: BindingTarget::Ident(arena.alloc_str(name)),
                ty: None, value,
            },
            span: Z,
        }
    }

    fn reassign_stmt<'a>(arena: &'a AstArena, name: &'a str, value: &'a Expr<'a>) -> Stmt<'a> {
        let target = ident(arena, name);
        let assign = arena.alloc(Expr {
            kind: ExprKind::Assign { op: AssignOp::Assign, target, value }, span: Z,
        });
        Stmt { kind: StmtKind::Expr(assign), span: Z }
    }

    fn return_stmt<'a>(value: &'a Expr<'a>) -> Stmt<'a> {
        Stmt { kind: StmtKind::Return(Some(value)), span: Z }
    }

    fn func<'a>(arena: &'a AstArena, stmts: &[Stmt<'a>]) -> FunctionDecl<'a> {
        FunctionDecl {
            tier: TierAnnotation::Low, attributes: &[], visibility: Visibility::default(),
            is_async: false, name: arena.alloc_str("f"), lifetime_params: &[], generic_params: &[],
            params: &[], return_type: None,
            body: Block { stmts: arena.alloc_slice_copy(stmts), span: Z },
            span: Z,
        }
    }

    fn violations_for<'a>(decl: &'a FunctionDecl<'a>) -> Vec<Violation> {
        check_function(decl)
    }

    #[test]
    fn conflict_while_carrier_still_live_is_a_real_violation() {
        // let p = &mut n; let y = n; return *p + y;
        // p is used AFTER the conflicting `let y = n` -> real error.
        let arena = AstArena::new();
        let n1 = ident(&arena, "n");
        let n2 = ident(&arena, "n");
        let p = ident(&arena, "p");
        let y = ident(&arena, "y");
        let ret = arena.alloc(Expr {
            kind: ExprKind::BinOp { op: BinOp::Add, lhs: deref(&arena, p), rhs: y }, span: Z,
        });
        let stmts = [
            let_stmt(&arena, "p", borrow(&arena, true, n1)),
            let_stmt(&arena, "y", n2),
            return_stmt(ret),
        ];
        let decl = arena.alloc(func(&arena, &stmts));
        let violations = violations_for(decl);

        assert_eq!(violations.len(), 1, "reading n while &mut n's carrier p is still live is a real conflict");
        assert!(violations[0].mutable);
        assert_eq!(violations[0].place, "n");
    }

    #[test]
    fn conflict_after_carrier_is_dead_is_not_a_violation() {
        // let p = &mut n; let doubled = *p; let y = n; return doubled + y;
        // p's LAST use is `*p`, strictly before `let y = n` -> the loan
        // is dead by then. A naive "any candidate = error" checker would
        // wrongly reject this; this is the test that proves it doesn't.
        let arena = AstArena::new();
        let n1 = ident(&arena, "n");
        let n2 = ident(&arena, "n");
        let p = ident(&arena, "p");
        let doubled = ident(&arena, "doubled");
        let y = ident(&arena, "y");
        let ret = arena.alloc(Expr {
            kind: ExprKind::BinOp { op: BinOp::Add, lhs: doubled, rhs: y }, span: Z,
        });
        let stmts = [
            let_stmt(&arena, "p", borrow(&arena, true, n1)),
            let_stmt(&arena, "doubled", deref(&arena, p)),
            let_stmt(&arena, "y", n2),
            return_stmt(ret),
        ];
        let decl = arena.alloc(func(&arena, &stmts));
        let violations = violations_for(decl);

        assert!(violations.is_empty(),
            "p is dead after its last use (*p) — the later read of n must NOT be flagged");
    }

    #[test]
    fn plain_reassignment_is_a_kill_not_a_violation() {
        // let p = &mut n; n = 5; return 0;
        // n = 5 is a Reassign (facts.rs), never even reaches
        // loan_invalidated_at, so this must never be flagged regardless
        // of liveness.
        let arena = AstArena::new();
        let n1 = ident(&arena, "n");
        let stmts = [
            let_stmt(&arena, "p", borrow(&arena, true, n1)),
            reassign_stmt(&arena, "n", lit_int(&arena, 5)),
            return_stmt(lit_int(&arena, 0)),
        ];
        let decl = arena.alloc(func(&arena, &stmts));
        assert!(violations_for(decl).is_empty());
    }

    #[test]
    fn shared_loan_conflict_is_not_flagged_in_v1() {
        // let p = &n; let y = n; return *p + y; — two SHARED accesses of
        // n coexisting is fine; see the module doc's "only mutable loans"
        // scope note.
        let arena = AstArena::new();
        let n1 = ident(&arena, "n");
        let n2 = ident(&arena, "n");
        let p = ident(&arena, "p");
        let y = ident(&arena, "y");
        let ret = arena.alloc(Expr {
            kind: ExprKind::BinOp { op: BinOp::Add, lhs: deref(&arena, p), rhs: y }, span: Z,
        });
        let stmts = [
            let_stmt(&arena, "p", borrow(&arena, false, n1)),
            let_stmt(&arena, "y", n2),
            return_stmt(ret),
        ];
        let decl = arena.alloc(func(&arena, &stmts));
        assert!(violations_for(decl).is_empty(), "two shared accesses of the same place never conflict");
    }

    #[test]
    fn borrow_consumed_inline_is_never_flagged() {
        // consume(&n); let y = n; return y; — the borrow has no
        // traceable carrier (Place::Unknown bound_place), so it's live
        // only at its own issue point, which is already excluded from
        // invalidation candidates.
        let arena = AstArena::new();
        let n1 = ident(&arena, "n");
        let n2 = ident(&arena, "n");
        let callee = ident(&arena, "consume");
        let args: &[crate::ast::expressions::Arg] = arena.alloc_slice_copy(&[
            crate::ast::expressions::Arg {
                kind: crate::ast::expressions::ArgKind::Positional(borrow(&arena, true, n1)), span: Z,
            },
        ]);
        let call = arena.alloc(Expr { kind: ExprKind::Call { callee, args }, span: Z });
        let stmts = [
            Stmt { kind: StmtKind::Expr(call), span: Z },
            let_stmt(&arena, "y", n2),
            return_stmt(ident(&arena, "y")),
        ];
        let decl = arena.alloc(func(&arena, &stmts));
        assert!(violations_for(decl).is_empty());
    }

    #[test]
    fn conflict_across_an_if_else_join_is_still_caught() {
        // let p = &mut n;
        // if: cond { let a = n; } else { let a = 0; }
        // return *p;
        // The conflicting read of n lives in the `if` branch only; p is
        // still used after the join at `return *p`. Proves the two-level
        // point graph correctly crosses block boundaries (branch ->
        // converge), not just straight-line code within one block.
        let arena = AstArena::new();
        let n1 = ident(&arena, "n");
        let n2 = ident(&arena, "n");
        let p = ident(&arena, "p");
        let cond = ident(&arena, "cond");

        let then_body = Block {
            stmts: arena.alloc_slice_copy(&[let_stmt(&arena, "a", n2)]), span: Z,
        };
        let else_body = Block {
            stmts: arena.alloc_slice_copy(&[let_stmt(&arena, "a", lit_int(&arena, 0))]), span: Z,
        };
        let if_expr = arena.alloc(IfExpr {
            condition: cond,
            then_body: crate::ast::expressions::IfBranchBody::Block(then_body),
            elif_branches: &[],
            else_body: Some(crate::ast::expressions::IfBranchBody::Block(else_body)),
            span: Z,
        });

        let stmts = [
            let_stmt(&arena, "p", borrow(&arena, true, n1)),
            Stmt { kind: StmtKind::If(if_expr), span: Z },
            return_stmt(deref(&arena, p)),
        ];
        let decl = arena.alloc(func(&arena, &stmts));
        let violations = violations_for(decl);

        assert_eq!(violations.len(), 1, "the conflicting read of n in the `if` branch, with p still live after the join, is a real violation");
    }

    // ── compute_loan_regions (Phase E1, docs/OUTLIVES_RULES.md) ──────────

    fn regions_for<'a>(decl: &'a FunctionDecl<'a>) -> HashMap<LoanId, HashSet<Point>> {
        let cfg = cfg::build(decl);
        let facts = facts::collect(&cfg);
        compute_loan_regions(&cfg, &facts)
    }

    #[test]
    fn used_loan_gets_a_nonempty_region() {
        // let p = &mut n; let y = *p; return y;
        let arena = AstArena::new();
        let n1 = ident(&arena, "n");
        let p = ident(&arena, "p");
        let stmts = [
            let_stmt(&arena, "p", borrow(&arena, true, n1)),
            let_stmt(&arena, "y", deref(&arena, p)),
            return_stmt(ident(&arena, "y")),
        ];
        let decl = arena.alloc(func(&arena, &stmts));
        let regions = regions_for(decl);

        assert_eq!(regions.len(), 1, "the one loan in this function is read later, so it must have a region");
        let region = regions.values().next().unwrap();
        assert!(!region.is_empty());
    }

    #[test]
    fn shared_loan_gets_a_region_even_though_check_would_skip_it() {
        // let p = &n; let y = *p; return y; — a SHARED borrow. `check()`
        // finds zero violations here (nothing to conflict with a shared
        // loan), but `compute_loan_regions` must still produce a region
        // for it: Phase E2 needs regions for shared loans crossing a
        // signature boundary just as much as mutable ones — the large
        // majority of `&L T` parameters in real code are shared, not
        // `&mut`. This is the test that would fail if this function were
        // ever "simplified" into reusing `check`'s own mutable-only loop.
        let arena = AstArena::new();
        let n1 = ident(&arena, "n");
        let p = ident(&arena, "p");
        let stmts = [
            let_stmt(&arena, "p", borrow(&arena, false, n1)),
            let_stmt(&arena, "y", deref(&arena, p)),
            return_stmt(ident(&arena, "y")),
        ];
        let decl = arena.alloc(func(&arena, &stmts));

        assert!(violations_for(decl).is_empty(), "sanity check: check() itself finds nothing here");

        let regions = regions_for(decl);
        assert_eq!(regions.len(), 1, "the shared loan still needs a region even though it can never conflict");
        assert!(!regions.values().next().unwrap().is_empty());
    }

    #[test]
    fn loan_never_read_again_has_no_region() {
        // let p = &mut n; n = 5; return 0; — p is bound but never read
        // again anywhere; its region is empty, so it must not appear in
        // the returned map at all (see this function's own doc comment
        // on why empty means absent, not present-and-empty).
        let arena = AstArena::new();
        let n1 = ident(&arena, "n");
        let stmts = [
            let_stmt(&arena, "p", borrow(&arena, true, n1)),
            reassign_stmt(&arena, "n", lit_int(&arena, 5)),
            return_stmt(lit_int(&arena, 0)),
        ];
        let decl = arena.alloc(func(&arena, &stmts));
        let regions = regions_for(decl);

        assert!(regions.is_empty(), "a loan that's never read again has no region and must be absent from the map");
    }

    #[test]
    fn inline_consumed_loan_has_no_region() {
        // consume(&n); let y = n; return y; — Place::Unknown bound_place,
        // same as `borrow_consumed_inline_is_never_flagged` above.
        let arena = AstArena::new();
        let n1 = ident(&arena, "n");
        let n2 = ident(&arena, "n");
        let callee = ident(&arena, "consume");
        let args: &[crate::ast::expressions::Arg] = arena.alloc_slice_copy(&[
            crate::ast::expressions::Arg {
                kind: crate::ast::expressions::ArgKind::Positional(borrow(&arena, true, n1)), span: Z,
            },
        ]);
        let call = arena.alloc(Expr { kind: ExprKind::Call { callee, args }, span: Z });
        let stmts = [
            Stmt { kind: StmtKind::Expr(call), span: Z },
            let_stmt(&arena, "y", n2),
            return_stmt(ident(&arena, "y")),
        ];
        let decl = arena.alloc(func(&arena, &stmts));
        let regions = regions_for(decl);

        assert!(regions.is_empty(), "a loan with no traceable carrier has no region either");
    }
}
