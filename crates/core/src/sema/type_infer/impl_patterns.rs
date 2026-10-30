// src/sema/type_infer/impl_patterns.rs
//! Impls and `extend` blocks for generic types (traits slice S3e).
//!
//! `impl<T: Show> Show for Box<T>`, `impl Show for Box<int>`,
//! `extend<T> List<T> { .. }`. See docs/TRAITS_DESIGN.md, "What S3e settled".
//!
//! # The idea
//!
//! A generic head (a generic struct or enum, or one of the built-in generic
//! heads `List`, `Dictionary`, `Queue`, `Stack`, `InlineList`) is given the
//! same tables a non-generic type has: its methods in `struct_methods`, its
//! impls in `trait_impls`, `trait_impl_args` and the ambiguity tables. What is
//! new is what an impl says about the head's ARGUMENTS, and that lives in an
//! [`ImplPattern`] built once per block:
//!
//! - Each impl parameter occupies exactly one top-level argument position of
//!   the head and is numbered by that position (`impl<U> Tr for Pair<int, U>`
//!   makes `U` the parameter `Param(1)`), so every method shape stays in the
//!   head's own numbering and a call substitutes the receiver's arguments
//!   exactly as it does for a method written inside the struct.
//! - An argument written as a concrete type (`Box<int>`) is a concrete slot.
//! - What the impl asks of the arguments (concrete slots, bounds written on
//!   its parameters) is an [`ImplGuard`], attached to every method the block
//!   supplies and to the `(trait, head)` pair. A guard is present only when
//!   there is something to check, so `impl<T> Tr for Box<T>` costs nothing.
//!
//! Generics are erased at run time, so a trait has one impl per head whatever
//! the arguments, and one method name has one inherent definition per head.
//! The second is reported with the run-time reason.

use std::rc::Rc;

use super::*;

// ── Feature texts for `TYPE-129` ─────────────────────────────────

pub(super) const TARGET_FOR_TRAIT: &str = "implementing a trait for a type that is not a struct, an enum, a primitive, a tuple, an array, a List, a Dictionary, a Queue, a Stack or an InlineList";
pub(super) const TARGET_FOR_EXTEND: &str = "extending a type that is not a struct, an enum, a primitive, a tuple, an array, a List, a Dictionary, a Queue, a Stack or an InlineList";
const WRAPPER_FOR_TRAIT: &str = "implementing a trait for an optional, a fallible or a task type, which are transparent wrappers (a method call on one resolves on the value inside it)";
const WRAPPER_FOR_EXTEND: &str = "extending an optional, a fallible or a task type, which are transparent wrappers (a method call on one resolves on the value inside it)";
const POOL_FOR_TRAIT: &str = "implementing a trait for a Pool or a Handle (a written `Pool<T>` or `Handle<T>` is not a type of its own yet)";
const POOL_FOR_EXTEND: &str = "extending a Pool or a Handle (a written `Pool<T>` or `Handle<T>` is not a type of its own yet)";
const BLANKET: &str = "a blanket impl (a type parameter as the impl target)";
const NESTED_OR_REPEATED: &str = "an impl target that repeats a type parameter or nests one inside another type (`Pair<T, T>`, `Box<List<T>>`)";
const UNUSED_PARAM: &str = "an impl type parameter that the target type does not use";
pub(super) const DUPLICATE_METHOD: &str = "an inherent method with the name of one that another block already gives the same type (a call cannot tell `Box<int>` from `Box<string>`, or `int` from `long`, at run time)";
pub(super) const DISPLAY_ON_BUILTIN: &str = "implementing `Display` by hand for a built-in type (only a struct or an enum can have a hand-written `Display`; a built-in type prints as it always does)";
pub(super) const DISPLAY_ON_COLLECTION: &str = "implementing `Display` by hand for a collection (a collection is printed element by element at run time, and a user-written `to_string` for it would not be called)";
pub(super) const PRELUDE_ON_COLLECTION: &str = "implementing a prelude trait by hand for a collection (a collection is compared, hashed and cloned by identity at run time)";

