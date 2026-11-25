// src/sema/type_infer/marker_traits.rs
//! `Send` and `Sync`, the two marker traits behind the sendable rule
//! (docs/ASYNC_SYNC_DESIGN.md, AS4a).
//!
//! `Send` says a value may cross into another task; `Sync` says a shared view of it may be used
//! by one. Both are prelude traits with no methods, and a struct or an enum has one only when it
//! says so with an `impl`: nothing is derived from the fields. The impl is checked against the
//! fields, though (`TYPE-145`): `impl Send for Job {}` is an error when `Job` holds something that
//! is not `Send`. `unsafe impl Send for Job {}` is trusted and not checked.
//!
//! A built-in type has no impl to write, and none is accepted (`TYPE-129`): what it is follows
//! from its parts, in `marker_status`.
//!
//! - Scalars, `string`, `char`, `bool` and `void`: both.
//! - A tuple, an array, an optional, a fallible, a `List`, `Set`, `Queue`, `Stack`, `InlineList` or
//!   `Dictionary`: the one it is made of, when every part has it.
//! - `Unique<T>`: what `T` is. `SyncShared<T>`: both, when `T` is `Send` (the lock is what makes
//!   sharing it safe).
//! - `Shared<T>`, a reference, a borrowed view, a `Pool`, a `Handle`, a `Linqerizer`, a function
//!   value and a task: neither.
//! - A struct or an enum: what its `impl` says, with the conditions the impl wrote on its
//!   parameters. The prelude's own plain types (`Ordering`) have both.
//! - A type parameter: what its bounds say. A `dyn Trait`: what the trait builds on.

use super::*;

pub(super) const MARKER_ON_BUILTIN: &str = "implementing `Send` or `Sync` by hand for a built-in type (a built-in type is `Send` and `Sync` when its parts are, and an impl cannot say otherwise)";

impl<'a> InferCtx<'a> {
    /// Whether `ty` has the marker trait `name` (`"Send"` or `"Sync"`): `Some(true)` or
    /// `Some(false)` once that can be told, `None` while it cannot (an unresolved inference
    /// variable, or a named type before the impls are registered). An unknown or error type has
    /// it, so an earlier error does not cascade into this one.
    pub(super) fn marker_status(&mut self, name: &'static str, ty: TypeId) -> Option<bool> {
        let ty = self.apply(ty);
        if ty == TypeId::ERROR { return Some(true); }
        match self.ctx.types.get(ty).clone() {
            SemaType::Unknown | SemaType::Null => Some(true),
            SemaType::Var(n) => if self.int_lit_vars.contains_key(&n) { Some(true) } else { None },
            SemaType::Int | SemaType::Uint | SemaType::Long | SemaType::Ulong
            | SemaType::Float | SemaType::Double
            | SemaType::Bool | SemaType::Char | SemaType::Str | SemaType::Void
            | SemaType::I8 | SemaType::I16 | SemaType::I32 | SemaType::I64
            | SemaType::U8 | SemaType::U16 | SemaType::U32 | SemaType::U64
            | SemaType::F32 | SemaType::F64 | SemaType::Isize | SemaType::Usize => Some(true),
            SemaType::Tuple(elems) => self.marker_all(name, &elems),
            SemaType::Array { elem, .. } => self.marker_status(name, elem),
            SemaType::Optional(t) | SemaType::Fallible(t)
            | SemaType::List(t) | SemaType::Set(t) | SemaType::Queue(t) | SemaType::Stack(t)
            | SemaType::InlineList(t) | SemaType::Unique(t) => self.marker_status(name, t),
            SemaType::Dictionary(k, v) => self.marker_all(name, &[k, v]),
            // A lock serialises every access, so sharing a `SyncShared<T>` needs only `T: Send`.
            SemaType::SyncShared(t) => self.marker_status("Send", t),
            SemaType::Param(_) => {
                let marker = self.marker_defs.get(name).copied();
                Some(marker.is_some_and(|d| self.current_bounds.get(&ty)
                    .is_some_and(|bs| bs.iter().any(|b| b.trait_def == d))))
            }
            SemaType::Dyn { .. } => {
                let marker = self.marker_defs.get(name).copied();
                let bounds = self.dyn_bounds(ty).unwrap_or_default();
                Some(marker.is_some_and(|d| bounds.iter().any(|b| b.trait_def == d)))
            }
            SemaType::Named { def, args } => self.marker_named(name, def, &args),
            // `Shared`, every reference and ownership handle, a borrowed view, a pool, a query, a
            // function value, a task.
            _ => Some(false),
        }
    }

    /// Every type in the list has the marker: `Some(false)` as soon as one lacks it, `None` while
    /// one is still unknown.
    fn marker_all(&mut self, name: &'static str, tys: &[TypeId]) -> Option<bool> {
        let mut all = Some(true);
        for t in tys {
            match self.marker_status(name, *t) {
                Some(false) => return Some(false),
                None        => all = None,
                Some(true)  => {}
            }
        }
        all
    }

