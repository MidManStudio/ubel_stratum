// ============================================================================
// NOTICE: Full documentation, design decisions, and fix history for this file
// live in docs/ubel_stratum.md, section "sema/name_resolution.rs"
// ============================================================================
// src/sema/name_resolution.rs
//! Pass 1 — Name Resolution.
//!
//! Walks the entire `Program` AST and resolves every identifier to a `DefId`.
//! Populates `ctx.symbols` (the flat definition table) and `ctx.resolutions`
//! (the use-site → DefId map), plus `ctx.top_level` (module-scope name lookup
//! used by type_infer for type-position resolution).
//!
//! # Walk order
//!
//! 1. **Pre-declare top-level items** — collect all top-level function, struct,
//!    enum, trait, const, and type-alias names into the module scope before
//!    walking any bodies.  This lets items forward-reference each other freely.
//!
//! 2. **Walk item bodies** — for each item, push a new scope, declare parameters
//!    and generics, then walk the body resolving uses.
//!
//! Qualified paths (`std.collections.List`) are resolved by walking the path
//! segment-by-segment through the import table built from `summon` declarations.

#![allow(dead_code)]

use crate::ast::common::{Visibility, Span};
use crate::ast::root::{Program, Item, ImportKind, ImportItems};
use crate::ast::declarations::{
    FunctionDecl, StructDecl, StructMember, EnumDecl,
    TraitDecl, TraitItem, ImplBlock, ExtendDecl,
    ConstDecl, StaticDecl, TypeAlias, MethodDecl, Param, ParamKind,
};
use crate::ast::statements::{Block, Stmt, StmtKind, UsingBinding};
use crate::ast::expressions::{Expr, ExprKind};
use crate::ast::literals::{InterpolationPart, Literal};
use crate::ast::patterns::{Pattern, PatternKind, DestructurePattern, DestructureElement, EnumPatternPayload};
use crate::error_management::{ErrorManager, errors::NameError};
use crate::sema::sema_context::SemaContext;
use crate::sema::symbol_table::{DefId, DefKind, ScopeStack};

// ── Entry point ───────────────────────────────────────────────────

pub fn resolve<'ast>(
    program: &Program<'ast>,
    ctx:     &mut SemaContext,
    errors:  &mut ErrorManager,
) {
    let mut resolver = Resolver::new(ctx, errors);
    resolver.resolve_program(program);
}

// ── Resolver ─────────────────────────────────────────────────────

struct Resolver<'a> {
    ctx:    &'a mut SemaContext,
    errors: &'a mut ErrorManager,
    scopes: ScopeStack,
    /// Whether we are currently inside a method body (validates `self` use).
    in_method: bool,
    /// The `DefId` of the function / method currently being resolved.
    current_fn: Option<DefId>,
    /// Whether the expression being resolved is a `const` initializer. A
    /// `static` read there is `NAME-008`.
    in_const_initializer: bool,
}

impl<'a> Resolver<'a> {
    fn new(ctx: &'a mut SemaContext, errors: &'a mut ErrorManager) -> Self {
        let mut r = Resolver { ctx, errors, scopes: ScopeStack::new(), in_method: false, current_fn: None, in_const_initializer: false };
        // Push the module (top-level) scope.
        r.scopes.push();
        r.declare_builtins();
        r
    }

    /// Pre-declare every native builtin (`println`, `len`, `sqrt`, ...) into
    /// the module scope, using the exact same list the interpreter registers
    /// at runtime (`interpreter::builtins::all_builtins`). Deriving from that
    /// one list — instead of hand-maintaining a second copy of the names
    /// here — is what keeps sema and the interpreter from silently
    /// disagreeing about what's in scope, the way they did before this fix.
    fn declare_builtins(&mut self) {
        const NO_SPAN: Span = Span { start: 0, end: 0, line: 0, column: 0 };
        for sig in crate::builtins::global::GLOBAL_BUILTINS {
            self.declare(sig.name.to_string(), DefKind::Builtin, NO_SPAN, Visibility::Public);
        }
        // Builtin static namespaces (`Math.sqrt(x)`) — declare the namespace
        // name itself so `Ident("Math")` resolves during name resolution.
        // Member validity (`Math.frobnicate` vs a real member) isn't
        // checked here; see `builtins::namespace_member_names`.
        for ns in crate::builtins::BUILTIN_NAMESPACES {
            self.declare(ns.to_string(), DefKind::Builtin, NO_SPAN, Visibility::Public);
        }
    }