// ── Data ─────────────────────────────────────────────────────────

/// What an impl asks of the arguments of the type it is written for.
/// Both lists have one entry per argument position of the head.
pub(super) struct ImplGuard {
    /// `Some(ty)` where the impl is written for one concrete argument
    /// (`Box<int>`), `None` for a parameter or an unconstrained position.
    pub(super) concrete: Vec<Option<TypeId>>,
    /// The bounds the impl wrote on the parameter at each position, as
    /// written and in the head's numbering.
    pub(super) bounds: Vec<Vec<BoundRef>>,
}

/// One `impl` or `extend` block for a generic head, analysed once.
pub(super) struct ImplPattern {
    /// The definition the block's methods and impls are filed under: the
    /// struct or enum, or the synthetic one for a built-in head.
    pub(super) head: DefId,
    /// How many type arguments the head has.
    pub(super) arity: usize,
    /// The head applied to `Param(i)` (a parameter or an unconstrained
    /// position) or to the concrete type written there.
    pub(super) self_ty: TypeId,
    /// Whether the head is one of the built-in generic heads.
    pub(super) builtin: bool,
    /// Each impl parameter's written name and the position it occupies.
    pub(super) names: Vec<(String, usize)>,
    /// The bounds in force inside the block: the impl's own, with what they
    /// build on, and the head's declared ones.
    pub(super) scope_bounds: HashMap<TypeId, Vec<BoundRef>>,
    pub(super) guard: Option<Rc<ImplGuard>>,
}

/// The state `enter_impl_scope` replaced, handed back to `leave_impl_scope`.
pub(super) struct ImplScope {
    names:               HashMap<String, TypeId>,
    bounds:              HashMap<TypeId, Vec<BoundRef>>,
    param_names:         HashMap<TypeId, String>,
    self_type:           Option<TypeId>,
    current_struct_type: Option<TypeId>,
}

impl<'a> InferCtx<'a> {
    // ── Small type helpers ───────────────────────────────────────

