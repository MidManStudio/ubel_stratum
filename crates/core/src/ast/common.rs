// src/ast/common.rs
//! Shared primitives reused across every other AST module.

#![allow(dead_code)]

pub use crate::lexer::Span;
use crate::ast::types::Type;

// ── Identifiers ───────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub struct Ident<'ast> {
    pub name: &'ast str,
    pub span: Span,
}

#[derive(Debug, Clone, Copy)]
pub struct QualifiedIdent<'ast> {
    pub segments: &'ast [&'ast str],
    pub span:     Span,
}

impl<'ast> QualifiedIdent<'ast> {
    pub fn is_simple(&self) -> bool { self.segments.len() == 1 }
    pub fn last(&self) -> Option<&'ast str> { self.segments.last().copied() }
}

// ── Visibility ────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Visibility { #[default] Private, Public }

// ── Tier annotation ───────────────────────────────────────────────

/// Which memory tier a function, method, or block runs in.
/// Default when no `@tier(...)` is present: `High`.
/// Developers opt DOWN into `Mid` (arena) or `Low` (manual) for performance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TierAnnotation {
    /// GC-managed. Full async + LINQ. Language default.
    #[default]
    High,
    /// Arena-allocated. `with arena(N) {}` required. Zero-copy callback/view/iterator patterns.
    Mid,
    /// Manual ownership. Borrow-checker enforced. Zero runtime overhead.
    Low,
}

// ── Attributes ────────────────────────────────────────────────────

/// A single `@name(args)` annotation before a declaration.
#[derive(Debug, Clone, Copy)]
pub struct Attribute<'ast> {
    pub name: &'ast str,
    pub args: &'ast [AttrArg<'ast>],
    pub span: Span,
}

/// The possible forms of an argument inside an attribute.
#[derive(Debug, Clone, Copy)]
pub enum AttrArg<'ast> {
    /// Bare identifier: `@deprecated`  /  `@cfg(debug)`
    Ident(&'ast str),

    /// String literal: `@doc("Allocates on the MID arena.")`
    Str(&'ast str),

    /// Integer: `@version(2)`
    Int(i64),

    /// Boolean: `@inline(true)`
    Bool(bool),

    /// Key=value pair: `@cfg(target = "wasm")`
    Named {
        key:   &'ast str,
        value: AttrValue<'ast>,
    },

    /// Nested composition: `@cfg(not(debug))`, `@cfg(any(target="wasm", target="native"))`.
    ///
    /// The `name` is the composition operator (`not`, `any`, `all`) and
    /// `args` are its children — which may themselves be `Named` or `Nested`.
    Nested {
        name: &'ast str,
        args: &'ast [AttrArg<'ast>],
    },
}

/// Extract the bare-ident trait names listed inside `@derive(...)` on an
/// item's attribute list (empty if there's no `@derive` attribute).
/// Non-`Ident` args (a string literal, a `key=value` pair, ...) are
/// silently excluded here — this helper is for consumers that only care
/// about "what was validly requested" (the interpreter, building its
/// runtime opt-in table after sema has already passed). Sema's own
/// validation (`type_infer.rs`, `TYPE-116`) walks `attribute.args`
/// directly instead of going through this helper, specifically so a
/// non-`Ident` arg doesn't just vanish unreported.
pub fn derive_trait_names<'ast>(attributes: &[Attribute<'ast>]) -> Vec<&'ast str> {
    attributes.iter()
        .filter(|a| a.name == "derive")
        .flat_map(|a| a.args.iter())
        .filter_map(|arg| match arg {
            AttrArg::Ident(name) => Some(*name),
            _ => None,
        })
        .collect()
}

/// The right-hand side of a `Named` attribute argument.
#[derive(Debug, Clone, Copy)]
pub enum AttrValue<'ast> {
    Str(&'ast str),
    Int(i64),
    Bool(bool),
    Ident(&'ast str),
}

// ── Lifetime parameters ───────────────────────────────────────────

/// `[lifetime L]` or `[lifetime L where L outlives M]`.
/// Ubel uses `[lifetime Name]` — NOT Rust's tick-prefix `'a` style.
#[derive(Debug, Clone, Copy)]
pub struct LifetimeParam<'ast> {
    pub name:       &'ast str,
    pub constraint: Option<LifetimeConstraint<'ast>>,
    pub span:       Span,
}

/// The `L outlives M` part of a lifetime bound.
#[derive(Debug, Clone, Copy)]
pub struct LifetimeConstraint<'ast> {
    pub longer:  &'ast str,
    pub shorter: &'ast str,
    pub span:    Span,
}

// ── Generic parameters ────────────────────────────────────────────

/// A trait named with the type arguments it is given: `Eq` or
/// `Convert<int>`. It is the shape of a bound on a type parameter
/// (`T: Convert<int>`), of a supertrait (`trait B: Convert<int>`) and of the
/// trait in an impl header (`impl Convert<int> for X`). `args` is empty for a
/// trait written without arguments. `span` covers the name, so a diagnostic
/// about one bound points at it and not at the whole declaration.
#[derive(Debug, Clone, Copy)]
pub struct TraitBound<'ast> {
    pub name: &'ast str,
    pub args: &'ast [&'ast Type<'ast>],
    pub span: Span,
}

/// A single generic type parameter: `T` or `T: Trait + OtherTrait<int>`.
#[derive(Debug, Clone, Copy)]
pub struct GenericParam<'ast> {
    pub name:   &'ast str,
    pub bounds: &'ast [TraitBound<'ast>],
    pub span:   Span,
}

// ── Operators ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add, Sub, Mul, Div, Rem,
    BitAnd, BitOr, BitXor, Shl, Shr,
    Eq, Ne, Lt, Le, Gt, Ge,
    And, Or,
    Range, RangeIncl,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    Neg, BitNot, Not, Await,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssignOp {
    Assign,
    AddAssign, SubAssign, MulAssign, DivAssign, RemAssign,
    BitAndAssign, BitOrAssign, BitXorAssign,
    ShlAssign, ShrAssign,
         }
