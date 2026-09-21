// crates/core/src/sema/outlives_check.rs
//! Phase E2 + E4 (`docs/OUTLIVES_RULES.md`) — boundary constraint
//! generation and checking, for both boundary shapes §3 names: a call
//! to a function with `[lifetime L]` params, and a struct literal for
//! a struct with `[lifetime L]` (an `edge struct` or otherwise — see
//! `check_struct_lit`'s own doc comment for why this isn't restricted
//! to `edge` specifically). Single declared lifetime per declaration
//! either way. Multi-lifetime `outlives` propagation (Phase E3) is a
//! later slice per §9's own landing order — not built here.
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
//! `@tier(low)` *and* `@tier(mid)` caller functions are walked — wider
//! than `borrow_check`/`move_check`'s own LOW-tier-only restriction,
//! and not the original plan. Found empirically, not assumed: `edge
//! struct` construction inside `with arena(...)` (§8's whole motivating
//! scenario) is necessarily MID-tier code (arenas don't exist in LOW
//! tier), so a LOW-tier-only gate would leave `unify_struct_field`'s
//! own arena-tag deferral (`type_infer.rs`) completely unchecked for
//! the one case it exists for — a real regression versus the blunt
//! rejection it replaces, not a deferral to something that actually
//! runs. `cfg::build`/`facts::collect`/`compute_loan_regions` don't
//! themselves gate on tier at all (confirmed by reading them, not
//! assumed from `borrow_check`'s own choice to restrict itself) — the
//! LOW-tier-only convention elsewhere is a decision made by each
//! checker's own `check_program`, not a limitation of the shared
//! machinery. `@tier(high)` is deliberately still excluded: it's never
//! been borrow-checked at all (no loan/conflict detection exists
//! there), so running this pass over it would mean the *only* safety
//! net for that tier is a phase built to be one piece of a larger,
//! still-partial story — a bigger step than this finding justifies.
//! Free functions only, not methods — inherits the same boundary
//! `borrow_check.rs`'s own module doc already states and this phase
//! explicitly defers fixing (`OUTLIVES_RULES.md` §4).

use std::collections::{HashMap, HashSet};

use crate::ast::common::TierAnnotation;
use crate::ast::declarations::{FunctionDecl, ParamKind, StructDecl, StructMember};
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
    BoundaryTooShort {
        lifetime: String,
        loan_span: Span,
        call_span: Span,
    },
    /// LIFETIME-006 — case 4 above: no traceable parameter or loan at
    /// all, conservative reject.
    NonLocalBoundaryValue {
        lifetime: String,
        call_span: Span,
    },
}

/// Runs Phase E2 + E4 for every `@tier(low)` free function in
/// `program`. Doesn't touch `ErrorManager` itself, same reasoning as
/// `borrow_check::check_program` — see that function's own doc note.
pub fn check_program<'ast>(program: &Program<'ast>) -> Vec<Violation> {
    let mut fn_table: HashMap<&'ast str, &'ast FunctionDecl<'ast>> = HashMap::new();
    let mut struct_table: HashMap<&'ast str, &'ast StructDecl<'ast>> = HashMap::new();
    for item in program.items {
        match item {
            Item::Function(f) => { fn_table.insert(f.name, f); }
            Item::Struct(s) => { struct_table.insert(s.name, s); }
            _ => {}
        }
    }

    let mut violations = Vec::new();
    for item in program.items {
        if let Item::Function(f) = item {
            if f.tier == TierAnnotation::Low || f.tier == TierAnnotation::Mid {
                violations.extend(check_function(f, &fn_table, &struct_table));
            }
        }
    }
    violations
}

fn check_function<'ast>(
    caller: &'ast FunctionDecl<'ast>,
    fn_table: &HashMap<&'ast str, &'ast FunctionDecl<'ast>>,
    struct_table: &HashMap<&'ast str, &'ast StructDecl<'ast>>,
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
                find_boundary_sites(e, &mut |site| match &site.kind {
                    ExprKind::Call { .. } => check_call(
                        site, point, fn_table, &caller_param_names,
                        &facts, &regions, &mut violations,
                    ),
                    ExprKind::StructLit { .. } => check_struct_lit(
                        site, point, struct_table, &caller_param_names,
                        &facts, &regions, &mut violations,
                    ),
                    _ => {}
                });
            });
        }
    }

    violations
}

