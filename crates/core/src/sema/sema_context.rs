// src/sema/sema_context.rs
//! `SemaContext` — the accumulated output of all semantic analysis passes.
//!
//! Instead of modifying the (immutable, arena-allocated) AST, every pass
//! records its discoveries in side tables stored here.
//! The interpreter and later the LLVM backend receive both the original `Program`
//! and a `SemaContext` and use them together.
//!
//! # Side table layout
//!
//! | Table              | Key        | Value              | Filled by        |
//! |--------------------|------------|--------------------|------------------|
//! | `resolutions`      | use `Span` | `DefId`            | name_resolution  |
//! | `top_level`        | name       | `DefId`            | name_resolution  |
//! | `def_types`        | `DefId`    | `TypeId`           | type_infer       |
//! | `expr_types`       | `Span`     | `TypeId`           | type_infer       |
//! | `binding_types`    | `Span`     | `TypeId`           | type_infer       |
//! | `symbols`          | —          | `SymbolTable`      | name_resolution  |
//! | `types`            | —          | `TypeTable`        | type_infer       |

#![allow(dead_code)]

use std::collections::HashMap;
use crate::ast::common::Span;
use crate::sema::symbol_table::{DefId, ResolutionMap, SymbolTable};
use crate::sema::type_table::{TypeId, TypeTable};

/// The complete output of semantic analysis for one compilation unit.
///
/// Pass this alongside `Program<'ast>` to any downstream consumer.
#[derive(Debug, Default)]
pub struct SemaContext {
    /// Symbol table: all definitions in the program.
    pub symbols: SymbolTable,

    /// Resolution map: every identifier use → the definition it refers to.
    pub resolutions: ResolutionMap,

    /// Type table: all types encountered / inferred.
    pub types: TypeTable,

    /// Inferred type for every expression node, keyed by the expression's `Span`.
    pub expr_types: HashMap<Span, TypeId>,

    /// Inferred type for every let-binding / parameter, keyed by the binding's `Span`.
    pub binding_types: HashMap<Span, TypeId>,

    /// Module-level (top-level item) name -> DefId, populated during
    /// name resolution's pre-declare step. Used by type_infer to resolve
    /// type positions (`: Foo`, `Foo<T>`, struct-literal paths) without
    /// re-walking scopes.
    pub top_level: HashMap<String, DefId>,

    /// The resolved type of a *definition* (function signature, field,
    /// struct/enum-as-type, local binding, parameter), keyed by DefId.
    /// Every identifier use-site already knows its DefId via `resolutions`,
    /// so this is the one lookup type_infer needs to get an Ident's type.
    pub def_types: HashMap<DefId, TypeId>,
}

impl SemaContext {
    pub fn new() -> Self { SemaContext::default() }

    // ── Resolution helpers ─────────────────────────────────────────

    pub fn resolution(&self, span: Span) -> Option<DefId> {
        self.resolutions.get(span)
    }

    pub fn def_at(&self, span: Span) -> Option<&crate::sema::symbol_table::Def> {
        self.resolutions.get(span).map(|id| self.symbols.lookup(id))
    }

    pub fn top_level_def(&self, name: &str) -> Option<DefId> {
        self.top_level.get(name).copied()
    }

    // ── Type helpers ───────────────────────────────────────────────

    pub fn expr_type(&self, span: Span) -> Option<TypeId> {
        self.expr_types.get(&span).copied()
    }

    pub fn set_expr_type(&mut self, span: Span, ty: TypeId) {
        self.expr_types.insert(span, ty);
    }

    pub fn set_binding_type(&mut self, span: Span, ty: TypeId) {
        self.binding_types.insert(span, ty);
    }

    pub fn binding_type(&self, span: Span) -> Option<TypeId> {
        self.binding_types.get(&span).copied()
    }

    /// The resolved type of a definition. `None` until type_infer visits it.
    pub fn def_type(&self, id: DefId) -> Option<TypeId> {
        self.def_types.get(&id).copied()
    }

    pub fn set_def_type(&mut self, id: DefId, ty: TypeId) {
        self.def_types.insert(id, ty);
    }
    }