    // ── Program ───────────────────────────────────────────────────

    fn resolve_program<'ast>(&mut self, program: &Program<'ast>) {
        // 1. Process imports so qualified paths can be resolved.
        for import in program.imports {
            self.register_import(import);
        }

        // 2. Pre-declare all top-level item names (enables forward refs).
        for item in program.items {
            self.pre_declare_item(item);
        }

        // 3. Walk each item body.
        for item in program.items {
            self.resolve_item(item);
        }
    }

    // ── Import registration ───────────────────────────────────────

    fn register_import<'ast>(&mut self, import: &crate::ast::root::Import<'ast>) {
        match &import.kind {
            ImportKind::Summon { path, alias } => {
                // The local name is either the alias or the last path segment.
                let local_name = alias
                    .unwrap_or_else(|| path.last().copied().unwrap_or(""))
                    .to_string();
                let canonical: Vec<String> = path.iter().map(|s| s.to_string()).collect();
                self.declare(
                    local_name,
                    DefKind::Import { canonical_path: canonical },
                    import.span,
                    Visibility::Public,
                );
            }
            ImportKind::FromSummon { module_path, items } => {
                let module: Vec<String> = module_path.iter().map(|s| s.to_string()).collect();
                let names: Vec<&str> = match items {
                    ImportItems::Single(n) => vec![n],
                    ImportItems::List(ns)  => ns.to_vec(),
                };
                for name in names {
                    let mut canonical = module.clone();
                    canonical.push(name.to_string());
                    self.declare(
                        name.to_string(),
                        DefKind::Import { canonical_path: canonical },
                        import.span,
                        Visibility::Public,
                    );
                }
            }
        }
    }

    // ── Pre-declaration ───────────────────────────────────────────

    /// Insert a top-level item's name into the module scope without
    /// walking its body, and also record it in `ctx.top_level` so
    /// type_infer can resolve type-position names without re-walking
    /// scopes. Called before `resolve_item` so any item can reference
    /// any other item regardless of source order.
    fn pre_declare_item<'ast>(&mut self, item: &Item<'ast>) {
        match item {
            Item::Function(f) => {
                self.declare_top_level(f.name.to_string(), DefKind::Function {
                    tier: f.tier, is_async: f.is_async,
                }, f.span, f.visibility);
            }
            Item::Struct(s) => {
                self.declare_top_level(s.name.to_string(), DefKind::Struct { is_edge: s.is_edge },
                    s.span, s.visibility);
            }
            Item::Enum(e) => {
                self.declare_top_level(e.name.to_string(), DefKind::Enum, e.span, e.visibility);
            }
            Item::Trait(t) => {
                self.declare_top_level(t.name.to_string(), DefKind::Trait, t.span, t.visibility);
            }
            Item::Const(c) => {
                self.declare_top_level(c.name.to_string(), DefKind::Const, c.span, Visibility::Private);
            }
            Item::Static(s) => {
                self.declare_top_level(s.name.to_string(), DefKind::Static, s.span, s.visibility);
            }
            Item::TypeAlias(a) => {
                self.declare_top_level(a.name.to_string(), DefKind::TypeAlias, a.span, Visibility::Private);
            }
            // Impl and Extend don't introduce a name into scope.
            Item::Impl(_) | Item::Extend(_) => {}
        }
    }

    /// `declare` plus a `ctx.top_level` insert. Module-scope items only —
    /// locals, params, and fields go through plain `declare`.
    fn declare_top_level(
        &mut self,
        name:       String,
        kind:       DefKind,
        span:       Span,
        visibility: Visibility,
    ) -> DefId {
        let id = self.declare(name.clone(), kind, span, visibility);
        self.ctx.top_level.insert(name, id);
        id
    }

    // ── Item resolution ───────────────────────────────────────────

    fn resolve_item<'ast>(&mut self, item: &Item<'ast>) {
        match item {
            Item::Function(f)  => self.resolve_function(f),
            Item::Struct(s)    => self.resolve_struct(s),
            Item::Enum(e)      => self.resolve_enum(e),
            Item::Trait(t)     => self.resolve_trait(t),
            Item::Impl(i)      => self.resolve_impl(i),
            Item::Extend(x)    => self.resolve_extend(x),
            Item::Const(c)     => self.resolve_const(c),
            Item::Static(s)    => self.resolve_static(s),
            Item::TypeAlias(a) => self.resolve_type_alias(a),
        }
    }

    fn resolve_function<'ast>(&mut self, f: &FunctionDecl<'ast>) {
        let fn_id = self.scopes.resolve(f.name).unwrap_or(DefId::INVALID);
        self.scopes.push();
        let prev_fn     = self.current_fn.replace(fn_id);
        let prev_method = self.in_method;

        // Declare generic / lifetime params.
        for gp in f.generic_params {
            self.declare(gp.name.to_string(), DefKind::TypeParam { parent: fn_id },
                gp.span, Visibility::Private);
        }
        // Declare parameters.
        for param in f.params {
            self.resolve_param(param, fn_id);
        }
        // Walk body.
        self.resolve_block(&f.body);

        self.in_method  = prev_method;
        self.current_fn = prev_fn;
        self.scopes.pop();
    }

    fn resolve_method<'ast>(&mut self, m: &MethodDecl<'ast>, parent: DefId) {
        let method_id = self.ctx.symbols.insert(
            m.name.to_string(),
            DefKind::Method { parent, tier: m.tier },
            m.span,
            m.visibility,
        );
        // Record the definition site so the method can be looked up later.
        self.ctx.resolutions.record(m.span, method_id);

        self.scopes.push();
        let prev_fn     = self.current_fn.replace(method_id);
        let prev_method = self.in_method;
        self.in_method  = true;

        for gp in m.generic_params {
            self.declare(gp.name.to_string(), DefKind::TypeParam { parent: method_id },
                gp.span, Visibility::Private);
        }
        for param in m.params {
            self.resolve_param(param, method_id);
        }
        self.resolve_block(&m.body);

        self.in_method  = prev_method;
        self.current_fn = prev_fn;
        self.scopes.pop();
    }

    fn resolve_param<'ast>(&mut self, param: &Param<'ast>, parent: DefId) {
        match &param.kind {
            ParamKind::Named { name, default, .. } => {
                let id = self.declare(name.to_string(), DefKind::Param { parent },
                    param.span, Visibility::Private);
                self.ctx.resolutions.record(param.span, id);
                if let Some(default_expr) = default {
                    self.resolve_expr(default_expr);
                }
            }
            // Nothing to declare: no name, no def_id, and unlike Named
            // there's no default expression that could reference other
            // names either. Deliberately its own arm, not folded into
            // the self-family catch-all below -- a `_: Type` param on a
            // plain free function is completely unrelated to `self`,
            // and that catch-all's SelfOutsideMethod check would
            // otherwise incorrectly fire for it too.
            ParamKind::Discard { .. } => {}
            // self / &self / mut self / &mut self — only valid inside a method.
            _ => {
                if !self.in_method {
                    self.errors.add_name_error(NameError::SelfOutsideMethod { span: param.span });
                }
            }
        }
    }

    fn resolve_struct<'ast>(&mut self, s: &StructDecl<'ast>) {
        let struct_id = self.scopes.resolve(s.name).unwrap_or(DefId::INVALID);
        self.scopes.push();

        for gp in s.generic_params {
            self.declare(gp.name.to_string(), DefKind::TypeParam { parent: struct_id },
                gp.span, Visibility::Private);
        }
        for member in s.members {
            match member {
                StructMember::Field(f) => {
                    let id = self.ctx.symbols.insert(
                        f.name.to_string(),
                        DefKind::Field { parent: struct_id },
                        f.span,
                        f.visibility,
                    );
                    self.ctx.resolutions.record(f.span, id);
                }
                StructMember::Method(m)   => self.resolve_method(m, struct_id),
                StructMember::Property(_) => { /* TODO */ }
            }
        }

        self.scopes.pop();
    }

    fn resolve_enum<'ast>(&mut self, e: &EnumDecl<'ast>) {
        let enum_id = self.scopes.resolve(e.name).unwrap_or(DefId::INVALID);
        for variant in e.variants {
            let id = self.ctx.symbols.insert(
                variant.name.to_string(),
                DefKind::Variant { parent: enum_id },
                variant.span,
                Visibility::Public,
            );
            self.ctx.resolutions.record(variant.span, id);
        }
    }

    fn resolve_trait<'ast>(&mut self, t: &TraitDecl<'ast>) {
        let trait_id = self.scopes.resolve(t.name).unwrap_or(DefId::INVALID);
        self.scopes.push();
        for item in t.items {
            match item {
                TraitItem::DefaultMethod(m) => self.resolve_method(m, trait_id),
                TraitItem::MethodSig(_)     => { /* signatures have no body */ }
                TraitItem::AssociatedType { .. } => { /* TODO */ }
            }
        }
        self.scopes.pop();
    }

    fn resolve_impl<'ast>(&mut self, i: &ImplBlock<'ast>) {
        // impl blocks don't introduce a name; find the target type's DefId.
        // For now we record method definitions without a specific parent DefId.
        for method in i.methods {
            self.resolve_method(method, DefId::INVALID);
        }
    }

    fn resolve_extend<'ast>(&mut self, x: &ExtendDecl<'ast>) {
        for method in x.methods {
            self.resolve_method(method, DefId::INVALID);
        }
    }

    fn resolve_const<'ast>(&mut self, c: &ConstDecl<'ast>) {
        // A const initializer is evaluated once at startup and, when it
        // refers to a later constant, re-evaluated, so it must not read a
        // mutable global. `in_const_initializer` lets the `Ident` arm of
        // `resolve_expr` report that (`NAME-008`).
        let prev = std::mem::replace(&mut self.in_const_initializer, true);
        self.resolve_expr(c.value);
        self.in_const_initializer = prev;
    }

    fn resolve_static<'ast>(&mut self, s: &StaticDecl<'ast>) {
        self.resolve_expr(s.value);
    }

    fn resolve_type_alias<'ast>(&mut self, _a: &TypeAlias<'ast>) {
        // Type alias bodies are walked during type inference (Pass 2).
    }

    // ── Block / Statement resolution ──────────────────────────────

    fn resolve_block<'ast>(&mut self, block: &Block<'ast>) {
        self.scopes.push();
        for stmt in block.stmts {
            self.resolve_stmt(stmt);
        }
        self.scopes.pop();
    }

    /// Resolve an `if`/`elif`/`else` branch body — either a `{ block }`
    /// or the single-line `then expr` form. Mirrors the existing
    /// `MatchArmBody` dispatch below.
    fn resolve_if_branch_body<'ast>(&mut self, body: &crate::ast::expressions::IfBranchBody<'ast>) {
        match body {
            crate::ast::expressions::IfBranchBody::Expr(e)  => self.resolve_expr(e),
            crate::ast::expressions::IfBranchBody::Block(b) => self.resolve_block(b),
        }
    }

    fn resolve_stmt<'ast>(&mut self, stmt: &Stmt<'ast>) {
        match &stmt.kind {
            StmtKind::Let { mutable, binding, value, .. } => {
                // Resolve the initialiser before introducing the binding,
                // so `let x = x` reports "undefined x" correctly.
                self.resolve_expr(value);
                self.resolve_binding_target(binding, *mutable, stmt.span);
            }
            StmtKind::Expr(e)     => self.resolve_expr(e),
            StmtKind::Return(e)   => { if let Some(e) = e { self.resolve_expr(e); } }

StmtKind::Fail(e)     => self.resolve_expr(e),

StmtKind::Break(e)    => { if let Some(e) = e { self.resolve_expr(e); } }

StmtKind::Continue    => {}

StmtKind::Defer(e)    => self.resolve_expr(e),StmtKind::If(if_node) => {
            self.resolve_expr(if_node.condition);
            self.resolve_if_branch_body(&if_node.then_body);
            for elif in if_node.elif_branches {
                self.resolve_expr(elif.condition);
                self.resolve_if_branch_body(&elif.body);
            }
            if let Some(else_b) = &if_node.else_body {
                self.resolve_if_branch_body(else_b);
            }
        }

        StmtKind::Match { scrutinee, arms } => {
            self.resolve_expr(scrutinee);
            for arm in arms.iter() {
                self.scopes.push();
                // Declare any bindings introduced by the pattern before
                // checking the guard — guards can reference those bindings.
                self.resolve_pattern_bindings(&arm.pattern);
                if let Some(guard) = arm.guard { self.resolve_expr(guard); }
                match &arm.body {
                    crate::ast::expressions::MatchArmBody::Expr(e) => self.resolve_expr(e),
                    crate::ast::expressions::MatchArmBody::Block(b) => self.resolve_block(b),
                }
                self.scopes.pop();
            }
        }

        StmtKind::For { binding, iter, body } => {
            self.resolve_expr(iter);
            self.scopes.push();
            self.resolve_binding_target(binding, false, stmt.span);
            self.resolve_block(body);
            self.scopes.pop();
        }

        StmtKind::While { condition, body } => {
            self.resolve_expr(condition);
            self.resolve_block(body);
        }

        StmtKind::Loop(body) => self.resolve_block(body),

        StmtKind::With { body, .. } => self.resolve_block(body),

        StmtKind::Using { bindings, body } => {
            self.scopes.push();
            for b in bindings.iter() {
                self.resolve_using_binding(b);
            }
            self.resolve_block(body);
            self.scopes.pop();
        }

        StmtKind::Extract { pattern, value } => {
            self.resolve_expr(value);
            self.resolve_destructure(pattern, stmt.span);
        }

        StmtKind::Try { body, catch_binding, catch_body } => {
            self.resolve_block(body);
            if let Some(catch_b) = catch_body {
                self.scopes.push();
                if let Some(name) = catch_binding {
                    self.declare(name.to_string(), DefKind::Local { mutable: false },
                        stmt.span, Visibility::Private);
                }
                self.resolve_block(catch_b);
                self.scopes.pop();
            }
        }

        StmtKind::Unsafe(body) => self.resolve_block(body),
    }
}

