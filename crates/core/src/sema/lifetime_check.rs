// src/sema/lifetime_check.rs
//! Well-formedness checking for `[lifetime L]` / `[lifetime L where L
//! outlives M]` declarations on functions and `edge struct`s.
//!
//! Purely structural: no CFG, no liveness, no type information needed,
//! runs directly over the raw AST, early in the pipeline (right after
//! name resolution, before type inference). Checks, per declaration (a
//! function's own `lifetime_params`, or a struct's own):
//!   - no lifetime name declared twice
//!   - every name used in a `where X outlives Y` clause is one of that
//!     same declaration's own declared names
//!   - the declared outlives constraints don't form a cycle (including
//!     the trivial self-outlives case, `L outlives L`)
//!   - every `&name T`/`ref name T` written directly in that same
//!     declaration's own signature (a function's param/return types) or
//!     fields (a struct's field types), however deeply nested inside
//!     other types, names one of the declared lifetimes
//!
//! Does NOT check that real usage in a function body actually respects
//! a declared outlives bound, that's the actual outlives/subset fixed
//! point, still the borrow checker's job, and still unbuilt (see
//! docs/MEMORY_MODEL.md §12, Open Decision left for a later slice).
//! Does NOT look at method bodies or param/return types at all:
//! `MethodDecl` has no `lifetime_params` of its own (only an enclosing
//! struct can declare any), and checking a method's own `&name` usage
//! against its enclosing struct's declared names needs a scope-
//! inheritance story this pass doesn't build yet, a real, separate,
//! documented follow-up, not silently assumed safe.

use std::collections::{HashMap, HashSet};

use crate::ast::common::{LifetimeParam, Span};
use crate::ast::declarations::{FunctionDecl, ParamKind, StructDecl, StructMember};
use crate::ast::root::{Item, Program};
use crate::ast::types::{Type, TypeKind};
use crate::error_management::{errors::LifetimeError, ErrorManager};

pub fn check(program: &Program, errors: &mut ErrorManager) {
    for item in program.items {
        match item {
            Item::Function(f) => check_function(f, errors),
            Item::Struct(s) => check_struct(s, errors),
            _ => {}
        }
    }
}

fn check_function<'ast>(f: &FunctionDecl<'ast>, errors: &mut ErrorManager) {
    let declared = check_declaration_well_formed(f.lifetime_params, errors);
    for p in f.params {
        let ty = match p.kind {
            ParamKind::Named { ty, .. } | ParamKind::Discard { ty } => ty,
            ParamKind::SelfVal | ParamKind::SelfMut | ParamKind::SelfRef | ParamKind::SelfRefMut => None,
        };
        if let Some(t) = ty {
            check_type_lifetimes(t, &declared, errors);
        }
    }
    if let Some(rt) = &f.return_type {
        check_type_lifetimes(rt.ty, &declared, errors);
    }
}

fn check_struct<'ast>(s: &StructDecl<'ast>, errors: &mut ErrorManager) {
    let declared = check_declaration_well_formed(s.lifetime_params, errors);
    for m in s.members {
        if let StructMember::Field(field) = m {
            check_type_lifetimes(field.ty, &declared, errors);
        }
    }
}

/// Checks a `[lifetime ...]` list itself is well-formed (no duplicate
/// names, every `where` clause name declared, no outlives cycle), and
/// returns the set of names it declares, for the caller to check its
/// own signature/field types against. A no-op returning an empty set
/// for a declaration with no lifetime params at all.
fn check_declaration_well_formed<'ast>(
    params: &'ast [LifetimeParam<'ast>],
    errors: &mut ErrorManager,
) -> HashSet<&'ast str> {
    let mut first_seen: HashMap<&'ast str, Span> = HashMap::new();
    for p in params {
        if let Some(&first_span) = first_seen.get(p.name) {
            errors.add_lifetime_error(LifetimeError::DuplicateLifetimeParam {
                name: p.name.to_string(),
                first_span,
                dup_span: p.span,
            });
        } else {
            first_seen.insert(p.name, p.span);
        }
    }
    let declared: HashSet<&'ast str> = first_seen.keys().copied().collect();

    let mut edges: Vec<(&'ast str, &'ast str, Span)> = Vec::new();
    for p in params {
        let Some(c) = &p.constraint else { continue };
        let mut names_to_check: Vec<&'ast str> = vec![c.longer];
        if c.shorter != c.longer {
            names_to_check.push(c.shorter);
        }
        for name in names_to_check {
            if !declared.contains(name) {
                errors.add_lifetime_error(LifetimeError::UndeclaredLifetime {
                    name: name.to_string(),
                    span: c.span,
                });
            }
        }
        if declared.contains(c.longer) && declared.contains(c.shorter) {
            edges.push((c.longer, c.shorter, c.span));
        }
    }

    if let Some((cycle, span)) = find_outlives_cycle(&declared, &edges) {
        errors.add_lifetime_error(LifetimeError::OutlivesCycle {
            names: cycle.into_iter().map(|s| s.to_string()).collect(),
            span,
        });
    }

    declared
}

