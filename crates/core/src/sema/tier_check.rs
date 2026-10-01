// src/sema/tier_check.rs
//! Pass 3 — Tier Rule Enforcement.
//!
//! Runs after name_resolution (Pass 1) and type_infer (Pass 2).
//! All identifier uses already have DefIds; all expressions already have TypeIds.
//! This pass simply enforces the tier rules by walking the AST and
//! consulting the symbol table (for callee tiers).
//!
//! ArenaRef/PoolRef escape detection (assignment / struct-field / indexed
//! storage / closure capture — MEMORY_MODEL.md §6 "Gap 2", extended to
//! pool in §11) is *not* done here. It's enforced eagerly in
//! type_infer.rs's Pass 2 instead, at the point each `Assign` expression
//! (and each `unify` call) is processed — Pass 2 already tracks live
//! arena/pool scope via `arena_stack`/`pool_stack` and
//! `maybe_arena_ref`/`current_pool`, so re-deriving that here in a
//! second AST walk would just be duplicated bookkeeping for the same
//! answer. See `InferCtx::check_assign_arena_escape` and
//! `InferCtx::scope_mismatch_side`.
//!
//! Builtin instance-method HIGH-only rejection (MEMORY_MODEL.md §8) is
//! *also* not done here, for a related reason: it needs to resolve a
//! receiver expression's type, and `expr_types` entries can still be
//! raw unresolved `Var`s at record time — resolving them correctly
//! needs the live `Unifier`/`apply()` Pass 2 has and this pass doesn't.
//! See `InferCtx::current_tier` and the `ExprKind::Call` handling in
//! `type_infer.rs`.
//!
//! # Rules enforced
//!
//! - `async fn` must be `@tier(high)`
//! - `@tier(mid)` function return type must not contain ArenaRef
//! - `with arena(…)` only inside `@tier(mid)`
//! - `await` only inside `@tier(high)`
//! - LINQ query syntax only inside `@tier(high)`
//! - `@tier(mid)` → `@tier(high)` call: forbidden
//! - `@tier(low)` → `@tier(high)` call: forbidden
//! - `@tier(low)` → `@tier(mid)` call: forbidden
//! - a `static` may only be read or written from `@tier(high)` code
//!   (`TIER-015`)

#![allow(dead_code)]

use crate::ast::common::{TierAnnotation, Span};
use crate::ast::declarations::{
    FunctionDecl, MethodDecl, StructDecl, StructMember, TraitItem,
};
use crate::ast::expressions::{
    ArgKind, Expr, ExprKind, IfBranchBody, LambdaBody,
    MatchArmBody, OrElseFallback,
};
use crate::ast::literals::{InterpolationPart, Literal};
use crate::ast::root::{Item, Program};
use crate::ast::statements::{AllocatorKind, Block, Stmt, StmtKind};
use crate::error_management::{ErrorManager, errors::TierError};
use crate::sema::sema_context::SemaContext;
use crate::sema::symbol_table::{DefId, DefKind};
use crate::sema::type_table::SemaType;

// ── Entry point ───────────────────────────────────────────────────

pub fn check<'ast>(
    program: &Program<'ast>,
    ctx:     &SemaContext,
    errors:  &mut ErrorManager,
) {
    let mut checker = TierChecker::new(ctx, errors);
    checker.check_program(program);
}

// ── Checker ───────────────────────────────────────────────────────

struct TierChecker<'a> {
    ctx:          &'a SemaContext,
    errors:       &'a mut ErrorManager,
    current_tier: TierAnnotation,
}

impl<'a> TierChecker<'a> {
    fn new(ctx: &'a SemaContext, errors: &'a mut ErrorManager) -> Self {
        TierChecker { ctx, errors, current_tier: TierAnnotation::High }
    }

    // ── Top level ─────────────────────────────────────────────────