fn resolve_using_binding<'ast>(&mut self, b: &UsingBinding<'ast>) {
    self.resolve_expr(b.value);
    let id = self.declare(b.name.to_string(),
        DefKind::Local { mutable: b.mutable }, b.span, Visibility::Private);
    self.ctx.resolutions.record(b.span, id);
}

fn resolve_binding_target<'ast>(
    &mut self,
    target: &crate::ast::statements::BindingTarget<'ast>,
    mutable: bool,
    span: Span,
) {
    use crate::ast::statements::BindingTarget;
    match target {
        BindingTarget::Ident(name) => {
            let id = self.declare(name.to_string(),
                DefKind::Local { mutable }, span, Visibility::Private);
            self.ctx.resolutions.record(span, id);
        }
        BindingTarget::Destructure(pat) => {
            self.resolve_destructure(pat, span);
        }
    }
}

fn resolve_destructure<'ast>(&mut self, pat: &DestructurePattern<'ast>, span: Span) {
    match pat {
        DestructurePattern::Ident(name) => {
            let id = self.declare(name.to_string(),
                DefKind::Local { mutable: false }, span, Visibility::Private);
            self.ctx.resolutions.record(span, id);
        }
        DestructurePattern::Tuple(t) => {
            for elem in t.elements {
                self.resolve_destructure_elem(elem, span);
            }
        }
        DestructurePattern::Array(a) => {
            for elem in a.elements {
                self.resolve_destructure_elem(elem, span);
            }
        }
        DestructurePattern::Struct(s) => {
            for field in s.fields {
                if let Some(pat) = &field.pattern {
                    self.resolve_destructure(pat, field.span);
                } else {
                    // shorthand: `{ name }` — bind `name` directly
                    let id = self.declare(field.field.to_string(),
                        DefKind::Local { mutable: false }, field.span, Visibility::Private);
                    self.ctx.resolutions.record(field.span, id);
                }
            }
        }
    }
}