/// The shared four-case decision (this module's own doc comment) for
/// one expression that's expected to satisfy a `&L T` boundary
/// position — a call argument or a struct-literal field value alike.
/// `site_span` is what a violation points at: the argument expression
/// itself for a call, the field-init expression for a struct literal.
fn check_boundary_value<'ast>(
    value_expr: &'ast Expr<'ast>,
    lifetime_name: &'ast str,
    point: Point,
    caller_param_names: &HashSet<&'ast str>,
    facts: &Facts<'ast>,
    regions: &HashMap<LoanId, HashSet<Point>>,
    violations: &mut Vec<Violation>,
) {
    let place = facts::expr_as_place(value_expr);
    let Place::Local(name) = place else { return }; // fresh inline borrow — always valid

    if caller_param_names.contains(name) { return; } // forwarding caller's own param — always valid

    let bound_loans: Vec<_> = facts.loans.iter()
        .filter(|loan| matches!(loan.bound_place, Place::Local(n) if n == name))
        .collect();

    if bound_loans.is_empty() {
        violations.push(Violation::NonLocalBoundaryValue {
            lifetime: lifetime_name.to_string(),
            call_span: value_expr.span,
        });
        return;
    }

    let reaches = bound_loans.iter()
        .any(|loan| regions.get(&loan.id).is_some_and(|region| region.contains(&point)));

    if !reaches {
        let loan_span = bound_loans.last().expect("checked non-empty above").span;
        violations.push(Violation::BoundaryTooShort {
            lifetime: lifetime_name.to_string(),
            loan_span,
            call_span: value_expr.span,
        });
    }
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

        check_boundary_value(
            arg_expr, lifetime_name, point, caller_param_names, facts, regions, violations,
        );
    }
}

/// The struct-literal-construction boundary shape (this module's own
/// doc comment) — `docs/OUTLIVES_RULES.md` §3's second shape, §8's
/// "free win" connection to the arena-escape checker
/// (`type_infer.rs::unify_struct_field`) depends on this actually
/// existing: deferring the blunt arena-tag rejection only stays sound
/// because *this* check is what now enforces the real thing (is the
/// field's actual borrow still live) instead. `path.len() != 1` (an
/// enum struct-payload variant, `Message.Move { .. }`) is out of scope
/// for this slice — see `type_infer.rs`'s own `ExprKind::StructLit`
/// handling for why that's a structurally separate case.
fn check_struct_lit<'ast>(
    lit_expr: &'ast Expr<'ast>,
    point: Point,
    struct_table: &HashMap<&'ast str, &'ast StructDecl<'ast>>,
    caller_param_names: &HashSet<&'ast str>,
    facts: &Facts<'ast>,
    regions: &HashMap<LoanId, HashSet<Point>>,
    violations: &mut Vec<Violation>,
) {
    let ExprKind::StructLit { path, fields } = &lit_expr.kind else { return };
    if path.len() != 1 { return; }
    let Some(&s) = struct_table.get(path[0]) else { return };

    // Single declared lifetime per declaration only — same restriction
    // as check_call, same reason (E3 not built yet).
    if s.lifetime_params.len() != 1 { return; }
    let lifetime_name = s.lifetime_params[0].name;

    for member in s.members.iter() {
        let StructMember::Field(field_decl) = member else { continue };
        let TypeKind::Reference { lifetime: Some(l), .. } = field_decl.ty.kind else { continue };
        if l != lifetime_name { continue; }

        let Some(field_init) = fields.iter().find(|f| f.name == field_decl.name) else { continue };

        check_boundary_value(
            field_init.value, lifetime_name, point, caller_param_names, facts, regions, violations,
        );
    }
}

// ── Boundary-site finder ─────────────────────────────────────────────
//
// Same opacity boundary as `facts.rs`'s own `ScopedExprWalker` (a
// lambda body, a `{ }` block expression, or an if/match used as a value
// is a separate scope this pass doesn't descend into) — `facts.rs`'s
// own walker is private to that module, so this is a second, small,
// deliberately-identical instance built directly on the shared public
// `ast::visitor`, not a reach into `facts.rs` internals. Finds both
// boundary shapes (`Call`, `StructLit`) in one pass; the caller
// dispatches on which one it got.

