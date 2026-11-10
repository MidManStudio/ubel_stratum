// src/ast/expressions.rs
//! Expression nodes.
//!
//! `Expr<'ast>` is the central recursive type.  Every variant that could
//! produce a large enum variant stores its payload behind an `&'ast`
//! pointer allocated in the arena, keeping the Expr struct itself small.

#![allow(dead_code)]

use crate::ast::common::{AssignOp, BinOp, Span, UnaryOp};
use crate::ast::literals::Literal;
use crate::ast::patterns::Pattern;
use crate::ast::statements::Block;
use crate::ast::types::Type;

/// An expression together with its source span.
#[derive(Debug, Clone, Copy)]
pub struct Expr<'ast> {
    pub kind: ExprKind<'ast>,
    pub span: Span,
}

#[derive(Debug, Clone, Copy)]
pub enum ExprKind<'ast> {
    // ── Leaf expressions ─────────────────────────────────────────
    Lit(Literal<'ast>),
    Ident(&'ast str),
    SelfExpr,

    // ── Arithmetic / logic ───────────────────────────────────────
    BinOp {
        op: BinOp,
        lhs: &'ast Expr<'ast>,
        rhs: &'ast Expr<'ast>,
    },
    UnaryOp {
        op: UnaryOp,
        operand: &'ast Expr<'ast>,
    },

    // ── Assignment ───────────────────────────────────────────────
    Assign {
        op: AssignOp,
        target: &'ast Expr<'ast>,
        value: &'ast Expr<'ast>,
    },

    // ── Pipe: `left |> right` ────────────────────────────────────
    // Not produced by `rd_parser`, which lowers `|>` to a `Call` while parsing
    // (docs/PARSER_RULES.md section 5.13). Kept for the reference LALRPOP
    // parser and the visitors.
    Pipe {
        left: &'ast Expr<'ast>,
        right: &'ast Expr<'ast>,
    },

    // ── Calls & access ───────────────────────────────────────────
    Call {
        callee: &'ast Expr<'ast>,
        args: &'ast [Arg<'ast>],
    },
    Field {
        target: &'ast Expr<'ast>,
        field: &'ast str,
    },
    Index {
        target: &'ast Expr<'ast>,
        index: &'ast Expr<'ast>,
    },

    // ── Safe navigation: `expr?.field` / `expr?.method()` ────────
    OptionalChain {
        target: &'ast Expr<'ast>,
        access: OptionalAccess<'ast>,
    },

    // ── Error propagation: `expr?` ────────────────────────────────
    Try(&'ast Expr<'ast>),

    // ── Await: `await expr` ───────────────────────────────────────
    Await(&'ast Expr<'ast>),

    // ── Borrow: `&place` / `&mut place` / `ref place` / `ref mut place` ──
    // `&`/`ref` and `&mut`/`ref mut` are dual spellings of the same
    // operator (same precedent as `and`/`&&`, `or`/`||`) — both forms
    // produce this one node. LOW-tier borrow checker consumes this
    // directly; see docs/PARSER_RULES.md §5.6.
    Borrow {
        mutable: bool,
        place: &'ast Expr<'ast>,
    },

    // ── Dereference: `*place` / `deref place` ─────────────────────
    // Dual-spelled the same way as Borrow above.
    Deref(&'ast Expr<'ast>),

    // ── Collection literals ───────────────────────────────────────
    Tuple(&'ast [&'ast Expr<'ast>]),
    Array(&'ast [&'ast Expr<'ast>]),
    /// Dictionary literal: `{ "key" = value }`
    Dict(&'ast [DictEntry<'ast>]),
    /// Anonymous object literal: `{ field = value }`
    AnonObject(&'ast [ObjectField<'ast>]),
    /// Named struct literal: `Point { x = 1, y = 2 }`
    StructLit {
        path: &'ast [&'ast str],
        fields: &'ast [FieldInit<'ast>],
    },

    // ── Complex expressions stored behind a pointer ───────────────
    /// Lambda / anonymous function — boxed because it contains a Block
    Lambda(&'ast Lambda<'ast>),
    /// Block expression `{ ... }` — boxed to keep ExprKind small
    Block(&'ast Block<'ast>),
    /// If/elif/else expression — boxed because it embeds Blocks
    If(&'ast IfExpr<'ast>),
    /// Match expression — boxed because it embeds MatchArms
    Match(&'ast MatchExpr<'ast>),

    // ── `expr or continue/break/return/default` ───────────────────
    OrElse {
        expr: &'ast Expr<'ast>,
        fallback: OrElseFallback<'ast>,
    },

    // ── Type coercion: `expr as Type` ─────────────────────────────
    As {
        expr: &'ast Expr<'ast>,
        ty: &'ast Type<'ast>,
    },

    // ── Short-hand declaration: `x := expr` (sugar for `let x = expr`) ──
    ShortDecl {
        name: &'ast str,
        value: &'ast Expr<'ast>,
    },
}