fn resolve_destructure_elem<'ast>(&mut self, elem: &DestructureElement<'ast>, span: Span) {
    match elem {
        DestructureElement::Ident(name) => {
            let id = self.declare(name.to_string(),
                DefKind::Local { mutable: false }, span, Visibility::Private);
            self.ctx.resolutions.record(span, id);
        }
        DestructureElement::Wildcard => {}
        DestructureElement::Nested(p) => self.resolve_destructure(p, span),
    }
}

/// Declare names introduced by a match arm pattern into the current scope.
/// Pattern use-sites (`Status.Active`, enum paths) are NOT declarations —
/// only `PatternKind::Ident` and extract fields introduce new bindings.
fn resolve_pattern_bindings<'ast>(&mut self, pat: &Pattern<'ast>) {
    match &pat.kind {
        PatternKind::Wildcard | PatternKind::Literal(_) => {}
        PatternKind::Ident { name, mutable } => {
            let id = self.declare(name.to_string(),
                DefKind::Local { mutable: *mutable }, pat.span, Visibility::Private);
            self.ctx.resolutions.record(pat.span, id);
        }
        PatternKind::Tuple(pats) | PatternKind::Or(pats) => {
            for p in pats.iter() { self.resolve_pattern_bindings(p); }
        }
        PatternKind::Array { elements, .. } => {
            for p in elements.iter() { self.resolve_pattern_bindings(p); }
        }
        PatternKind::Struct { fields, .. } => {
            for f in fields.iter() {
                if let Some(sub_pat) = &f.pattern {
                    self.resolve_pattern_bindings(sub_pat);
                } else {
                    // shorthand `{ name }` — binds `name`
                    let id = self.declare(f.field.to_string(),
                        DefKind::Local { mutable: false }, pat.span, Visibility::Private);
                    self.ctx.resolutions.record(pat.span, id);
                }
            }
        }
        PatternKind::Enum { path, payload } => {
            // A 2+-segment path (`Direction.North`) already only
            // resolves its root here via `resolve_qual_path`, deferring
            // the variant segment to Pass 2. But a 1-segment path with a
            // payload (`Ok(value)`, `Some(x)`) ALSO parses as this same
            // PatternKind::Enum (payload is what makes it Enum instead
            // of Ident — see ENUM_RULES.md), and `resolve_qual_path`'s
            // 1-segment branch does a *strict* top-level name lookup,
            // which fails immediately for `Ok` — it's a nested variant
            // of `Result`, never a top-level declaration on its own. Skip
            // resolution entirely for the 1-segment case, deferring the
            // whole thing to Pass 2's `check_enum_pattern` (which already
            // knows the scrutinee's type and validates the variant name
            // for real) — same principle as the 2+-segment case, just
            // pushed one segment further back.
            if path.len() >= 2 {
                self.resolve_qual_path(path, pat.span);
            }
            match payload {
                EnumPatternPayload::Tuple(pats) => {
                    for p in pats.iter() { self.resolve_pattern_bindings(p); }
                }
                EnumPatternPayload::Struct(fields) => {
                    for f in fields.iter() {
                        if let Some(sub_pat) = &f.pattern {
                            self.resolve_pattern_bindings(sub_pat);
                        } else {
                            let id = self.declare(f.field.to_string(),
                                DefKind::Local { mutable: false }, pat.span, Visibility::Private);
                            self.ctx.resolutions.record(pat.span, id);
                        }
                    }
                }
                EnumPatternPayload::None => {}
            }
        }
        PatternKind::Range { .. } => {}
        PatternKind::Extract(fields) => {
            for f in fields.iter() {
                if let Some(sub_pat) = &f.pattern {
                    self.resolve_pattern_bindings(sub_pat);
                } else {
                    let id = self.declare(f.field.to_string(),
                        DefKind::Local { mutable: false }, pat.span, Visibility::Private);
                    self.ctx.resolutions.record(pat.span, id);
                }
            }
        }
    }
}

