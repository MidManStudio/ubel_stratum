// crates/core/src/sema/move_check.rs
//! The move/ownership fixed point — consumes `cfg.rs`'s graph,
//! `facts.rs`'s `place_defined_at` (redefinition points), and
//! `move_facts.rs`'s move candidates to decide which candidates are
//! real "use after move" violations. Same relationship to
//! `move_facts.rs` that `borrow_check.rs` (Phase D) has to `facts.rs`
//! (Phase C) for the loan half — this is the phase that actually turns
//! a candidate into something worth rejecting a program over.
//!
//! ## The core idea
//!
//! For a tracked place with move candidates `M1, M2, ..., Mn`
//! (`move_facts::MoveFacts::moved_at`), a candidate `Mj` is a real
//! violation if it's forward-reachable — same point-level CFG walk
//! `borrow_check::compute_reaches_before` already uses for loans,
//! stopping propagation at any redefinition of the place
//! (`facts::Facts::place_defined_at`) — from *any* candidate `Mi`,
//! **including `Mi == Mj` itself**. That last part is what makes this
//! one rule correctly cover two distinct-looking cases at once:
//!
//! - **Straight-line / branching:** `let b = a; let c = a;` — `c`'s use
//!   point is reachable from `b`'s, with no redefinition of `a` in
//!   between, so `c` is flagged. `b`'s own use point is *not*
//!   reachable from anywhere (nothing comes before it), so `b` isn't.
//! - **Loop-carried:** a single move candidate sitting inside a loop
//!   body, with no reinitialization anywhere in the loop, is reachable
//!   *from itself* via the loop's back-edge — exactly "moved again on
//!   the next iteration, without ever being given a fresh value in
//!   between." Flagging `Mi == Mj` self-reachability is what catches
//!   this without a separate loop-specific rule.
//!
//! May-analysis (union merge, not intersection): matches
//! `compute_reaches_before`'s own philosophy for loans, and matches
//! real move-checker behavior more broadly — a value that *might*
//! already be moved on some incoming path is rejected unconditionally,
//! not only when it's moved on literally every path.
//!
//! Each violated place is reported at most once (the *first* candidate
//! found to reach it wins the "moved here" secondary span) even if
//! multiple earlier candidates would independently reach it — one
//! diagnostic per bad use, not one per path that makes it bad.
//!
//! Not wired into anything until this exact module lands — same
//! staged-until-consumed relationship every phase in this checker
//! family has had to the one before it.

use std::collections::{HashMap, HashSet, VecDeque};

use crate::ast::common::TierAnnotation;
use crate::ast::declarations::FunctionDecl;
use crate::ast::root::{Item, Program};
use crate::lexer::Span;
use crate::sema::borrow_check::succ_points;
use crate::sema::cfg::{self, Cfg};
use crate::sema::facts::{self, Facts, Place, Point};
use crate::sema::move_facts::{self, MoveFacts};

/// Pure representation of one real violation — same split `borrow_check
/// ::Violation` uses (`errors::MoveError` conversion happens one layer
/// up, in `sema::analyse`, not here).
#[derive(Debug, Clone)]
pub struct Violation {
    pub place:      String,
    pub moved_span: Span,
    pub used_span:  Span,
}

/// Runs move checking on every `@tier(low)` free function in `program`.
/// Same `Item::Function` + `f.tier == TierAnnotation::Low` filter, same
/// "methods not yet walked" scope limit `borrow_check::check_program`
/// already has — `cfg::build` only accepts `&FunctionDecl` today.
pub fn check_program<'ast>(program: &Program<'ast>) -> Vec<Violation> {
    let user_methods = move_facts::user_method_names(program);
    let mut violations = Vec::new();
    for item in program.items {
        if let Item::Function(f) = item {
            if f.tier == TierAnnotation::Low {
                violations.extend(check_function_with(f, &user_methods));
            }
        }
    }
    violations
}

pub fn check_function<'ast>(decl: &'ast FunctionDecl<'ast>) -> Vec<Violation> {
    check_function_with(decl, &HashSet::new())
}

