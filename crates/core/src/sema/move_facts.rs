// crates/core/src/sema/move_facts.rs
//! Move-fact collection for `@tier(low)` functions' `Unique<T>`-typed
//! local bindings — the ownership half of LOW-tier checking `facts.rs`'s
//! own module doc has flagged as deliberately out of scope since before
//! `Unique`/`Shared`/`SyncShared` even had a settled type story
//! (`MEMORY_MODEL.md` §9: *"building move-checking against it now would
//! mean building it twice"*). That prerequisite is now landed — the
//! type axis, its `Unique.new(value)` construction syntax, and its
//! `@tier(low)`-only construction gate (`TierError::
//! OwnershipWrapperOutsideLowTier`, `TIER-014`) are all live — so this
//! is the natural next slice.
//!
//! Mirrors `facts.rs`'s own division of labor exactly: pure, cheap,
//! per-function AST scan, zero fixed-point computation, zero
//! type-checker dependency. The actual reachability/violation check —
//! whether a later use of a moved-from local genuinely reaches a point
//! after the move on some real CFG path — is deliberately **not** built
//! here, the same relationship `facts.rs` (Phase C) has to
//! `borrow_check.rs` (Phase D). This is fact collection only. Not wired
//! into `sema::analyse` yet either, for the same reason cfg.rs/facts.rs
//! weren't until the phase that actually consumes them landed — there's
//! nothing user-visible to report until the fixed point exists.
//!
//! ## How a move-tracked local is identified
//!
//! Syntactically — same philosophy as everywhere else in this checker
//! (`Place::Unknown`, `Loan::bound_place`, etc.), deliberately *not*
//! threading real type inference into this pass. A `let`-bound local
//! counts as move-tracked if either:
//!
//!   1. its type annotation is syntactically `Unique<...>`
//!      (`TypeKind::Named { path: ["Unique"], .. }`), or
//!   2. its initializer is directly a `Unique.new(...)` call
//!      (`Call { callee: Field { target: Ident("Unique"), field: "new" },
//!      .. }`).
//!
//! Anything more indirect — a `Unique<T>` returned from another
//! function, round-tripped through a field, reassigned from a branch
//! that only sometimes produces one — is simply not tracked.
//! Under-approximation, not unsoundness: same spirit as every other
//! scope limit in this checker family. A tracked-but-missed case just
//! means a real bug goes uncaught by *this* pass, not that a safe
//! program gets rejected.
//!
//! **Only `let`-bound locals, not parameters.** A `@tier(low)`
//! function's own `Unique<T>`-typed *parameter* isn't tracked by this
//! collector — real, separate follow-up, not silently assumed safe.
//!
//! ## What counts as a "move"
//!
//! Any bare use of a move-tracked local's name that *isn't* wrapped in
//! a `&`/`&mut` (`ExprKind::Borrow`) — assignment to another binding, a
//! by-value call/method argument, a return value, a struct-literal
//! field value. Concretely, `walk_expr_move_aware` below is
//! `facts.rs`'s own `walk_expr` opacity boundary (`Lambda`/`Block`/
//! `If`/`Match`-as-expression stay undescended) plus two more rules of
//! its own:
//!
//!   - `Borrow { place, .. }`: never recurses into `place` at all —
//!     whatever's inside `&`/`&mut` is being borrowed, never moved,
//!     the one distinction this whole module exists to draw.
//!   - `Assign { target, value, .. }`: only recurses into `value`.
//!     `target` is a definition site — the old value there is simply
//!     dropped and overwritten, not read — matching
//!     `facts::classify_access`'s own treatment of this exact shape
//!     ("Reassign", never "Conflict").
//!
//! A method-call receiver (`a.method()`) is not a move of `a` when the
//! method name is a known builtin instance method
//! (`instance::is_builtin_instance_method_name`) or the name of any
//! user-declared instance method in the program (a method whose first
//! parameter is a `self` form, in a struct body or an `extend`/`impl`
//! block). The user-declared set is built once per program by
//! `user_method_names` and threaded in through `collect_with`. Both
//! exemptions are by name, not by type, so a same-named consuming
//! method on an unrelated type is a known, narrow, accepted gap: the
//! checker under-reports there, it never rejects a program that is
//! safe. Every other method call still counts as a bare use of its
//! receiver.

