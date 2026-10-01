// src/error_management/errors/naming/mod.rs
//! Errors produced during the name-resolution pass.

use crate::lexer::Span;
use std::fmt;

/// Every error that can be raised while resolving names to definitions.
#[derive(Debug, Clone)]
pub enum NameError {
    /// An identifier was used but never defined in any reachable scope.
    UndefinedName {
        name: String,
        span: Span,
        /// If we found something close, suggest it.
        did_you_mean: Option<String>,
    },

    /// The same name was declared twice in the same scope.
    DuplicateDefinition {
        name:          String,
        first_defined: Span,
        redefined_at:  Span,
    },

    /// `summon` (or `from ... summon`) referred to a path that does not exist.
    UnresolvedImport {
        path: String,
        span: Span,
    },

    /// A dotted path like `std.io.File` was partially resolved but the
    /// final segment was not found inside the resolved module.
    UnresolvedPathSegment {
        full_path:        String,
        unresolved_at:    String,
        resolved_so_far:  String,
        span:             Span,
    },

    /// `self` was used outside of a method body.
    SelfOutsideMethod {
        span: Span,
    },

    /// A type parameter (generic) name was referenced but not declared.
    UnresolvedTypeParam {
        name: String,
        span: Span,
    },

    /// A global `const` was the target of an assignment or compound
    /// assignment. Constants are initialized once and never reassigned.
    AssignToConst {
        name: String,
        span: Span,
    },

    /// A `const` initializer read a `static`. A const is evaluated once at
    /// startup (and again if it had to wait for a later constant), so it
    /// cannot depend on a mutable global's current value.
    StaticInConst {
        name: String,
        span: Span,
    },
}

impl NameError {
    pub fn span(&self) -> Span {
        match self {
            NameError::UndefinedName          { span, .. } => *span,
            NameError::DuplicateDefinition    { redefined_at, .. } => *redefined_at,
            NameError::UnresolvedImport       { span, .. } => *span,
            NameError::UnresolvedPathSegment  { span, .. } => *span,
            NameError::SelfOutsideMethod      { span }     => *span,
            NameError::UnresolvedTypeParam    { span, .. } => *span,
            NameError::AssignToConst          { span, .. } => *span,
            NameError::StaticInConst          { span, .. } => *span,
        }
    }

    pub fn message(&self) -> String {
        match self {
            NameError::UndefinedName { name, .. } =>
                format!("undefined name `{}`", name),

            NameError::DuplicateDefinition { name, .. } =>
                format!("`{}` is already defined in this scope", name),

            NameError::UnresolvedImport { path, .. } =>
                format!("cannot resolve import path `{}`", path),

            NameError::UnresolvedPathSegment { full_path, unresolved_at, resolved_so_far, .. } =>
                format!(
                    "no member `{}` in `{}` (while resolving `{}`)",
                    unresolved_at, resolved_so_far, full_path
                ),

            NameError::SelfOutsideMethod { .. } =>
                "`self` can only be used inside a method body".to_string(),

            NameError::UnresolvedTypeParam { name, .. } =>
                format!("unknown type parameter `{}`", name),

            NameError::AssignToConst { name, .. } =>
                format!("cannot assign to constant `{}`", name),

            NameError::StaticInConst { name, .. } =>
                format!("a constant cannot read the static `{}`", name),
        }
    }

    pub fn suggestion(&self) -> Option<String> {
        match self {
            NameError::UndefinedName { did_you_mean: Some(s), .. } =>
                Some(format!("did you mean `{}`?", s)),

            NameError::SelfOutsideMethod { .. } =>
                Some("move this code into a method that takes `self` as a parameter".to_string()),

            NameError::AssignToConst { .. } =>
                Some("constants are initialized once; use a `let` binding if the value needs to change".to_string()),

            NameError::StaticInConst { .. } =>
                Some("make the other item a `const` too, or initialize this value from a `static`".to_string()),

            _ => None,
        }
    }
}

impl fmt::Display for NameError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.message())
    }
}

impl std::error::Error for NameError {}

impl crate::error_management::render::Diagnosable for NameError {
    // See docs/DIAGNOSTICS_RULES.md, "Error Code Registry" — NAME-0xx.
    fn code(&self) -> &'static str {
        match self {
            NameError::UndefinedName { .. }         => "NAME-001",
            NameError::DuplicateDefinition { .. }    => "NAME-002",
            NameError::UnresolvedImport { .. }       => "NAME-003",
            NameError::UnresolvedPathSegment { .. }  => "NAME-004",
            NameError::SelfOutsideMethod { .. }      => "NAME-005",
            NameError::UnresolvedTypeParam { .. }    => "NAME-006",
            NameError::AssignToConst { .. }          => "NAME-007",
            NameError::StaticInConst { .. }          => "NAME-008",
        }
    }
    fn span(&self) -> Span { self.span() }
    fn message(&self) -> String { self.message() }
    fn suggestion(&self) -> Option<String> { self.suggestion() }

    fn secondary_spans(&self) -> Vec<(Span, String)> {
        match self {
            NameError::DuplicateDefinition { first_defined, .. } =>
                vec![(*first_defined, "first defined here".to_string())],
            _ => Vec::new(),
        }
    }
}