/// Returns the lifetime names forming a cycle, and the span of the edge
/// that closes it, if the given outlives edges (each `(longer, shorter,
/// span)`, meaning `longer outlives shorter`) contain one. Plain DFS
/// over what's always a tiny graph, a handful of declared lifetimes at
/// most per declaration, not anything perf-sensitive.
fn find_outlives_cycle<'ast>(
    names: &HashSet<&'ast str>,
    edges: &[(&'ast str, &'ast str, Span)],
) -> Option<(Vec<&'ast str>, Span)> {
    let mut adj: HashMap<&'ast str, Vec<(&'ast str, Span)>> = HashMap::new();
    for &(from, to, span) in edges {
        adj.entry(from).or_default().push((to, span));
    }

    let mut visited: HashSet<&'ast str> = HashSet::new();
    for &start in names {
        if visited.contains(start) {
            continue;
        }
        let mut path: Vec<&'ast str> = Vec::new();
        let mut on_path: HashSet<&'ast str> = HashSet::new();
        if let Some(found) = dfs_find_cycle(start, &adj, &mut path, &mut on_path, &mut visited) {
            return Some(found);
        }
    }
    None
}

fn dfs_find_cycle<'ast>(
    node: &'ast str,
    adj: &HashMap<&'ast str, Vec<(&'ast str, Span)>>,
    path: &mut Vec<&'ast str>,
    on_path: &mut HashSet<&'ast str>,
    visited: &mut HashSet<&'ast str>,
) -> Option<(Vec<&'ast str>, Span)> {
    path.push(node);
    on_path.insert(node);
    visited.insert(node);

    if let Some(neighbors) = adj.get(node) {
        for &(next, span) in neighbors {
            if on_path.contains(next) {
                let start_idx = path.iter().position(|&n| n == next).unwrap();
                return Some((path[start_idx..].to_vec(), span));
            }
            if !visited.contains(next) {
                if let Some(found) = dfs_find_cycle(next, adj, path, on_path, visited) {
                    return Some(found);
                }
            }
        }
    }

    path.pop();
    on_path.remove(node);
    None
}

/// Walks a type expression looking for `Reference { lifetime: Some(name),
/// .. }` nodes, however deeply nested inside other types (`List<&L T>`,
/// a tuple element, a function type's own param/return, ...), and checks
/// each named lifetime resolves against `declared`. An unnamed `&T`/
/// `&mut T` (`lifetime: None`) is never checked, nothing to validate.
fn check_type_lifetimes<'ast>(
    ty: &'ast Type<'ast>,
    declared: &HashSet<&'ast str>,
    errors: &mut ErrorManager,
) {
    match ty.kind {
        TypeKind::Reference { lifetime, inner, .. } => {
            if let Some(name) = lifetime {
                if !declared.contains(name) {
                    errors.add_lifetime_error(LifetimeError::UndeclaredLifetime {
                        name: name.to_string(),
                        span: ty.span,
                    });
                }
            }
            check_type_lifetimes(inner, declared, errors);
        }
        TypeKind::List(Some(t))
        | TypeKind::Set(Some(t))
        | TypeKind::Queue(Some(t))
        | TypeKind::Stack(Some(t))
        | TypeKind::InlineList(Some(t))
        | TypeKind::Task(Some(t))
        | TypeKind::Slice(t)
        | TypeKind::Fallible(t)
        | TypeKind::Optional(t) => check_type_lifetimes(t, declared, errors),
        TypeKind::Dictionary(Some((k, v))) => {
            check_type_lifetimes(k, declared, errors);
            check_type_lifetimes(v, declared, errors);
        }
        TypeKind::Named { args, .. } => {
            for a in args {
                check_type_lifetimes(a, declared, errors);
            }
        }
        TypeKind::Tuple(elems) => {
            for e in elems {
                check_type_lifetimes(e, declared, errors);
            }
        }
        TypeKind::Array { elem, .. } => check_type_lifetimes(elem, declared, errors),
        TypeKind::Function(ft) => {
            for p in ft.params {
                check_type_lifetimes(p, declared, errors);
            }
            if let Some(rt) = ft.return_type {
                check_type_lifetimes(rt, declared, errors);
            }
        }
        _ => {} // primitives, Infer, empty-generic collection forms, etc.
    }
}