use std::collections::{HashMap, HashSet};

use crate::ast::common::TierAnnotation;
use crate::ast::expressions::{Expr, ExprKind};
use crate::ast::root::{Item, Program};
use crate::ast::statements::{BindingTarget, StmtKind};
use crate::ast::types::{Type, TypeKind};
use crate::lexer::Span;
use crate::sema::cfg::{self, Cfg};
use crate::sema::facts::{for_each_top_expr, Place, Point};

// ── Facts ────────────────────────────────────────────────────────────────

/// One bare (non-borrow) use of a move-tracked local.
#[derive(Debug, Clone, Copy)]
pub struct MoveFact<'ast> {
    pub place: Place<'ast>,
    pub point: Point,
    pub span:  Span,
}

#[derive(Debug, Default)]
pub struct MoveFacts<'ast> {
    /// Every local this function's own scan identified as move-tracked
    /// — see the module doc's identification rule. Exposed separately
    /// from `moved_at` (rather than only implicitly via its keys) so a
    /// caller can ask "is this place tracked at all" even before any
    /// use has been recorded for it.
    pub tracked_locals: HashSet<Place<'ast>>,
    /// Every point where a tracked local is used by value. The
    /// *chronologically first* such point on any given CFG path is the
    /// legitimate move; a later one reachable from an earlier one on
    /// the same path is a real "use after move" — but that reachability
    /// determination is deliberately not made here, same relationship
    /// `facts::loan_invalidated_at` has to `borrow_check.rs`'s liveness
    /// fixed point. This is the raw candidate list only.
    pub moved_at: HashMap<Place<'ast>, Vec<MoveFact<'ast>>>,
}

// ── Collection ───────────────────────────────────────────────────────────

/// Collects move facts for one function's already-built CFG. Doesn't
/// check tier itself — same division of responsibility `facts::collect`
/// and `cfg::build` already use; callers decide which functions are
/// worth running this on. See `collect_program` for the `@tier(low)`
/// walk this module provides ready-made.
pub fn collect<'ast>(cfg: &Cfg<'ast>) -> MoveFacts<'ast> {
    collect_with(cfg, &HashSet::new())
}

/// Every instance-method name (a method whose first parameter is a
/// `self` form) declared anywhere in `program`: struct bodies, `extend`
/// blocks and `impl` blocks. Used as `collect_with`'s second exemption
/// set; see the module doc.
pub fn user_method_names<'ast>(program: &Program<'ast>) -> HashSet<&'ast str> {
    use crate::ast::declarations::{MethodDecl, ParamKind, StructMember};
    fn add<'ast>(out: &mut HashSet<&'ast str>, m: &MethodDecl<'ast>) {
        let has_self = m.params.first()
            .map(|p| matches!(p.kind,
                ParamKind::SelfVal | ParamKind::SelfMut
                | ParamKind::SelfRef | ParamKind::SelfRefMut))
            .unwrap_or(false);
        if has_self {
            out.insert(m.name);
        }
    }
    let mut names = HashSet::new();
    for item in program.items {
        match item {
            Item::Struct(s) => {
                for member in s.members.iter() {
                    if let StructMember::Method(m) = member {
                        add(&mut names, m);
                    }
                }
            }
            Item::Extend(x) => {
                for m in x.methods.iter() {
                    add(&mut names, m);
                }
            }
            Item::Impl(i) => {
                for m in i.methods.iter() {
                    add(&mut names, m);
                }
            }
            _ => {}
        }
    }
    names
}

