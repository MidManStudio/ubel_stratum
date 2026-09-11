// crates/core/src/ast/visitor.rs
//! Generic AST traversal. `PARSER_RULES.md` §10 marked this "proposed,
//! future" for a long time; it stopped being optional once `cfg.rs` and
//! `facts.rs` had each independently hand-written their own exhaustive
//! `StmtKind`/`ExprKind` walk this session — same traversal, written
//! twice, slightly differently each time. That's the actual bug class
//! this module exists to close off: a hand-rolled walk with a wildcard
//! catch-all (`_ => { /* treat as opaque */ }`) silently does the wrong
//! thing for any node kind nobody thought to add explicitly — which is
//! exactly how `unsafe { }` blocks went unwalked by `cfg.rs` for a while
//! (see that module's own history). One correct, exhaustive walk, used
//! by every pass that just wants to *observe* the tree.
//!
//! ## Design
//!
//! One method per node kind, each with a default implementation that
//! just walks that node's children via the matching free `walk_*`
//! function below. An implementor overrides only the node kinds it
//! actually cares about; everything else falls through to the default
//! and keeps recursing correctly on its own — the same "default impl
//! calls the walker" shape `PARSER_RULES.md` §10 originally sketched,
//! filled in completely instead of partially.
//!
//! **This is a plain observer pattern — methods return `()`.** It's the
//! right shape for a pass that records facts about nodes as it visits
//! them (see `facts.rs`'s retrofit). It is deliberately *not* the shape
//! `cfg.rs`'s own per-statement dispatch uses, and `cfg.rs` does not
//! implement this trait for its CFG-construction algorithm — see that
//! module's own doc comment for exactly why (short version: building a
//! control-flow graph needs `Option<BlockId>` return values and
//! early-exit-on-`return`/`break`/`continue` semantics that don't fit a
//! unit-returning visit method without an awkward, risk-adding
//! workaround for no real safety gain over just keeping its dispatch
//! match exhaustive, which is the actual fix for the bug class above).
//!
//! ## Controlling recursion depth
//!
//! Every `visit_*` method's default calls the matching `walk_*`
//! function, which is what actually recurses into children. An
//! implementor that wants to visit a node's own content but *not*
//! automatically recurse into some part of it — e.g. `facts.rs`'s
//! per-CFG-point collector, which must NOT descend into a nested `if`/
//! `while`/`for` body's statements (those are already separate points in
//! `cfg.rs`'s own block list; recursing into them here would double-count
//! every loan and access inside one) — overrides `visit_block` to a
//! no-op. Since every compound statement's nested body is reached
//! through `visit_block`, that one override is enough to make the whole
//! traversal "shallow" without needing seventeen individual overrides.

use crate::ast::declarations::{
    ConstDecl, EnumDecl, ExtendDecl, FunctionDecl, ImplBlock, MethodDecl,
    ParamKind, StructDecl, StructMember, TraitDecl, TraitItem, TypeAlias,
};
use crate::ast::expressions::{
    ArgKind, ElifBranch, Expr, ExprKind, IfBranchBody, IfExpr, LambdaBody,
    MatchArm, MatchArmBody, MatchExpr, OptionalAccess, OrElseFallback,
};
use crate::ast::root::{Item, Program};
use crate::ast::statements::{AllocatorKind, Block, SizeExpr, Stmt, StmtKind};
use crate::ast::declarations::EnumVariantPayload;