    /// A struct or an enum has the marker when an `impl` says so, under the conditions the impl
    /// wrote on its arguments. The prelude's own types need no impl.
    fn marker_named(&mut self, name: &'static str, def: DefId, args: &[TypeId]) -> Option<bool> {
        if !matches!(self.ctx.symbols.lookup(def).kind, DefKind::Struct { .. } | DefKind::Enum) {
            return Some(false);
        }
        let Some(marker) = self.marker_defs.get(name).copied() else { return Some(false); };
        if self.trait_impls.contains_key(&(marker, def)) {
            return self.impl_status(marker, def, args);
        }
        if self.ctx.symbols.lookup(def).defined_at.is_prelude() { return Some(true); }
        if self.impls_registered { Some(false) } else { None }
    }

    /// The first part of a struct or an enum that lacks the marker: a field of a struct, a
    /// positional or a named field of a variant. The text names the part, and the type is the
    /// part's type with the impl's own parameters in it.
    fn marker_offender(&mut self, name: &'static str, self_ty: TypeId) -> Option<(String, TypeId)> {
        let self_ty = self.apply(self_ty);
        let SemaType::Named { def, args } = self.ctx.types.get(self_ty).clone() else { return None; };
        let is_struct = matches!(self.ctx.symbols.lookup(def).kind, DefKind::Struct { .. });
        let is_enum   = matches!(self.ctx.symbols.lookup(def).kind, DefKind::Enum);
        match () {
            _ if is_struct => {
                let fields = self.struct_fields.get(&def).cloned().unwrap_or_default();
                for (field, raw) in fields {
                    let ty = self.substitute(raw, &args);
                    if self.marker_status(name, ty) == Some(false) {
                        return Some((format!("field `{}`", field), ty));
                    }
                }
                None
            }
            _ if is_enum => {
                let variants = self.enum_variants.get(&def).cloned().unwrap_or_default();
                for (variant, shape) in variants {
                    match shape {
                        VariantShape::Tuple(tys) => {
                            for (at, raw) in tys.into_iter().enumerate() {
                                let ty = self.substitute(raw, &args);
                                if self.marker_status(name, ty) == Some(false) {
                                    return Some((format!("field {} of variant `{}`", at, variant), ty));
                                }
                            }
                        }
                        VariantShape::Struct(fields) => {
                            for (field, raw) in fields {
                                let ty = self.substitute(raw, &args);
                                if self.marker_status(name, ty) == Some(false) {
                                    return Some((format!("field `{}` of variant `{}`", field, variant), ty));
                                }
                            }
                        }
                        _ => {}
                    }
                }
                None
            }
            _ => None,
        }
    }

    /// Check every `impl Send` and `impl Sync` against the fields of its type (`TYPE-145`), and
    /// every `unsafe impl` against the traits that may be unsafe (`TYPE-146`). Runs once the impls
    /// are registered, so a field of a type declared later is known. An `unsafe impl` of `Send` or
    /// `Sync` is trusted. An impl that registration refused (an overlap, a built-in target) is
    /// not checked again: one mistake is one diagnostic.
    pub(super) fn check_marker_impls<'ast>(&mut self, program: &Program<'ast>) {
        for item in program.items {
            let Item::Impl(i) = item else { continue; };
            let trait_def = i.trait_path
                .and_then(|p| p.last().copied())
                .and_then(|n| self.resolve_trait_name(n).ok());
            let marker = trait_def.and_then(|d| self.marker_traits.get(&d).copied());
            if i.is_unsafe && marker.is_none() {
                let found = match i.trait_path.and_then(|p| p.last().copied()) {
                    Some(t) => format!("an impl of `{}`", t),
                    None    => "an inherent impl".to_string(),
                };
                self.errors.add_type_error(TypeError::UnsafeImplNotMarker { found, span: i.span });
                continue;
            }
            let (Some(name), Some(trait_def)) = (marker, trait_def) else { continue; };
            if i.is_unsafe || self.impl_failed.contains(&i.span) { continue; }

            let pattern = self.impl_patterns.get(&i.span).cloned();
            let target = match &pattern {
                Some(p) if p.builtin => None,
                Some(p)              => Some(p.head),
                None                 => self.target_type_def_id(i.target_type),
            };
            let Some(target) = target else { continue; };
            if self.trait_impls.get(&(trait_def, target)) != Some(&i.span) { continue; }

            // An impl for a generic head is read in its own scope, with the bounds it wrote in
            // force: `impl<T: Send> Send for Box<T>` has a `T` that is `Send`.
            let scope = pattern.as_ref().map(|p| self.enter_impl_scope(p));
            let self_ty = match &pattern {
                Some(p) => p.self_ty,
                None    => self.ctx.types.insert(SemaType::Named { def: target, args: Vec::new() }),
            };
            // The names are written while the impl's parameters are still in scope: `T`, not
            // the number of the parameter.
            let offender = self.marker_offender(name, self_ty)
                .map(|(part, ty)| (part, self.display_type(ty)));
            let type_name = self.display_type(self_ty);
            if let Some(scope) = scope { self.leave_impl_scope(scope); }
            if let Some((part, part_type)) = offender {
                self.errors.add_type_error(TypeError::MarkerImplField {
                    marker: name.to_string(), type_name, part, part_type, span: i.span,
                });
            }
        }
    }
}
