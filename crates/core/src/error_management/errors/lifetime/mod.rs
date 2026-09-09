// src/error_management/errors/lifetime/mod.rs
//! Errors produced by `sema/lifetime_check.rs`, well-formedness
//! checking for `[lifetime L]` / `[lifetime L where L outlives M]`
//! declarations on functions and `edge struct`s, and for the lifetime
//! names those declarations' own signatures/fields use. New family, not
//! folded into `BORROW-0xx`/`MOVE-0xx` even though all three eventually
//! serve the same LOW-tier memory-safety story (see
//! docs/MEMORY_MODEL.md §9/§12): this one is a purely structural check
//! over declared names and constraints, no CFG or liveness fixed point
//! involved, so it doesn't share `borrow_check.rs`/`move_check.rs`'s
//! machinery or scope. It does not check that real usage respects a
//! declared `outlives` bound (the actual outlives/subset fixed point),
//! that remains the borrow checker's unbuilt job. See
//! `sema/lifetime_check.rs`'s own module doc for exactly what today's
//! well-formedness pass does and doesn't cover.

use crate::lexer::Span;

/// Every error the lifetime well-formedness pass can raise.
#[derive(Debug, Clone)]
pub enum LifetimeError {
    /// A lifetime name was written (in a `where L outlives M` clause, or
    /// as `&name T`/`ref name T` in a function's own param/return types
    /// or an `edge struct`'s own field types) that isn't one of the
    /// names that same declaration's own `[lifetime ...]` list declares.
    UndeclaredLifetime {
        name: String,
        span: Span,
    },
    /// The same lifetime name appears twice in one `[lifetime ...]` list.
    DuplicateLifetimeParam {
        name: String,
        first_span: Span,
        dup_span: Span,
    },
    /// The declared `outlives` constraints among one declaration's own
    /// lifetime parameters form a cycle. Includes the trivial one-element
    /// case, `L outlives L` (a lifetime declared to outlive itself),
    /// reported with a clearer message than the general N-cycle case.
    OutlivesCycle {
        names: Vec<String>,
        span: Span,
    },
}

impl LifetimeError {
    pub fn span(&self) -> Span {
        match self {
            LifetimeError::UndeclaredLifetime { span, .. } => *span,
            LifetimeError::DuplicateLifetimeParam { dup_span, .. } => *dup_span,
            LifetimeError::OutlivesCycle { span, .. } => *span,
        }
    }

    pub fn message(&self) -> String {
        match self {
            LifetimeError::UndeclaredLifetime { name, .. } =>
                format!("undeclared lifetime `{}`", name),
            LifetimeError::DuplicateLifetimeParam { name, .. } =>
                format!("lifetime `{}` declared more than once", name),
            LifetimeError::OutlivesCycle { names, .. } if names.len() == 1 =>
                format!("lifetime `{}` cannot outlive itself", names[0]),
            LifetimeError::OutlivesCycle { names, .. } => format!(
                "outlives constraints form a cycle among {}",
                names.iter().map(|n| format!("`{}`", n)).collect::<Vec<_>>().join(", ")
            ),
        }
    }

    pub fn suggestion(&self) -> Option<String> {
        match self {
            LifetimeError::UndeclaredLifetime { name, .. } => Some(format!(
                "add `lifetime {}` to this declaration's `[...]` list, or use a name it already declares",
                name
            )),
            LifetimeError::DuplicateLifetimeParam { name, .. } => Some(format!(
                "remove one of the two `lifetime {}` entries, or rename one of them",
                name
            )),
            LifetimeError::OutlivesCycle { .. } =>
                Some("outlives relationships must form a strict ordering, with no lifetime (directly or indirectly) outliving itself".to_string()),
        }
    }
}

impl std::fmt::Display for LifetimeError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}", self.message())
    }
}

impl std::error::Error for LifetimeError {}

impl crate::error_management::render::Diagnosable for LifetimeError {
    // See docs/DIAGNOSTICS_RULES.md, "Error Code Registry", LIFETIME-0xx.
    fn code(&self) -> &'static str {
        match self {
            LifetimeError::UndeclaredLifetime { .. }     => "LIFETIME-001",
            LifetimeError::DuplicateLifetimeParam { .. } => "LIFETIME-002",
            LifetimeError::OutlivesCycle { .. }           => "LIFETIME-003",
        }
    }
    fn span(&self) -> Span { self.span() }
    fn message(&self) -> String { self.message() }
    fn suggestion(&self) -> Option<String> { self.suggestion() }

    fn secondary_spans(&self) -> Vec<(Span, String)> {
        match self {
            LifetimeError::DuplicateLifetimeParam { first_span, .. } =>
                vec![(*first_span, "first declared here".to_string())],
            _ => Vec::new(),
        }
    }
}