/// Implement this to run custom logic while traversing an AST. Every
/// method has a default that walks that node's children — override only
/// what you need. See the module doc for the "make `visit_block` a
/// no-op to get a shallow, single-statement-scoped walk" trick.
pub trait AstVisitor<'ast> {
    // ── Top level ────────────────────────────────────────────────
    fn visit_program(&mut self, p: &Program<'ast>) { walk_program(self, p) }
    fn visit_item(&mut self, item: &Item<'ast>) { walk_item(self, item) }
    fn visit_function_decl(&mut self, f: &FunctionDecl<'ast>) { walk_function_decl(self, f) }
    fn visit_method_decl(&mut self, m: &MethodDecl<'ast>) { walk_method_decl(self, m) }
    fn visit_struct_decl(&mut self, s: &StructDecl<'ast>) { walk_struct_decl(self, s) }
    fn visit_enum_decl(&mut self, e: &EnumDecl<'ast>) { walk_enum_decl(self, e) }
    fn visit_trait_decl(&mut self, t: &TraitDecl<'ast>) { walk_trait_decl(self, t) }
    fn visit_impl_block(&mut self, i: &ImplBlock<'ast>) { walk_impl_block(self, i) }
    fn visit_extend_decl(&mut self, e: &ExtendDecl<'ast>) { walk_extend_decl(self, e) }
    fn visit_const_decl(&mut self, c: &ConstDecl<'ast>) { walk_const_decl(self, c) }
    /// `TypeAlias` has no `Stmt`/`Expr` children — nothing to walk into.
    fn visit_type_alias(&mut self, _t: &TypeAlias<'ast>) {}

    // ── Statements / blocks ─────────────────────────────────────────
    fn visit_block(&mut self, b: &Block<'ast>) { walk_block(self, b) }
    fn visit_stmt(&mut self, s: &'ast Stmt<'ast>) { walk_stmt(self, s) }

    // ── Expressions ──────────────────────────────────────────────────
    fn visit_expr(&mut self, e: &'ast Expr<'ast>) { walk_expr(self, e) }
}

// ── Declaration-level walkers ────────────────────────────────────────

pub fn walk_program<'ast, V: AstVisitor<'ast> + ?Sized>(v: &mut V, p: &Program<'ast>) {
    for item in p.items { v.visit_item(item); }
}

pub fn walk_item<'ast, V: AstVisitor<'ast> + ?Sized>(v: &mut V, item: &Item<'ast>) {
    match item {
        Item::Function(f)  => v.visit_function_decl(f),
        Item::Struct(s)    => v.visit_struct_decl(s),
        Item::Enum(e)      => v.visit_enum_decl(e),
        Item::Trait(t)     => v.visit_trait_decl(t),
        Item::Impl(i)      => v.visit_impl_block(i),
        Item::Extend(e)    => v.visit_extend_decl(e),
        Item::Const(c)     => v.visit_const_decl(c),
        Item::TypeAlias(a) => v.visit_type_alias(a),
    }
}

pub fn walk_function_decl<'ast, V: AstVisitor<'ast> + ?Sized>(v: &mut V, f: &FunctionDecl<'ast>) {
    for p in f.params { walk_param_kind(v, &p.kind); }
    v.visit_block(&f.body);
}

pub fn walk_method_decl<'ast, V: AstVisitor<'ast> + ?Sized>(v: &mut V, m: &MethodDecl<'ast>) {
    for p in m.params { walk_param_kind(v, &p.kind); }
    v.visit_block(&m.body);
}

fn walk_param_kind<'ast, V: AstVisitor<'ast> + ?Sized>(v: &mut V, k: &ParamKind<'ast>) {
    if let ParamKind::Named { default: Some(d), .. } = k { v.visit_expr(d); }
}

pub fn walk_struct_decl<'ast, V: AstVisitor<'ast> + ?Sized>(v: &mut V, s: &StructDecl<'ast>) {
    for m in s.members {
        match m {
            StructMember::Method(md) => v.visit_method_decl(md),
            StructMember::Property(p) => {
                v.visit_block(&p.getter);
                if let Some(setter) = &p.setter { v.visit_block(setter); }
            }
            StructMember::Field(_) => {}
        }
    }
}

pub fn walk_enum_decl<'ast, V: AstVisitor<'ast> + ?Sized>(v: &mut V, e: &EnumDecl<'ast>) {
    for variant in e.variants {
        if let EnumVariantPayload::Discriminant(expr) = &variant.payload {
            v.visit_expr(expr);
        }
    }
}

pub fn walk_trait_decl<'ast, V: AstVisitor<'ast> + ?Sized>(v: &mut V, t: &TraitDecl<'ast>) {
    for item in t.items {
        if let TraitItem::DefaultMethod(m) = item { v.visit_method_decl(m); }
    }
}

pub fn walk_impl_block<'ast, V: AstVisitor<'ast> + ?Sized>(v: &mut V, i: &ImplBlock<'ast>) {
    for m in i.methods { v.visit_method_decl(m); }
}

pub fn walk_extend_decl<'ast, V: AstVisitor<'ast> + ?Sized>(v: &mut V, e: &ExtendDecl<'ast>) {
    for m in e.methods { v.visit_method_decl(m); }
}

pub fn walk_const_decl<'ast, V: AstVisitor<'ast> + ?Sized>(v: &mut V, c: &ConstDecl<'ast>) {
    v.visit_expr(c.value);
}

// ── Block / statement walkers ────────────────────────────────────────

pub fn walk_block<'ast, V: AstVisitor<'ast> + ?Sized>(v: &mut V, b: &Block<'ast>) {
    for stmt in b.stmts { v.visit_stmt(stmt); }
}

pub fn walk_stmt<'ast, V: AstVisitor<'ast> + ?Sized>(v: &mut V, s: &'ast Stmt<'ast>) {
    match &s.kind {
        StmtKind::Let { value, .. }   => v.visit_expr(value),
        StmtKind::Expr(e)             => v.visit_expr(e),
        StmtKind::Return(e)           => { if let Some(e) = e { v.visit_expr(e); } }
        StmtKind::Fail(e)             => v.visit_expr(e),
        StmtKind::If(if_expr)         => walk_if_expr(v, if_expr),
        StmtKind::Match { scrutinee, arms } => {
            v.visit_expr(scrutinee);
            for arm in *arms { walk_match_arm(v, arm); }
        }
        StmtKind::For { iter, body, .. } => {
            v.visit_expr(iter);
            v.visit_block(body);
        }
        StmtKind::While { condition, body } => {
            v.visit_expr(condition);
            v.visit_block(body);
        }
        StmtKind::Loop(body)          => v.visit_block(body),
        StmtKind::Break(e)            => { if let Some(e) = e { v.visit_expr(e); } }
        StmtKind::Continue            => {}
        StmtKind::With { allocator, body } => {
            walk_allocator_kind(v, allocator);
            v.visit_block(body);
        }
        StmtKind::Using { bindings, body } => {
            for b in *bindings { v.visit_expr(b.value); }
            v.visit_block(body);
        }
        StmtKind::Extract { value, .. } => v.visit_expr(value),
        StmtKind::Defer(e)            => v.visit_expr(e),
        StmtKind::Try { body, catch_body, .. } => {
            v.visit_block(body);
            if let Some(cb) = catch_body { v.visit_block(cb); }
        }
        StmtKind::Unsafe(body)        => v.visit_block(body),
    }
}

fn walk_allocator_kind<'ast, V: AstVisitor<'ast> + ?Sized>(v: &mut V, a: &AllocatorKind<'ast>) {
    match a {
        AllocatorKind::Arena(SizeExpr::Expr(e)) => v.visit_expr(e),
        AllocatorKind::Arena(SizeExpr::WithUnit { .. }) => {}
        AllocatorKind::Pool { count, .. } => v.visit_expr(count),
        AllocatorKind::Gc | AllocatorKind::Heap => {}
    }
}

// ── Expression-internal walkers ──────────────────────────────────────

fn walk_if_expr<'ast, V: AstVisitor<'ast> + ?Sized>(v: &mut V, ie: &IfExpr<'ast>) {
    v.visit_expr(ie.condition);
    walk_if_branch_body(v, &ie.then_body);
    for elif in ie.elif_branches { walk_elif_branch(v, elif); }
    if let Some(eb) = &ie.else_body { walk_if_branch_body(v, eb); }
}

fn walk_elif_branch<'ast, V: AstVisitor<'ast> + ?Sized>(v: &mut V, e: &ElifBranch<'ast>) {
    v.visit_expr(e.condition);
    walk_if_branch_body(v, &e.body);
}

fn walk_if_branch_body<'ast, V: AstVisitor<'ast> + ?Sized>(v: &mut V, b: &IfBranchBody<'ast>) {
    match b {
        IfBranchBody::Block(blk) => v.visit_block(blk),
        IfBranchBody::Expr(e)    => v.visit_expr(e),
    }
}

fn walk_match_expr<'ast, V: AstVisitor<'ast> + ?Sized>(v: &mut V, m: &MatchExpr<'ast>) {
    v.visit_expr(m.scrutinee);
    for arm in m.arms { walk_match_arm(v, arm); }
}

fn walk_match_arm<'ast, V: AstVisitor<'ast> + ?Sized>(v: &mut V, arm: &MatchArm<'ast>) {
    if let Some(guard) = arm.guard { v.visit_expr(guard); }
    match &arm.body {
        MatchArmBody::Expr(e)  => v.visit_expr(e),
        MatchArmBody::Block(b) => v.visit_block(b),
    }
}

/// The main expression walker — every `ExprKind` variant, exhaustively.
/// This is the one hand-rolled `facts.rs` had before the retrofit; same
/// coverage, now shared.
pub fn walk_expr<'ast, V: AstVisitor<'ast> + ?Sized>(v: &mut V, e: &'ast Expr<'ast>) {
    match &e.kind {
        ExprKind::Lit(_) | ExprKind::Ident(_) | ExprKind::SelfExpr => {}

        ExprKind::BinOp { lhs, rhs, .. } => { v.visit_expr(lhs); v.visit_expr(rhs); }
        ExprKind::UnaryOp { operand, .. } => v.visit_expr(operand),
        ExprKind::Assign { target, value, .. } => { v.visit_expr(target); v.visit_expr(value); }
        ExprKind::Pipe { left, right } => { v.visit_expr(left); v.visit_expr(right); }

        ExprKind::Call { callee, args } => {
            v.visit_expr(callee);
            for a in *args { walk_arg_kind(v, &a.kind); }
        }
        ExprKind::Field { target, .. } => v.visit_expr(target),
        ExprKind::Index { target, index } => { v.visit_expr(target); v.visit_expr(index); }
        ExprKind::OptionalChain { target, access } => {
            v.visit_expr(target);
            if let OptionalAccess::Method { args, .. } = access {
                for a in *args { walk_arg_kind(v, &a.kind); }
            }
        }

        ExprKind::Try(inner) | ExprKind::Await(inner) | ExprKind::Deref(inner) => v.visit_expr(inner),
        ExprKind::Borrow { place, .. } => v.visit_expr(place),

        ExprKind::Tuple(items) | ExprKind::Array(items) => {
            for it in *items { v.visit_expr(it); }
        }
        ExprKind::Dict(entries) => {
            for entry in *entries { v.visit_expr(entry.key); v.visit_expr(entry.value); }
        }
        ExprKind::AnonObject(fields) => {
            for f in *fields { v.visit_expr(f.value); }
        }
        ExprKind::StructLit { fields, .. } => {
            for f in *fields { v.visit_expr(f.value); }
        }

        ExprKind::Lambda(lambda) => {
            for p in lambda.params {
                if let Some(_ty) = p.ty { /* Type has no Expr children — nothing to visit */ }
            }
            match &lambda.body {
                LambdaBody::Block(b) => v.visit_block(b),
                LambdaBody::Expr(e)  => v.visit_expr(e),
            }
        }
        ExprKind::Block(b) => v.visit_block(b),
        ExprKind::If(if_expr) => walk_if_expr(v, if_expr),
        ExprKind::Match(m) => walk_match_expr(v, m),

        ExprKind::OrElse { expr, fallback } => {
            v.visit_expr(expr);
            match fallback {
                OrElseFallback::Expr(e) => v.visit_expr(e),
                OrElseFallback::Return(Some(e)) => v.visit_expr(e),
                OrElseFallback::Return(None) | OrElseFallback::Continue | OrElseFallback::Break => {}
            }
        }
        ExprKind::As { expr, .. } => v.visit_expr(expr),
        ExprKind::ShortDecl { value, .. } => v.visit_expr(value),
    }
}

pub(crate) fn walk_arg_kind<'ast, V: AstVisitor<'ast> + ?Sized>(v: &mut V, a: &ArgKind<'ast>) {
    match a {
        ArgKind::Positional(e)    => v.visit_expr(e),
        ArgKind::Named { value, .. } => v.visit_expr(value),
    }
}

// ── Tests ────────────────────────────────────────────────────────────
//
// Same self-contained, hand-built-AST approach as cfg.rs/facts.rs — real
// AST fragments via the arena, run through the real trait, asserted on
// real counts. A visitor that silently skips a node kind is exactly the
// bug class this module exists to close off, so these count nodes rather
// than just checking "did it panic."

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::arena::AstArena;
    use crate::ast::common::{BinOp, Span, TierAnnotation, Visibility};
    use crate::ast::declarations::{FunctionDecl, ImplBlock, MethodDecl, Param, ParamKind};
    use crate::ast::literals::Literal;
    use crate::ast::statements::{BindingTarget, StmtKind};
    use crate::ast::types::{Type, TypeKind};

    const Z: Span = Span { start: 0, end: 0, line: 0, column: 0 };

    fn ident<'a>(arena: &'a AstArena, name: &'a str) -> &'a Expr<'a> {
        arena.alloc(Expr { kind: ExprKind::Ident(arena.alloc_str(name)), span: Z })
    }
    fn lit_int<'a>(arena: &'a AstArena, n: i64) -> &'a Expr<'a> {
        arena.alloc(Expr { kind: ExprKind::Lit(Literal::Int(n)), span: Z })
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
    fn block<'a>(arena: &'a AstArena, stmts: &[Stmt<'a>]) -> Block<'a> {
        Block { stmts: arena.alloc_slice_copy(stmts), span: Z }
    }
    fn func<'a>(arena: &'a AstArena, body: Block<'a>) -> FunctionDecl<'a> {
        FunctionDecl {
            tier: TierAnnotation::default(), attributes: &[], visibility: Visibility::default(),
            is_async: false, name: arena.alloc_str("f"), lifetime_params: &[], generic_params: &[],
            params: &[], return_type: None, body, span: Z,
        }
    }

    /// Counts every `Expr`/`Stmt` node it's shown — used to prove the
    /// default `walk_*` chain actually reaches every node, not just that
    /// it doesn't panic.
    #[derive(Default)]
    struct Counter { exprs: usize, stmts: usize }
    impl<'ast> AstVisitor<'ast> for Counter {
        fn visit_expr(&mut self, e: &'ast Expr<'ast>) { self.exprs += 1; walk_expr(self, e); }
        fn visit_stmt(&mut self, s: &'ast Stmt<'ast>) { self.stmts += 1; walk_stmt(self, s); }
    }

    #[test]
    fn counts_every_expr_in_a_binop_tree() {
        let arena = AstArena::new();
        // (1 + 2) * 3 — 3 leaves + 2 BinOps = 5 expr nodes.
        let one = lit_int(&arena, 1);
        let two = lit_int(&arena, 2);
        let three = lit_int(&arena, 3);
        let sum = arena.alloc(Expr { kind: ExprKind::BinOp { op: BinOp::Add, lhs: one, rhs: two }, span: Z });
        let product = arena.alloc(Expr { kind: ExprKind::BinOp { op: BinOp::Mul, lhs: sum, rhs: three }, span: Z });

        let mut c = Counter::default();
        c.visit_expr(product);
        assert_eq!(c.exprs, 5);
    }

    #[test]
    fn walks_into_call_arguments() {
        let arena = AstArena::new();
        let callee = ident(&arena, "f");
        let a = ident(&arena, "a");
        let b = ident(&arena, "b");
        let args: &[crate::ast::expressions::Arg] = arena.alloc_slice_copy(&[
            crate::ast::expressions::Arg { kind: ArgKind::Positional(a), span: Z },
            crate::ast::expressions::Arg { kind: ArgKind::Positional(b), span: Z },
        ]);
        let call = arena.alloc(Expr { kind: ExprKind::Call { callee, args }, span: Z });

        let mut c = Counter::default();
        c.visit_expr(call);
        assert_eq!(c.exprs, 4, "callee + 2 args + the call itself");
    }

    #[test]
    fn walks_into_if_condition_and_both_branches() {
        let arena = AstArena::new();
        let cond = ident(&arena, "cond");
        let then_b = block(&arena, &[let_stmt(&arena, "a", lit_int(&arena, 1))]);
        let else_b = block(&arena, &[let_stmt(&arena, "b", lit_int(&arena, 2))]);
        let if_expr = arena.alloc(IfExpr {
            condition: cond,
            then_body: IfBranchBody::Block(then_b),
            elif_branches: &[],
            else_body: Some(IfBranchBody::Block(else_b)),
            span: Z,
        });
        let stmt = Stmt { kind: StmtKind::If(if_expr), span: Z };

        let mut c = Counter::default();
        c.visit_stmt(arena.alloc(stmt));
        // 1 If-stmt + 1 condition + (1 let-stmt + 1 value expr) * 2 branches
        assert_eq!(c.stmts, 3, "the if itself + one let per branch");
        assert_eq!(c.exprs, 3, "condition + one literal per branch");
    }

    #[test]
    fn shallow_visitor_via_no_op_visit_block_does_not_descend_into_bodies() {
        // The exact trick facts.rs's retrofit relies on: overriding
        // visit_block to a no-op should visit an if's condition but NOT
        // its branch bodies' statements.
        struct Shallow { exprs: usize, stmts_seen_in_bodies: usize }
        impl<'ast> AstVisitor<'ast> for Shallow {
            fn visit_expr(&mut self, e: &'ast Expr<'ast>) { self.exprs += 1; walk_expr(self, e); }
            fn visit_block(&mut self, _b: &Block<'ast>) { /* no-op: don't descend */ }
            fn visit_stmt(&mut self, s: &'ast Stmt<'ast>) {
                self.stmts_seen_in_bodies += 1;
                walk_stmt(self, s);
            }
        }

        let arena = AstArena::new();
        let cond = ident(&arena, "cond");
        let then_b = block(&arena, &[let_stmt(&arena, "a", lit_int(&arena, 1))]);
        let if_expr = arena.alloc(IfExpr {
            condition: cond, then_body: IfBranchBody::Block(then_b),
            elif_branches: &[], else_body: None, span: Z,
        });
        let stmt = arena.alloc(Stmt { kind: StmtKind::If(if_expr), span: Z });

        let mut v = Shallow { exprs: 0, stmts_seen_in_bodies: 0 };
        v.visit_stmt(stmt);
        assert_eq!(v.stmts_seen_in_bodies, 1, "only the if-statement itself — not the let inside its body");
        assert_eq!(v.exprs, 1, "only the condition — visit_block being a no-op stops the body's let from ever being reached");
    }

    #[test]
    fn walks_program_through_item_function_block_to_expr() {
        let arena = AstArena::new();
        let body = block(&arena, &[let_stmt(&arena, "x", lit_int(&arena, 42))]);
        let decl = func(&arena, body);
        let items: &[Item] = arena.alloc_slice_copy(&[Item::Function(decl)]);
        let program = Program { package: None, imports: &[], items, span: Z };

        let mut c = Counter::default();
        c.visit_program(&program);
        assert_eq!(c.stmts, 1);
        assert_eq!(c.exprs, 1, "the literal 42 — proves the full Program -> Item -> FunctionDecl -> Block chain reaches real expressions");
    }

    #[test]
    fn walks_impl_block_methods() {
        let arena = AstArena::new();
        let body = block(&arena, &[let_stmt(&arena, "x", lit_int(&arena, 1))]);
        let method = MethodDecl {
            attributes: &[], tier: TierAnnotation::default(), visibility: Visibility::default(),
            is_async: false, name: arena.alloc_str("m"), generic_params: &[], params: &[],
            return_type: None, body, span: Z,
        };
        let ty = arena.alloc(Type { kind: TypeKind::Named { path: &["Foo"], args: &[] }, span: Z });
        let methods: &[MethodDecl] = arena.alloc_slice_copy(&[method]);
        let impl_block = ImplBlock {
            attributes: &[], tier: None, trait_path: None, target_type: ty, methods, span: Z,
        };

        let mut c = Counter::default();
        c.visit_impl_block(&impl_block);
        assert_eq!(c.stmts, 1);
        assert_eq!(c.exprs, 1, "proves Item::Impl's methods are actually reached, not just structs skipped over");
    }

    #[test]
    fn param_default_value_is_visited() {
        let arena = AstArena::new();
        let default_val = lit_int(&arena, 7);
        let param = Param {
            kind: ParamKind::Named { mutable: false, name: "x", ty: None, default: Some(default_val) },
            span: Z,
        };
        let params: &[Param] = arena.alloc_slice_copy(&[param]);
        let body = block(&arena, &[]);
        let decl = FunctionDecl {
            tier: TierAnnotation::default(), attributes: &[], visibility: Visibility::default(),
            is_async: false, name: arena.alloc_str("f"), lifetime_params: &[], generic_params: &[],
            params, return_type: None, body, span: Z,
        };

        let mut c = Counter::default();
        c.visit_function_decl(&decl);
        assert_eq!(c.exprs, 1, "a parameter's default value expression must be reachable too");
    }
}
