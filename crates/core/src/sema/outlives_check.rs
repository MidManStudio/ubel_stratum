// crates/core/src/sema/outlives_check.rs
//! Phase E2 + E4 (`docs/OUTLIVES_RULES.md`) — boundary constraint
//! generation and checking, function-call boundary shape only, single
//! declared lifetime per callee signature. The `edge struct`
//! construction boundary (`OUTLIVES_RULES.md` §3's second shape) and
//! multi-lifetime `outlives` propagation (Phase E3) are later slices
//! per §9's own landing order — not built here.
//!
//! ## What this actually checks, worked out concretely
//!
//! §3 of the design doc frames Phase E2 as "generate a constraint: this
//! call site requires L's bound region to be a superset of the loan's
//! actual region" — but that phrasing presupposes Phase E3's
//! constraint-propagation machinery, which per §9 lands *after* this
//! step. Resolved here, concretely, for the no-propagation-yet case
//! (this is a real design-bearing decision made while implementing,
//! not something the doc spelled out — flagged so it can be corrected
//! if it doesn't match intent):
//!
//! At a call `g(p)` where `g` has exactly one `[lifetime L]` and the
//! matched parameter type is `&L T`, the actual argument `p` falls into
//! one of four cases:
//!
//! 1. **A fresh inline borrow** (`g(&x)`) — `facts::expr_as_place`
//!    returns `Place::Unknown` for a bare `Borrow` node (it isn't an
//!    `Ident`). Always valid, freshly created at this exact point,
//!    nothing to check. Skipped, same as everywhere else in this
//!    checker family `Place::Unknown` means "no traceable carrier."
//! 2. **One of the *caller's own* parameters** (`fn caller [lifetime L]
//!    (p: &L Item) { g(p) }`) — valid for the caller's entire body by
//!    construction (that's what being a parameter means), and also the
//!    single most common real use of a lifetime-parameterized function,
//!    so treating it as a v1 rejection would make the feature nearly
//!    useless on its first real workload. Checked by name against the
//!    enclosing `FunctionDecl`'s own `params`, not by any loan lookup —
//!    a parameter binding was never a `Borrow` expression, so it never
//!    gets a `facts::Loan` entry at all.
//! 3. **A local bound to a loan within this function body** (`let p =
//!    &x; ...; g(p)`) — the real case this phase exists for. Look up
//!    every loan whose `bound_place == Place::Local(p)` and its region
//!    from `borrow_check::compute_loan_regions` (Phase E1); if none of
//!    their regions contains the call's own `Point`, the argument's
//!    carrier is already dead by the time it reaches this call —
//!    `LIFETIME-004`. (More than one loan can share a bound-place name
//!    across reassignment; checking "does *any* of them cover this
//!    point" rather than just the first/last is what makes that safe —
//!    an earlier loan's region correctly stops covering the rebind
//!    point, since `compute_reaches_before` already treats
//!    `place_defined_at` as a kill for the *older* loan.)
//! 4. **Anything else traceable to a `Place::Local` but matching
//!    neither a parameter nor a known loan** (e.g. `let p =
//!    other_call_returning_a_reference(); g(p)` — a call result typed
//!    `&T` is never itself registered as a `facts::Loan`, since
//!    `facts.rs`'s loans only ever come from literal `Borrow` nodes) —
//!    v1 can't prove it's fine, so it's conservatively rejected (§4's
//!    stated v1 stance) — `LIFETIME-006`.
//!
//! Positional arguments only for this slice — `ArgKind::Named` at a
//! lifetime-bearing parameter position isn't matched. A real, narrow,
//! documented under-approximation, same spirit as `Place::Unknown`
//! elsewhere in this checker family: fewer diagnostics than a fully
//! precise pass would give, never a false accusation.
//!
//! Only `@tier(low)` *caller* functions are walked — same restriction
//! `borrow_check`/`move_check` already have, for the same reason
//! (`cfg::build`/`facts::collect` are built for one `FunctionDecl` body
//! at a time, and only LOW-tier functions get that treatment at all).
//! Free functions only, not methods — inherits the same boundary
//! `borrow_check.rs`'s own module doc already states and this phase
//! explicitly defers fixing (`OUTLIVES_RULES.md` §4).

use std::collections::{HashMap, HashSet};

