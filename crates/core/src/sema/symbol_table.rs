// src/sema/symbol_table.rs
//! The symbol table: every name defined in the program, what it refers to,
//! and where it lives.
//!
//! # Design
//!
//! Names are resolved in two stages:
//!
//! 1. **Definition collection** — walk declarations and assign every name a `DefId`.
//!    Top-level items are collected before expressions so forward references work.
//!
//! 2. **Use resolution** — walk expressions and statements; for each identifier,
//!    walk the scope stack inward-to-outward to find its `DefId`.
//!
//! The output is a flat `SymbolTable` containing every `Def` and a
//! `ResolutionMap` mapping each use-site `Span` to the `DefId` it resolved to.
//! The rest of the compiler operates on `DefId`s, never raw strings.

#![allow(dead_code)]

use std::collections::HashMap;
use crate::ast::common::{Span, TierAnnotation, Visibility};

// ── Identifier for definitions ─────────────────────────────────────

/// A stable integer handle for a definition.
/// Every function, struct, field, parameter, local variable, and type alias
/// gets its own unique `DefId`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DefId(pub usize);

impl DefId {
    pub const INVALID: DefId = DefId(usize::MAX);
}

// ── What a definition refers to ────────────────────────────────────

/// The kind of entity a `DefId` names.
#[derive(Debug, Clone)]
pub enum DefKind {
    /// A top-level or nested function.
    Function {
        tier:     TierAnnotation,
        is_async: bool,
    },
    /// A struct type.
    Struct {
        is_edge: bool,
    },
    /// An enum type.
    Enum,
    /// A trait.
    Trait,
    /// A type alias.
    TypeAlias,
    /// A struct field.
    Field {
        parent: DefId,
    },
    /// An enum variant.
    Variant {
        parent: DefId,
    },
    /// A method (associated function on a struct/trait/impl).
    Method {
        parent: DefId,
        tier:   TierAnnotation,
    },
    /// A function/method parameter.
    Param {
        parent: DefId,
    },
    /// A local `let` binding.
    Local {
        mutable: bool,
    },
    /// A top-level constant.
    Const,
    /// A top-level mutable global (`static NAME: T = expr`). HIGH tier
    /// only; unlike `Const` it can be assigned to.
    Static,
    /// A generic type parameter (e.g. `T` in `fn foo<T>`).
    TypeParam {
        parent: DefId,
    },
    /// A lifetime parameter (e.g. `L` in `[lifetime L]`).
    LifetimeParam {
        parent: DefId,
    },
    /// An imported name brought in via `summon`.
    Import {
        /// The canonical qualified path, e.g. `["std", "collections", "List"]`.
        canonical_path: Vec<String>,
    },
    /// A built-in type (int, string, bool, List, …) — no definition site.
    Builtin,
}

/// One entry in the symbol table: a name with its kind, location, and visibility.
#[derive(Debug, Clone)]
pub struct Def {
    pub id:         DefId,
    pub name:       String,
    pub kind:       DefKind,
    pub defined_at: Span,
    pub visibility: Visibility,
}

// ── Symbol table ──────────────────────────────────────────────────

/// The complete flat table of definitions for one compilation unit.
///
/// Use `lookup(id)` to retrieve a `Def` from a `DefId`.
/// The `ResolutionMap` (built by the resolver) maps use-site `Span`s to `DefId`s.
#[derive(Debug, Default)]
pub struct SymbolTable {
    defs: Vec<Def>,
}

impl SymbolTable {
    pub fn new() -> Self {
        SymbolTable { defs: Vec::new() }
    }

    /// Insert a new definition and return its assigned `DefId`.
    pub fn insert(
        &mut self,
        name:       String,
        kind:       DefKind,
        defined_at: Span,
        visibility: Visibility,
    ) -> DefId {
        let id = DefId(self.defs.len());
        self.defs.push(Def { id, name, kind, defined_at, visibility });
        id
    }

    /// Retrieve a definition by its `DefId`. Panics on an invalid id.
    pub fn lookup(&self, id: DefId) -> &Def {
        &self.defs[id.0]
    }

    /// Iterate over all definitions.
    pub fn iter(&self) -> impl Iterator<Item = &Def> {
        self.defs.iter()
    }

    pub fn len(&self) -> usize { self.defs.len() }
    pub fn is_empty(&self) -> bool { self.defs.is_empty() }
}

// ── Resolution map ─────────────────────────────────────────────────

/// Maps every identifier use-site (by its `Span`) to the `DefId` it resolved to.
///
/// This is the primary output of the name-resolution pass.
/// All subsequent passes (type inference, tier checking, interpreter) start here.
#[derive(Debug, Default)]
pub struct ResolutionMap {
    inner: HashMap<Span, DefId>,
}

impl ResolutionMap {
    pub fn new() -> Self {
        ResolutionMap { inner: HashMap::new() }
    }

    /// Record that the identifier at `span` resolved to `def`.
    pub fn record(&mut self, span: Span, def: DefId) {
        self.inner.insert(span, def);
    }

    /// Look up the `DefId` for a use-site `Span`.
    /// Returns `None` for spans that were not recorded (e.g. error recovery sites).
    pub fn get(&self, span: Span) -> Option<DefId> {
        self.inner.get(&span).copied()
    }
}

// ── Scope stack ───────────────────────────────────────────────────

/// A single lexical scope: a flat name→DefId mapping.
#[derive(Debug, Default)]
pub struct Scope {
    bindings: HashMap<String, DefId>,
}

impl Scope {
    pub fn new() -> Self { Scope::default() }

    /// Add a binding to this scope.
    pub fn define(&mut self, name: String, id: DefId) {
        self.bindings.insert(name, id);
    }

    /// Look up a name in this scope only (does not walk outward).
    pub fn get(&self, name: &str) -> Option<DefId> {
        self.bindings.get(name).copied()
    }

    /// Returns `true` if a name is already defined in this scope.
    /// Used to detect duplicate definitions before inserting.
    pub fn contains(&self, name: &str) -> bool {
        self.bindings.contains_key(name)
    }
}

/// The scope stack maintained during the resolution walk.
/// The last element is the innermost (current) scope.
#[derive(Debug, Default)]
pub struct ScopeStack {
    scopes: Vec<Scope>,
}

impl ScopeStack {
    pub fn new() -> Self { ScopeStack::default() }

    /// Enter a new scope (e.g. a function body or block).
    pub fn push(&mut self) {
        self.scopes.push(Scope::new());
    }

    /// Exit the current scope.
    pub fn pop(&mut self) {
        self.scopes.pop();
    }

    /// Define a name in the *current* (innermost) scope.
    /// Returns the existing `DefId` if the name was already defined in this scope
    /// (for duplicate-definition error reporting), or `None` if the insert succeeded.
    pub fn define(&mut self, name: String, id: DefId) -> Option<DefId> {
        let scope = self.scopes.last_mut().expect("define called with empty scope stack");
        if scope.contains(&name) {
            return Some(scope.get(&name).unwrap());
        }
        scope.define(name, id);
        None
    }

    /// Resolve a name by walking from the innermost scope outward.
    /// Returns `None` if the name is not defined in any enclosing scope.
    pub fn resolve(&self, name: &str) -> Option<DefId> {
        for scope in self.scopes.iter().rev() {
            if let Some(id) = scope.get(name) {
                return Some(id);
            }
        }
        None
    }

    /// How many scopes are currently on the stack.
    pub fn depth(&self) -> usize { self.scopes.len() }
                                     }
