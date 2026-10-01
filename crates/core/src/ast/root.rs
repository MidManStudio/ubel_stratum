// src/ast/root.rs
//! The root of a parsed Ubel Stratum source file.

#![allow(dead_code)]

use crate::ast::common::Span;
use crate::ast::declarations::{
    ConstDecl, EnumDecl, ExtendDecl, FunctionDecl,
    ImplBlock, StaticDecl, StructDecl, TraitDecl, TypeAlias,
};

/// The root node returned by the parser for one `.ubl` source file.
#[derive(Debug, Clone, Copy)]
pub struct Program<'ast> {
    pub package: Option<PackageDecl<'ast>>,
    pub imports: &'ast [Import<'ast>],
    pub items: &'ast [Item<'ast>],
    /// Span covering the entire file (start = 0, end = file length).
    pub span: Span,
}

// ── Package declaration ───────────────────────────────────────────

/// `package std.collections`
#[derive(Debug, Clone, Copy)]
pub struct PackageDecl<'ast> {
    /// Path segments split on `.`: `["std", "collections"]`
    pub path: &'ast [&'ast str],
    pub span: Span,
}

// ── Import declarations ───────────────────────────────────────────

/// Any form of import statement.
#[derive(Debug, Clone, Copy)]
pub struct Import<'ast> {
    pub kind: ImportKind<'ast>,
    pub span: Span,
}

#[derive(Debug, Clone, Copy)]
pub enum ImportKind<'ast> {
    /// `summon std.collections.List`
    /// `summon std.collections.List as L`
    Summon {
        /// Qualified path: `["std", "collections", "List"]`
        path: &'ast [&'ast str],
        /// Optional `as Alias`
        alias: Option<&'ast str>,
    },

    /// `from std.collections summon List`
    /// `from std.collections summon [List, Dictionary, Set]`
    FromSummon {
        /// Module path: `["std", "collections"]`
        module_path: &'ast [&'ast str],
        items: ImportItems<'ast>,
    },
}

/// What is being imported in a `from ... summon` statement.
#[derive(Debug, Clone, Copy)]
pub enum ImportItems<'ast> {
    /// `summon List`
    Single(&'ast str),
    /// `summon [List, Dictionary, Set]`
    List(&'ast [&'ast str]),
}

// ── Top-level item ────────────────────────────────────────────────

/// Every kind of declaration that can appear at the top level of a file.
#[derive(Debug, Clone, Copy)]
pub enum Item<'ast> {
    Function(FunctionDecl<'ast>),
    Struct(StructDecl<'ast>),
    Enum(EnumDecl<'ast>),
    Trait(TraitDecl<'ast>),
    Impl(ImplBlock<'ast>),
    Extend(ExtendDecl<'ast>),
    Const(ConstDecl<'ast>),
    Static(StaticDecl<'ast>),
    TypeAlias(TypeAlias<'ast>),
}

impl<'ast> Item<'ast> {
    /// The source span of this item, regardless of its kind.
    pub fn span(&self) -> Span {
        match self {
            Item::Function(f)  => f.span,
            Item::Struct(s)    => s.span,
            Item::Enum(e)      => e.span,
            Item::Trait(t)     => t.span,
            Item::Impl(i)      => i.span,
            Item::Extend(e)    => e.span,
            Item::Const(c)     => c.span,
            Item::Static(s)    => s.span,
            Item::TypeAlias(a) => a.span,
        }
    }

    /// A short string name for this item kind, useful in error messages.
    pub fn kind_name(&self) -> &'static str {
        match self {
            Item::Function(_)  => "function",
            Item::Struct(_)    => "struct",
            Item::Enum(_)      => "enum",
            Item::Trait(_)     => "trait",
            Item::Impl(_)      => "impl block",
            Item::Extend(_)    => "extend declaration",
            Item::Const(_)     => "constant",
            Item::Static(_)    => "static",
            Item::TypeAlias(_) => "type alias",
        }
    }
  }
