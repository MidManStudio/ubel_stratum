// src/error_management/errors/parse/mod.rs

use crate::lexer::{Span, TokenType};
use std::fmt;

#[derive(Debug, Clone)]
pub enum ParseError {
    /// Got a token we didn't expect at all.
    UnexpectedToken {
        found:    TokenType,
        expected: Vec<String>,
        span:     Span,
        context:  ParseContext,
    },

    /// Hit EOF mid-parse.
    UnexpectedEof {
        expected: Vec<String>,
        span:     Span,
        context:  ParseContext,
    },

    /// Opened a delimiter but never closed it.
    UnclosedDelimiter {
        delimiter:  char,
        opened_at:  Span,
        closed_by:  Option<char>,
        span:       Span,
    },

    /// A syntactically valid token that's illegal in this position.
    /// e.g. `await` in a @tier(low) function, or `async` outside HIGH tier.
    IllegalInContext {
        what:       String,
        reason:     String,
        span:       Span,
        suggestion: Option<String>,
    },

    /// Catch-all for anything not yet mapped to a structured variant.
    Raw {
        message: String,
        span:    Span,
    },
}

/// Tells the user "while parsing ___" in diagnostics.
/// Add a variant here whenever you add a new grammar sub-parser.
#[derive(Debug, Clone, PartialEq)]
pub enum ParseContext {
    // ── Top level ──────────────────────────────────────────────────
    TopLevel,
    ImportDecl,

    // ── Declarations ──────────────────────────────────────────────
    FunctionDecl,
    FunctionParam,
    ReturnType,
    StructDecl,
    EnumDecl,
    TraitDecl,
    ImplBlock,
    ExtendDecl,
    ConstDecl,
    StaticDecl,
    TypeAliasDecl,

    // ── Attributes ────────────────────────────────────────────────
    /// Parsing an `@name(args)` attribute — e.g. `@tier`, `@cfg`, `@core`.
    AttributeDecl,
    /// Parsing the argument list of a `@cfg(...)` attribute.
    CfgAttribute,
    /// Parsing a `@tier(high|mid|low)` annotation specifically.
    TierAnnotation,

    // ── Expressions ───────────────────────────────────────────────
    Expr,

    // ── Statements ────────────────────────────────────────────────
    Statement,
    Block,
    MatchArm,
    /// Parsing a `with arena(N) { }` memory block.
    ArenaBlock,

    // ── Types & patterns ──────────────────────────────────────────
    TypeExpr,
    Pattern,
}

impl ParseContext {
    pub fn as_str(&self) -> &'static str {
        match self {
            ParseContext::TopLevel        => "top-level declaration",
            ParseContext::ImportDecl      => "import declaration",
            ParseContext::FunctionDecl    => "function declaration",
            ParseContext::FunctionParam   => "function parameter",
            ParseContext::ReturnType      => "return type",
            ParseContext::StructDecl      => "struct declaration",
            ParseContext::EnumDecl        => "enum declaration",
            ParseContext::TraitDecl       => "trait declaration",
            ParseContext::ImplBlock       => "impl block",
            ParseContext::ExtendDecl      => "extend declaration",
            ParseContext::ConstDecl       => "const declaration",
            ParseContext::StaticDecl      => "static declaration",
            ParseContext::TypeAliasDecl   => "type alias declaration",
            ParseContext::AttributeDecl   => "attribute",
            ParseContext::CfgAttribute    => "@cfg attribute",
            ParseContext::TierAnnotation  => "tier annotation",
            ParseContext::Expr            => "expression",
            ParseContext::Statement       => "statement",
            ParseContext::Block           => "block",
            ParseContext::MatchArm        => "match arm",
            ParseContext::ArenaBlock      => "arena block",
            ParseContext::TypeExpr        => "type expression",
            ParseContext::Pattern         => "pattern",
        }
    }
}

impl ParseError {
    pub fn span(&self) -> Span {
        match self {
            ParseError::UnexpectedToken   { span, .. } => *span,
            ParseError::UnexpectedEof     { span, .. } => *span,
            ParseError::UnclosedDelimiter { span, .. } => *span,
            ParseError::IllegalInContext  { span, .. } => *span,
            ParseError::Raw               { span, .. } => *span,
        }
    }

    pub fn message(&self) -> String {
        match self {
            ParseError::UnexpectedToken { found, expected, context, .. } => {
                let exp = expected.join(", ");
                format!(
                    "unexpected `{}` while parsing {}, expected: {}",
                    found, context.as_str(), exp
                )
            }
            ParseError::UnexpectedEof { expected, context, .. } => {
                let exp = expected.join(", ");
                format!(
                    "unexpected end of file while parsing {}, expected: {}",
                    context.as_str(), exp
                )
            }
            ParseError::UnclosedDelimiter { delimiter, closed_by, .. } => {
                match closed_by {
                    Some(c) => format!(
                        "unclosed `{}` — found `{}` instead of closing delimiter",
                        delimiter, c
                    ),
                    None => format!(
                        "unclosed `{}` — reached end of file without closing",
                        delimiter
                    ),
                }
            }
            ParseError::IllegalInContext { what, reason, .. } => {
                format!("`{}` is not allowed here: {}", what, reason)
            }
            ParseError::Raw { message, .. } => message.clone(),
        }
    }

    pub fn suggestion(&self) -> Option<String> {
        match self {
            ParseError::UnexpectedToken { found, expected, .. } => {
                if expected.len() == 1 {
                    Some(format!("try replacing `{}` with {}", found, expected[0]))
                } else {
                    None
                }
            }
            ParseError::UnclosedDelimiter { delimiter, .. } => {
                let closing = match delimiter {
                    '(' => ')', '{' => '}', '[' => ']', c => *c,
                };
                Some(format!("add a closing `{}`", closing))
            }
            ParseError::IllegalInContext { suggestion, .. } => suggestion.clone(),
            _ => None,
        }
    }
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.message())
    }
}

impl std::error::Error for ParseError {}

impl crate::error_management::render::Diagnosable for ParseError {
    // See docs/DIAGNOSTICS_RULES.md, "Error Code Registry" — PARSE-0xx.
    fn code(&self) -> &'static str {
        match self {
            ParseError::UnexpectedToken { .. }   => "PARSE-001",
            ParseError::UnexpectedEof { .. }     => "PARSE-002",
            ParseError::UnclosedDelimiter { .. } => "PARSE-003",
            ParseError::IllegalInContext { .. }  => "PARSE-004",
            ParseError::Raw { .. }               => "PARSE-005",
        }
    }
    fn span(&self) -> Span { self.span() }
    fn message(&self) -> String { self.message() }
    fn suggestion(&self) -> Option<String> { self.suggestion() }
}