use crate::ast::common::TierAnnotation;
use crate::ast::declarations::{FunctionDecl, ParamKind};
use crate::ast::expressions::{ArgKind, Expr, ExprKind};
use crate::ast::root::{Item, Program};
use crate::ast::types::TypeKind;
use crate::lexer::Span;
use crate::sema::borrow_check::compute_loan_regions;
use crate::sema::cfg;
use crate::sema::facts::{self, Facts, LoanId, Place, Point};

/// One real outlives-boundary violation, ready to become a
/// `LifetimeError`. Mirrors `borrow_check::Violation` /
/// `move_check::Violation`'s "pure data, caller decides how to report
/// it" shape — keeps this trivially unit-testable without standing up
/// `ErrorManager`.
#[derive(Debug, Clone)]
pub enum Violation {
    /// LIFETIME-004 — case 3 above: a traced loan exists, but its
    /// region doesn't reach this call's point.
    CallBoundaryTooShort {
        lifetime: String,
        loan_span: Span,
        call_span: Span,
    },
    /// LIFETIME-006 — case 4 above: no traceable parameter or loan at
    /// all, conservative reject.
    NonLocalBoundaryArgument {
        lifetime: String,
        call_span: Span,
    },
}

/// Runs Phase E2 + E4 for every `@tier(low)` free function in
/// `program`. Doesn't touch `ErrorManager` itself, same reasoning as
/// `borrow_check::check_program` — see that function's own doc note.
pub fn check_program<'ast>(program: &Program<'ast>) -> Vec<Violation> {
    let mut fn_table: HashMap<&'ast str, &'ast FunctionDecl<'ast>> = HashMap::new();
    for item in program.items {
        if let Item::Function(f) = item {
            fn_table.insert(f.name, f);
        }
    }

    let mut violations = Vec::new();
    for item in program.items {
        if let Item::Function(f) = item {
            if f.tier == TierAnnotation::Low {
                violations.extend(check_function(f, &fn_table));
            }
        }
    }
    violations
}

fn check_function<'ast>(
    caller: &'ast FunctionDecl<'ast>,
    fn_table: &HashMap<&'ast str, &'ast FunctionDecl<'ast>>,
) -> Vec<Violation> {
    let cfg = cfg::build(caller);
    let facts = facts::collect(&cfg);
    let regions = compute_loan_regions(&cfg, &facts);

    let caller_param_names: HashSet<&'ast str> = caller.params.iter()
        .filter_map(|p| match p.kind {
            ParamKind::Named { name, .. } => Some(name),
            _ => None,
        })
        .collect();

    let mut violations = Vec::new();

    for block in &cfg.blocks {
        for (stmt_index, stmt) in block.stmts.iter().enumerate() {
            let point = Point { block: block.id, stmt_index };
            facts::for_each_top_expr(stmt, &mut |e| {
                find_calls(e, &mut |call_expr| {
                    check_call(
                        call_expr, point, fn_table, &caller_param_names,
                        &facts, &regions, &mut violations,
                    );
                });
            });
        }
    }

    violations
}

fn check_call<'ast>(
    call_expr: &'ast Expr<'ast>,
    point: Point,
    fn_table: &HashMap<&'ast str, &'ast FunctionDecl<'ast>>,
    caller_param_names: &HashSet<&'ast str>,
    facts: &Facts<'ast>,
    regions: &HashMap<LoanId, HashSet<Point>>,
    violations: &mut Vec<Violation>,
) {
    let ExprKind::Call { callee, args } = &call_expr.kind else { return };
    let ExprKind::Ident(callee_name) = callee.kind else { return };
    let Some(&g) = fn_table.get(callee_name) else { return };

    // Single declared lifetime per signature only — E3 (multi-lifetime
    // `outlives` propagation) is a later slice, not this one.
    if g.lifetime_params.len() != 1 { return; }
    let lifetime_name = g.lifetime_params[0].name;

    for (param, arg) in g.params.iter().zip(args.iter()) {
        let ParamKind::Named { ty: Some(ty), .. } = param.kind else { continue };
        let TypeKind::Reference { lifetime: Some(l), .. } = ty.kind else { continue };
        if l != lifetime_name { continue; }

        let arg_expr = match &arg.kind {
            ArgKind::Positional(e) => *e,
            // Named args at a lifetime-bearing position aren't matched
            // in this slice — see this module's own doc comment.
            ArgKind::Named { .. } => continue,
        };

        let place = facts::expr_as_place(arg_expr);
        let Place::Local(name) = place else { continue }; // fresh inline borrow — always valid

        if caller_param_names.contains(name) { continue; } // forwarding caller's own param — always valid

        let bound_loans: Vec<_> = facts.loans.iter()
            .filter(|loan| matches!(loan.bound_place, Place::Local(n) if n == name))
            .collect();

        if bound_loans.is_empty() {
            violations.push(Violation::NonLocalBoundaryArgument {
                lifetime: lifetime_name.to_string(),
                call_span: arg_expr.span,
            });
            continue;
        }

        let reaches = bound_loans.iter()
            .any(|loan| regions.get(&loan.id).is_some_and(|region| region.contains(&point)));

        if !reaches {
            let loan_span = bound_loans.last().expect("checked non-empty above").span;
            violations.push(Violation::CallBoundaryTooShort {
                lifetime: lifetime_name.to_string(),
                loan_span,
                call_span: arg_expr.span,
            });
        }
    }
}