    /// The name of a built-in generic head, for the types that have one.
    pub(super) fn builtin_head_name(&self, ty: TypeId) -> Option<&'static str> {
        match self.ctx.types.get(ty) {
            SemaType::List(_)       => Some("List"),
            SemaType::Dictionary(..) => Some("Dictionary"),
            SemaType::Queue(_)      => Some("Queue"),
            SemaType::Stack(_)      => Some("Stack"),
            SemaType::InlineList(_) => Some("InlineList"),
            _ => None,
        }
    }

    /// The type arguments of a built-in generic head (none for any other
    /// type), in the head's own order.
    pub(super) fn builtin_head_args(&self, ty: TypeId) -> Vec<TypeId> {
        match self.ctx.types.get(ty) {
            SemaType::List(e) | SemaType::Queue(e) | SemaType::Stack(e)
            | SemaType::InlineList(e)            => vec![*e],
            SemaType::Dictionary(k, v)           => vec![*k, *v],
            _ => Vec::new(),
        }
    }

    /// The head applied to `args`: the inverse of `builtin_head_args`.
    fn builtin_head_applied(&mut self, name: &str, args: &[TypeId]) -> TypeId {
        let ty = match name {
            "List"       => SemaType::List(args[0]),
            "Dictionary" => SemaType::Dictionary(args[0], args[1]),
            "Queue"      => SemaType::Queue(args[0]),
            "Stack"      => SemaType::Stack(args[0]),
            _            => SemaType::InlineList(args[0]),
        };
        self.ctx.types.insert(ty)
    }

    /// The synthetic definition of a built-in generic head, made on first
    /// use. One per head, whatever its arguments.
    fn builtin_head_def(&mut self, name: &'static str, arity: usize, span: Span) -> DefId {
        if let Some(d) = self.builtin_defs.get(name) { return *d; }
        let d = self.ctx.symbols.insert(
            name.to_string(), DefKind::BuiltinType, span, Visibility::Private,
        );
        self.builtin_defs.insert(name.to_string(), d);
        let params: Vec<TypeId> = (0..arity)
            .map(|i| self.ctx.types.intern(SemaType::Param(i)))
            .collect();
        let ty = self.builtin_head_applied(name, &params);
        self.builtin_def_types.insert(d, ty);
        d
    }

    /// The types directly inside `ty`.
    fn type_children(&self, ty: TypeId) -> Vec<TypeId> {
        match self.ctx.types.get(ty) {
            SemaType::List(e) | SemaType::Set(e) | SemaType::Queue(e) | SemaType::Stack(e)
            | SemaType::InlineList(e) | SemaType::Linqerizer(e) | SemaType::Slice(e)
            | SemaType::Pool(e) | SemaType::Handle(e) | SemaType::Fallible(e)
            | SemaType::Task(e) | SemaType::Optional(e) | SemaType::GcRef(e)
            | SemaType::Unique(e) | SemaType::Shared(e) | SemaType::SyncShared(e) => vec![*e],
            SemaType::Dictionary(k, v) => vec![*k, *v],
            SemaType::Tuple(ts) => ts.clone(),
            SemaType::Array { elem, .. } => vec![*elem],
            SemaType::ArenaRef { inner, .. } | SemaType::PoolRef { inner, .. }
            | SemaType::OwnedRef { inner, .. } | SemaType::Reference { inner, .. } => vec![*inner],
            SemaType::Function { params, return_type, .. } => {
                let mut v = params.clone();
                v.push(*return_type);
                v
            }
            SemaType::Named { args, .. } => args.clone(),
            _ => Vec::new(),
        }
    }

    fn mentions_param(&self, ty: TypeId) -> bool {
        matches!(self.ctx.types.get(ty), SemaType::Param(_))
            || self.type_children(ty).into_iter().any(|c| self.mentions_param(c))
    }

    /// A name that did not resolve or a `_`: nothing is known there.
    fn mentions_open(&self, ty: TypeId) -> bool {
        matches!(self.ctx.types.get(ty), SemaType::Unknown | SemaType::Var(_))
            || self.type_children(ty).into_iter().any(|c| self.mentions_open(c))
    }

    /// `ty` with every inference variable inside it replaced by what it was
    /// solved to, for a diagnostic: `Box<?T9>` is `Box<Plain>` once `?T9` is.
    pub(super) fn deep_apply(&mut self, ty: TypeId) -> TypeId {
        let ty = self.apply(ty);
        let t = self.ctx.types.get(ty).clone();
        match t {
            SemaType::Named { def, args } => {
                let args: Vec<TypeId> = args.iter().map(|a| self.deep_apply(*a)).collect();
                self.ctx.types.insert(SemaType::Named { def, args })
            }
            SemaType::List(e)       => { let e = self.deep_apply(e); self.ctx.types.insert(SemaType::List(e)) }
            SemaType::Queue(e)      => { let e = self.deep_apply(e); self.ctx.types.insert(SemaType::Queue(e)) }
            SemaType::Stack(e)      => { let e = self.deep_apply(e); self.ctx.types.insert(SemaType::Stack(e)) }
            SemaType::InlineList(e) => { let e = self.deep_apply(e); self.ctx.types.insert(SemaType::InlineList(e)) }
            SemaType::Set(e)        => { let e = self.deep_apply(e); self.ctx.types.insert(SemaType::Set(e)) }
            SemaType::Optional(e)   => { let e = self.deep_apply(e); self.ctx.types.insert(SemaType::Optional(e)) }
            SemaType::Dictionary(k, v) => {
                let k = self.deep_apply(k);
                let v = self.deep_apply(v);
                self.ctx.types.insert(SemaType::Dictionary(k, v))
            }
            SemaType::Tuple(ts) => {
                let ts: Vec<TypeId> = ts.iter().map(|t| self.deep_apply(*t)).collect();
                self.ctx.types.insert(SemaType::Tuple(ts))
            }
            SemaType::Array { len, elem } => {
                let elem = self.deep_apply(elem);
                self.ctx.types.insert(SemaType::Array { len, elem })
            }
            _ => ty,
        }
    }

    pub(super) fn report_unsupported(&mut self, feature: &'static str, span: Span) {
        self.errors.add_type_error(TypeError::UnsupportedTraitFeature { feature, span });
    }

    // ── Which blocks take this path ──────────────────────────────

    /// Whether the target type is a generic head, or the block declares
    /// parameters, or the target is a type that is only reported here. A block
    /// for a primitive, a tuple, an array or a non-generic struct or enum
    /// without parameters of its own keeps the older paths untouched, and so
    /// does `extend Box { .. }` on a generic struct (the legacy meaning: the
    /// struct's own parameters, not named).
    pub(super) fn block_needs_pattern<'ast>(
        &self,
        params:     &[GenericParam<'ast>],
        target:     &Type<'ast>,
        trait_impl: bool,
    ) -> bool {
        if !params.is_empty() { return true; }
        match target.kind {
            TypeKind::List(_) | TypeKind::Dictionary(_) | TypeKind::Queue(_)
            | TypeKind::Stack(_) | TypeKind::InlineList(_) | TypeKind::Set(_)
            | TypeKind::Slice(_) | TypeKind::Optional(_) | TypeKind::Fallible(_)
            | TypeKind::Task(_) | TypeKind::Function(_) => true,
            TypeKind::Named { path, args } => {
                let Some(root) = path.first().copied() else { return false; };
                let def = self.ctx.top_level_def(root);
                let user = def.is_some_and(|d| matches!(
                    self.ctx.symbols.lookup(d).kind,
                    DefKind::Struct { .. } | DefKind::Enum | DefKind::TypeAlias,
                ));
                if path.len() == 1 && matches!(root, "Pool" | "Handle") && !user {
                    return true;
                }
                let Some(def) = def else { return false; };
                match self.ctx.symbols.lookup(def).kind {
                    DefKind::TypeAlias => self.alias_target_needs_pattern(def),
                    DefKind::Struct { .. } | DefKind::Enum => {
                        let arity = self.generic_arity.get(&def).copied().unwrap_or(0);
                        if arity > 0 { !args.is_empty() || trait_impl } else { !args.is_empty() }
                    }
                    _ => false,
                }
            }
            _ => false,
        }
    }

    /// An alias whose target is a generic head, a collection or a wrapper: a
    /// block written for it is the same block written for the target.
    fn alias_target_needs_pattern(&self, alias: DefId) -> bool {
        let Some(exp) = self.alias_expansions.get(&alias) else { return false; };
        match self.ctx.types.get(exp.target) {
            SemaType::List(_) | SemaType::Dictionary(..) | SemaType::Queue(_)
            | SemaType::Stack(_) | SemaType::InlineList(_) | SemaType::Set(_)
            | SemaType::Slice(_) | SemaType::Optional(_) | SemaType::Fallible(_)
            | SemaType::Task(_) | SemaType::Function { .. } => true,
            SemaType::Named { def, .. } =>
                self.generic_arity.get(def).copied().unwrap_or(0) > 0,
            _ => false,
        }
    }

    // ── Building the pattern ─────────────────────────────────────

    /// Analyse the target of a block that takes this path. `None` once a
    /// diagnostic has been reported (or the target names nothing known, which
    /// is silently skipped as everywhere else in this pass). `at` is where a
    /// problem with the target is reported.
    pub(super) fn build_impl_pattern<'ast>(
        &mut self,
        params:     &[GenericParam<'ast>],
        target:     &Type<'ast>,
        at:         Span,
        trait_impl: bool,
    ) -> Option<ImplPattern> {
        let wrapper = if trait_impl { WRAPPER_FOR_TRAIT } else { WRAPPER_FOR_EXTEND };
        let other   = if trait_impl { TARGET_FOR_TRAIT } else { TARGET_FOR_EXTEND };
        let pool    = if trait_impl { POOL_FOR_TRAIT } else { POOL_FOR_EXTEND };

        // Targets that are reported by what is written.
        match target.kind {
            TypeKind::Optional(_) | TypeKind::Fallible(_) | TypeKind::Task(_) => {
                self.report_unsupported(wrapper, at);
                return None;
            }
            TypeKind::Set(_) | TypeKind::Slice(_) | TypeKind::Function(_) => {
                self.report_unsupported(other, at);
                return None;
            }
            TypeKind::Named { path, args } if path.len() == 1 => {
                let root = path[0];
                let user = self.ctx.top_level_def(root).is_some_and(|d| matches!(
                    self.ctx.symbols.lookup(d).kind,
                    DefKind::Struct { .. } | DefKind::Enum | DefKind::TypeAlias,
                ));
                if matches!(root, "Pool" | "Handle") && !user {
                    self.report_unsupported(pool, at);
                    return None;
                }
                if args.is_empty() && params.iter().any(|g| g.name == root) {
                    self.report_unsupported(BLANKET, at);
                    return None;
                }
            }
            _ => {}
        }

        // The target converted with every parameter standing for itself by
        // its place in the list. Only the shape is read from this: which
        // parameter sits at which argument of the head.
        let provisional: HashMap<String, TypeId> = params.iter().enumerate()
            .map(|(k, g)| (g.name.to_string(), self.ctx.types.intern(SemaType::Param(k))))
            .collect();
        let saved_names = std::mem::replace(&mut self.current_generic_params, provisional);
        let obligations = self.pending_obligations.len();
        let t0 = self.ast_type_to_sema(target);
        // Obligations raised while converting were about the provisional
        // numbering and would be worded with it; the bounds are checked below.
        self.pending_obligations.truncate(obligations);
        self.current_generic_params = saved_names;

        let (head, builtin_name, args): (DefId, Option<&'static str>, Vec<TypeId>) =
            match self.ctx.types.get(t0).clone() {
                SemaType::Named { def, args } if matches!(
                    self.ctx.symbols.lookup(def).kind,
                    DefKind::Struct { .. } | DefKind::Enum,
                ) => (def, None, args),
                SemaType::List(_) | SemaType::Dictionary(..) | SemaType::Queue(_)
                | SemaType::Stack(_) | SemaType::InlineList(_) => {
                    let name = self.builtin_head_name(t0)?;
                    let args = self.builtin_head_args(t0);
                    let def = self.builtin_head_def(name, args.len(), target.span);
                    (def, Some(name), args)
                }
                // A name that does not resolve, or an error already reported.
                SemaType::Unknown => return None,
                _ => {
                    if !params.is_empty() && self.mentions_param(t0) {
                        self.report_unsupported(NESTED_OR_REPEATED, at);
                    } else if !params.is_empty() {
                        self.report_unsupported(UNUSED_PARAM, at);
                    } else {
                        self.report_unsupported(other, at);
                    }
                    return None;
                }
            };

        let arity = match builtin_name {
            Some(_) => args.len(),
            None    => self.generic_arity.get(&head).copied().unwrap_or(0),
        };
        // A count mistake was reported where the type was converted.
        if args.len() != arity { return None; }

        // `List` written without arguments: for a trait impl that is a count
        // mistake, for `extend` it is every `List` (the legacy meaning).
        let bare = matches!(target.kind,
            TypeKind::List(None) | TypeKind::Dictionary(None) | TypeKind::Queue(None)
            | TypeKind::Stack(None) | TypeKind::InlineList(None));
        if bare && trait_impl {
            self.errors.add_type_error(TypeError::GenericArgCountMismatch {
                type_name: builtin_name.unwrap_or("").to_string(),
                expected:  arity,
                found:     0,
                span:      at,
            });
            return None;
        }

        // Which parameter sits where.
        let mut used: Vec<Option<usize>> = vec![None; params.len()];
        let mut slot_param: Vec<Option<usize>> = vec![None; arity];
        let mut concrete: Vec<Option<TypeId>> = vec![None; arity];
        if !bare {
            for i in 0..arity {
                let a = args[i];
                match self.ctx.types.get(a).clone() {
                    SemaType::Param(k) if k < params.len() => {
                        if used[k].is_some() {
                            self.report_unsupported(NESTED_OR_REPEATED, at);
                            return None;
                        }
                        used[k] = Some(i);
                        slot_param[i] = Some(k);
                    }
                    _ if self.mentions_param(a) => {
                        self.report_unsupported(NESTED_OR_REPEATED, at);
                        return None;
                    }
                    // A name that did not resolve or a `_`: nothing is said
                    // about this position, as for `extend Box { .. }`.
                    _ if self.mentions_open(a) => {}
                    _ => concrete[i] = Some(a),
                }
            }
        }
        if used.iter().any(|u| u.is_none()) {
            self.report_unsupported(UNUSED_PARAM, at);
            return None;
        }

        // The block's scope: each parameter is the `Param` of its position.
        let names: Vec<(String, usize)> = params.iter().enumerate()
            .map(|(k, g)| (g.name.to_string(), used[k].unwrap_or(0)))
            .collect();
        let final_names: HashMap<String, TypeId> = names.iter()
            .map(|(n, pos)| (n.clone(), self.ctx.types.intern(SemaType::Param(*pos))))
            .collect();
        let saved_names  = std::mem::replace(&mut self.current_generic_params, final_names);
        let saved_bounds = std::mem::take(&mut self.current_bounds);

        // What the impl writes on its parameters, checked once here (the
        // names, the argument counts, conflicting bounds).
        let mut declared: Vec<Vec<BoundRef>> = vec![Vec::new(); arity];
        for (k, g) in params.iter().enumerate() {
            let pos = used[k].unwrap_or(0);
            declared[pos] = self.check_bound_list(g, true);
        }
        let mut scope_bounds: HashMap<TypeId, Vec<BoundRef>> = HashMap::new();
        for (pos, bs) in declared.iter().enumerate() {
            if bs.is_empty() { continue; }
            let ty = self.ctx.types.intern(SemaType::Param(pos));
            let widened = self.expand_supertraits(bs.clone(), ty);
            scope_bounds.insert(ty, widened);
        }

        // The head's own requirements on its arguments hold for this impl:
        // restated on a parameter, checked for a concrete type.
        let slot_args: Vec<TypeId> = (0..arity).map(|i| match concrete[i] {
            Some(c) => c,
            None    => self.ctx.types.intern(SemaType::Param(i)),
        }).collect();
        let mut head_bounds: Vec<Vec<BoundRef>> = match builtin_name {
            Some(_) => vec![Vec::new(); arity],
            None => self.generic_bounds.get(&head).cloned().unwrap_or_default(),
        };
        head_bounds.resize(arity, Vec::new());
        if builtin_name == Some("Dictionary") {
            // A dictionary key must be `Hash` (and so `Eq`).
            if let Some(hash) = self.prelude_defs.get("Hash").copied() {
                head_bounds[0].push(BoundRef { trait_def: hash, args: Vec::new() });
            }
        }
        for pos in 0..arity {
            for hb in head_bounds[pos].clone() {
                let applied: Vec<TypeId> = hb.args.iter()
                    .map(|t| self.substitute(*t, &slot_args))
                    .collect();
                match (slot_param[pos], concrete[pos]) {
                    (Some(k), _) => {
                        let ty = self.ctx.types.intern(SemaType::Param(pos));
                        let have = scope_bounds.get(&ty)
                            .is_some_and(|bs| bs.iter().any(|b| b.trait_def == hb.trait_def));
                        if !have && self.reported_obligations.insert((at, hb.trait_def)) {
                            let trait_name = self.trait_ref_text(hb.trait_def, &applied);
                            self.errors.add_type_error(TypeError::UnsatisfiedBound {
                                type_name: params[k].name.to_string(), trait_name,
                                is_param: true, span: at,
                            });
                        }
                    }
                    (None, Some(c)) => self.require_bound_args(c, hb.trait_def, applied, at),
                    (None, None) => {}
                }
            }
        }

        // The head's own bounds are in force inside the block too, so a body
        // that uses what the struct promises does not need them twice.
        for pos in 0..arity {
            if head_bounds[pos].is_empty() || slot_param[pos].is_none() { continue; }
            let ty = self.ctx.types.intern(SemaType::Param(pos));
            let mut have = scope_bounds.get(&ty).cloned().unwrap_or_default();
            for hb in &head_bounds[pos] {
                if !have.iter().any(|b| b.trait_def == hb.trait_def) { have.push(hb.clone()); }
            }
            let widened = self.expand_supertraits(have, ty);
            scope_bounds.insert(ty, widened);
        }

        self.current_generic_params = saved_names;
        self.current_bounds = saved_bounds;

        let self_ty = match builtin_name {
            Some(name) => self.builtin_head_applied(name, &slot_args),
            None => self.ctx.types.insert(SemaType::Named { def: head, args: slot_args.clone() }),
        };
        let guard = if declared.iter().any(|b| !b.is_empty()) || concrete.iter().any(|c| c.is_some()) {
            Some(Rc::new(ImplGuard { concrete, bounds: declared }))
        } else {
            None
        };
        Some(ImplPattern {
            head, arity, self_ty, builtin: builtin_name.is_some(), names, scope_bounds, guard,
        })
    }

    // ── Scope ────────────────────────────────────────────────────

    /// Put a block's parameters, bounds and `Self` in force. Pairs with
    /// `leave_impl_scope`.
    pub(super) fn enter_impl_scope(&mut self, p: &ImplPattern) -> ImplScope {
        let names: HashMap<String, TypeId> = p.names.iter()
            .map(|(n, pos)| (n.clone(), self.ctx.types.intern(SemaType::Param(*pos))))
            .collect();
        // A position the block does not name still prints as the head wrote it.
        let mut shown: HashMap<TypeId, String> = HashMap::new();
        if let Some(written) = self.generic_param_names.get(&p.head).cloned() {
            for (i, n) in written.into_iter().enumerate() {
                shown.insert(self.ctx.types.intern(SemaType::Param(i)), n);
            }
        }
        for (n, pos) in &p.names {
            shown.insert(self.ctx.types.intern(SemaType::Param(*pos)), n.clone());
        }
        ImplScope {
            names:               std::mem::replace(&mut self.current_generic_params, names),
            bounds:              std::mem::replace(&mut self.current_bounds, p.scope_bounds.clone()),
            param_names:         std::mem::replace(&mut self.param_names, shown),
            self_type:           self.self_type.replace(p.self_ty),
            current_struct_type: self.current_struct_type.replace(p.self_ty),
        }
    }

    pub(super) fn leave_impl_scope(&mut self, prev: ImplScope) {
        self.current_generic_params = prev.names;
        self.current_bounds         = prev.bounds;
        self.param_names            = prev.param_names;
        self.self_type              = prev.self_type;
        self.current_struct_type    = prev.current_struct_type;
    }

    /// Collect the method signatures of an `impl` or `extend` block, in the
    /// block's own scope when it has one.
    pub(super) fn collect_block_sigs<'ast>(
        &mut self,
        params:     &[GenericParam<'ast>],
        target:     &Type<'ast>,
        methods:    &'ast [MethodDecl<'ast>],
        key:        Span,
        at:         Span,
        trait_impl: bool,
    ) {
        let prev = self.self_type;
        if self.block_needs_pattern(params, target, trait_impl) {
            match self.build_impl_pattern(params, target, at, trait_impl) {
                Some(p) => {
                    let p = Rc::new(p);
                    self.impl_patterns.insert(key, p.clone());
                    let scope = self.enter_impl_scope(&p);
                    for m in methods { self.collect_method_sig(m, p.arity); }
                    self.leave_impl_scope(scope);
                }
                None => {
                    // Reported already. The signatures are still collected, so
                    // the methods have types and the bodies are not read.
                    self.impl_failed.insert(key);
                    self.self_type = None;
                    for m in methods { self.collect_method_sig(m, 0); }
                }
            }
        } else {
            self.self_type = self.self_type_for_target(target);
            let base = self.target_arity(target);
            for m in methods { self.collect_method_sig(m, base); }
        }
        self.self_type = prev;
    }

    // ── Guards ───────────────────────────────────────────────────

    /// Whether a guard holds for a head applied to `args`: `Some(false)` when a
    /// concrete position disagrees or a bound is known to fail, `None` while
    /// something is still undecided. A concrete position binds an argument
    /// that is still an inference variable (the only impl decides it).
    pub(super) fn guard_status(&mut self, g: &ImplGuard, args: &[TypeId]) -> Option<bool> {
        let span = Span::at(0);
        for (i, c) in g.concrete.iter().enumerate() {
            let (Some(c), Some(&a)) = (c, args.get(i)) else { continue; };
            let a = self.apply(a);
            if !self.types_agree(a, *c, span) { return Some(false); }
        }
        let mut undecided = false;
        for (i, bs) in g.bounds.iter().enumerate() {
            let Some(&a) = args.get(i) else { continue; };
            for b in bs {
                let applied: Vec<TypeId> = b.args.iter().map(|t| self.substitute(*t, args)).collect();
                match self.bound_args_status(a, b.trait_def, &applied, span) {
                    Some(false) => return Some(false),
                    None        => undecided = true,
                    Some(true)  => {}
                }
            }
        }
        if undecided { None } else { Some(true) }
    }

    /// Whether the impl of `trait_def` for `head` (which exists) applies to a
    /// type with these arguments.
    pub(super) fn impl_status(&mut self, trait_def: DefId, head: DefId, args: &[TypeId]) -> Option<bool> {
        match self.trait_impl_guards.get(&(trait_def, head)).cloned() {
            Some(g) => self.guard_status(&g, args),
            None    => Some(true),
        }
    }

    /// A call of a method a block gave a generic head: the block's concrete
    /// positions must be the receiver's arguments, and its bounds become
    /// obligations at the call. `false` when a concrete position disagrees,
    /// which means the receiver has no such method.
    pub(super) fn apply_impl_guard(
        &mut self,
        guard: &Option<Rc<ImplGuard>>,
        args:  &[TypeId],
        span:  Span,
    ) -> bool {
        let Some(g) = guard.clone() else { return true; };
        for (i, c) in g.concrete.iter().enumerate() {
            let (Some(c), Some(&a)) = (c, args.get(i)) else { continue; };
            if !self.types_agree(a, *c, span) { return false; }
        }
        for (i, bs) in g.bounds.iter().enumerate() {
            let Some(&a) = args.get(i) else { continue; };
            for b in bs {
                let applied: Vec<TypeId> = b.args.iter().map(|t| self.substitute(*t, args)).collect();
                self.require_bound_args(a, b.trait_def, applied, span);
            }
        }
        true
    }
}
