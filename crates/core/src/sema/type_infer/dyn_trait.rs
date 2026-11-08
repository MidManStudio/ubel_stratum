// src/sema/type_infer/dyn_trait.rs
//! `dyn Trait` (traits slice S6). See docs/TRAITS_DESIGN.md, "What S6 settled".
//!
//! A `dyn Trait` type stands for a value of any type that implements the
//! trait, the type being known only at run time. Three rules make it work:
//!
//! - It is checked where it is written. The trait has to be object safe
//!   (`TYPE-138`), by Rust's rule: every method takes `self`, has no generic
//!   parameters of its own and does not mention `Self` outside the receiver,
//!   and the same holds for each trait it builds on.
//! - A value becomes a `dyn` value by coercion at a typed site
//!   ([`InferCtx::unify_coerce`]): a binding with a written type, an argument,
//!   a return, a field, an element handed to a collection. The wrappers on
//!   both sides have to be the same, so a coercion never changes a tier.
//! - A call, an operator, an index or a `for` on a `dyn` value goes through the
//!   trait's own methods, exactly as through a bound on a type parameter.
//!
//! The run time needs nothing of its own. It already finds every method from
//! the value's own type, so a `dyn` value is just the value.

use super::*;

impl<'a> InferCtx<'a> {
    /// Lower `dyn Trait<args>` written at `span`: resolve the trait, check how
    /// many arguments it takes, and queue the object-safety check (a trait
    /// declared further down the file has no methods collected yet while
    /// signatures are being collected).
    pub(super) fn lower_dyn_type<'ast>(&mut self, name: &str, args: &[&'ast Type<'ast>], span: Span) -> TypeId {
        // A written type can be lowered more than once; it is reported once.
        let fresh = self.dyn_seen.insert(span);
        let trait_def = match self.resolve_trait_name(name) {
            Ok(id) => id,
            Err(found) => {
                if fresh {
                    self.errors.add_type_error(TypeError::NotATrait {
                        name: name.to_string(), found, span,
                    });
                }
                return self.unknown();
            }
        };
        let arity = self.trait_arity.get(&trait_def).copied().unwrap_or(args.len());
        if fresh && args.len() != arity {
            self.errors.add_type_error(TypeError::GenericArgCountMismatch {
                type_name: name.to_string(),
                expected:  arity,
                found:     args.len(),
                span,
            });
        }
        let targs = self.trait_arg_types(args, arity);
        if fresh {
            self.dyn_uses.push((trait_def, span));
            self.check_dyn_uses(false);
        }
        self.ctx.types.insert(SemaType::Dyn { trait_def, args: targs })
    }

    /// Check the queued `dyn` types for object safety. A use whose trait (or
    /// one it builds on) has not been collected yet waits, unless `force`.
    pub(super) fn check_dyn_uses(&mut self, force: bool) {
        if self.dyn_uses.is_empty() { return; }
        let pending = std::mem::take(&mut self.dyn_uses);
        for (trait_def, span) in pending {
            if !force && !self.dyn_trait_ready(trait_def) {
                self.dyn_uses.push((trait_def, span));
                continue;
            }
            let reasons = self.dyn_unsafe_reasons(trait_def);
            if !reasons.is_empty() {
                let trait_name = self.trait_display_name(trait_def);
                self.errors.add_type_error(TypeError::NotObjectSafe { trait_name, reasons, span });
            }
        }
    }

    /// The trait and every trait it builds on, the trait itself first.
    fn dyn_trait_family(&self, trait_def: DefId) -> Vec<DefId> {
        let mut order = vec![trait_def];
        let mut i = 0;
        while i < order.len() {
            let t = order[i];
            i += 1;
            for s in self.trait_supers.get(&t).into_iter().flatten() {
                if !order.contains(s) { order.push(*s); }
            }
        }
        order
    }

    fn dyn_trait_ready(&self, trait_def: DefId) -> bool {
        self.dyn_trait_family(trait_def).iter().all(|t| self.traits.contains_key(t))
    }

    /// Why `dyn Trait` is not allowed, one entry per offending method, for the
    /// trait and then for each trait it builds on. Empty when it is allowed.
    fn dyn_unsafe_reasons(&self, trait_def: DefId) -> Vec<String> {
        let mut reasons: Vec<String> = Vec::new();
        for t in self.dyn_trait_family(trait_def) {
            let Some(info) = self.traits.get(&t) else { continue; };
            // The abstract `Self` is the parameter after the trait's own.
            let self_idx = info.arity;
            for m in &info.methods {
                let label = format!("{}.{}", info.name, m.name);
                if !m.shape.has_self {
                    reasons.push(format!("`{}` has no `self` receiver", label));
                } else if !m.shape.generics.own.is_empty() {
                    reasons.push(format!("`{}` has generic parameters of its own", label));
                } else if m.shape.params.iter()
                    .chain(std::iter::once(&m.shape.return_type))
                    .any(|ty| self.mentions_param_index(*ty, self_idx))
                {
                    reasons.push(format!("`{}` mentions `Self` outside its receiver", label));
                }
            }
        }
        reasons
    }

    /// The traits a `dyn` type stands for: its own and every one it builds on,
    /// with their arguments. `None` when `ty` is not a `dyn` type.
    pub(super) fn dyn_bounds(&mut self, ty: TypeId) -> Option<Vec<BoundRef>> {
        let SemaType::Dyn { trait_def, args } = self.ctx.types.get(ty).clone() else { return None; };
        Some(self.supertrait_closure(&[BoundRef { trait_def, args }], ty))
    }

    /// `unify(expected, found)` at a site where a value may also become a `dyn`
    /// value: the found type is accepted when the expected one is `dyn Trait`
    /// (inside the same wrappers) and the found type implements the trait.
    pub(super) fn unify_coerce(&mut self, expected: TypeId, found: TypeId, span: Span) {
        if self.try_dyn_coercion(expected, found, span) { return; }
        self.unify(expected, found, span);
    }

    /// Whether `found` was accepted as a `dyn` value of `expected`. `false`
    /// leaves the pair to ordinary unification, which is also what reports a
    /// mismatch. Only the wrappers present on both sides are looked through,
    /// so `Unique<Circle>` becomes `Unique<dyn Shape>` but a bare `Circle`
    /// does not become a `Unique`, and a value built in an arena does not
    /// become a plain `dyn` value (the found side carries a wrapper the
    /// expected side lacks, which is left to ordinary unification).
    pub(super) fn try_dyn_coercion(&mut self, expected: TypeId, found: TypeId, span: Span) -> bool {
        if expected.is_error() || found.is_error() { return false; }
        let mut e = self.apply(expected);
        let mut f = self.apply(found);
        loop {
            let et = self.ctx.types.get(e).clone();
            let ft = self.ctx.types.get(f).clone();
            let (ie, inner_f) = match (et, ft) {
                (SemaType::Dyn { .. }, _) => break,
                (SemaType::Unique(a), SemaType::Unique(b))
                | (SemaType::Shared(a), SemaType::Shared(b))
                | (SemaType::SyncShared(a), SemaType::SyncShared(b))
                | (SemaType::Optional(a), SemaType::Optional(b)) => (a, b),
                (SemaType::Reference { mutable: m, inner: a, .. }, SemaType::Reference { mutable: n, inner: b, .. })
                    if m == n => (a, b),
                // A plain value handed to an optional: `let s: dyn Shape? = circle`.
                (SemaType::Optional(a), other)
                    if !matches!(other, SemaType::Optional(_) | SemaType::Null
                        | SemaType::Var(_) | SemaType::Unknown) =>
                {
                    e = self.apply(a);
                    continue;
                }
                _ => return false,
            };
            e = self.apply(ie);
            f = self.apply(inner_f);
        }
        match self.ctx.types.get(f).clone() {
            // A wrapper the expected type does not have is a tier or ownership
            // difference, not a coercion; `dyn` to `dyn` is plain unification.
            SemaType::GcRef(_) | SemaType::Unique(_) | SemaType::Shared(_) | SemaType::SyncShared(_)
            | SemaType::ArenaRef { .. } | SemaType::PoolRef { .. } | SemaType::OwnedRef { .. }
            | SemaType::Reference { .. } | SemaType::Dyn { .. } | SemaType::Null
            // Already failed to type: ordinary unification absorbs it.
            | SemaType::Unknown => return false,
            _ => {}
        }
        let SemaType::Dyn { trait_def, args } = self.ctx.types.get(e).clone() else { return false; };
        self.require_bound_args(f, trait_def, args, span);
        true
    }

    /// The collection methods that take an element: the argument has to be
    /// accepted as the collection's element type, which a `dyn` element makes
    /// a real requirement (`shapes.push(5)` on a `List<dyn Shape>`). Nothing
    /// is checked for an element type that is not a `dyn` type, as before.
    pub(super) fn check_dyn_element_arg<'ast>(
        &mut self,
        kind:   instance::ReceiverKind,
        method: &str,
        recv:   TypeId,
        args:   &[Arg<'ast>],
    ) {
        let slot = match (kind, method, self.ctx.types.get(recv).clone()) {
            (instance::ReceiverKind::List,       "push",    SemaType::List(e))       => Some((0, e)),
            (instance::ReceiverKind::List,       "set",     SemaType::List(e))       => Some((1, e)),
            (instance::ReceiverKind::Queue,      "enqueue", SemaType::Queue(e))      => Some((0, e)),
            (instance::ReceiverKind::Stack,      "push",    SemaType::Stack(e))      => Some((0, e)),
            (instance::ReceiverKind::InlineList, "push",    SemaType::InlineList(e)) => Some((0, e)),
            (instance::ReceiverKind::Dict,       "set",     SemaType::Dictionary(_, v)) => Some((1, v)),
            _ => None,
        };
        let Some((pos, elem)) = slot else { return; };
        let Some(arg) = args.get(pos) else { return; };
        let arg_span = match &arg.kind {
            ArgKind::Positional(e)       => e.span,
            ArgKind::Named { value, .. } => value.span,
        };
        let Some(arg_ty) = self.ctx.expr_type(arg_span).map(|t| self.apply(t)) else { return; };
        self.try_dyn_coercion(elem, arg_ty, arg_span);
    }

    /// Whether `ty` is a `dyn` type or has one anywhere inside it.
    fn mentions_dyn(&self, ty: TypeId) -> bool {
        matches!(self.ctx.types.get(ty), SemaType::Dyn { .. })
            || self.type_children(ty).into_iter().any(|c| self.mentions_dyn(c))
    }

    /// The element type an array literal has to be coerced to, for a `let`
    /// whose written type is a list, array or slice with a `dyn` in its
    /// element type. `None` for every other element type, which keeps the
    /// literal's own inference.
    pub(super) fn array_hint_elem(&mut self, annotated: TypeId) -> Option<TypeId> {
        let t = self.apply(annotated);
        let elem = match self.ctx.types.get(t).clone() {
            SemaType::List(e) | SemaType::Slice(e) | SemaType::Array { elem: e, .. } => self.apply(e),
            _ => return None,
        };
        if self.mentions_dyn(elem) { Some(elem) } else { None }
    }
}