/// `check_function` with the program's user-declared instance-method
/// names, so a call to one is not treated as a move of its receiver.
pub fn check_function_with<'ast>(
    decl:         &'ast FunctionDecl<'ast>,
    user_methods: &HashSet<&'ast str>,
) -> Vec<Violation> {
    let graph = cfg::build(decl);
    // Needs the LOAN facts too -- specifically place_defined_at, which
    // is general-purpose (not loan-specific, see facts.rs's own doc
    // comment on it) and is exactly "where does this place get a fresh
    // value" -- precisely what should stop move-reachability from
    // propagating past a reinitialization.
    let loan_facts = facts::collect(&graph);
    let mfacts = move_facts::collect_with(&graph, user_methods);
    check(&graph, &loan_facts, &mfacts)
}

pub fn check<'ast>(cfg: &Cfg<'ast>, facts: &Facts<'ast>, mfacts: &MoveFacts<'ast>) -> Vec<Violation> {
    let mut violations = Vec::new();

    for place in &mfacts.tracked_locals {
        let Some(candidates) = mfacts.moved_at.get(place) else { continue; };
        if candidates.is_empty() { continue; }

        let kill_points: HashSet<Point> = facts.place_defined_at
            .get(place).into_iter().flatten().copied().collect();

        let mut already_reported: HashSet<Point> = HashSet::new();

        for source in candidates {
            let reaches = compute_moved_reaches(cfg, source.point, &kill_points);
            for candidate in candidates {
                if already_reported.contains(&candidate.point) { continue; }
                if reaches.contains(&candidate.point) {
                    already_reported.insert(candidate.point);
                    violations.push(Violation {
                        place: match place { Place::Local(n) => n.to_string(), Place::Unknown => "<unknown>".to_string() },
                        moved_span: source.span,
                        used_span:  candidate.span,
                    });
                }
            }
        }
    }

    violations
}

/// "May already be moved" forward propagation from one candidate's
/// point: a point is in the result if there's *some* path from `from`
/// to it with no redefinition of the tracked place in between. Same
/// worklist/union-merge shape `borrow_check::compute_reaches_before`
/// uses for loans, `kill_points` playing the exact role `is_kill` does
/// there (propagation reaches a kill point — inserted into the result —
/// but never propagates past it).
fn compute_moved_reaches(cfg: &Cfg<'_>, from: Point, kill_points: &HashSet<Point>) -> HashSet<Point> {
    let mut reaches: HashSet<Point> = HashSet::new();
    let mut worklist: VecDeque<Point> = VecDeque::new();

    for s in succ_points(cfg, from) {
        if reaches.insert(s) { worklist.push_back(s); }
    }
    while let Some(p) = worklist.pop_front() {
        if kill_points.contains(&p) { continue; }
        for s in succ_points(cfg, p) {
            if reaches.insert(s) { worklist.push_back(s); }
        }
    }

    reaches
}