/// `collect` with the program's user-declared instance-method names, so
/// a call to one of them is not counted as a move of its receiver.
pub fn collect_with<'ast>(
    cfg:          &Cfg<'ast>,
    user_methods: &HashSet<&'ast str>,
) -> MoveFacts<'ast> {
    let mut facts = MoveFacts::default();

    for block in &cfg.blocks {
        for stmt in &block.stmts {
            if let StmtKind::Let { binding: BindingTarget::Ident(name), ty, value, .. } = &stmt.kind {
                if is_unique_tracked(*ty, value) {
                    facts.tracked_locals.insert(Place::Local(name));
                }
            }
        }
    }
    // Nothing tracked in this function -- skip the second walk entirely,
    // same early-out shape facts::collect itself would use if a
    // function had zero borrows.
    if facts.tracked_locals.is_empty() {
        return facts;
    }

    for block in &cfg.blocks {
        for (stmt_index, stmt) in block.stmts.iter().enumerate() {
            let point = Point { block: block.id, stmt_index };
            for_each_top_expr(stmt, &mut |e| {
                walk_expr_move_aware(e, user_methods, &mut |use_expr, name| {
                    let place = Place::Local(name);
                    if facts.tracked_locals.contains(&place) {
                        facts.moved_at.entry(place).or_default()
                            .push(MoveFact { place, point, span: use_expr.span });
                    }
                });
            });
        }
    }

    facts
}

/// Collects move facts for every `@tier(low)` free function in
/// `program`. Mirrors `borrow_check::check_program`'s exact walk — same
/// `Item::Function` + `f.tier == TierAnnotation::Low` filter, same
/// "methods not yet walked" scope limit (`cfg::build`/`facts::collect`
/// only accept `&FunctionDecl` today) — so whenever the fixed-point
/// slice needs this, the wiring shape is already identical to Phase D's.
pub fn collect_program<'ast>(program: &Program<'ast>) -> HashMap<&'ast str, MoveFacts<'ast>> {
    let mut all = HashMap::new();
    for item in program.items {
        if let Item::Function(f) = item {
            if f.tier == TierAnnotation::Low {
                let cfg = cfg::build(f);
                all.insert(f.name, collect(&cfg));
            }
        }
    }
    all
}

// ── Move-tracked local identification ───────────────────────────────────

/// See the module doc's identification rule.
fn is_unique_tracked<'ast>(ty: Option<&'ast Type<'ast>>, value: &'ast Expr<'ast>) -> bool {
    if let Some(t) = ty {
        if let TypeKind::Named { path, .. } = &t.kind {
            if path.len() == 1 && path[0] == "Unique" {
                return true;
            }
        }
    }
    is_unique_new_call(value)
}

/// Is `expr` directly `Unique.new(...)`? Not "does it evaluate to a
/// `Unique<T>`" — purely the syntactic shape, same restraint as
/// `facts::stmt_bare_borrow_target`'s "value is a `Borrow` node itself,
/// not nested inside a call or another expression" rule.
fn is_unique_new_call<'ast>(expr: &'ast Expr<'ast>) -> bool {
    let ExprKind::Call { callee, .. } = &expr.kind else { return false; };
    let ExprKind::Field { target, field } = &callee.kind else { return false; };
    if *field != "new" { return false; }
    matches!(target.kind, ExprKind::Ident(name) if name == "Unique")
}

// ── Move-aware expression walking ───────────────────────────────────────
//
// Retrofit onto `ast::visitor::AstVisitor`, same reason and same shape
// facts.rs's own `ScopedExprWalker` uses (see that module's doc) --
// with two extra opacity rules of this module's own layered on top; see
// the module doc's "What counts as a move" section for why each exists.

struct MoveExprWalker<'w, 'ast, F: FnMut(&'ast Expr<'ast>, &'ast str)> {
    visit:        &'w mut F,
    user_methods: &'w HashSet<&'ast str>,
    _marker: std::marker::PhantomData<&'ast ()>,
}