// ── Expression resolution ─────────────────────────────────────

fn resolve_expr<'ast>(&mut self, expr: &Expr<'ast>) {
    match &expr.kind {
        ExprKind::Ident(name) => {
            self.resolve_name(name, expr.span);
        }

        ExprKind::SelfExpr => {
            if !self.in_method {
                self.errors.add_name_error(NameError::SelfOutsideMethod { span: expr.span });
            }
        }

        // Interpolation holes were a complete no-op here until now --
        // `ExprKind::Lit(_) => {}` matched everything including
        // `InterpolatedStr`, so nothing inside a `{expr}` hole was ever
        // registered in `resolutions` at all. Invisible at runtime (the
        // interpreter resolves identifiers by name via `interp.lookup`,
        // entirely independent of this map -- see eval/expr.rs), but
        // real: it surfaced the moment sema's `infer_literal` started
        // actually calling `infer_expr` on hole expressions (needed for
        // format-spec validation) and got `<unknown>` back for a bound,
        // perfectly ordinary variable. Root cause was here, one layer
        // before type_infer.rs -- name resolution runs first, and it was
        // never visiting hole expressions to begin with.
        ExprKind::Lit(Literal::InterpolatedStr(parts))
        | ExprKind::Lit(Literal::InterpolatedVerbatimStr(parts)) => {
            for part in parts.iter() {
                if let InterpolationPart::Expr { expr, .. } = part {
                    self.resolve_expr(expr);
                }
            }
        }

        ExprKind::Lit(_) => {}

        // Short-decl `x := expr` — resolve rhs, then declare x.
        ExprKind::ShortDecl { name, value } => {
            self.resolve_expr(value);
            let id = self.declare(name.to_string(),
                DefKind::Local { mutable: true }, expr.span, Visibility::Private);
            self.ctx.resolutions.record(expr.span, id);
        }

        ExprKind::BinOp { lhs, rhs, .. } => {
            self.resolve_expr(lhs);
            self.resolve_expr(rhs);
        }
        ExprKind::UnaryOp { operand, .. } => self.resolve_expr(operand),
        ExprKind::Assign { target, value, .. } => {
            self.resolve_expr(target);
            self.resolve_expr(value);
            // A global `const` is never a valid assignment target. Checked
            // against the scope-resolved definition, not the bare name, so a
            // local `let` that shadows a constant's name stays assignable.
            if let ExprKind::Ident(name) = &target.kind {
                if let Some(id) = self.scopes.resolve(name) {
                    if matches!(self.ctx.symbols.lookup(id).kind, DefKind::Const) {
                        self.errors.add_name_error(NameError::AssignToConst {
                            name: name.to_string(),
                            span: target.span,
                        });
                    }
                }
            }
        }
        ExprKind::Pipe { left, right } => {
            self.resolve_expr(left);
            self.resolve_expr(right);
        }
        ExprKind::Call { callee, args } => {
            self.resolve_expr(callee);
            for arg in args.iter() {
                match &arg.kind {
                    crate::ast::expressions::ArgKind::Positional(e) => self.resolve_expr(e),
                    crate::ast::expressions::ArgKind::Named { value, .. } => self.resolve_expr(value),
                }
            }
        }
        ExprKind::Field { target, .. }
        | ExprKind::OptionalChain { target, .. } => self.resolve_expr(target),
        ExprKind::Index { target, index } => {
            self.resolve_expr(target);
            self.resolve_expr(index);
        }
        ExprKind::Try(e) | ExprKind::Await(e) | ExprKind::Deref(e) => self.resolve_expr(e),
        ExprKind::Borrow { place, .. } => self.resolve_expr(place),
        ExprKind::As { expr: e, .. } => self.resolve_expr(e),
        ExprKind::Tuple(es) | ExprKind::Array(es) => {
            for e in es.iter() { self.resolve_expr(e); }
        }
        ExprKind::Dict(entries) => {
            for entry in entries.iter() {
                self.resolve_expr(entry.key);
                self.resolve_expr(entry.value);
            }
        }
        ExprKind::AnonObject(fields) => {
            for f in fields.iter() { self.resolve_expr(f.value); }
        }
        ExprKind::StructLit { path, fields } => {
            self.resolve_qual_path(path, expr.span);
            for f in fields.iter() { self.resolve_expr(f.value); }
        }
        ExprKind::Lambda(lambda) => {
            self.scopes.push();
            for p in lambda.params.iter() {
                // FIX: record lambda params against their span so type_infer
                // can look them up when resolving Ident uses inside the body.
                let id = self.declare(p.name.to_string(),
                    DefKind::Local { mutable: false }, p.span, Visibility::Private);
                self.ctx.resolutions.record(p.span, id);
            }
            match &lambda.body {
                crate::ast::expressions::LambdaBody::Block(b) => self.resolve_block(b),
                crate::ast::expressions::LambdaBody::Expr(e)  => self.resolve_expr(e),
            }
            self.scopes.pop();
        }
        ExprKind::Block(b) => self.resolve_block(b),
        ExprKind::If(if_node) => {
            self.resolve_expr(if_node.condition);
            self.resolve_if_branch_body(&if_node.then_body);
            for elif in if_node.elif_branches {
                self.resolve_expr(elif.condition);
                self.resolve_if_branch_body(&elif.body);
            }
            if let Some(else_b) = &if_node.else_body {
                self.resolve_if_branch_body(else_b);
            }
        }
        ExprKind::Match(m) => {
            self.resolve_expr(m.scrutinee);
            for arm in m.arms.iter() {
                self.scopes.push();
                self.resolve_pattern_bindings(&arm.pattern);
                if let Some(guard) = arm.guard { self.resolve_expr(guard); }
                match &arm.body {
                    crate::ast::expressions::MatchArmBody::Expr(e) => self.resolve_expr(e),
                    crate::ast::expressions::MatchArmBody::Block(b) => self.resolve_block(b),
                }
                self.scopes.pop();
            }
        }
        ExprKind::OrElse { expr: e, fallback } => {
            self.resolve_expr(e);
            if let crate::ast::expressions::OrElseFallback::Expr(fb) = fallback {
                self.resolve_expr(fb);
            }
        }
    }
}