// ── Tests ────────────────────────────────────────────────────────────
//
// Same self-contained, hand-built-AST approach every phase in this
// checker family uses — real function bodies via the arena, run through
// the real cfg::build -> facts::collect + move_facts::collect -> check
// pipeline, asserted on real output.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::arena::AstArena;
    use crate::ast::common::{AssignOp, Span as AstSpan, Visibility};
    use crate::ast::expressions::{Arg, ArgKind, Expr, ExprKind};
    use crate::ast::literals::Literal;
    use crate::ast::statements::{BindingTarget, Block, Stmt, StmtKind};
    use crate::ast::types::{Type, TypeKind};

    const Z: AstSpan = AstSpan { start: 0, end: 0, line: 0, column: 0 };

    fn ident<'a>(arena: &'a AstArena, name: &'a str) -> &'a Expr<'a> {
        arena.alloc(Expr { kind: ExprKind::Ident(arena.alloc_str(name)), span: Z })
    }

    fn lit_int<'a>(arena: &'a AstArena, n: i64) -> &'a Expr<'a> {
        arena.alloc(Expr { kind: ExprKind::Lit(Literal::Int(n)), span: Z })
    }

    fn borrow_expr<'a>(arena: &'a AstArena, mutable: bool, place: &'a Expr<'a>) -> &'a Expr<'a> {
        arena.alloc(Expr { kind: ExprKind::Borrow { mutable, place }, span: Z })
    }

    fn unique_new<'a>(arena: &'a AstArena, arg: &'a Expr<'a>) -> &'a Expr<'a> {
        let target = ident(arena, "Unique");
        let callee = arena.alloc(Expr { kind: ExprKind::Field { target, field: "new" }, span: Z });
        let args = arena.alloc_slice_copy(&[Arg { kind: ArgKind::Positional(arg), span: Z }]);
        arena.alloc(Expr { kind: ExprKind::Call { callee, args }, span: Z })
    }

    fn let_stmt<'a>(arena: &'a AstArena, name: &'a str, value: &'a Expr<'a>) -> Stmt<'a> {
        Stmt {
            kind: StmtKind::Let {
                mutable: false, binding: BindingTarget::Ident(arena.alloc_str(name)), ty: None, value,
            },
            span: Z,
        }
    }

    fn reassign_stmt<'a>(arena: &'a AstArena, name: &'a str, value: &'a Expr<'a>) -> Stmt<'a> {
        let target = ident(arena, name);
        let assign = arena.alloc(Expr { kind: ExprKind::Assign { op: AssignOp::Assign, target, value }, span: Z });
        Stmt { kind: StmtKind::Expr(assign), span: Z }
    }

    fn expr_stmt<'a>(e: &'a Expr<'a>) -> Stmt<'a> {
        Stmt { kind: StmtKind::Expr(e), span: Z }
    }

    fn while_stmt<'a>(arena: &'a AstArena, condition: &'a Expr<'a>, body: &[Stmt<'a>]) -> Stmt<'a> {
        let body = arena.alloc(Block { stmts: arena.alloc_slice_copy(body), span: Z });
        Stmt { kind: StmtKind::While { condition, body }, span: Z }
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
        let graph = cfg::build(decl);
        let loan_facts = facts::collect(&graph);
        let mfacts = move_facts::collect(&graph);
        check(&graph, &loan_facts, &mfacts)
    }

    #[test]
    fn second_bare_use_after_a_move_is_a_violation() {
        // let a = Unique.new(5); let b = a; let c = a;
        let arena = AstArena::new();
        let stmts = [
            let_stmt(&arena, "a", unique_new(&arena, lit_int(&arena, 5))),
            let_stmt(&arena, "b", ident(&arena, "a")),
            let_stmt(&arena, "c", ident(&arena, "a")),
        ];
        let decl = arena.alloc(func(&arena, &stmts));
        let violations = violations_for(decl);
        assert_eq!(violations.len(), 1, "exactly one violation: the second use of `a`");
        assert_eq!(violations[0].place, "a");
    }

    #[test]
    fn a_single_move_is_never_a_violation_on_its_own() {
        // let a = Unique.new(5); let b = a;  -- only one use, nothing to conflict with.
        let arena = AstArena::new();
        let stmts = [
            let_stmt(&arena, "a", unique_new(&arena, lit_int(&arena, 5))),
            let_stmt(&arena, "b", ident(&arena, "a")),
        ];
        let decl = arena.alloc(func(&arena, &stmts));
        let violations = violations_for(decl);
        assert!(violations.is_empty());
    }

    #[test]
    fn a_borrow_between_two_moves_does_not_interfere_with_detecting_the_second() {
        // let a = Unique.new(5); let b = a; let p = &a; let c = a;
        // move_facts.rs never records `&a` as a move candidate at all,
        // so there are only two real candidates here (`b = a`, `c = a`)
        // with the borrow sitting inertly between them. This checks
        // that the borrow doesn't accidentally act as a "kill" (it's
        // not a redefinition of `a`, just a read of its current value)
        // and so doesn't clear the violation between the two real moves.
        let arena = AstArena::new();
        let stmts = [
            let_stmt(&arena, "a", unique_new(&arena, lit_int(&arena, 5))),
            let_stmt(&arena, "b", ident(&arena, "a")),
            let_stmt(&arena, "p", borrow_expr(&arena, false, ident(&arena, "a"))),
            let_stmt(&arena, "c", ident(&arena, "a")),
        ];
        let decl = arena.alloc(func(&arena, &stmts));
        let violations = violations_for(decl);
        assert_eq!(violations.len(), 1, "the borrow must neither add a violation of its own nor mask the real one between b=a and c=a");
    }

    #[test]
    fn reinitializing_before_the_second_use_clears_the_violation() {
        // let a = Unique.new(5); let b = a; a = Unique.new(10); let c = a;
        // The reassignment gives `a` a fresh value before `c`'s use, so
        // `c` reads the NEW value, not the moved-from one -- no violation.
        let arena = AstArena::new();
        let stmts = [
            let_stmt(&arena, "a", unique_new(&arena, lit_int(&arena, 5))),
            let_stmt(&arena, "b", ident(&arena, "a")),
            reassign_stmt(&arena, "a", unique_new(&arena, lit_int(&arena, 10))),
            let_stmt(&arena, "c", ident(&arena, "a")),
        ];
        let decl = arena.alloc(func(&arena, &stmts));
        let violations = violations_for(decl);
        assert!(violations.is_empty(), "reinitializing `a` should clear the earlier move, not carry it forward");
    }

    #[test]
    fn moving_every_loop_iteration_without_reinit_is_a_violation() {
        // let a = Unique.new(5);
        // while cond { let b = a; }
        // One syntactic move candidate, but the loop's back-edge makes
        // it reachable from itself -- "moved again next iteration."
        let arena = AstArena::new();
        let stmts = [
            let_stmt(&arena, "a", unique_new(&arena, lit_int(&arena, 5))),
            while_stmt(&arena, ident(&arena, "cond"), &[
                let_stmt(&arena, "b", ident(&arena, "a")),
            ]),
        ];
        let decl = arena.alloc(func(&arena, &stmts));
        let violations = violations_for(decl);
        assert_eq!(violations.len(), 1, "the loop-carried move should flag itself via the back-edge");
        assert_eq!(violations[0].place, "a");
    }

    #[test]
    fn reinit_inside_the_loop_body_clears_the_loop_carried_violation() {
        // let a = Unique.new(5);
        // while cond { let b = a; a = Unique.new(1); }
        // Reinitializing INSIDE the loop, after the move, means the next
        // iteration's move reads the fresh value -- no violation.
        let arena = AstArena::new();
        let stmts = [
            let_stmt(&arena, "a", unique_new(&arena, lit_int(&arena, 5))),
            while_stmt(&arena, ident(&arena, "cond"), &[
                let_stmt(&arena, "b", ident(&arena, "a")),
                reassign_stmt(&arena, "a", unique_new(&arena, lit_int(&arena, 1))),
            ]),
        ];
        let decl = arena.alloc(func(&arena, &stmts));
        let violations = violations_for(decl);
        assert!(violations.is_empty(), "reinitializing inside the loop body should clear the loop-carried move");
    }

    #[test]
    fn call_argument_use_after_a_move_is_a_violation() {
        // let a = Unique.new(5); let b = a; consume(a);
        let arena = AstArena::new();
        let callee = ident(&arena, "consume");
        let args = arena.alloc_slice_copy(&[Arg { kind: ArgKind::Positional(ident(&arena, "a")), span: Z }]);
        let call = arena.alloc(Expr { kind: ExprKind::Call { callee, args }, span: Z });
        let stmts = [
            let_stmt(&arena, "a", unique_new(&arena, lit_int(&arena, 5))),
            let_stmt(&arena, "b", ident(&arena, "a")),
            expr_stmt(call),
        ];
        let decl = arena.alloc(func(&arena, &stmts));
        let violations = violations_for(decl);
        assert_eq!(violations.len(), 1);
    }

    #[test]
    fn untracked_local_never_produces_a_violation() {
        // A plain (non-Unique) let, used twice -- nothing here is
        // move-tracked at all, so nothing should ever be flagged.
        let arena = AstArena::new();
        let stmts = [
            let_stmt(&arena, "a", lit_int(&arena, 5)),
            let_stmt(&arena, "b", ident(&arena, "a")),
            let_stmt(&arena, "c", ident(&arena, "a")),
        ];
        let decl = arena.alloc(func(&arena, &stmts));
        let violations = violations_for(decl);
        assert!(violations.is_empty());
    }
}