// ── Call-expression finder ──────────────────────────────────────────
//
// Same opacity boundary as `facts.rs`'s own `ScopedExprWalker` (a
// lambda body, a `{ }` block expression, or an if/match used as a value
// is a separate scope this pass doesn't descend into) — `facts.rs`'s
// own walker is private to that module, so this is a second, small,
// deliberately-identical instance built directly on the shared public
// `ast::visitor`, not a reach into `facts.rs` internals.

struct CallFinder<'w, 'ast, F: FnMut(&'ast Expr<'ast>)> {
    visit: &'w mut F,
    _marker: std::marker::PhantomData<&'ast ()>,
}

impl<'w, 'ast, F: FnMut(&'ast Expr<'ast>)> crate::ast::visitor::AstVisitor<'ast>
    for CallFinder<'w, 'ast, F>
{
    fn visit_expr(&mut self, e: &'ast Expr<'ast>) {
        if matches!(e.kind, ExprKind::Call { .. }) {
            (self.visit)(e);
        }
        if matches!(e.kind, ExprKind::Lambda(_) | ExprKind::Block(_) | ExprKind::If(_) | ExprKind::Match(_)) {
            return;
        }
        crate::ast::visitor::walk_expr(self, e);
    }
}

fn find_calls<'ast>(expr: &'ast Expr<'ast>, visit: &mut impl FnMut(&'ast Expr<'ast>)) {
    use crate::ast::visitor::AstVisitor;
    CallFinder { visit, _marker: std::marker::PhantomData }.visit_expr(expr);
}