// ── Qualified path resolution ─────────────────────────────────

/// Resolve a dotted path like `std.io.File` or just `MyStruct`.
/// Records the resolution against `span`.
fn resolve_qual_path(&mut self, path: &[&str], span: Span) {
    if path.is_empty() { return; }

    if path.len() == 1 {
        self.resolve_name(path[0], span);
        return;
    }

    // For a multi-segment path, the root segment must be in scope.
    // Subsequent segments are field/variant accesses resolved during
    // type checking (Pass 2). We record only the root here.
    let root = path[0];
    if let Some(def_id) = self.scopes.resolve(root) {
        self.ctx.resolutions.record(span, def_id);
    } else {
        self.errors.add_name_error(NameError::UnresolvedPathSegment {
            full_path:       path.join("."),
            unresolved_at:   root.to_string(),
            resolved_so_far: String::new(),
            span,
        });
    }
}

/// Resolve a single name and record it.
fn resolve_name(&mut self, name: &str, span: Span) {
    if let Some(id) = self.scopes.resolve(name) {
        self.ctx.resolutions.record(span, id);
        if self.in_const_initializer
            && matches!(self.ctx.symbols.lookup(id).kind, DefKind::Static)
        {
            self.errors.add_name_error(NameError::StaticInConst {
                name: name.to_string(),
                span,
            });
        }
    } else {
        let suggestion = self.find_similar(name);
        self.errors.add_name_error(NameError::UndefinedName {
            name:         name.to_string(),
            span,
            did_you_mean: suggestion,
        });
    }
}