    fn check_program<'ast>(&mut self, program: &Program<'ast>) {
        for item in program.items {
            self.check_item(item);
        }
    }

    fn check_item<'ast>(&mut self, item: &Item<'ast>) {
        match item {
            Item::Function(f) => self.check_function(f),
            Item::Struct(s)   => self.check_struct(s),
            Item::Impl(i) => {
                for m in i.methods { self.check_method(m); }
            }
            Item::Extend(x) => {
                for m in x.methods { self.check_method(m); }
            }
            Item::Trait(t) => {
                for it in t.items {
                    if let TraitItem::DefaultMethod(m) = it {
                        self.check_method(m);
                    }
                }
            }
            // A static's initializer runs at startup in the HIGH tier, so
            // it may read other statics; check it as HIGH code.
            Item::Static(s) => {
                let prev = self.current_tier;
                self.current_tier = TierAnnotation::High;
                self.check_expr(s.value);
                self.current_tier = prev;
            }
            _ => {}
        }
    }

    fn check_function<'ast>(&mut self, f: &FunctionDecl<'ast>) {
        // Rule: async functions must be HIGH tier.
        if f.is_async && f.tier != TierAnnotation::High {
            self.errors.add_tier_error(TierError::AsyncFunctionNotHigh {
                actual: f.tier,
                span:   f.span,
            });
        }

        // Rule: MID-tier return type must not contain an ArenaRef.
        if f.tier == TierAnnotation::Mid {
            self.check_mid_return_type(f.name, f.span);
        }

        let prev = self.current_tier;
        self.current_tier = f.tier;
        self.check_block(&f.body);
        self.current_tier = prev;
    }

    fn check_method<'ast>(&mut self, m: &MethodDecl<'ast>) {
        if m.is_async && m.tier != TierAnnotation::High {
            self.errors.add_tier_error(TierError::AsyncFunctionNotHigh {
                actual: m.tier,
                span:   m.span,
            });
        }
        let prev = self.current_tier;
        self.current_tier = m.tier;
        self.check_block(&m.body);
        self.current_tier = prev;
    }

    fn check_struct<'ast>(&mut self, s: &StructDecl<'ast>) {
        for member in s.members {
            if let StructMember::Method(m) = member {
                self.check_method(m);
            }
        }
    }

    /// For a MID-tier function named `name`, look up its function TypeId and
    /// check that the return type doesn't contain an ArenaRef or PoolRef.
    fn check_mid_return_type(&mut self, name: &str, span: Span) {
        let Some(def_id) = self.ctx.top_level.get(name).copied() else { return; };
        let Some(fn_ty)  = self.ctx.def_types.get(&def_id).copied() else { return; };

        // Extract the return TypeId without holding a borrow on ctx.types.
        let ret_ty = match self.ctx.types.get(fn_ty) {
            SemaType::Function { return_type, .. } => *return_type,
            _ => return,
        };

        // Now safe to call scope_ref_kind with a fresh borrow.
        match self.ctx.types.get(ret_ty).scope_ref_kind(&self.ctx.types) {
            Some(crate::sema::type_table::ScopeKind::Arena) => {
                let display = self.ctx.types.get(ret_ty).display(&self.ctx.types, &self.ctx.symbols);
                self.errors.add_tier_error(TierError::MidReturnContainsArenaRef {
                    return_type: display,
                    span,
                });
            }
            Some(crate::sema::type_table::ScopeKind::Pool) => {
                let display = self.ctx.types.get(ret_ty).display(&self.ctx.types, &self.ctx.symbols);
                self.errors.add_tier_error(TierError::MidReturnContainsPoolRef {
                    return_type: display,
                    span,
                });
            }
            None => {}
        }
    }

    // ── Block / Statement ─────────────────────────────────────────

    fn check_block<'ast>(&mut self, block: &Block<'ast>) {
        for stmt in block.stmts {
            self.check_stmt(stmt);
        }
    }

    /// Check an `if`/`elif`/`else` branch body — either a `{ block }`
    /// or the single-line `then expr` form. Mirrors the `MatchArmBody`
    /// dispatch used elsewhere in this file.
    fn check_if_branch_body<'ast>(&mut self, body: &IfBranchBody<'ast>) {
        match body {
            IfBranchBody::Expr(e)  => self.check_expr(e),
            IfBranchBody::Block(b) => self.check_block(b),
        }
    }

    fn check_stmt<'ast>(&mut self, stmt: &Stmt<'ast>) {
        match &stmt.kind {
            // Rule: `with arena(…)` / `with pool<T>(count)` only in MID
            // tier — MEMORY_MODEL.md §11: pool follows "the same
            // relationship `with arena(...)` already has."
            StmtKind::With { allocator, body } => {
                match allocator {
                    AllocatorKind::Arena(_) => {
                        if self.current_tier != TierAnnotation::Mid {
                            self.errors.add_tier_error(TierError::ArenaInWrongTier {
                                actual: self.current_tier,
                                span:   stmt.span,
                            });
                        }
                    }
                    AllocatorKind::Pool { .. } => {
                        if self.current_tier != TierAnnotation::Mid {
                            self.errors.add_tier_error(TierError::PoolInWrongTier {
                                actual: self.current_tier,
                                span:   stmt.span,
                            });
                        }
                    }
                    AllocatorKind::Gc | AllocatorKind::Heap => {}
                }
                self.check_block(body);
            }

            StmtKind::Let { value, .. }    => self.check_expr(value),
            StmtKind::Expr(e)              => self.check_expr(e),
            StmtKind::Return(Some(e))      => self.check_expr(e),
            StmtKind::Return(None)         => {}
            StmtKind::Fail(e)              => self.check_expr(e),
            StmtKind::Break(Some(e))       => self.check_expr(e),
            StmtKind::Break(None)          => {}
            StmtKind::Continue             => {}
            StmtKind::Defer(e)             => self.check_expr(e),

            StmtKind::If(if_node) => {
                self.check_expr(if_node.condition);
                self.check_if_branch_body(&if_node.then_body);
                for elif in if_node.elif_branches {
                    self.check_expr(elif.condition);
                    self.check_if_branch_body(&elif.body);
                }
                if let Some(else_b) = &if_node.else_body {
                    self.check_if_branch_body(else_b);
                }
            }

            StmtKind::Match { scrutinee, arms } => {
                self.check_expr(scrutinee);
                for arm in arms.iter() {
                    if let Some(g) = arm.guard { self.check_expr(g); }
                    match &arm.body {
                        MatchArmBody::Expr(e)  => self.check_expr(e),
                        MatchArmBody::Block(b) => self.check_block(b),
                    }
                }
            }

            StmtKind::For { iter, body, .. } => {
                self.check_expr(iter);
                self.check_block(body);
            }

            StmtKind::While { condition, body } => {
                self.check_expr(condition);
                self.check_block(body);
            }

            StmtKind::Loop(body)  => self.check_block(body),

            StmtKind::Using { bindings, body } => {
                for b in bindings.iter() { self.check_expr(b.value); }
                self.check_block(body);
            }

            StmtKind::Extract { value, .. } => self.check_expr(value),

            StmtKind::Try { body, catch_body, .. } => {
                self.check_block(body);
                if let Some(cb) = catch_body { self.check_block(cb); }
            }

            StmtKind::Unsafe(body) => self.check_block(body),
        }
    }

    // ── Expressions ───────────────────────────────────────────────

    fn check_expr<'ast>(&mut self, expr: &Expr<'ast>) {
        match &expr.kind {
            // Rule: `await` only in HIGH tier.
            ExprKind::Await(inner) => {
                if self.current_tier != TierAnnotation::High {
                    self.errors.add_tier_error(TierError::AwaitInWrongTier {
                        actual: self.current_tier,
                        span:   expr.span,
                    });
                }
                self.check_expr(inner);
            }

            // Rule: cross-tier call restrictions.
            ExprKind::Call { callee, args } => {
                self.check_expr(callee);
                // Look up callee's DefId from the resolution map using the
                // callee expression's span, then check its tier.
                if let Some(callee_def_id) = self.ctx.resolutions.get(callee.span) {
                    self.check_callee_tier(callee_def_id, expr.span);
                }
                for arg in args.iter() {
                    match &arg.kind {
                        ArgKind::Positional(e)       => self.check_expr(e),
                        ArgKind::Named { value, .. } => self.check_expr(value),
                    }
                }
            }

            // ── Leaf / structural traversal ────────────────────────
            // Reading or writing a static (an assignment target is an
            // ordinary `Ident` expression) is HIGH-tier only.
            ExprKind::Ident(name) => {
                if self.current_tier != TierAnnotation::High {
                    if let Some(id) = self.ctx.resolutions.get(expr.span) {
                        if matches!(self.ctx.symbols.lookup(id).kind, DefKind::Static) {
                            self.errors.add_tier_error(TierError::StaticAccessOutsideHigh {
                                name:   name.to_string(),
                                actual: self.current_tier,
                                span:   expr.span,
                            });
                        }
                    }
                }
            }
            // Interpolation holes are ordinary expressions (`$"{COUNTER}"`
            // reads a static), and were not visited before.
            ExprKind::Lit(Literal::InterpolatedStr(parts))
            | ExprKind::Lit(Literal::InterpolatedVerbatimStr(parts)) => {
                for part in parts.iter() {
                    if let InterpolationPart::Expr { expr, .. } = part {
                        self.check_expr(expr);
                    }
                }
            }
            ExprKind::Lit(_) | ExprKind::SelfExpr => {}
            ExprKind::ShortDecl { value, .. } => self.check_expr(value),

            ExprKind::BinOp { lhs, rhs, .. } => {
                self.check_expr(lhs);
                self.check_expr(rhs);
            }
            ExprKind::UnaryOp { operand, .. } => self.check_expr(operand),
            ExprKind::Assign { target, value, .. } => {
                self.check_expr(target);
                self.check_expr(value);
            }
            ExprKind::Pipe { left, right } => {
                self.check_expr(left);
                self.check_expr(right);
            }
            ExprKind::Field { target, .. }
            | ExprKind::OptionalChain { target, .. } => self.check_expr(target),
            ExprKind::Index { target, index } => {
                self.check_expr(target);
                self.check_expr(index);
            }
            ExprKind::Try(inner) | ExprKind::Deref(inner) => self.check_expr(inner),
            // No tier restriction on the operator itself — references
            // are valid in every tier (a HIGH/MID function can safely
            // hand out a read-only view too). What LOW tier actually
            // gates is *enforcement* (move/loan checking), which is a
            // separate, still-unbuilt pass — not this one.
            ExprKind::Borrow { place, .. } => self.check_expr(place),
            ExprKind::As { expr: inner, .. } => self.check_expr(inner),
            ExprKind::Tuple(elems) | ExprKind::Array(elems) => {
                for e in elems.iter() { self.check_expr(e); }
            }
            ExprKind::Dict(entries) => {
                for entry in entries.iter() {
                    self.check_expr(entry.key);
                    self.check_expr(entry.value);
                }
            }
            ExprKind::AnonObject(fields) => {
                for f in fields.iter() { self.check_expr(f.value); }
            }
            ExprKind::StructLit { fields, .. } => {
                for f in fields.iter() { self.check_expr(f.value); }
            }
            ExprKind::Lambda(lambda) => {
                match &lambda.body {
                    LambdaBody::Block(b) => self.check_block(b),
                    LambdaBody::Expr(e)  => self.check_expr(e),
                }
            }
            ExprKind::Block(b) => self.check_block(b),
            ExprKind::If(if_node) => {
                self.check_expr(if_node.condition);
                self.check_if_branch_body(&if_node.then_body);
                for elif in if_node.elif_branches {
                    self.check_expr(elif.condition);
                    self.check_if_branch_body(&elif.body);
                }
                if let Some(else_b) = &if_node.else_body {
                    self.check_if_branch_body(else_b);
                }
            }
            ExprKind::Match(m) => {
                self.check_expr(m.scrutinee);
                for arm in m.arms.iter() {
                    if let Some(g) = arm.guard { self.check_expr(g); }
                    match &arm.body {
                        MatchArmBody::Expr(e)  => self.check_expr(e),
                        MatchArmBody::Block(b) => self.check_block(b),
                    }
                }
            }
            ExprKind::OrElse { expr: inner, fallback } => {
                self.check_expr(inner);
                if let OrElseFallback::Expr(fb) = fallback {
                    self.check_expr(fb);
                }
            }
        }
    }

    /// Look up a callee's tier from the symbol table and enforce cross-tier rules.
    /// Uses bounds-checked access to avoid panicking on DefId::INVALID.
    fn check_callee_tier(&mut self, callee_def_id: DefId, call_span: Span) {
        // DefId::INVALID.0 == usize::MAX, which is always >= symbols.len().
        if callee_def_id.0 >= self.ctx.symbols.len() {
            return; // unresolved or builtin — skip
        }
        let callee_def  = self.ctx.symbols.lookup(callee_def_id);
        let callee_tier = match &callee_def.kind {
            DefKind::Function { tier, .. } => *tier,
            DefKind::Method   { tier, .. } => *tier,
            _ => return, // not a callable def kind
        };
        let callee_name = callee_def.name.clone();
        self.enforce_tier_call(self.current_tier, callee_tier, &callee_name, call_span);
    }

    /// Enforce the cross-tier call matrix:
    ///
    /// | Caller | Callee | Allowed |
    /// |--------|--------|---------|
    /// | HIGH   | MID    | ✓ (callback / view patterns encouraged but not enforced here) |
    /// | HIGH   | LOW    | ✓ |
    /// | MID    | HIGH   | ✗ — would escape arena lifetime |
    /// | MID    | LOW    | ✓ |
    /// | LOW    | HIGH   | ✗ |
    /// | LOW    | MID    | ✗ |
    fn enforce_tier_call(
        &mut self,
        caller: TierAnnotation,
        callee: TierAnnotation,
        callee_name: &str,
        span: Span,
    ) {
        let forbidden = matches!(
            (caller, callee),
            (TierAnnotation::Mid, TierAnnotation::High)
            | (TierAnnotation::Low, TierAnnotation::High)
            | (TierAnnotation::Low, TierAnnotation::Mid)
        );
        if forbidden {
            self.errors.add_tier_error(TierError::IllegalTierCall {
                caller_tier: caller,
                callee_tier: callee,
                callee_name: callee_name.to_string(),
                span,
            });
        }
    }
                }
