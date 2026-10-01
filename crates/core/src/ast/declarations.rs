// ============================================================================
// NOTICE: Full documentation, design decisions, and fix history for this file
// live in docs/ubel_stratum.md, section "ast/declarations.rs"
// ============================================================================
// src/ast/declarations.rs
//! All declaration-level AST nodes.

#![allow(dead_code)]

use crate::ast::common::{
    Attribute, GenericParam, LifetimeParam, Span, TierAnnotation, Visibility,
};
use crate::ast::expressions::Expr;
use crate::ast::statements::Block;
use crate::ast::types::Type;

// ── Function parameters ───────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub struct Param<'ast> {
    pub kind: ParamKind<'ast>,
    pub span: Span,
}

#[derive(Debug, Clone, Copy)]
pub enum ParamKind<'ast> {
    Named {
        mutable: bool,
        name:    &'ast str,
        ty:      Option<&'ast Type<'ast>>,
        default: Option<&'ast Expr<'ast>>,
    },
    /// `_: Type`. An unused parameter kept only to satisfy a
    /// callback/interface signature. No name and no default; see
    /// docs/ubel_stratum.md for why.
    Discard {
        ty: Option<&'ast Type<'ast>>,
    },
    SelfVal,
    SelfMut,
    SelfRef,
    SelfRefMut,
}

// ── Return type ───────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub struct ReturnType<'ast> {
    pub ty:          &'ast Type<'ast>,
    pub is_fallible: bool,
}

// ── Function declaration ──────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub struct FunctionDecl<'ast> {
    /// Attributes before the `fn` keyword: `@tier`, `@cfg`, `@system`, etc.
    pub attributes:      &'ast [Attribute<'ast>],
    /// Resolved from `@tier(...)` attribute; defaults to `High`.
    pub tier:            TierAnnotation,
    pub visibility:      Visibility,
    pub is_async:        bool,
    pub name:            &'ast str,
    pub lifetime_params: &'ast [LifetimeParam<'ast>],
    pub generic_params:  &'ast [GenericParam<'ast>],
    pub params:          &'ast [Param<'ast>],
    pub return_type:     Option<ReturnType<'ast>>,
    pub body:            Block<'ast>,
    pub span:            Span,
}

// ── Struct declaration ────────────────────────────────────────────

/// A `struct` or `edge struct` declaration.
///
/// The `attributes` field carries ECS annotations (`@core`, `@tag`) and any
/// other user-defined or compiler-built-in attributes appearing before the
/// `struct` keyword.
///
/// `lifetime_params` supports `edge struct Foo [lifetime parse] { ... }` —
/// necessary for arena-resident types that hold references into the same arena.
#[derive(Debug, Clone, Copy)]
pub struct StructDecl<'ast> {
    /// `@core`, `@tag`, `@cfg(...)`, `@doc(...)`, or user-defined attributes.
    pub attributes:      &'ast [Attribute<'ast>],
    pub visibility:      Visibility,
    /// `true` for `edge struct` — manual / arena-resident heap layout.
    pub is_edge:         bool,
    pub name:            &'ast str,
    /// Lifetime parameters: `edge struct Buf [lifetime arena] { ... }`.
    /// Empty for structs that don't hold arena references.
    pub lifetime_params: &'ast [LifetimeParam<'ast>],
    pub generic_params:  &'ast [GenericParam<'ast>],
    pub members:         &'ast [StructMember<'ast>],
    pub span:            Span,
}

#[derive(Debug, Clone, Copy)]
pub enum StructMember<'ast> {
    Field(FieldDecl<'ast>),
    Method(MethodDecl<'ast>),
    Property(PropertyDecl<'ast>),
}

#[derive(Debug, Clone, Copy)]
pub struct FieldDecl<'ast> {
    pub visibility: Visibility,
    pub name:       &'ast str,
    pub ty:         &'ast Type<'ast>,
    pub span:       Span,
}

#[derive(Debug, Clone, Copy)]
pub struct MethodDecl<'ast> {
    /// Attributes on this method (`@cfg`, `@inline`, `@cold`, etc.).
    pub attributes:     &'ast [Attribute<'ast>],
    /// Inherits the enclosing struct/impl tier unless overridden by `@tier`.
    pub tier:           TierAnnotation,
    pub visibility:     Visibility,
    pub is_async:       bool,
    pub name:           &'ast str,
    pub generic_params: &'ast [GenericParam<'ast>],
    pub params:         &'ast [Param<'ast>],
    pub return_type:    Option<ReturnType<'ast>>,
    pub body:           Block<'ast>,
    pub span:           Span,
}

#[derive(Debug, Clone, Copy)]
pub struct PropertyDecl<'ast> {
    pub visibility: Visibility,
    pub name:       &'ast str,
    pub ty:         &'ast Type<'ast>,
    pub getter:     Block<'ast>,
    pub setter:     Option<Block<'ast>>,
    pub span:       Span,
}