// ── Helpers ───────────────────────────────────────────────────

fn declare(
    &mut self,
    name:       String,
    kind:       DefKind,
    span:       Span,
    visibility: Visibility,
) -> DefId {
    let id = self.ctx.symbols.insert(name.clone(), kind, span, visibility);
    if let Some(existing_id) = self.scopes.define(name.clone(), id) {
        // Colliding with a pre-declared builtin isn't a real duplicate —
        // `summon std.io.println` naming the same thing println already
        // is is expected, not an error. (The scope keeps pointing at the
        // builtin's own DefId either way; see ScopeStack::define — this
        // only changes whether we report it as a conflict.)
        let existing_kind = &self.ctx.symbols.lookup(existing_id).kind;
        if !matches!(existing_kind, DefKind::Builtin) {
            self.errors.add_name_error(NameError::DuplicateDefinition {
                name,
                first_defined: self.ctx.symbols.lookup(existing_id).defined_at,
                redefined_at:  span,
            });
        }
    }
    id
}

fn find_similar(&self, target: &str) -> Option<String> {
    let mut best: Option<(usize, String)> = None;
    for def in self.ctx.symbols.iter() {
        let dist = edit_distance(&def.name, target);
        if dist <= 1 {
            if best.as_ref().map_or(true, |(d, _)| dist < *d) {
                best = Some((dist, def.name.clone()));
            }
        }
    }
    best.map(|(_, name)| name)
                }}
// ── Utility ───────────────────────────────────────────────────────
fn edit_distance(a: &str, b: &str) -> usize {

let a: Vec<char> = a.chars().collect();

let b: Vec<char> = b.chars().collect();

let m = a.len();

let n = b.len();

if m.abs_diff(n) > 2 { return 2; }

let mut dp = vec![vec![0usize; n + 1]; m + 1];

for i in 0..=m { dp[i][0] = i; }

for j in 0..=n { dp[0][j] = j; }

for i in 1..=m {

for j in 1..=n {

dp[i][j] = if a[i - 1] == b[j - 1] {

dp[i - 1][j - 1]

} else {

1 + dp[i - 1][j].min(dp[i][j - 1]).min(dp[i - 1][j - 1])

};

}

}

dp[m][n].min(2)

        }