// ── Tests ────────────────────────────────────────────────────────────
//
// Same self-contained, hand-built-AST approach borrow_check.rs/facts.rs
// already use — real function bodies via the arena, run through the
// real cfg::build → facts::collect → check_program pipeline, asserted
// on real output.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::arena::AstArena;
    use crate::ast::common::{LifetimeParam, Span, Visibility};
    use crate::ast::declarations::{Param, ReturnType};
    use crate::ast::expressions::{Arg, ArgKind};
    use crate::ast::statements::{BindingTarget, Block, Stmt, StmtKind};
    use crate::ast::types::Type;

    const Z: Span = Span { start: 0, end: 0, line: 0, column: 0 };

    fn ident<'a>(arena: &'a AstArena, name: &'a str) -> &'a Expr<'a> {
        arena.alloc(Expr { kind: ExprKind::Ident(arena.alloc_str(name)), span: Z })
    }

    fn borrow<'a>(arena: &'a AstArena, mutable: bool, place: &'a Expr<'a>) -> &'a Expr<'a> {
        arena.alloc(Expr { kind: ExprKind::Borrow { mutable, place }, span: Z })
    }

    fn lit_int<'a>(arena: &'a AstArena, n: i64) -> &'a Expr<'a> {
        arena.alloc(Expr { kind: ExprKind::Lit(crate::ast::literals::Literal::Int(n)), span: Z })
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
            kind: ExprKind::Assign { op: crate::ast::common::AssignOp::Assign, target, value }, span: Z,
        });
        Stmt { kind: StmtKind::Expr(assign), span: Z }
    }

    fn call_stmt<'a>(arena: &'a AstArena, callee: &'a str, args: &[&'a Expr<'a>]) -> Stmt<'a> {
        let callee_expr = ident(arena, callee);
        let arg_nodes: Vec<Arg<'a>> = args.iter()
            .map(|e| Arg { kind: ArgKind::Positional(e), span: Z })
            .collect();
        let call = arena.alloc(Expr {
            kind: ExprKind::Call { callee: callee_expr, args: arena.alloc_slice_copy(&arg_nodes) },
            span: Z,
        });
        Stmt { kind: StmtKind::Expr(call), span: Z }
    }

    fn ref_type<'a>(arena: &'a AstArena, lifetime: &'a str) -> &'a Type<'a> {
        let inner = arena.alloc(Type { kind: TypeKind::Int, span: Z });
        arena.alloc(Type {
            kind: TypeKind::Reference { mutable: false, lifetime: Some(lifetime), inner },
            span: Z,
        })
    }

    fn lifetime_fn<'a>(arena: &'a AstArena, name: &'a str, lifetime: &'a str) -> FunctionDecl<'a> {
        let param = Param {
            kind: ParamKind::Named {
                mutable: false, name: arena.alloc_str("v"),
                ty: Some(ref_type(arena, lifetime)), default: None,
            },
            span: Z,
        };
        FunctionDecl {
            tier: TierAnnotation::Low, attributes: &[], visibility: Visibility::default(),
            is_async: false, name: arena.alloc_str(name),
            lifetime_params: arena.alloc_slice_copy(&[LifetimeParam { name: lifetime, constraint: None, span: Z }]),
            generic_params: &[],
            params: arena.alloc_slice_copy(&[param]),
            return_type: Some(ReturnType { ty: ref_type(arena, lifetime), is_fallible: false }),
            body: Block { stmts: &[], span: Z },
            span: Z,
        }
    }

    fn caller_fn<'a>(arena: &'a AstArena, params: &[Param<'a>], stmts: &[Stmt<'a>]) -> FunctionDecl<'a> {
        FunctionDecl {
            tier: TierAnnotation::Low, attributes: &[], visibility: Visibility::default(),
            is_async: false, name: arena.alloc_str("caller"), lifetime_params: &[], generic_params: &[],
            params: arena.alloc_slice_copy(params), return_type: None,
            body: Block { stmts: arena.alloc_slice_copy(stmts), span: Z },
            span: Z,
        }
    }

    fn program_with<'a>(arena: &'a AstArena, items: &[Item<'a>]) -> Program<'a> {
        Program { package: None, imports: &[], items: arena.alloc_slice_copy(items), span: Z }
    }

    #[test]
    fn loan_still_live_at_call_is_accepted() {
        // fn take [lifetime L] (v: &L int) &L int { ... }
        // fn caller() void { let p = &n; take(p); }
        let arena = AstArena::new();
        let n1 = ident(&arena, "n");
        let p = ident(&arena, "p");
        let stmts = [
            let_stmt(&arena, "p", borrow(&arena, false, n1)),
            call_stmt(&arena, "take", &[p]),
        ];
        let caller = caller_fn(&arena, &[], &stmts);
        let take = lifetime_fn(&arena, "take", "L");
        let program = program_with(&arena, &[Item::Function(caller), Item::Function(take)]);

        let violations = check_program(&program);
        assert!(violations.is_empty(), "p is passed to take() right after its own borrow, well within its live range");
    }

    #[test]
    fn loan_dead_before_call_is_rejected() {
        // fn caller() void { let p = &n; n = 5; take(p); } — n's
        // reassignment kills p's underlying loan before the call.
        let arena = AstArena::new();
        let n1 = ident(&arena, "n");
        let p = ident(&arena, "p");
        let stmts = [
            let_stmt(&arena, "p", borrow(&arena, false, n1)),
            reassign_stmt(&arena, "n", lit_int(&arena, 5)),
            call_stmt(&arena, "take", &[p]),
        ];
        let caller = caller_fn(&arena, &[], &stmts);
        let take = lifetime_fn(&arena, "take", "L");
        let program = program_with(&arena, &[Item::Function(caller), Item::Function(take)]);

        let violations = check_program(&program);
        assert_eq!(violations.len(), 1, "n's reassignment kills p's loan before it reaches the call");
        assert!(matches!(violations[0], Violation::CallBoundaryTooShort { .. }));
    }

    #[test]
    fn forwarding_callers_own_param_is_accepted() {
        // fn caller [lifetime L] (p: &L int) void { take(p); }
        let arena = AstArena::new();
        let p = ident(&arena, "p");
        let stmts = [call_stmt(&arena, "take", &[p])];
        let param = Param {
            kind: ParamKind::Named { mutable: false, name: arena.alloc_str("p"), ty: Some(ref_type(&arena, "L")), default: None },
            span: Z,
        };
        let caller = caller_fn(&arena, &[param], &stmts);
        let take = lifetime_fn(&arena, "take", "L");
        let program = program_with(&arena, &[Item::Function(caller), Item::Function(take)]);

        let violations = check_program(&program);
        assert!(violations.is_empty(), "forwarding the caller's own reference parameter is always valid");
    }

    #[test]
    fn untraceable_local_is_conservatively_rejected() {
        // fn caller() void { let p = other(); take(p); } — p comes from
        // a call result, not a Borrow node, so it has no loan at all.
        let arena = AstArena::new();
        let other_call = arena.alloc(Expr {
            kind: ExprKind::Call { callee: ident(&arena, "other"), args: &[] }, span: Z,
        });
        let p = ident(&arena, "p");
        let stmts = [
            let_stmt(&arena, "p", other_call),
            call_stmt(&arena, "take", &[p]),
        ];
        let caller = caller_fn(&arena, &[], &stmts);
        let take = lifetime_fn(&arena, "take", "L");
        let program = program_with(&arena, &[Item::Function(caller), Item::Function(take)]);

        let violations = check_program(&program);
        assert_eq!(violations.len(), 1);
        assert!(matches!(violations[0], Violation::NonLocalBoundaryArgument { .. }));
    }

    #[test]
    fn fresh_inline_borrow_is_always_accepted() {
        // fn caller() void { take(&n); } — no local to trace, and always
        // safe since it's freshly created at this exact point.
        let arena = AstArena::new();
        let n1 = ident(&arena, "n");
        let stmts = [call_stmt(&arena, "take", &[borrow(&arena, false, n1)])];
        let caller = caller_fn(&arena, &[], &stmts);
        let take = lifetime_fn(&arena, "take", "L");
        let program = program_with(&arena, &[Item::Function(caller), Item::Function(take)]);

        let violations = check_program(&program);
        assert!(violations.is_empty());
    }

    #[test]
    fn callee_with_no_lifetime_params_is_never_checked() {
        // A plain call to a function with no [lifetime ...] at all must
        // never be walked by this pass, regardless of its arguments.
        let arena = AstArena::new();
        let other_call = arena.alloc(Expr {
            kind: ExprKind::Call { callee: ident(&arena, "other"), args: &[] }, span: Z,
        });
        let p = ident(&arena, "p");
        let stmts = [
            let_stmt(&arena, "p", other_call),
            call_stmt(&arena, "plain", &[p]),
        ];
        let caller = caller_fn(&arena, &[], &stmts);
        let plain = FunctionDecl {
            tier: TierAnnotation::Low, attributes: &[], visibility: Visibility::default(),
            is_async: false, name: arena.alloc_str("plain"), lifetime_params: &[], generic_params: &[],
            params: &[], return_type: None, body: Block { stmts: &[], span: Z }, span: Z,
        };
        let program = program_with(&arena, &[Item::Function(caller), Item::Function(plain)]);

        let violations = check_program(&program);
        assert!(violations.is_empty(), "plain() has no [lifetime ...] params, so this pass has nothing to check");
    }

    #[test]
    fn multi_lifetime_callee_is_skipped_for_this_slice() {
        // fn take [lifetime L, lifetime M] (a: &L int, b: &M int) void
        // — multi-lifetime signatures are Phase E3's job, not this
        // slice's; confirms this pass doesn't guess at them early.
        let arena = AstArena::new();
        let n1 = ident(&arena, "n");
        let p = ident(&arena, "p");
        let stmts = [
            let_stmt(&arena, "p", borrow(&arena, false, n1)),
            reassign_stmt(&arena, "n", lit_int(&arena, 5)),
            call_stmt(&arena, "take2", &[p]),
        ];
        let caller = caller_fn(&arena, &[], &stmts);
        let param_a = Param { kind: ParamKind::Named { mutable: false, name: arena.alloc_str("a"), ty: Some(ref_type(&arena, "L")), default: None }, span: Z };
        let take2 = FunctionDecl {
            tier: TierAnnotation::Low, attributes: &[], visibility: Visibility::default(),
            is_async: false, name: arena.alloc_str("take2"),
            lifetime_params: arena.alloc_slice_copy(&[
                LifetimeParam { name: "L", constraint: None, span: Z },
                LifetimeParam { name: "M", constraint: None, span: Z },
            ]),
            generic_params: &[], params: arena.alloc_slice_copy(&[param_a]), return_type: None,
            body: Block { stmts: &[], span: Z }, span: Z,
        };
        let program = program_with(&arena, &[Item::Function(caller), Item::Function(take2)]);

        let violations = check_program(&program);
        assert!(violations.is_empty(), "a two-lifetime signature is out of scope for this slice, even with an otherwise-dead loan passed to it");
    }
}