// ── Enum declaration ──────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub struct EnumDecl<'ast> {
    /// `@cfg(...)`, `@doc(...)`, or other pre-enum attributes.
    pub attributes:     &'ast [Attribute<'ast>],
    pub visibility:     Visibility,
    pub name:           &'ast str,
    pub generic_params: &'ast [GenericParam<'ast>],
    pub variants:       &'ast [EnumVariant<'ast>],
    pub span:           Span,
}

#[derive(Debug, Clone, Copy)]
pub struct EnumVariant<'ast> {
    pub name:    &'ast str,
    pub payload: EnumVariantPayload<'ast>,
    pub span:    Span,
}

#[derive(Debug, Clone, Copy)]
pub enum EnumVariantPayload<'ast> {
    None,
    Discriminant(&'ast Expr<'ast>),
    Tuple(&'ast [&'ast Type<'ast>]),
    Struct(&'ast [FieldDecl<'ast>]),
}

// ── Trait declaration ─────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub struct TraitDecl<'ast> {
    /// `@cfg(...)`, `@doc(...)`, or other pre-trait attributes.
    pub attributes:     &'ast [Attribute<'ast>],
    pub visibility:     Visibility,
    pub name:           &'ast str,
    pub generic_params: &'ast [GenericParam<'ast>],
    pub items:          &'ast [TraitItem<'ast>],
    pub span:           Span,
}

#[derive(Debug, Clone, Copy)]
pub enum TraitItem<'ast> {
    MethodSig(MethodSig<'ast>),
    DefaultMethod(MethodDecl<'ast>),
    AssociatedType { name: &'ast str, span: Span },
}

#[derive(Debug, Clone, Copy)]
pub struct MethodSig<'ast> {
    pub name:           &'ast str,
    pub generic_params: &'ast [GenericParam<'ast>],
    pub params:         &'ast [Param<'ast>],
    pub return_type:    Option<ReturnType<'ast>>,
    pub span:           Span,
}

// ── Impl block ────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub struct ImplBlock<'ast> {
    /// `@cfg(...)`, `@doc(...)` before the `impl` keyword.
    pub attributes:  &'ast [Attribute<'ast>],
    /// Tier applied to all methods in this block unless individually overridden.
    /// `None` = methods default to `High` unless they carry `@tier` themselves.
    pub tier:        Option<TierAnnotation>,
    /// `None` = inherent impl; `Some` = trait impl (`impl Trait for Type`).
    pub trait_path:  Option<&'ast [&'ast str]>,
    pub target_type: &'ast Type<'ast>,
    pub methods:     &'ast [MethodDecl<'ast>],
    pub span:        Span,
}

// ── Extend declaration ────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub struct ExtendDecl<'ast> {
    /// `@cfg(...)` and other pre-extend attributes.
    pub attributes:  &'ast [Attribute<'ast>],
    pub target_type: &'ast Type<'ast>,
    pub methods:     &'ast [MethodDecl<'ast>],
    pub span:        Span,
}

// ── Const and type alias ──────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub struct ConstDecl<'ast> {
    /// `@cfg(platform = "windows")` and similar compile-time guards.
    pub attributes: &'ast [Attribute<'ast>],
    pub name:       &'ast str,
    pub ty:         Option<&'ast Type<'ast>>,
    pub value:      &'ast Expr<'ast>,
    pub span:       Span,
}

/// `static NAME: T = expr`: a mutable module-level global.
///
/// Unlike [`ConstDecl`] the value can be reassigned after startup, which
/// is why it has rules a `const` does not: it lives in the HIGH tier only,
/// only `@tier(high)` code may read or write it, the type annotation is
/// required (many functions assign to it, so its type must not depend on
/// which body is inferred first), and it is private unless marked `pub`.
/// The initializer runs once, before `main`.
#[derive(Debug, Clone, Copy)]
pub struct StaticDecl<'ast> {
    /// `@cfg(...)` and other pre-static attributes. A block's `@tier` is
    /// not applied to a static (it is always HIGH); an explicit
    /// `@tier(...)` on the item itself other than `high` is a parse error.
    pub attributes: &'ast [Attribute<'ast>],
    pub visibility: Visibility,
    pub name:       &'ast str,
    pub ty:         &'ast Type<'ast>,
    pub value:      &'ast Expr<'ast>,
    pub span:       Span,
}

#[derive(Debug, Clone, Copy)]
pub struct TypeAlias<'ast> {
    /// `@doc(...)` and other pre-type-alias attributes.
    pub attributes:     &'ast [Attribute<'ast>],
    pub name:           &'ast str,
    pub generic_params: &'ast [GenericParam<'ast>],
    pub ty:             &'ast Type<'ast>,
    pub span:           Span,
        }