// ── Call arguments ────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub struct Arg<'ast> {
    pub kind: ArgKind<'ast>,
    pub span: Span,
}

#[derive(Debug, Clone, Copy)]
pub enum ArgKind<'ast> {
    /// A positional argument
    Positional(&'ast Expr<'ast>),
    /// A named argument: `name = expr`
    Named {
        name: &'ast str,
        value: &'ast Expr<'ast>,
    },
}

// ── Optional chaining ─────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub enum OptionalAccess<'ast> {
    Field(&'ast str),
    Method {
        name: &'ast str,
        args: &'ast [Arg<'ast>],
    },
}

// ── Collection literal helpers ────────────────────────────────────

/// One entry in a dictionary literal: `"key" = value`
#[derive(Debug, Clone, Copy)]
pub struct DictEntry<'ast> {
    pub key: &'ast Expr<'ast>,
    pub value: &'ast Expr<'ast>,
    pub span: Span,
}

/// One field in an anonymous-object literal: `name = value`
#[derive(Debug, Clone, Copy)]
pub struct ObjectField<'ast> {
    pub name: &'ast str,
    pub value: &'ast Expr<'ast>,
    pub span: Span,
}

/// One field initialiser in a struct literal: `x = 10`
#[derive(Debug, Clone, Copy)]
pub struct FieldInit<'ast> {
    pub name: &'ast str,
    pub value: &'ast Expr<'ast>,
    pub span: Span,
}

// ── Lambda ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub struct Lambda<'ast> {
    pub params: &'ast [LambdaParam<'ast>],
    pub body: LambdaBody<'ast>,
    pub span: Span,
}

#[derive(Debug, Clone, Copy)]
pub struct LambdaParam<'ast> {
    pub name: &'ast str,
    pub ty: Option<&'ast Type<'ast>>,
    pub span: Span,
}

#[derive(Debug, Clone, Copy)]
pub enum LambdaBody<'ast> {
    /// `fn(x) { stmts }`
    Block(Block<'ast>),
    /// `fn(x) x * 2`
    Expr(&'ast Expr<'ast>),
}

// ── If expression ─────────────────────────────────────────────────

/// An `if / elif / else` expression (also usable as a statement).
///
/// Stored behind `&'ast` in `ExprKind::If` to keep enum variants small.
#[derive(Debug, Clone, Copy)]
pub struct IfExpr<'ast> {
    pub condition: &'ast Expr<'ast>,
    pub then_body: IfBranchBody<'ast>,
    pub elif_branches: &'ast [ElifBranch<'ast>],
    pub else_body: Option<IfBranchBody<'ast>>,
    pub span: Span,
}

#[derive(Debug, Clone, Copy)]
pub struct ElifBranch<'ast> {
    pub condition: &'ast Expr<'ast>,
    pub body: IfBranchBody<'ast>,
    pub span: Span,
}

/// The body of an `if` / `elif` / `else` branch — either a full
/// `{ block }` or the single-line `then expr` form. Mirrors
/// `MatchArmBody` exactly; every consumer dispatches on this the same
/// way it already dispatches on `MatchArmBody`.
///
/// `then` exists (rather than reusing the brace-optional trick `Lambda`
/// and `MatchArmBody` use) because the condition has no hard delimiter
/// before it — `Lambda` has `)` and match arms have `=>`, but an `if`
/// condition is parsed by the general Pratt expression parser with no
/// closing token, so `if a - 1 { ... }` would be genuinely ambiguous
/// (`- 1` reads as more condition) without an explicit marker.
#[derive(Debug, Clone, Copy)]
pub enum IfBranchBody<'ast> {
    Block(Block<'ast>),
    Expr(&'ast Expr<'ast>),
}

// ── Match expression ──────────────────────────────────────────────

/// A `match` expression.  Stored behind `&'ast` in `ExprKind::Match`.
#[derive(Debug, Clone, Copy)]
pub struct MatchExpr<'ast> {
    pub scrutinee: &'ast Expr<'ast>,
    pub arms: &'ast [MatchArm<'ast>],
    pub span: Span,
}

/// A single arm inside a `match`.
#[derive(Debug, Clone, Copy)]
pub struct MatchArm<'ast> {
    pub pattern: Pattern<'ast>,
    /// Optional `where` guard: `where x > 0`
    pub guard: Option<&'ast Expr<'ast>>,
    pub body: MatchArmBody<'ast>,
    pub span: Span,
}

#[derive(Debug, Clone, Copy)]
pub enum MatchArmBody<'ast> {
    Expr(&'ast Expr<'ast>),
    Block(Block<'ast>),
}

// ── Or-else fallback ──────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub enum OrElseFallback<'ast> {
    /// `or <expr>` — use a default value
    Expr(&'ast Expr<'ast>),
    Continue,
    Break,
    /// `or return` or `or return expr`
    Return(Option<&'ast Expr<'ast>>),
  }
