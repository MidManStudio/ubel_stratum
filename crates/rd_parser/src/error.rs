// crates/rd_parser/src/error.rs
//! Error constructor helpers for the recursive-descent parser.
//!
//! The RD parser uses the same `ParseError` and `ParseContext` types from
//! `ubel_stratum::error_management` as the LALRPOP parser, so all diagnostics
//! pass through the same `miette`-powered rendering pipeline.
//!
//! These helpers exist so each `parse_*` module can emit errors in one line
//! without re-spelling the full struct literal every time.

use ubel_stratum::{
    error_management::errors::{ParseContext, ParseError},
    lexer::{Span, Token, TokenType},
};

use crate::cursor::CursorError;

// ── Conversion: CursorError → ParseError ─────────────────────────────────────

/// Lift a raw `CursorError` (from `Cursor::expect`) into a full `ParseError`
/// with the current grammar context attached.
pub fn from_cursor(err: CursorError, ctx: ParseContext) -> ParseError {
    match err {
        CursorError::UnexpectedToken { expected, found, span } => {
            ParseError::UnexpectedToken {
                found,
                expected: vec![expected],
                span,
                context: ctx,
            }
        }
        CursorError::UnexpectedEof { expected, span } => {
            ParseError::UnexpectedEof {
                expected: vec![expected],
                span,
                context: ctx,
            }
        }
    }
}

// ── Direct constructors ───────────────────────────────────────────────────────

/// "Expected one of these, got that."
///
/// `expected` is a slice of human-readable strings, e.g. `["'fn'", "identifier"]`.
pub fn unexpected(found: &Token, expected: &[&str], ctx: ParseContext) -> ParseError {
    if matches!(found.kind, TokenType::Eof) {
        ParseError::UnexpectedEof {
            expected: expected.iter().map(|s| s.to_string()).collect(),
            span:     found.span,
            context:  ctx,
        }
    } else {
        ParseError::UnexpectedToken {
            found:    found.kind.clone(),
            expected: expected.iter().map(|s| s.to_string()).collect(),
            span:     found.span,
            context:  ctx,
        }
    }
}

/// "I opened a `{` at span X, but never found the matching `}`."
pub fn unclosed(delimiter: char, opened_at: Span, closed_by: Option<char>, at: Span) -> ParseError {
    ParseError::UnclosedDelimiter { delimiter, opened_at, closed_by, span: at }
}

/// "This token/construct is syntactically valid but illegal in this position."
///
/// Used for things like `await` in a `@tier(low)` function — the parser can
/// *see* it's an `await` keyword, but the tier rules forbid it here.
pub fn illegal_here(what: &str, reason: &str, span: Span, hint: Option<&str>) -> ParseError {
    ParseError::IllegalInContext {
        what:       what.to_string(),
        reason:     reason.to_string(),
        span,
        suggestion: hint.map(str::to_string),
    }
}

/// Expressions or blocks nested past `limit` levels.
pub fn nesting_too_deep(limit: usize, span: Span) -> ParseError {
    ParseError::NestingTooDeep { limit, span }
}

/// A catch-all for situations where none of the structured variants fit.
pub fn raw(message: impl Into<String>, span: Span) -> ParseError {
    ParseError::Raw { message: message.into(), span }
}

// ── Context shortcuts ─────────────────────────────────────────────────────────
// These are thin wrappers so parse_* modules can write
//   `error::in_fn(...)` instead of `error::unexpected(..., ParseContext::FunctionDecl)`

pub fn in_top_level(found: &Token, expected: &[&str]) -> ParseError {
    unexpected(found, expected, ParseContext::TopLevel)
}
pub fn in_fn(found: &Token, expected: &[&str]) -> ParseError {
    unexpected(found, expected, ParseContext::FunctionDecl)
}
pub fn in_struct(found: &Token, expected: &[&str]) -> ParseError {
    unexpected(found, expected, ParseContext::StructDecl)
}
pub fn in_enum(found: &Token, expected: &[&str]) -> ParseError {
    unexpected(found, expected, ParseContext::EnumDecl)
}
pub fn in_trait(found: &Token, expected: &[&str]) -> ParseError {
    unexpected(found, expected, ParseContext::TraitDecl)
}
pub fn in_impl(found: &Token, expected: &[&str]) -> ParseError {
    unexpected(found, expected, ParseContext::ImplBlock)
}
pub fn in_expr(found: &Token, expected: &[&str]) -> ParseError {
    unexpected(found, expected, ParseContext::Expr)
}
pub fn in_stmt(found: &Token, expected: &[&str]) -> ParseError {
    unexpected(found, expected, ParseContext::Statement)
}
pub fn in_type(found: &Token, expected: &[&str]) -> ParseError {
    unexpected(found, expected, ParseContext::TypeExpr)
}
pub fn in_pattern(found: &Token, expected: &[&str]) -> ParseError {
    unexpected(found, expected, ParseContext::Pattern)
}
pub fn in_attr(found: &Token, expected: &[&str]) -> ParseError {
    unexpected(found, expected, ParseContext::TierAnnotation)
}
pub fn in_import(found: &Token, expected: &[&str]) -> ParseError {
    unexpected(found, expected, ParseContext::ImportDecl)
          }