impl<'w, 'ast, F: FnMut(&'ast Expr<'ast>, &'ast str)> crate::ast::visitor::AstVisitor<'ast>
    for MoveExprWalker<'w, 'ast, F>
{
    fn visit_expr(&mut self, e: &'ast Expr<'ast>) {
        match &e.kind {
            ExprKind::Ident(name) => { (self.visit)(e, name); return; }
            ExprKind::Borrow { .. } => return,
            ExprKind::Assign { value, .. } => { self.visit_expr(value); return; }
            ExprKind::Lambda(_) | ExprKind::Block(_) | ExprKind::If(_) | ExprKind::Match(_) => return,
            ExprKind::Call { callee, args } => {
                // A call to a known builtin instance method (`.push()`,
                // `.len()`, ...) borrows its receiver in spirit: no
                // builtin instance method takes a consuming `self`/`mut
                // self`, so `target` here isn't a bare/moving use the
                // way a plain function call's callee, or an unrecognized
                // (e.g. user-declared) method's receiver, still is.
                // Name-based, not type-based, same syntactic restraint
                // `is_unique_new_call` already uses; a same-named user-
                // declared consuming method on some unrelated type is a
                // known, narrow, accepted gap (see
                // `instance::is_builtin_instance_method_name`'s own doc).
                // Still walk `target` normally when it is not a bare
                // identifier (a move could be buried deeper inside it,
                // e.g. an index expression), and always walk every arg.
                if let ExprKind::Field { target, field } = &callee.kind {
                    if crate::builtins::instance::is_builtin_instance_method_name(field)
                        || self.user_methods.contains(field)
                    {
                        if !matches!(target.kind, ExprKind::Ident(_)) {
                            self.visit_expr(target);
                        }
                        for a in *args {
                            crate::ast::visitor::walk_arg_kind(self, &a.kind);
                        }
                        return;
                    }
                }
            }
            _ => {}
        }
        crate::ast::visitor::walk_expr(self, e);
    }
}

fn walk_expr_move_aware<'ast>(
    expr:         &'ast Expr<'ast>,
    user_methods: &HashSet<&'ast str>,
    visit:        &mut impl FnMut(&'ast Expr<'ast>, &'ast str),
) {
    use crate::ast::visitor::AstVisitor;
    MoveExprWalker { visit, user_methods, _marker: std::marker::PhantomData }.visit_expr(expr);
}

// ── Tests ────────────────────────────────────────────────────────────
//
// Same self-contained, hand-built-AST approach facts.rs's/cfg.rs's own
// tests use — builds real function bodies via the arena, runs them
// through cfg::build then move_facts::collect, asserts on real output.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::arena::AstArena;
    use crate::ast::common::{AssignOp, Span, TierAnnotation, Visibility};
    use crate::ast::declarations::FunctionDecl;
    use crate::ast::expressions::{Arg, ArgKind};
    use crate::ast::literals::Literal;
    use crate::ast::statements::{Block, Stmt};
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

    /// `Unique.new(arg)`.
    fn unique_new<'a>(arena: &'a AstArena, arg: &'a Expr<'a>) -> &'a Expr<'a> {
        let target = ident(arena, "Unique");
        let callee = arena.alloc(Expr { kind: ExprKind::Field { target, field: "new" }, span: Z });
        let args = arena.alloc_slice_copy(&[Arg { kind: ArgKind::Positional(arg), span: Z }]);
        arena.alloc(Expr { kind: ExprKind::Call { callee, args }, span: Z })
    }

    fn unique_type<'a>(arena: &'a AstArena) -> &'a Type<'a> {
        arena.alloc(Type {
            kind: TypeKind::Named { path: arena.alloc_slice_copy(&[arena.alloc_str("Unique")]), args: &[] },
            span: Z,
        })
    }

    fn let_stmt<'a>(arena: &'a AstArena, name: &'a str, value: &'a Expr<'a>) -> Stmt<'a> {
        Stmt {
            kind: StmtKind::Let {
                mutable: false, binding: BindingTarget::Ident(arena.alloc_str(name)), ty: None, value,
            },
            span: Z,
        }
    }

    fn let_stmt_typed<'a>(arena: &'a AstArena, name: &'a str, ty: &'a Type<'a>, value: &'a Expr<'a>) -> Stmt<'a> {
        Stmt {
            kind: StmtKind::Let {
                mutable: false, binding: BindingTarget::Ident(arena.alloc_str(name)), ty: Some(ty), value,
            },
            span: Z,
        }
    }

    fn reassign_stmt<'a>(arena: &'a AstArena, name: &'a str, value: &'a Expr<'a>) -> Stmt<'a> {
        let target = ident(arena, name);
        let assign = arena.alloc(Expr { kind: ExprKind::Assign { op: AssignOp::Assign, target, value }, span: Z });
        Stmt { kind: StmtKind::Expr(assign), span: Z }
    }

    fn call_stmt<'a>(arena: &'a AstArena, fn_name: &'a str, arg: &'a Expr<'a>) -> Stmt<'a> {
        let callee = ident(arena, fn_name);
        let args = arena.alloc_slice_copy(&[Arg { kind: ArgKind::Positional(arg), span: Z }]);
        let call = arena.alloc(Expr { kind: ExprKind::Call { callee, args }, span: Z });
        Stmt { kind: StmtKind::Expr(call), span: Z }
    }

    fn func<'a>(arena: &'a AstArena, stmts: &[Stmt<'a>]) -> FunctionDecl<'a> {
        FunctionDecl {
            tier: TierAnnotation::default(), attributes: &[], visibility: Visibility::default(),
            is_async: false, name: arena.alloc_str("f"), lifetime_params: &[], generic_params: &[],
            params: &[], return_type: None,
            body: Block { stmts: arena.alloc_slice_copy(stmts), span: Z },
            span: Z,
        }
    }

    fn move_facts_for<'a>(decl: &'a FunctionDecl<'a>) -> MoveFacts<'a> {
        let graph = cfg::build(decl);
        collect(&graph)
    }

    #[test]
    fn unique_new_initializer_is_tracked() {
        let arena = AstArena::new();
        let stmts = [let_stmt(&arena, "a", unique_new(&arena, lit_int(&arena, 5)))];
        let decl = arena.alloc(func(&arena, &stmts));
        let facts = move_facts_for(decl);
        assert!(facts.tracked_locals.contains(&Place::Local("a")));
    }

    #[test]
    fn explicit_unique_annotation_is_tracked_regardless_of_initializer_shape() {
        // `let a: Unique<T> = 0` isn't something real sema would accept
        // (a genuine type mismatch) -- irrelevant here, this module does
        // no type-checking of its own, purely a syntactic scan. The
        // annotation alone must be enough to track `a`.
        let arena = AstArena::new();
        let ty = unique_type(&arena);
        let stmts = [let_stmt_typed(&arena, "a", ty, lit_int(&arena, 0))];
        let decl = arena.alloc(func(&arena, &stmts));
        let facts = move_facts_for(decl);
        assert!(facts.tracked_locals.contains(&Place::Local("a")));
    }

    #[test]
    fn untracked_plain_let_is_not_tracked() {
        let arena = AstArena::new();
        let stmts = [let_stmt(&arena, "a", lit_int(&arena, 5))];
        let decl = arena.alloc(func(&arena, &stmts));
        let facts = move_facts_for(decl);
        assert!(facts.tracked_locals.is_empty());
    }

    #[test]
    fn plain_reassignment_use_is_recorded_as_a_move() {
        // let a = Unique.new(5); let b = a;
        let arena = AstArena::new();
        let stmts = [
            let_stmt(&arena, "a", unique_new(&arena, lit_int(&arena, 5))),
            let_stmt(&arena, "b", ident(&arena, "a")),
        ];
        let decl = arena.alloc(func(&arena, &stmts));
        let facts = move_facts_for(decl);
        let moves = facts.moved_at.get(&Place::Local("a")).cloned().unwrap_or_default();
        assert_eq!(moves.len(), 1, "expected exactly one recorded use of `a`");
    }

    #[test]
    fn borrowing_a_tracked_local_is_never_recorded_as_a_move() {
        // let a = Unique.new(5); let p = &a;
        let arena = AstArena::new();
        let stmts = [
            let_stmt(&arena, "a", unique_new(&arena, lit_int(&arena, 5))),
            let_stmt(&arena, "p", borrow(&arena, false, ident(&arena, "a"))),
        ];
        let decl = arena.alloc(func(&arena, &stmts));
        let facts = move_facts_for(decl);
        assert!(facts.moved_at.get(&Place::Local("a")).map_or(true, |v| v.is_empty()));
    }

    #[test]
    fn reassignment_target_is_never_recorded_as_a_move() {
        // let a = Unique.new(5); a = Unique.new(10);
        // The LHS `a` in the second statement is a definition site, not
        // a use of the OLD value -- must not be recorded as a move.
        let arena = AstArena::new();
        let stmts = [
            let_stmt(&arena, "a", unique_new(&arena, lit_int(&arena, 5))),
            reassign_stmt(&arena, "a", unique_new(&arena, lit_int(&arena, 10))),
        ];
        let decl = arena.alloc(func(&arena, &stmts));
        let facts = move_facts_for(decl);
        assert!(facts.moved_at.get(&Place::Local("a")).map_or(true, |v| v.is_empty()));
    }

    #[test]
    fn call_argument_use_is_recorded_as_a_move() {
        // let a = Unique.new(5); consume(a);
        let arena = AstArena::new();
        let stmts = [
            let_stmt(&arena, "a", unique_new(&arena, lit_int(&arena, 5))),
            call_stmt(&arena, "consume", ident(&arena, "a")),
        ];
        let decl = arena.alloc(func(&arena, &stmts));
        let facts = move_facts_for(decl);
        let moves = facts.moved_at.get(&Place::Local("a")).cloned().unwrap_or_default();
        assert_eq!(moves.len(), 1);
    }

    #[test]
    fn two_uses_are_both_recorded_as_separate_move_candidates() {
        // let a = Unique.new(5); let b = a; let c = a;
        // Both later uses are recorded -- this pass doesn't decide which
        // one (if any) is the real violation; that's the future fixed
        // point's job (see module doc).
        let arena = AstArena::new();
        let stmts = [
            let_stmt(&arena, "a", unique_new(&arena, lit_int(&arena, 5))),
            let_stmt(&arena, "b", ident(&arena, "a")),
            let_stmt(&arena, "c", ident(&arena, "a")),
        ];
        let decl = arena.alloc(func(&arena, &stmts));
        let facts = move_facts_for(decl);
        let moves = facts.moved_at.get(&Place::Local("a")).cloned().unwrap_or_default();
        assert_eq!(moves.len(), 2);
    }

    #[test]
    fn collect_program_only_walks_tier_low_free_functions() {
        // Two functions: one @tier(low) with a tracked local and a move,
        // one HIGH-tier (the default) that would look identical if it
        // were walked -- it must be completely absent from the result,
        // same `f.tier == TierAnnotation::Low` filter borrow_check.rs's
        // own check_program already uses.
        let arena = AstArena::new();

        let low_stmts = arena.alloc_slice_copy(&[
            let_stmt(&arena, "a", unique_new(&arena, lit_int(&arena, 5))),
            let_stmt(&arena, "b", ident(&arena, "a")),
        ]);
        let mut low_fn = func(&arena, low_stmts);
        low_fn.tier = TierAnnotation::Low;
        low_fn.name = arena.alloc_str("low_fn");

        let high_stmts = arena.alloc_slice_copy(&[
            let_stmt(&arena, "a", unique_new(&arena, lit_int(&arena, 5))),
            let_stmt(&arena, "b", ident(&arena, "a")),
        ]);
        let mut high_fn = func(&arena, high_stmts);
        high_fn.name = arena.alloc_str("high_fn"); // tier defaults to non-Low

        let items = arena.alloc_slice_copy(&[Item::Function(low_fn), Item::Function(high_fn)]);
        let program = Program { package: None, imports: &[], items, span: Z };

        let all = collect_program(&program);
        assert!(all.contains_key("low_fn"), "the @tier(low) function must be walked");
        assert!(!all.contains_key("high_fn"), "a non-LOW function must not be walked at all");
        assert_eq!(
            all["low_fn"].moved_at.get(&Place::Local("a")).map(|v| v.len()).unwrap_or(0),
            1
        );
    }
    /// `recv.method(arg)` as an expression statement.
    fn method_call_stmt<'a>(
        arena: &'a AstArena, recv: &'a str, method: &'a str, arg: &'a Expr<'a>,
    ) -> Stmt<'a> {
        let target = ident(arena, recv);
        let callee = arena.alloc(Expr { kind: ExprKind::Field { target, field: arena.alloc_str(method) }, span: Z });
        let args = arena.alloc_slice_copy(&[Arg { kind: ArgKind::Positional(arg), span: Z }]);
        let call = arena.alloc(Expr { kind: ExprKind::Call { callee, args }, span: Z });
        Stmt { kind: StmtKind::Expr(call), span: Z }
    }

    #[test]
    fn user_declared_method_call_is_not_a_move_when_its_name_is_supplied() {
        // let a = Unique.new(5); a.custom_op(1); a.custom_op(2)
        // `custom_op` is not a builtin instance method name, so with no
        // user-method set both calls count as uses of `a`; with it in the
        // set neither does.
        let arena = AstArena::new();
        let stmts = [
            let_stmt(&arena, "a", unique_new(&arena, lit_int(&arena, 5))),
            method_call_stmt(&arena, "a", "custom_op", lit_int(&arena, 1)),
            method_call_stmt(&arena, "a", "custom_op", lit_int(&arena, 2)),
        ];
        let decl = arena.alloc(func(&arena, &stmts));
        let graph = cfg::build(decl);

        let without = collect_with(&graph, &HashSet::new());
        assert_eq!(
            without.moved_at.get(&Place::Local("a")).map(|v| v.len()).unwrap_or(0), 2,
            "an unknown method name still counts as a bare use of its receiver"
        );

        let mut user = HashSet::new();
        user.insert("custom_op");
        let with = collect_with(&graph, &user);
        assert!(
            with.moved_at.get(&Place::Local("a")).is_none(),
            "a user-declared method name must not count as a move of its receiver"
        );
    }

    #[test]
    fn user_method_set_does_not_hide_a_genuine_move_of_the_same_local() {
        // let a = Unique.new(5); a.custom_op(1); let b = a;
        let arena = AstArena::new();
        let stmts = [
            let_stmt(&arena, "a", unique_new(&arena, lit_int(&arena, 5))),
            method_call_stmt(&arena, "a", "custom_op", lit_int(&arena, 1)),
            let_stmt(&arena, "b", ident(&arena, "a")),
        ];
        let decl = arena.alloc(func(&arena, &stmts));
        let graph = cfg::build(decl);
        let mut user = HashSet::new();
        user.insert("custom_op");
        let facts = collect_with(&graph, &user);
        assert_eq!(
            facts.moved_at.get(&Place::Local("a")).map(|v| v.len()).unwrap_or(0), 1,
            "only `let b = a` is a move"
        );
    }

    #[test]
    fn user_method_names_only_collects_methods_with_a_self_receiver() {
        use crate::ast::declarations::{
            ExtendDecl, MethodDecl, Param, ParamKind,
        };
        let arena = AstArena::new();
        let self_param = Param { kind: ParamKind::SelfRef, span: Z };
        let named_param = Param {
            kind: ParamKind::Named { mutable: false, name: arena.alloc_str("x"), ty: None, default: None },
            span: Z,
        };
        fn method<'a>(arena: &'a AstArena, name: &str, params: &[Param<'a>]) -> MethodDecl<'a> {
            MethodDecl {
                attributes: &[], tier: TierAnnotation::default(), visibility: Visibility::default(),
                is_async: false, name: arena.alloc_str(name), generic_params: &[],
                params: arena.alloc_slice_copy(params), return_type: None,
                body: Block { stmts: &[], span: Z }, span: Z,
            }
        }
        let methods = arena.alloc_slice_copy(&[
            method(&arena, "bump", &[self_param]),   // instance method
            method(&arena, "make", &[named_param]),  // associated function, no receiver
        ]);
        let ty = arena.alloc(Type {
            kind: TypeKind::Named { path: arena.alloc_slice_copy(&[arena.alloc_str("Counter")]), args: &[] },
            span: Z,
        });
        let items = arena.alloc_slice_copy(&[Item::Extend(ExtendDecl {
            attributes: &[], target_type: ty, methods, span: Z,
        })]);
        let program = Program { package: None, imports: &[], items, span: Z };
        let names = user_method_names(&program);
        assert!(names.contains("bump"));
        assert!(!names.contains("make"), "an associated function is not an instance method");
    }
}