struct BoundarySiteFinder<'w, 'ast, F: FnMut(&'ast Expr<'ast>)> {
    visit: &'w mut F,
    _marker: std::marker::PhantomData<&'ast ()>,
}

impl<'w, 'ast, F: FnMut(&'ast Expr<'ast>)> crate::ast::visitor::AstVisitor<'ast>
    for BoundarySiteFinder<'w, 'ast, F>
{
    fn visit_expr(&mut self, e: &'ast Expr<'ast>) {
        if matches!(e.kind, ExprKind::Call { .. } | ExprKind::StructLit { .. }) {
            (self.visit)(e);
        }
        if matches!(e.kind, ExprKind::Lambda(_) | ExprKind::Block(_) | ExprKind::If(_) | ExprKind::Match(_)) {
            return;
        }
        crate::ast::visitor::walk_expr(self, e);
    }
}

fn find_boundary_sites<'ast>(expr: &'ast Expr<'ast>, visit: &mut impl FnMut(&'ast Expr<'ast>)) {
    use crate::ast::visitor::AstVisitor;
    BoundarySiteFinder { visit, _marker: std::marker::PhantomData }.visit_expr(expr);
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

    // ── Struct-literal boundary helpers ─────────────────────────────

    fn field_decl<'a>(arena: &'a AstArena, name: &'a str, lifetime: &'a str) -> StructMember<'a> {
        StructMember::Field(crate::ast::declarations::FieldDecl {
            visibility: Visibility::default(), name: arena.alloc_str(name),
            ty: ref_type(arena, lifetime), span: Z,
        })
    }

    fn edge_struct_decl<'a>(arena: &'a AstArena, name: &'a str, lifetime: &'a str, is_edge: bool) -> StructDecl<'a> {
        StructDecl {
            attributes: &[], visibility: Visibility::default(), is_edge,
            name: arena.alloc_str(name),
            lifetime_params: arena.alloc_slice_copy(&[LifetimeParam { name: lifetime, constraint: None, span: Z }]),
            generic_params: &[],
            members: arena.alloc_slice_copy(&[field_decl(arena, "item", lifetime)]),
            span: Z,
        }
    }

    fn struct_lit_stmt<'a>(arena: &'a AstArena, struct_name: &'a str, field_value: &'a Expr<'a>) -> Stmt<'a> {
        let field = crate::ast::expressions::FieldInit { name: arena.alloc_str("item"), value: field_value, span: Z };
        let lit = arena.alloc(Expr {
            kind: ExprKind::StructLit {
                path: arena.alloc_slice_copy(&[struct_name]),
                fields: arena.alloc_slice_copy(&[field]),
            },
            span: Z,
        });
        Stmt { kind: StmtKind::Expr(lit), span: Z }
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
        assert!(matches!(violations[0], Violation::BoundaryTooShort { .. }));
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
        assert!(matches!(violations[0], Violation::NonLocalBoundaryValue { .. }));
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

    // ── Struct-literal boundary shape ───────────────────────────────

    #[test]
    fn struct_field_loan_still_live_is_accepted() {
        // struct Cache [lifetime L] { item: &L int }
        // fn caller() void { let n = 5; let p = &n; Cache { item = p } }
        let arena = AstArena::new();
        let n1 = ident(&arena, "n");
        let p = ident(&arena, "p");
        let stmts = [
            let_stmt(&arena, "p", borrow(&arena, false, n1)),
            struct_lit_stmt(&arena, "Cache", p),
        ];
        let caller = caller_fn(&arena, &[], &stmts);
        let cache = edge_struct_decl(&arena, "Cache", "L", true);
        let program = program_with(&arena, &[Item::Function(caller), Item::Struct(cache)]);

        let violations = check_program(&program);
        assert!(violations.is_empty(), "p's own loan is still live right at the construction site");
    }

    #[test]
    fn struct_field_loan_dead_before_construction_is_rejected() {
        // Same shape as the call-boundary dead-loan test, but the
        // boundary site is a struct literal instead of a call.
        let arena = AstArena::new();
        let n1 = ident(&arena, "n");
        let p = ident(&arena, "p");
        let stmts = [
            let_stmt(&arena, "p", borrow(&arena, false, n1)),
            reassign_stmt(&arena, "n", lit_int(&arena, 5)),
            struct_lit_stmt(&arena, "Cache", p),
        ];
        let caller = caller_fn(&arena, &[], &stmts);
        let cache = edge_struct_decl(&arena, "Cache", "L", true);
        let program = program_with(&arena, &[Item::Function(caller), Item::Struct(cache)]);

        let violations = check_program(&program);
        assert_eq!(violations.len(), 1, "n's reassignment kills p's loan before it reaches the struct literal");
        assert!(matches!(violations[0], Violation::BoundaryTooShort { .. }));
    }

    #[test]
    fn struct_field_check_does_not_require_is_edge() {
        // A struct with [lifetime L] but is_edge = false still gets
        // checked — the safety question (is the field's own loan still
        // live) doesn't care whether the struct happens to be `edge`;
        // only `unify_struct_field`'s arena-tag deferral (type_infer.rs)
        // is `is_edge`-gated, not this check.
        let arena = AstArena::new();
        let n1 = ident(&arena, "n");
        let p = ident(&arena, "p");
        let stmts = [
            let_stmt(&arena, "p", borrow(&arena, false, n1)),
            reassign_stmt(&arena, "n", lit_int(&arena, 5)),
            struct_lit_stmt(&arena, "Plain", p),
        ];
        let caller = caller_fn(&arena, &[], &stmts);
        let plain = edge_struct_decl(&arena, "Plain", "L", false);
        let program = program_with(&arena, &[Item::Function(caller), Item::Struct(plain)]);

        let violations = check_program(&program);
        assert_eq!(violations.len(), 1, "a stale loan into a non-edge struct's reference field is just as real a bug");
    }

    #[test]
    fn struct_field_untraceable_local_is_conservatively_rejected() {
        let arena = AstArena::new();
        let other_call = arena.alloc(Expr {
            kind: ExprKind::Call { callee: ident(&arena, "other"), args: &[] }, span: Z,
        });
        let p = ident(&arena, "p");
        let stmts = [
            let_stmt(&arena, "p", other_call),
            struct_lit_stmt(&arena, "Cache", p),
        ];
        let caller = caller_fn(&arena, &[], &stmts);
        let cache = edge_struct_decl(&arena, "Cache", "L", true);
        let program = program_with(&arena, &[Item::Function(caller), Item::Struct(cache)]);

        let violations = check_program(&program);
        assert_eq!(violations.len(), 1);
        assert!(matches!(violations[0], Violation::NonLocalBoundaryValue { .. }));
    }

    #[test]
    fn struct_field_forwarding_callers_own_param_is_accepted() {
        let arena = AstArena::new();
        let p = ident(&arena, "p");
        let stmts = [struct_lit_stmt(&arena, "Cache", p)];
        let param = Param {
            kind: ParamKind::Named { mutable: false, name: arena.alloc_str("p"), ty: Some(ref_type(&arena, "L")), default: None },
            span: Z,
        };
        let caller = caller_fn(&arena, &[param], &stmts);
        let cache = edge_struct_decl(&arena, "Cache", "L", true);
        let program = program_with(&arena, &[Item::Function(caller), Item::Struct(cache)]);

        let violations = check_program(&program);
        assert!(violations.is_empty(), "forwarding the caller's own reference parameter into a struct field is always valid");
    }

    #[test]
    fn enum_variant_path_is_not_treated_as_a_plain_struct_literal() {
        // path.len() != 1 (Message.Move { .. }-style) is out of scope
        // for this slice — confirms it's skipped, not mishandled.
        let arena = AstArena::new();
        let n1 = ident(&arena, "n");
        let p = ident(&arena, "p");
        let field = crate::ast::expressions::FieldInit { name: arena.alloc_str("item"), value: p, span: Z };
        let lit = arena.alloc(Expr {
            kind: ExprKind::StructLit {
                path: arena.alloc_slice_copy(&["Message", "Move"]),
                fields: arena.alloc_slice_copy(&[field]),
            },
            span: Z,
        });
        let stmts = [
            let_stmt(&arena, "p", borrow(&arena, false, n1)),
            reassign_stmt(&arena, "n", lit_int(&arena, 5)),
            Stmt { kind: StmtKind::Expr(lit), span: Z },
        ];
        let caller = caller_fn(&arena, &[], &stmts);
        let program = program_with(&arena, &[Item::Function(caller)]);

        let violations = check_program(&program);
        assert!(violations.is_empty(), "a 2-segment path is an enum variant construction, not a plain struct literal this slice checks");
    }
}
