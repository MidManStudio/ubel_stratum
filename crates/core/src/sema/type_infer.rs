// ============================================================================
// NOTICE: Full documentation, design decisions, and fix history for this file
// live in docs/ubel_stratum.md, section "sema/type_infer.rs"
// ============================================================================
// src/sema/type_infer.rs
//! Pass 2 — Type Inference and Arena Coloring.
//!
//! 2a. Signature collection — records def_types for every top-level
//!     declaration without touching bodies, enabling forward references.
//!
//! 2b. Body inference — bidirectional checking inside each body.
//!     Expected type known → check; otherwise infer and record.
//!
//! Arena coloring is woven into 2b. Entering a `with arena(…)` block
//! pushes a fresh ArenaId; every constructed value inside gets stamped
//! SemaType::ArenaRef.
//!
//! ## Gap 2 — escape boundary enforcement (docs/MEMORY_MODEL.md §6)
//!
//! Whether an ArenaRef is *allowed* to flow to a given site is also
//! enforced here, not in tier_check — Pass 2 already tracks live arena
//! scope via `arena_stack`/`maybe_arena_ref`, so this is where the
//! answer is cheapest to compute, right as each site is inferred:
//!
//! - **Assignment** — `check_assign_arena_escape`, called from the
//!   `ExprKind::Assign` arm, compares the *specific* `ArenaId` (never
//!   just tagged/untagged — see §3) a value carries against the target's
//!   home arena: an `Ident`'s original declared type, or a `Field`/
//!   `Index` target's receiver/container's own tag.
//! - **Struct field / indexed storage** — the same function; no struct
//!   field can ever be declared `ArenaRef` (no surface syntax for it),
//!   so a receiver that isn't itself tagged has a home arena of `None`.
//! - **Closure capture** — a lambda built inside a `with arena(…)` block
//!   is conservatively `maybe_arena_ref`-tagged too, the same as any
//!   other constructed value. Lexical scoping guarantees a lambda can
//!   only reference an arena-scoped local from inside that local's own
//!   block, so this over-approximates (a closure that captures nothing
//!   arena-related still gets tagged) but never under-approximates —
//!   once tagged, the lambda value rides the same assignment/return
//!   checks as any other arena-scoped value, no separate free-variable
//!   capture analysis needed.
//! - **Return position** was already caught before Gap 2, incidentally,
//!   via a generic `TypeMismatch` in `unify` (declared return type vs.
//!   actual `ArenaRef`-tagged value). `unify` now recognizes this
//!   specific shape and reports the more precise `ArenaRefEscapesBoundary`
//!   instead — see `scope_mismatch_side`.
//!
//! # Known rough edges (not airtight yet)
//! - No occurs-check in unification.
//! - Multi-element destructuring shares one Span; all get collection elem type.
//! - A method name pre-inferred as the callee of an enclosing `Call`
//!   (e.g. `Rectangle.doesNotExist()`) can surface both `NoSuchField`
//!   (from the callee's own standalone inference) and `NoSuchMethod`
//!   (from the Call arm's dedicated dispatch) for the same typo — a
//!   real diagnostic duplication, not a false positive; see
//!   GENERICS_RULES.md's own "Known gaps" for why it's left as-is.
//! - A struct/enum method's *own* extra generic params (beyond whatever
//!   generic params the enclosing struct/enum itself declares) aren't
//!   substituted at call sites — GENERICS_RULES.md.
//! - `impl`/`extend` blocks on a generic struct aren't wired to that
//!   struct's own generic scope (no current fixture exercises this;
//!   inline struct methods — the common case — are unaffected).
//!
//! Previously listed here, now real (GENERICS_RULES.md):
//! generic struct/enum/fn instantiation, `self` typing, general struct
//! field access, and struct static/instance method dispatch all used to
//! be `Unknown`-typed no-ops; they're substituted/checked for real now.

#![allow(dead_code, unused_variables, unused_imports)]

use std::collections::{HashMap, HashSet};

use crate::ast::common::{BinOp, GenericParam, Span, TierAnnotation, UnaryOp, Attribute, AttrArg};
use crate::ast::declarations::{
    ConstDecl, EnumDecl, EnumVariantPayload, ExtendDecl, FunctionDecl, ImplBlock,
    MethodDecl, Param, ParamKind, ReturnType, StructDecl, StructMember,
    TraitItem, TypeAlias, MethodSig,
};
use crate::ast::expressions::{
    Arg, ArgKind, Expr, ExprKind, IfBranchBody, LambdaBody,
    MatchArmBody, OrElseFallback,
};
use crate::ast::literals::{FormatSpec, InterpolationPart, Literal};
use crate::ast::patterns::{EnumPatternPayload, Pattern, PatternKind};
use crate::ast::root::{Item, Program};
use crate::ast::statements::{AllocatorKind, BindingTarget, Block, Stmt, StmtKind};
use crate::ast::types::{Type, TypeKind};
use crate::builtins::instance::{self, MethodReturn, ReceiverWrap};
use crate::error_management::{ErrorManager, errors::{TypeError, TierError}};
use crate::sema::sema_context::SemaContext;
use crate::sema::symbol_table::DefId;
use crate::sema::type_table::{ArenaId, PoolId, SemaType, TypeId};

// ── Entry point ───────────────────────────────────────────────────

pub fn infer<'ast>(
    program: &Program<'ast>,
    ctx:     &mut SemaContext,
    errors:  &mut ErrorManager,
) {
    let mut icx = InferCtx::new(ctx, errors);
    icx.collect_signatures(program);
    icx.infer_bodies(program);
}

// ── Unifier ───────────────────────────────────────────────────────

struct Unifier {
    subst: HashMap<u32, TypeId>,
}

impl Unifier {
    fn new() -> Self { Unifier { subst: HashMap::new() } }

    fn apply(&self, mut id: TypeId, types: &crate::sema::type_table::TypeTable) -> TypeId {
        loop {
            match types.get(id) {
                SemaType::Var(v) => match self.subst.get(v) {
                    Some(&next) if next != id => id = next,
                    _                         => return id,
                },
                _ => return id,
            }
        }
    }

    fn bind(&mut self, var: u32, ty: TypeId) -> bool {
        match self.subst.get(&var) {
            None            => { self.subst.insert(var, ty); true }
            Some(&existing) => existing == ty,
        }
    }
}

// ── Enum variant metadata ───────────────────────────────────────────

/// Element/field-type info for one enum variant, computed once during
/// signature collection (`collect_enum_sig`) and consulted by every other
/// enum usage site — bare `EnumName.Variant` access, `EnumName.Variant(..)`
/// tuple construction, `EnumName.Variant { .. }` struct construction, and
/// pattern checking. ENUM_RULES.md.
#[derive(Clone)]
enum VariantShape {
    None,
    /// Behaves identically to `None` for every purpose except declaration-
    /// time validation (ENUM_RULES.md §4 item 1/2) — the chosen ordinal
    /// isn't tracked past sema; there's no `as int` cast to read it back
    /// yet (see this section's "Known gap").
    Discriminant,
    Tuple(Vec<TypeId>),
    Struct(Vec<(String, TypeId)>),
}

// ── Struct member metadata (GENERICS_RULES.md) ──────────────────────

/// One method's signature, computed once during `collect_struct_sig` and
/// consulted at every call site — mirrors `VariantShape`'s role for enums.
/// `params`/`return_type` may contain `Param` placeholders for a generic
/// struct; substituted per call site via `InferCtx::substitute`.
#[derive(Clone)]
struct MethodShape {
    /// Whether the method's first param is `self`/`mut self`/`&self`/
    /// `&mut self` — distinguishes `Box.new(v)` (associated/static call,
    /// dispatched on the type name) from `boxed.unwrap()` (instance call,
    /// dispatched on a value's own type).
    has_self:    bool,
    params:      Vec<TypeId>,
    return_type: TypeId,
    is_fallible: bool,
}

/// What a checked pattern definitively covers, for exhaustiveness.
enum PatternCoverage {
    /// Matches unconditionally regardless of the scrutinee's shape —
    /// `_`, or a genuine (non-variant-name) binding.
    CatchAll,
    /// Matches only these specific enum variant names.
    Variants(Vec<String>),
    /// Not enum-related (non-enum scrutinee, or a pattern shape this
    /// pass doesn't do deep exhaustiveness analysis on) — doesn't affect
    /// the enum coverage count either way.
    Other,
}

// ── InferCtx ─────────────────────────────────────────────────────

struct InferCtx<'a> {
    ctx:              &'a mut SemaContext,
    errors:           &'a mut ErrorManager,
    unifier:          Unifier,
    current_return:   Option<TypeId>,
    current_fallible: bool,
    arena_stack:      Vec<ArenaId>,
    next_arena:       usize,
    /// Mirrors `arena_stack`, one entry per open `with pool<T>(count) { }`
    /// — MEMORY_MODEL.md §11. Stores the pool's element type alongside
    /// its id since `Pool.new()` (unlike `List.new()`) has no generic
    /// argument of its own to infer from; it picks up both id and
    /// element type from the innermost enclosing pool block.
    pool_stack:       Vec<(PoolId, TypeId)>,
    next_pool:        usize,
    /// Populated once per enum by `collect_enum_sig`, consulted by every
    /// other enum usage site for the rest of Pass 2 — ENUM_RULES.md.
    enum_variants:    HashMap<DefId, Vec<(String, VariantShape)>>,
    /// Generic-param name -> `Param(i)` placeholder, active only while
    /// collecting the signature or checking the body of the struct/enum/fn
    /// currently being processed (GENERICS_RULES.md). Empty outside any
    /// generic decl. `ast_type_to_sema`'s `TypeKind::Named` arm checks
    /// this *before* falling back to `top_level_def`, so a bare `T` that
    /// names one of the enclosing decl's own generic params resolves to
    /// its placeholder instead of silently becoming `Unknown`.
    current_generic_params: HashMap<String, TypeId>,
    /// `self`'s type while inside a struct's own method bodies — the
    /// abstract `Named { def, args: [Param(0), Param(1), ...] }` for a
    /// generic struct, or `Named { def, args: [] }` for a non-generic
    /// one. Method bodies are checked once, generically — the same way
    /// Rust checks a generic impl's body once rather than per
    /// instantiation — with concrete substitution happening at each call
    /// site instead. `None` outside any method body.
    current_struct_type: Option<TypeId>,
    /// Populated once per struct by `collect_struct_sig`, mirrors
    /// `enum_variants`'s role. Field types may contain `Param`
    /// placeholders for a generic struct.
    struct_fields:    HashMap<DefId, Vec<(String, TypeId)>>,
    /// Populated once per struct by `collect_struct_sig`. Same `Param`-
    /// placeholder treatment as `struct_fields`.
    struct_methods:   HashMap<DefId, Vec<(String, MethodShape)>>,
    /// Populated once per struct by `collect_struct_sig`, alongside
    /// `check_derive_attrs`'s own validation of the same `@derive(...)`
    /// list. Sema's own copy, not shared with `Interpreter::
    /// struct_derives`, which is type-name-keyed and built later, at
    /// `run_program` time, from the same source (each pass re-derives
    /// this fact from the AST rather than one borrowing the other's
    /// table, the same relationship this file's other struct tables
    /// already have with the interpreter's). Consulted so far only by
    /// the `.clone()` instance-method special case below (`ExprKind::
    /// Call`'s struct-instance-method arm), recognizing `.clone()` as
    /// a real, typed method when `@derive(Clone)` is present, instead
    /// of it always reaching `NoSuchMethod`.
    struct_derives:   HashMap<DefId, HashSet<String>>,
    /// How many generic params the struct/enum at this `DefId` declares.
    /// Consulted at `TypeKind::Named` use sites to validate argument
    /// count (`GenericArgCountMismatch`) and at construction/call sites
    /// to know how many fresh `Var`s to allocate before substituting.
    generic_arity:    HashMap<DefId, usize>,
    /// §8 — the enclosing function/method's `@tier(...)`. Needed here
    /// (not just in tier_check.rs) because `expr_types` entries can
    /// still be raw unresolved `Var`s at record-time — resolving a
    /// receiver's type to check `instance::is_high_only` needs the same
    /// live `Unifier`/`apply()` this pass already has, which tier_check
    /// (Pass 3, no `Unifier` of its own) does not. Defaults to `High`
    /// — the same default the whole file already uses for top-level
    /// inference performed outside any function body.
    current_tier:     TierAnnotation,
}

impl<'a> InferCtx<'a> {
    fn new(ctx: &'a mut SemaContext, errors: &'a mut ErrorManager) -> Self {
        InferCtx {
            ctx,
            errors,
            unifier:          Unifier::new(),
            current_return:   None,
            current_fallible: false,
            arena_stack:      Vec::new(),
            next_arena:       0,
            pool_stack:       Vec::new(),
            next_pool:        0,
            enum_variants:    HashMap::new(),
            current_generic_params: HashMap::new(),
            current_struct_type:    None,
            struct_fields:    HashMap::new(),
            struct_methods:   HashMap::new(),
            struct_derives:   HashMap::new(),
            generic_arity:    HashMap::new(),
            current_tier:     TierAnnotation::High,
        }
    }

    // ── Generics (GENERICS_RULES.md) ────────────────────────────────

    /// Enter a struct/enum/fn's own generic scope, returning the
    /// previous scope so the caller can restore it on the way out.
    /// Builds one `Param(i)` placeholder per declared generic param,
    /// keyed by name — repeated references to the same param name within
    /// one declaration (e.g. `T` in both `Some(T)` and a sibling variant)
    /// resolve to the identical `TypeId` for free, since the map is only
    /// built once per `push`.
    fn push_generic_scope(&mut self, params: &[GenericParam]) -> HashMap<String, TypeId> {
        let mut new_map = HashMap::with_capacity(params.len());
        for (i, gp) in params.iter().enumerate() {
            new_map.insert(gp.name.to_string(), self.ctx.types.intern(SemaType::Param(i)));
        }
        std::mem::replace(&mut self.current_generic_params, new_map)
    }

    fn pop_generic_scope(&mut self, prev: HashMap<String, TypeId>) {
        self.current_generic_params = prev;
    }

    /// Build a fresh instantiation of a struct/enum def: `Named { def,
    /// args }` with one fresh `Var` per its own declared generic param
    /// (or just its stored, already-computed `def_type` — cheap, no
    /// allocation — for a non-generic one). Used at every *bare*
    /// construction site (enum fieldless/tuple/struct-payload variant
    /// construction, struct literals, struct associated-function calls)
    /// where there's no already-known instantiation to substitute
    /// against and the concrete type args must instead be *inferred*
    /// from how the constructed value's parts are used — unification
    /// does the rest, the same way this file's existing `fresh_var()`
    /// already resolves an untyped `[]` list literal's element type from
    /// context (GENERICS_RULES.md).
    fn instantiate(&mut self, def_id: DefId) -> TypeId {
        let arity = self.generic_arity.get(&def_id).copied().unwrap_or(0);
        if arity == 0 {
            return self.ctx.def_type(def_id).unwrap_or_else(|| self.unknown());
        }
        let args: Vec<TypeId> = (0..arity).map(|_| self.fresh_var()).collect();
        self.ctx.types.insert(SemaType::Named { def: def_id, args })
    }

    /// Recursively replace every `Param(i)` inside `ty` with `args[i]`.
    /// A type containing no `Param` (i.e. anything already concrete —
    /// every non-generic type) is reconstructed unchanged; correctness
    /// over cheapness here, matching the rest of this pass's existing
    /// style (e.g. `unify`'s `apply`-then-compare).  Every real use site
    /// of a generic struct/enum/fn's stored (raw, `Param`-containing)
    /// signature goes through this before the result is unified,
    /// displayed, or bound to a variable — `enum_shapes_of`, struct field
    /// access, struct static/instance method dispatch, and generic
    /// free-function calls.
    fn substitute(&mut self, ty: TypeId, args: &[TypeId]) -> TypeId {
        let t = self.ctx.types.get(ty).clone();
        match t {
            SemaType::Param(i) => args.get(i).copied().unwrap_or(ty),

            SemaType::List(e) => { let e = self.substitute(e, args); self.ctx.types.insert(SemaType::List(e)) }
            SemaType::Set(e)  => { let e = self.substitute(e, args); self.ctx.types.insert(SemaType::Set(e)) }
            SemaType::Queue(e) => { let e = self.substitute(e, args); self.ctx.types.insert(SemaType::Queue(e)) }
            SemaType::Stack(e) => { let e = self.substitute(e, args); self.ctx.types.insert(SemaType::Stack(e)) }
            SemaType::Slice(e) => { let e = self.substitute(e, args); self.ctx.types.insert(SemaType::Slice(e)) }
            SemaType::Pool(e)  => { let e = self.substitute(e, args); self.ctx.types.insert(SemaType::Pool(e)) }
            SemaType::Handle(e) => { let e = self.substitute(e, args); self.ctx.types.insert(SemaType::Handle(e)) }
            SemaType::Fallible(e) => { let e = self.substitute(e, args); self.ctx.types.insert(SemaType::Fallible(e)) }
            SemaType::Task(e) => { let e = self.substitute(e, args); self.ctx.types.insert(SemaType::Task(e)) }
            SemaType::Optional(e) => { let e = self.substitute(e, args); self.ctx.types.insert(SemaType::Optional(e)) }
            SemaType::GcRef(e) => { let e = self.substitute(e, args); self.ctx.types.insert(SemaType::GcRef(e)) }
            // InlineList/Linqerizer/Reference were missing here entirely
            // until now — same bug class as the `structurally_compatible`
            // Set/Queue/Stack/Pool/Handle fix (that function's own
            // comment): a generic struct/fn whose signature contains one
            // of these three (e.g. `struct Box<T> { r: &T }`) would have
            // its `Param(0)` survive substitution unresolved, since these
            // three fell through to the primitives-only catch-all below
            // instead of recursing into `inner`. Found while adding
            // `Unique`/`Shared`/`SyncShared` alongside them and confirming
            // every wrapper type actually substitutes correctly — leaving
            // three known instances of the same bug sitting right next to
            // the fix would've been an odd thing to knowingly walk past.
            SemaType::InlineList(e) => { let e = self.substitute(e, args); self.ctx.types.insert(SemaType::InlineList(e)) }
            SemaType::Linqerizer(e) => { let e = self.substitute(e, args); self.ctx.types.insert(SemaType::Linqerizer(e)) }
            SemaType::Reference { mutable, lifetime, inner } => {
                let inner = self.substitute(inner, args);
                self.ctx.types.insert(SemaType::Reference { mutable, lifetime, inner })
            }
            SemaType::Unique(e)     => { let e = self.substitute(e, args); self.ctx.types.insert(SemaType::Unique(e)) }
            SemaType::Shared(e)     => { let e = self.substitute(e, args); self.ctx.types.insert(SemaType::Shared(e)) }
            SemaType::SyncShared(e) => { let e = self.substitute(e, args); self.ctx.types.insert(SemaType::SyncShared(e)) }

            SemaType::Dictionary(k, v) => {
                let k = self.substitute(k, args);
                let v = self.substitute(v, args);
                self.ctx.types.insert(SemaType::Dictionary(k, v))
            }
            SemaType::Tuple(ts) => {
                let ts: Vec<TypeId> = ts.iter().map(|t| self.substitute(*t, args)).collect();
                self.ctx.types.insert(SemaType::Tuple(ts))
            }
            SemaType::Array { len, elem } => {
                let elem = self.substitute(elem, args);
                self.ctx.types.insert(SemaType::Array { len, elem })
            }
            SemaType::ArenaRef { arena, mutable, inner } => {
                let inner = self.substitute(inner, args);
                self.ctx.types.insert(SemaType::ArenaRef { arena, mutable, inner })
            }
            SemaType::PoolRef { pool, mutable, inner } => {
                let inner = self.substitute(inner, args);
                self.ctx.types.insert(SemaType::PoolRef { pool, mutable, inner })
            }
            SemaType::OwnedRef { mutable, inner } => {
                let inner = self.substitute(inner, args);
                self.ctx.types.insert(SemaType::OwnedRef { mutable, inner })
            }
            SemaType::Function { params, return_type, is_fallible, generic_arity } => {
                let params: Vec<TypeId> = params.iter().map(|p| self.substitute(*p, args)).collect();
                let return_type = self.substitute(return_type, args);
                self.ctx.types.insert(SemaType::Function { params, return_type, is_fallible, generic_arity })
            }
            SemaType::Named { def, args: inner_args } => {
                let inner_args: Vec<TypeId> = inner_args.iter().map(|t| self.substitute(*t, args)).collect();
                self.ctx.types.insert(SemaType::Named { def, args: inner_args })
            }

            // Primitives, Var, Null, Unknown, Void — nothing to substitute.
            _ => ty,
        }
    }

    // ── Cheap constructors ────────────────────────────────────────

    fn fresh_var(&mut self) -> TypeId { self.ctx.types.fresh_var() }
    fn unknown(&mut self)   -> TypeId { self.ctx.types.intern(SemaType::Unknown) }
    fn void_ty(&mut self)   -> TypeId { self.ctx.types.intern(SemaType::Void) }
    fn bool_ty(&mut self)   -> TypeId { self.ctx.types.intern(SemaType::Bool) }
    fn int_ty(&mut self)    -> TypeId { self.ctx.types.intern(SemaType::Int) }
    fn str_ty(&mut self)    -> TypeId { self.ctx.types.intern(SemaType::Str) }

    fn apply(&self, id: TypeId) -> TypeId {
        self.unifier.apply(id, &self.ctx.types)
    }

    // ── Arena tracking ────────────────────────────────────────────

    fn push_arena(&mut self) -> ArenaId {
        let id = ArenaId(self.next_arena);
        self.next_arena += 1;
        self.arena_stack.push(id);
        id
    }

    fn pop_arena(&mut self) { self.arena_stack.pop(); }

    fn current_arena(&self) -> Option<ArenaId> {
        self.arena_stack.last().copied()
    }

    // ── Pool tracking ─────────────────────────────────────────────

    fn push_pool(&mut self, elem_ty: TypeId) -> PoolId {
        let id = PoolId(self.next_pool);
        self.next_pool += 1;
        self.pool_stack.push((id, elem_ty));
        id
    }

    fn pop_pool(&mut self) { self.pool_stack.pop(); }

    fn current_pool(&self) -> Option<(PoolId, TypeId)> {
        self.pool_stack.last().copied()
    }

    /// Wrap `inner_ty` as an ArenaRef if inside a `with arena` block.
    /// Called on freshly *constructed* values only.
    fn maybe_arena_ref(&mut self, inner_ty: TypeId) -> TypeId {
        match self.current_arena() {
            Some(arena) => self.ctx.types.insert(SemaType::ArenaRef {
                arena,
                mutable: false,
                inner:   inner_ty,
            }),
            None => inner_ty,
        }
    }

    /// GAP 2 — resolve `ty` and return `Some(arena)` only if its
    /// *outermost* layer is `SemaType::ArenaRef`. This is deliberately
    /// shallow (unlike `SemaType::scope_ref_kind`, which recurses):
    /// it answers "is this specific reference itself bound to an arena",
    /// which is what determines a binding's or receiver's *own* home
    /// arena for escape comparisons — not "does this type contain an
    /// arena reference somewhere inside it."
    fn top_level_arena(&mut self, ty: TypeId) -> Option<ArenaId> {
        let resolved = self.apply(ty);
        match self.ctx.types.get(resolved) {
            SemaType::ArenaRef { arena, .. } => Some(*arena),
            _ => None,
        }
    }

    /// GAP 2 — report `ty` as a value that has crossed an arena boundary
    /// it shouldn't have.
    fn arena_escape(&mut self, ty: TypeId, span: Span) {
        let escaped_type = self.display_type(ty);
        self.errors.add_tier_error(TierError::ArenaRefEscapesBoundary {
            escaped_type,
            span,
        });
    }

    /// Same shallow-resolve shape as `top_level_arena`, for `PoolRef` —
    /// MEMORY_MODEL.md §11.
    fn top_level_pool(&mut self, ty: TypeId) -> Option<PoolId> {
        let resolved = self.apply(ty);
        match self.ctx.types.get(resolved) {
            SemaType::PoolRef { pool, .. } => Some(*pool),
            _ => None,
        }
    }

    /// Same shape as `arena_escape`, reporting the pool-specific
    /// diagnostic so the message says "&pool"/"with pool<...>(...)"
    /// rather than reusing arena's wording for a different block kind.
    fn pool_escape(&mut self, ty: TypeId, span: Span) {
        let escaped_type = self.display_type(ty);
        self.errors.add_tier_error(TierError::PoolRefEscapesBoundary {
            escaped_type,
            span,
        });
    }

    /// Type produced by `<Namespace>.new()` for the builtin collection
    /// constructors (`crates/core/src/builtins/constructors.rs`). `None`
    /// for anything not in that list — including builtin namespaces with
    /// no constructor member, like `Math` — so callers fall through to
    /// generic call inference.
    ///
    /// This exists so `List.new()` / `Dictionary.new()` / `Queue.new()` /
    /// `Stack.new()` get a real collection type *and* participate in arena
    /// coloring via `maybe_arena_ref`, the same way array/tuple/dict/struct
    /// literals already do. Without it, these calls fall through the
    /// generic `Field`/`Call` path, which has no field table yet (see that
    /// arm's own TODO) and always infers `Unknown` — silently skipping
    /// arena tagging inside a `with arena(...)` block. See
    /// `docs/MEMORY_MODEL.md` §5 ("Gap 1").
    ///
    /// Keep this in sync with `constructors.rs` and `BUILTIN_NAMESPACES` if
    /// a new collection constructor is added (e.g. a future `Pool.new()`).
    fn builtin_constructor_type(&mut self, namespace: &str) -> Option<TypeId> {
        match namespace {
            "List" => {
                let elem = self.fresh_var();
                Some(self.ctx.types.intern(SemaType::List(elem)))
            }
            "Dictionary" => {
                let k = self.fresh_var();
                let v = self.fresh_var();
                Some(self.ctx.types.insert(SemaType::Dictionary(k, v)))
            }
            "Queue" => {
                let elem = self.fresh_var();
                Some(self.ctx.types.insert(SemaType::Queue(elem)))
            }
            "Stack" => {
                let elem = self.fresh_var();
                Some(self.ctx.types.insert(SemaType::Stack(elem)))
            }
            _ => None,
        }
    }

    /// §8 — turn an `instance::MethodReturn` shape into a concrete
    /// `TypeId`, given the receiver's wrapper (for allocation-producing
    /// methods, §8's "same arena as the receiver" rule) and its bare
    /// composite `TypeId` (for pulling out element type(s)).
    fn method_return_type(
        &mut self,
        ret:     MethodReturn,
        wrap:    ReceiverWrap,
        bare_ty: TypeId,
    ) -> TypeId {
        match ret {
            MethodReturn::Void => self.void_ty(),
            MethodReturn::Int  => self.int_ty(),
            MethodReturn::Bool => self.bool_ty(),

            // Already-stored value (List<T>/Queue<T>/Stack<T>'s T, or
            // Dictionary<K,V>'s V for `at`) — already correctly tiered
            // from whenever it was inserted, nothing new to wrap for
            // tier purposes. Every current caller of this shape
            // (pop/first/last/dequeue/peek/at) falls back to
            // `Value::Null` at runtime on an empty/missing receiver
            // (see each method's own `unwrap_or(Value::Null)` in
            // `builtins/instance/*.rs`), so the static type has to be
            // genuinely nullable — `Optional(elem)`, not bare `elem` —
            // or `x.pop() == null` fails a real TypeMismatch despite
            // being exactly the documented way to check for "empty."
            MethodReturn::Elem => {
                let elem = match self.ctx.types.get(bare_ty) {
                    SemaType::List(e)
                    | SemaType::Queue(e)
                    | SemaType::Stack(e)
                    | SemaType::Pool(e)
                    | SemaType::Linqerizer(e)  => *e,
                    SemaType::Dictionary(_, v) => *v,
                    _ => self.fresh_var(),
                };
                self.ctx.types.insert(SemaType::Optional(elem))
            }

            // The rest all construct a brand-new value — inherit the
            // receiver's own wrapper rather than defaulting to GC.
            MethodReturn::NewSelf => self.apply_receiver_wrap(wrap, bare_ty),
            MethodReturn::NewListOfChar => {
                let elem = self.ctx.types.intern(SemaType::Char);
                let list = self.ctx.types.insert(SemaType::List(elem));
                self.apply_receiver_wrap(wrap, list)
            }
            MethodReturn::NewListOfStr => {
                let elem = self.str_ty();
                let list = self.ctx.types.insert(SemaType::List(elem));
                self.apply_receiver_wrap(wrap, list)
            }
            MethodReturn::NewListOfKey => {
                let key = match self.ctx.types.get(bare_ty) {
                    SemaType::Dictionary(k, _) => *k,
                    _ => self.fresh_var(),
                };
                let list = self.ctx.types.insert(SemaType::List(key));
                self.apply_receiver_wrap(wrap, list)
            }
            MethodReturn::NewListOfValue => {
                let val = match self.ctx.types.get(bare_ty) {
                    SemaType::Dictionary(_, v) => *v,
                    _ => self.fresh_var(),
                };
                let list = self.ctx.types.insert(SemaType::List(val));
                self.apply_receiver_wrap(wrap, list)
            }
            // `Pool<T>.acquire(value)` — `Optional<Handle<T>>`, wrapped
            // in the receiver's own wrap (MEMORY_MODEL.md §11: acquired
            // handles must not escape the `with pool<T>(count) { }`
            // block either, same "same as arena" answer that applies to
            // the pool itself).
            MethodReturn::AcquireHandle => {
                let elem = match self.ctx.types.get(bare_ty) {
                    SemaType::Pool(e) => *e,
                    _ => self.fresh_var(),
                };
                let handle = self.ctx.types.insert(SemaType::Handle(elem));
                let optional = self.ctx.types.insert(SemaType::Optional(handle));
                self.apply_receiver_wrap(wrap, optional)
            }
            // `List<T>.query()` — `Linqerizer<T>`. `T` comes from the
            // receiver `List<T>`, same pull-the-elem-out shape as
            // `NewListOfKey`/`NewListOfValue` pull from `Dictionary`.
            MethodReturn::NewLinqerizerOfElem => {
                let elem = match self.ctx.types.get(bare_ty) {
                    SemaType::List(e) => *e,
                    _ => self.fresh_var(),
                };
                let linq = self.ctx.types.insert(SemaType::Linqerizer(elem));
                self.apply_receiver_wrap(wrap, linq)
            }
            // `Linqerizer<T>.to_list()` — `List<T>`. `T` comes from the
            // receiver `Linqerizer<T>` itself, not a fixed type — this
            // is the mirror image of `NewLinqerizerOfElem` above.
            MethodReturn::NewListOfLinqElem => {
                let elem = match self.ctx.types.get(bare_ty) {
                    SemaType::Linqerizer(e) => *e,
                    _ => self.fresh_var(),
                };
                let list = self.ctx.types.insert(SemaType::List(elem));
                self.apply_receiver_wrap(wrap, list)
            }
        }
    }

    /// Re-wrap a freshly-constructed value in the same ref kind the
    /// receiver carried. `Gc` re-wraps to nothing (bare) rather than an
    /// explicit `SemaType::GcRef` — matching `maybe_arena_ref`'s existing
    /// convention that "no wrapper" already means GC-tier by default; a
    /// bare type and `GcRef(bare)` aren't currently treated as
    /// interchangeable by `unify`, so introducing the explicit form here
    /// would risk spurious mismatches against every other GC-default site.
    fn apply_receiver_wrap(&mut self, wrap: ReceiverWrap, inner: TypeId) -> TypeId {
        match wrap {
            ReceiverWrap::Gc => inner,
            ReceiverWrap::Arena { arena, mutable } =>
                self.ctx.types.insert(SemaType::ArenaRef { arena, mutable, inner }),
            ReceiverWrap::Pool { pool, mutable } =>
                self.ctx.types.insert(SemaType::PoolRef { pool, mutable, inner }),
            ReceiverWrap::Owned { mutable } =>
                self.ctx.types.insert(SemaType::OwnedRef { mutable, inner }),
        }
    }

    /// Convert a syntactic `Type<'ast>` node into a TypeId.
    /// Uses `match ty.kind` (not `&ty.kind`) since TypeKind is Copy,
    /// avoiding match-ergonomics reference confusion throughout.
    fn ast_type_to_sema<'ast>(&mut self, ty: &Type<'ast>) -> TypeId {
        match ty.kind {
            TypeKind::Int    => self.ctx.types.intern(SemaType::Int),
            TypeKind::Uint   => self.ctx.types.intern(SemaType::Uint),
            TypeKind::Long   => self.ctx.types.intern(SemaType::Long),
            TypeKind::Ulong  => self.ctx.types.intern(SemaType::Ulong),
            TypeKind::Short  => self.ctx.types.intern(SemaType::Int),   // widen
            TypeKind::Ushort => self.ctx.types.intern(SemaType::Uint),  // widen
            TypeKind::Byte   => self.ctx.types.intern(SemaType::I8),
            TypeKind::Ubyte  => self.ctx.types.intern(SemaType::U8),
            TypeKind::Float  => self.ctx.types.intern(SemaType::Float),
            TypeKind::Double => self.ctx.types.intern(SemaType::Double),
            TypeKind::Bool   => self.ctx.types.intern(SemaType::Bool),
            TypeKind::Char   => self.ctx.types.intern(SemaType::Char),
            TypeKind::Str    => self.ctx.types.intern(SemaType::Str),
            TypeKind::Void   => self.ctx.types.intern(SemaType::Void),
            TypeKind::I8     => self.ctx.types.intern(SemaType::I8),
            TypeKind::I16    => self.ctx.types.intern(SemaType::I16),
            TypeKind::I32    => self.ctx.types.intern(SemaType::I32),
            TypeKind::I64    => self.ctx.types.intern(SemaType::I64),
            TypeKind::U8     => self.ctx.types.intern(SemaType::U8),
            TypeKind::U16    => self.ctx.types.intern(SemaType::U16),
            TypeKind::U32    => self.ctx.types.intern(SemaType::U32),
            TypeKind::U64    => self.ctx.types.intern(SemaType::U64),
            TypeKind::F32    => self.ctx.types.intern(SemaType::F32),
            TypeKind::F64    => self.ctx.types.intern(SemaType::F64),
            TypeKind::Isize  => self.ctx.types.intern(SemaType::Isize),
            TypeKind::Usize  => self.ctx.types.intern(SemaType::Usize),

            TypeKind::List(inner) => {
                let elem = inner.map(|t| self.ast_type_to_sema(t))
                    .unwrap_or_else(|| self.fresh_var());
                self.ctx.types.intern(SemaType::List(elem))
            }
            TypeKind::Dictionary(kv) => {
                let (k, v) = kv.map(|(k, v)| (self.ast_type_to_sema(k), self.ast_type_to_sema(v)))
                    .unwrap_or_else(|| (self.fresh_var(), self.fresh_var()));
                self.ctx.types.insert(SemaType::Dictionary(k, v))
            }
            TypeKind::Set(inner) => {
                let elem = inner.map(|t| self.ast_type_to_sema(t))
                    .unwrap_or_else(|| self.fresh_var());
                self.ctx.types.intern(SemaType::Set(elem))
            }
            TypeKind::Queue(inner) => {
                let elem = inner.map(|t| self.ast_type_to_sema(t))
                    .unwrap_or_else(|| self.fresh_var());
                self.ctx.types.insert(SemaType::Queue(elem))
            }
            TypeKind::Stack(inner) => {
                let elem = inner.map(|t| self.ast_type_to_sema(t))
                    .unwrap_or_else(|| self.fresh_var());
                self.ctx.types.insert(SemaType::Stack(elem))
            }
            TypeKind::InlineList(inner) => {
                let elem = inner.map(|t| self.ast_type_to_sema(t))
                    .unwrap_or_else(|| self.fresh_var());
                self.ctx.types.insert(SemaType::InlineList(elem))
            }
            TypeKind::Named { path, args } => {
                let root = path.first().copied().unwrap_or("");
                // GENERICS_RULES.md — a bare single-segment name that
                // matches one of the enclosing struct/enum/fn's own
                // generic params (`T` inside `struct Box<T> { value: T }`)
                // resolves to its `Param` placeholder, not a top-level
                // lookup. Checked before `top_level_def` so a generic
                // param can't be shadowed by an unrelated top-level type
                // of the same name.
                if path.len() == 1 && args.is_empty() {
                    if let Some(&param_ty) = self.current_generic_params.get(root) {
                        return param_ty;
                    }
                }
                let arg_ids: Vec<TypeId> = args.iter()
                    .map(|a| self.ast_type_to_sema(a))
                    .collect();
                // Ownership-model wrappers (MEMORY_MODEL.md §9,
                // DATASTRUCTURES.md "Decisions locked in") — real compiler
                // builtins, not user-declared structs, so they're checked
                // before `top_level_def` the same way `List`/`Set`/etc.
                // never reach it at all (those get their own dedicated
                // `TypeKind` variant instead; these three don't need one
                // since bare `Identifier<Args>` parsing already produces
                // the exact shape needed here). A user who separately
                // declares `struct Unique<T> { ... }` themselves will find
                // this shadows it — same class of reserved-name collision
                // as `get`/`set`/`where` being reserved tokens elsewhere
                // in the language; not diagnosed as a name collision yet.
                if path.len() == 1 && matches!(root, "Unique" | "Shared" | "SyncShared") {
                    if arg_ids.len() != 1 {
                        self.errors.add_type_error(TypeError::GenericArgCountMismatch {
                            type_name: root.to_string(),
                            expected:  1,
                            found:     arg_ids.len(),
                            span:      ty.span,
                        });
                        return self.unknown();
                    }
                    let inner = arg_ids[0];
                    return self.ctx.types.insert(match root {
                        "Unique" => SemaType::Unique(inner),
                        "Shared" => SemaType::Shared(inner),
                        _        => SemaType::SyncShared(inner), // exhaustive via the outer matches! guard
                    });
                }
                match self.ctx.top_level_def(root) {
                    Some(def_id) => {
                        if let Some(&expected) = self.generic_arity.get(&def_id) {
                            if expected != arg_ids.len() {
                                self.errors.add_type_error(TypeError::GenericArgCountMismatch {
                                    type_name: root.to_string(),
                                    expected,
                                    found:     arg_ids.len(),
                                    span:      ty.span,
                                });
                            }
                        }
                        self.ctx.types.insert(SemaType::Named {
                            def: def_id, args: arg_ids,
                        })
                    }
                    None => self.unknown(),
                }
            }
            TypeKind::Tuple(fields) => {
                let ids: Vec<TypeId> = fields.iter()
                    .map(|f| self.ast_type_to_sema(f))
                    .collect();
                self.ctx.types.insert(SemaType::Tuple(ids))
            }
            TypeKind::Array { len, elem } => {
                let elem_id = self.ast_type_to_sema(elem);
                self.ctx.types.insert(SemaType::Array { len, elem: elem_id })
            }
            TypeKind::Slice(elem) => {
                let elem_id = self.ast_type_to_sema(elem);
                self.ctx.types.intern(SemaType::Slice(elem_id))
            }
            TypeKind::Fallible(inner) => {
                let inner_id = self.ast_type_to_sema(inner);
                self.ctx.types.intern(SemaType::Fallible(inner_id))
            }
            TypeKind::Task(inner) => {
                let inner_id = inner.map(|t| self.ast_type_to_sema(t))
                    .unwrap_or_else(|| self.void_ty());
                self.ctx.types.intern(SemaType::Task(inner_id))
            }
            TypeKind::Reference { mutable, lifetime, inner } => {
                // Was `SemaType::GcRef(inner_id)` — a stub that silently
                // discarded `mutable` and `lifetime` and made every
                // user-written `&T`/`&mut T` type annotation compile as
                // a plain HIGH-tier GC reference, in any tier. Nothing
                // exercised this path before now (no expression-level
                // borrow operator existed to construct a value of the
                // type), so it went uncaught. Fixed to the real
                // `SemaType::Reference` — see type_table.rs and TYPE-114.
                let inner_id = self.ast_type_to_sema(inner);
                self.ctx.types.insert(SemaType::Reference {
                    mutable,
                    lifetime: lifetime.map(|s| s.to_string()),
                    inner:    inner_id,
                })
            }
            TypeKind::Optional(inner) => {
                let inner_id = self.ast_type_to_sema(inner);
                self.ctx.types.intern(SemaType::Optional(inner_id))
            }
            TypeKind::Function(ft) => {
                let params: Vec<TypeId> = ft.params.iter()
                    .map(|p| self.ast_type_to_sema(p))
                    .collect();
                let ret = ft.return_type
                    .map(|r| self.ast_type_to_sema(r))
                    .unwrap_or_else(|| self.void_ty());
                self.ctx.types.insert(SemaType::Function {
                    params,
                    return_type: ret,
                    is_fallible: ft.is_fallible,
                    generic_arity: 0, // bare `fn(..)`  type annotations are never generic
                })
            }
            TypeKind::Infer => self.fresh_var(),
        }
    }

    /// Convert an optional return annotation to `(TypeId, is_fallible)`.
    fn return_type_to_sema<'ast>(&mut self, ret: Option<&ReturnType<'ast>>) -> (TypeId, bool) {
        match ret {
            None => (self.void_ty(), false),
            Some(rt) => {
                let inner = self.ast_type_to_sema(rt.ty);
                let ty = if rt.is_fallible {
                    self.ctx.types.intern(SemaType::Fallible(inner))
                } else {
                    inner
                };
                (ty, rt.is_fallible)
            }
        }
    }

    // ── Phase 2a: Signature collection ────────────────────────────

    fn collect_signatures<'ast>(&mut self, program: &Program<'ast>) {
        for item in program.items {
            match item {
                Item::Function(f) => { self.collect_fn_sig(f); }
                Item::Struct(s)   => { self.collect_struct_sig(s); }
                Item::Enum(e)     => { self.collect_enum_sig(e); }
                Item::Const(c)    => { self.collect_const_sig(c); }
                Item::Impl(i)     => {
                    for m in i.methods { self.collect_method_sig(m); }
                }
                Item::Extend(x) => {
                    for m in x.methods { self.collect_method_sig(m); }
                }
                Item::Trait(t) => {
                    // FIX: split DefaultMethod (MethodDecl) and MethodSig
                    // into separate arms — they are different types.
                    for it in t.items {
                        match it {
                            TraitItem::DefaultMethod(m) => {
                                self.collect_method_sig(m);
                            }
                            TraitItem::MethodSig(sig) => {
                                self.collect_trait_method_sig(sig);
                            }
                            TraitItem::AssociatedType { .. } => {}
                        }
                    }
                }
                Item::TypeAlias(_) => {} // alias expansion deferred
            }
        }
    }

    fn collect_fn_sig<'ast>(&mut self, f: &FunctionDecl<'ast>) -> TypeId {
        let prev_generics = self.push_generic_scope(f.generic_params);
        let param_tys: Vec<TypeId> = f.params.iter()
            .filter_map(|p| match p.kind {
                ParamKind::Named { ty, .. } | ParamKind::Discard { ty } => ty.map(|t| self.ast_type_to_sema(t)),
                _ => None,
            })
            .collect();
        let (ret_ty, is_fallible) = self.return_type_to_sema(f.return_type.as_ref());
        self.pop_generic_scope(prev_generics);
        let fn_ty = self.ctx.types.insert(SemaType::Function {
            params: param_tys,
            return_type: ret_ty,
            is_fallible,
            generic_arity: f.generic_params.len(),
        });
        if let Some(def_id) = self.ctx.top_level_def(f.name) {
            self.ctx.set_def_type(def_id, fn_ty);
            self.generic_arity.insert(def_id, f.generic_params.len());
        }
        fn_ty
    }

    fn collect_method_sig<'ast>(&mut self, m: &MethodDecl<'ast>) -> TypeId {
        // NOTE: a method's *own* extra generic params (beyond whatever
        // struct-level scope the caller may already have pushed — see
        // `collect_struct_sig`) aren't substituted at call sites yet;
        // scoped out for now (GENERICS_RULES.md "Known gaps"). Pushing
        // here would only matter for that unimplemented case, so it's
        // deliberately skipped rather than half-wired.
        let param_tys: Vec<TypeId> = m.params.iter()
            .filter_map(|p| match p.kind {
                ParamKind::Named { ty, .. } | ParamKind::Discard { ty } => ty.map(|t| self.ast_type_to_sema(t)),
                _ => None,
            })
            .collect();
        let (ret_ty, is_fallible) = self.return_type_to_sema(m.return_type.as_ref());
        let fn_ty = self.ctx.types.insert(SemaType::Function {
            params: param_tys,
            return_type: ret_ty,
            is_fallible,
            generic_arity: m.generic_params.len(),
        });
        if let Some(def_id) = self.ctx.resolutions.get(m.span) {
            self.ctx.set_def_type(def_id, fn_ty);
        }
        fn_ty
    }

    /// Collect type for a required method signature (no body) in a trait.
    fn collect_trait_method_sig<'ast>(&mut self, sig: &MethodSig<'ast>) {
        let param_tys: Vec<TypeId> = sig.params.iter()
            .filter_map(|p| match p.kind {
                ParamKind::Named { ty, .. } | ParamKind::Discard { ty } => ty.map(|t| self.ast_type_to_sema(t)),
                _ => None,
            })
            .collect();
        let (ret_ty, is_fallible) = self.return_type_to_sema(sig.return_type.as_ref());
        let fn_ty = self.ctx.types.insert(SemaType::Function {
            params: param_tys,
            return_type: ret_ty,
            is_fallible,
            generic_arity: sig.generic_params.len(),
        });
        if let Some(def_id) = self.ctx.resolutions.get(sig.span) {
            self.ctx.set_def_type(def_id, fn_ty);
        }
    }

    fn collect_struct_sig<'ast>(&mut self, s: &StructDecl<'ast>) {
        let Some(def_id) = self.ctx.top_level_def(s.name) else { return; };
        self.check_derive_attrs(s.attributes, false);
        let derived: HashSet<String> = crate::ast::common::derive_trait_names(s.attributes)
            .into_iter().map(str::to_string).collect();
        self.struct_derives.insert(def_id, derived);
        self.generic_arity.insert(def_id, s.generic_params.len());
        let prev_generics = self.push_generic_scope(s.generic_params);
        // The abstract "Self" type while collecting this struct's own
        // fields/methods — `Named { def, args: [Param(0), Param(1), ..] }`
        // for a generic struct, `Named { def, args: [] }` for a plain
        // one. Field/method signatures are stored in these (possibly
        // `Param`-containing) terms; every real use site substitutes
        // concrete args in via `substitute` (GENERICS_RULES.md).
        let self_args: Vec<TypeId> = (0..s.generic_params.len())
            .map(|i| self.ctx.types.intern(SemaType::Param(i)))
            .collect();
        let struct_ty = self.ctx.types.insert(SemaType::Named { def: def_id, args: self_args });
        self.ctx.set_def_type(def_id, struct_ty);

        let mut fields:  Vec<(String, TypeId)>      = Vec::with_capacity(s.members.len());
        let mut methods: Vec<(String, MethodShape)> = Vec::new();

        for member in s.members {
            match member {
                StructMember::Field(f) => {
                    let field_ty = self.ast_type_to_sema(f.ty);
                    if let Some(field_def) = self.ctx.resolutions.get(f.span) {
                        self.ctx.set_def_type(field_def, field_ty);
                    }
                    fields.push((f.name.to_string(), field_ty));
                }
                StructMember::Method(m) => {
                    let fn_ty = self.collect_method_sig(m);
                    let has_self = m.params.first()
                        .map(|p| matches!(p.kind,
                            ParamKind::SelfVal | ParamKind::SelfMut
                            | ParamKind::SelfRef | ParamKind::SelfRefMut))
                        .unwrap_or(false);
                    if let SemaType::Function { params, return_type, is_fallible, .. } =
                        self.ctx.types.get(fn_ty).clone()
                    {
                        methods.push((m.name.to_string(), MethodShape {
                            has_self, params, return_type, is_fallible,
                        }));
                    }
                }
                StructMember::Property(_) => {}
            }
        }

        self.struct_fields.insert(def_id, fields);
        self.struct_methods.insert(def_id, methods);
        self.pop_generic_scope(prev_generics);
    }

    fn collect_enum_sig<'ast>(&mut self, e: &EnumDecl<'ast>) {
        let Some(def_id) = self.ctx.top_level_def(e.name) else { return; };
        self.check_derive_attrs(e.attributes, true);
        self.generic_arity.insert(def_id, e.generic_params.len());
        let prev_generics = self.push_generic_scope(e.generic_params);
        let self_args: Vec<TypeId> = (0..e.generic_params.len())
            .map(|i| self.ctx.types.intern(SemaType::Param(i)))
            .collect();
        let enum_ty = self.ctx.types.insert(SemaType::Named { def: def_id, args: self_args });
        self.ctx.set_def_type(def_id, enum_ty);

        let mut shapes: Vec<(String, VariantShape)> = Vec::with_capacity(e.variants.len());
        let mut has_discriminant = false;
        let mut has_payload      = false;

        for variant in e.variants {
            if let Some(var_def) = self.ctx.resolutions.get(variant.span) {
                self.ctx.set_def_type(var_def, enum_ty);
            }
            let shape = match &variant.payload {
                EnumVariantPayload::None => VariantShape::None,
                EnumVariantPayload::Discriminant(expr) => {
                    has_discriminant = true;
                    // Auto-increment (ENUM_RULES.md §4 item 1) needs no
                    // active tracking here — nothing downstream currently
                    // reads a discriminant's chosen value back out (no
                    // `as int` cast exists yet), so this validates that
                    // the written expression is at least int-shaped and
                    // stops there. See this section's "Known gap."
                    let d_ty  = self.infer_expr(expr);
                    let int_ty = self.int_ty();
                    self.unify(int_ty, d_ty, expr.span);
                    VariantShape::Discriminant
                }
                EnumVariantPayload::Tuple(tys) => {
                    has_payload = true;
                    VariantShape::Tuple(tys.iter().map(|t| self.ast_type_to_sema(t)).collect())
                }
                EnumVariantPayload::Struct(fields) => {
                    has_payload = true;
                    VariantShape::Struct(
                        fields.iter()
                            .map(|f| (f.name.to_string(), self.ast_type_to_sema(f.ty)))
                            .collect()
                    )
                }
            };
            shapes.push((variant.name.to_string(), shape));
        }

        // ENUM_RULES.md §4 item 2 — an explicit discriminant and a
        // payload-carrying variant have no defined combined runtime
        // representation; reject the enum declaration outright rather
        // than silently picking one interpretation.
        if has_discriminant && has_payload {
            self.errors.add_type_error(TypeError::MixedDiscriminantAndPayload {
                enum_name: e.name.to_string(),
                span:      e.span,
            });
        }

        self.pop_generic_scope(prev_generics);
        self.enum_variants.insert(def_id, shapes);
    }

    /// Resolve `ty` down to `(enum DefId, variant shapes)` if it names a
    /// known enum, else `None` (covers non-enum types and unresolved
    /// vars uniformly — every enum caller below just treats "not an
    /// enum" the same way regardless of why). For a generic enum, the
    /// stored shapes (raw, `Param`-containing — see `collect_enum_sig`)
    /// are substituted against `ty`'s own concrete `args` before being
    /// returned, so every caller (`check_pattern`, tuple/struct-variant
    /// construction) gets already-concrete types for free and needs no
    /// generics-awareness of its own (GENERICS_RULES.md).
    fn enum_shapes_of(&mut self, ty: TypeId) -> Option<(DefId, Vec<(String, VariantShape)>)> {
        let resolved = self.apply(ty);
        let SemaType::Named { def, args } = self.ctx.types.get(resolved).clone() else { return None; };
        let raw_shapes = self.enum_variants.get(&def)?.clone();
        if args.is_empty() {
            return Some((def, raw_shapes));
        }
        let shapes: Vec<(String, VariantShape)> = raw_shapes.into_iter()
            .map(|(name, shape)| {
                let shape = match shape {
                    VariantShape::None         => VariantShape::None,
                    VariantShape::Discriminant => VariantShape::Discriminant,
                    VariantShape::Tuple(tys) => VariantShape::Tuple(
                        tys.into_iter().map(|t| self.substitute(t, &args)).collect()
                    ),
                    VariantShape::Struct(fields) => VariantShape::Struct(
                        fields.into_iter()
                            .map(|(n, t)| (n, self.substitute(t, &args)))
                            .collect()
                    ),
                };
                (name, shape)
            })
            .collect();
        Some((def, shapes))
    }

    /// Type-checks a match-arm pattern against the scrutinee's type,
    /// binding any names it introduces. Returns what it definitively
    /// covers, for the exhaustiveness check in `StmtKind::Match`/
    /// `ExprKind::Match`. ENUM_RULES.md.
    fn check_pattern<'ast>(&mut self, pat: &Pattern<'ast>, scrutinee_ty: TypeId) -> PatternCoverage {
        match &pat.kind {
            PatternKind::Wildcard => PatternCoverage::CatchAll,

            PatternKind::Ident { name, .. } => {
                if let Some((enum_def, shapes)) = self.enum_shapes_of(scrutinee_ty) {
                    if let Some((vname, shape)) = shapes.iter().find(|(n, _)| n == name) {
                        // The actual point of this section's fix: a bare
                        // unqualified variant name (`North`, not
                        // `Direction.North`) used to parse as this same
                        // PatternKind::Ident and silently behave as an
                        // unconditional binding — "always matches, binds
                        // name" per the interpreter's own comment — so
                        // whichever arm's name happened to come first
                        // fired regardless of the scrutinee's real value.
                        // Reinterpreted here now that the scrutinee's
                        // type is known: matches only this one variant,
                        // introduces no binding. Pass 1 already declared
                        // `name` as a fresh Local for the general-binding
                        // case; that declaration goes simply unused here,
                        // which is harmless.
                        let expected = match shape {
                            VariantShape::None | VariantShape::Discriminant => {
                                return PatternCoverage::Variants(vec![vname.clone()]);
                            }
                            VariantShape::Tuple(t)  => t.len(),
                            VariantShape::Struct(t) => t.len(),
                        };
                        // A payload-carrying variant referenced bare with
                        // no payload pattern at all (`Ok => ...` instead
                        // of `Ok(x)`) — surface it instead of silently
                        // treating `name` as a binding for a shape it
                        // doesn't fit.
                        self.errors.add_type_error(TypeError::VariantArityMismatch {
                            enum_name:    self.ctx.symbols.lookup(enum_def).name.clone(),
                            variant_name: vname.clone(),
                            expected,
                            found:        0,
                            span:         pat.span,
                        });
                        return PatternCoverage::Variants(vec![vname.clone()]);
                    }
                }
                // Genuine catch-all binding.
                if let Some(def_id) = self.ctx.resolutions.get(pat.span) {
                    self.ctx.set_def_type(def_id, scrutinee_ty);
                }
                PatternCoverage::CatchAll
            }

            PatternKind::Literal(lit) => {
                let lit_ty = self.infer_literal(lit);
                self.unify(scrutinee_ty, lit_ty, pat.span);
                PatternCoverage::Other
            }

            PatternKind::Enum { path, payload } => {
                self.check_enum_pattern(path, payload, scrutinee_ty, pat.span)
            }

            PatternKind::Or(pats) => {
                let mut variants   = Vec::new();
                let mut catch_all  = false;
                for p in pats.iter() {
                    match self.check_pattern(p, scrutinee_ty) {
                        PatternCoverage::CatchAll     => catch_all = true,
                        PatternCoverage::Variants(vs) => variants.extend(vs),
                        PatternCoverage::Other        => {}
                    }
                }
                if catch_all { PatternCoverage::CatchAll }
                else if !variants.is_empty() { PatternCoverage::Variants(variants) }
                else { PatternCoverage::Other }
            }

            PatternKind::Tuple(pats) => {
                let resolved = self.apply(scrutinee_ty);
                if let SemaType::Tuple(elem_tys) = self.ctx.types.get(resolved) {
                    let elem_tys = elem_tys.clone();
                    for (p, t) in pats.iter().zip(elem_tys.iter()) {
                        self.check_pattern(p, *t);
                    }
                    for p in pats.iter().skip(elem_tys.len()) {
                        let fresh = self.fresh_var();
                        self.check_pattern(p, fresh);
                    }
                } else {
                    for p in pats.iter() {
                        let fresh = self.fresh_var();
                        self.check_pattern(p, fresh);
                    }
                }
                PatternCoverage::Other
            }

            // Struct-destructure / array / range / other-language-level
            // patterns aren't part of this section's scope — walked
            // permissively (bindings still get *some* type, so later
            // references don't dangle) without deep shape validation.
            // Doesn't affect enum exhaustiveness either way.
            // `Name { field, ... }` with a 1-segment name always parses
            // as this (not PatternKind::Enum) regardless of whether
            // `Name` turns out to be a plain struct or an enum's
            // struct-payload variant — the parser can't tell without
            // type info (`parse_pattern.rs`: 2+ segments is the only
            // syntactic signal it has, `Result.Err { code }` vs
            // `Point { x, y }`). Same reinterpretation this section
            // already does for bare `PatternKind::Ident` variant names —
            // if `name` matches a Struct-payload variant of the known
            // enum scrutinee, treat it as that; otherwise it's a genuine
            // struct pattern.
            PatternKind::Struct { name: Some(n), fields } => {
                if let Some((enum_def, shapes)) = self.enum_shapes_of(scrutinee_ty) {
                    if let Some((vname, shape)) = shapes.iter().find(|(sn, _)| sn == n) {
                        let vname = vname.clone();
                        return match shape {
                            VariantShape::Struct(field_tys) => {
                                let field_tys = field_tys.clone();
                                for f in fields.iter() {
                                    match field_tys.iter().find(|(fn_, _)| fn_ == f.field) {
                                        Some((_, t)) => {
                                            if let Some(sub_pat) = &f.pattern {
                                                self.check_pattern(sub_pat, *t);
                                            } else if let Some(def_id) = self.ctx.resolutions.get(f.span) {
                                                self.ctx.set_def_type(def_id, *t);
                                            }
                                        }
                                        None => {
                                            self.errors.add_type_error(TypeError::UnknownVariant {
                                                enum_name:    self.ctx.symbols.lookup(enum_def).name.clone(),
                                                variant_name: format!("{}.{{{}}}", vname, f.field),
                                                span:         f.span,
                                            });
                                        }
                                    }
                                }
                                PatternCoverage::Variants(vec![vname])
                            }
                            _ => {
                                // Real variant, wrong payload shape.
                                let expected = match shape {
                                    VariantShape::Tuple(t)  => t.len(),
                                    _                        => 0,
                                };
                                self.errors.add_type_error(TypeError::VariantArityMismatch {
                                    enum_name:    self.ctx.symbols.lookup(enum_def).name.clone(),
                                    variant_name: vname.clone(),
                                    expected,
                                    found:        fields.len(),
                                    span:         pat.span,
                                });
                                PatternCoverage::Variants(vec![vname])
                            }
                        };
                    }
                }
                for f in fields.iter() {
                    if let Some(sub_pat) = &f.pattern {
                        let fresh = self.fresh_var();
                        self.check_pattern(sub_pat, fresh);
                    } else if let Some(def_id) = self.ctx.resolutions.get(f.span) {
                        let fresh = self.fresh_var();
                        self.ctx.set_def_type(def_id, fresh);
                    }
                }
                PatternCoverage::Other
            }
            PatternKind::Struct { name: None, fields } => {
                for f in fields.iter() {
                    if let Some(sub_pat) = &f.pattern {
                        let fresh = self.fresh_var();
                        self.check_pattern(sub_pat, fresh);
                    } else if let Some(def_id) = self.ctx.resolutions.get(f.span) {
                        let fresh = self.fresh_var();
                        self.ctx.set_def_type(def_id, fresh);
                    }
                }
                PatternCoverage::Other
            }
            PatternKind::Array { elements, .. } => {
                for p in elements.iter() {
                    let fresh = self.fresh_var();
                    self.check_pattern(p, fresh);
                }
                PatternCoverage::Other
            }
            PatternKind::Extract(fields) => {
                for f in fields.iter() {
                    if let Some(sub_pat) = &f.pattern {
                        let fresh = self.fresh_var();
                        self.check_pattern(sub_pat, fresh);
                    } else if let Some(def_id) = self.ctx.resolutions.get(f.span) {
                        let fresh = self.fresh_var();
                        self.ctx.set_def_type(def_id, fresh);
                    }
                }
                PatternCoverage::Other
            }
            PatternKind::Range { .. } => PatternCoverage::Other,
        }
    }

    fn check_enum_pattern<'ast>(
        &mut self,
        path: &[&'ast str],
        payload: &EnumPatternPayload<'ast>,
        scrutinee_ty: TypeId,
        span: Span,
    ) -> PatternCoverage {
        let Some((enum_def, shapes)) = self.enum_shapes_of(scrutinee_ty) else {
            // Scrutinee isn't a known enum (Unknown/unresolved var, or a
            // genuinely non-enum type — that mismatch surfaces on its
            // own via the scrutinee's own inference). Still walk payload
            // sub-patterns with fresh vars so their bindings get *some*
            // type instead of dangling.
            self.check_enum_payload_fallback(payload);
            return PatternCoverage::Other;
        };

        let Some(variant_name) = path.last().copied() else { return PatternCoverage::Other; };

        let Some((vname, shape)) = shapes.iter().find(|(n, _)| n == variant_name) else {
            self.errors.add_type_error(TypeError::UnknownVariant {
                enum_name:    self.ctx.symbols.lookup(enum_def).name.clone(),
                variant_name: variant_name.to_string(),
                span,
            });
            self.check_enum_payload_fallback(payload);
            return PatternCoverage::Other;
        };
        let vname = vname.clone();

        match (shape, payload) {
            (VariantShape::None, EnumPatternPayload::None)
            | (VariantShape::Discriminant, EnumPatternPayload::None) => {}

            (VariantShape::Tuple(elem_tys), EnumPatternPayload::Tuple(pats)) => {
                let elem_tys = elem_tys.clone();
                if elem_tys.len() != pats.len() {
                    self.errors.add_type_error(TypeError::VariantArityMismatch {
                        enum_name:    self.ctx.symbols.lookup(enum_def).name.clone(),
                        variant_name: vname.clone(),
                        expected:     elem_tys.len(),
                        found:        pats.len(),
                        span,
                    });
                }
                for (sub_pat, elem_ty) in pats.iter().zip(elem_tys.iter()) {
                    self.check_pattern(sub_pat, *elem_ty);
                }
                for sub_pat in pats.iter().skip(elem_tys.len()) {
                    let fresh = self.fresh_var();
                    self.check_pattern(sub_pat, fresh);
                }
            }

            (VariantShape::Struct(field_tys), EnumPatternPayload::Struct(fields)) => {
                let field_tys = field_tys.clone();
                for f in fields.iter() {
                    match field_tys.iter().find(|(n, _)| n == f.field) {
                        Some((_, t)) => {
                            if let Some(sub_pat) = &f.pattern {
                                self.check_pattern(sub_pat, *t);
                            } else if let Some(def_id) = self.ctx.resolutions.get(f.span) {
                                self.ctx.set_def_type(def_id, *t);
                            }
                        }
                        None => {
                            self.errors.add_type_error(TypeError::UnknownVariant {
                                enum_name:    self.ctx.symbols.lookup(enum_def).name.clone(),
                                variant_name: format!("{}.{{{}}}", vname, f.field),
                                span:         f.span,
                            });
                            if let Some(sub_pat) = &f.pattern {
                                let fresh = self.fresh_var();
                                self.check_pattern(sub_pat, fresh);
                            }
                        }
                    }
                }
            }

            // Payload shape doesn't match the pattern's payload shape at
            // all (e.g. matching a tuple-payload variant with no parens,
            // or a fieldless variant with parens) — a degenerate arity
            // mismatch (0 either side vs. however many the pattern wrote).
            _ => {
                let (expected, found) = match (shape, payload) {
                    (VariantShape::Tuple(t), EnumPatternPayload::None)  => (t.len(), 0),
                    (VariantShape::Struct(t), EnumPatternPayload::None) => (t.len(), 0),
                    (VariantShape::None | VariantShape::Discriminant, EnumPatternPayload::Tuple(p))
                        => (0, p.len()),
                    (VariantShape::None | VariantShape::Discriminant, EnumPatternPayload::Struct(p))
                        => (0, p.len()),
                    _ => (0, 0),
                };
                self.errors.add_type_error(TypeError::VariantArityMismatch {
                    enum_name: self.ctx.symbols.lookup(enum_def).name.clone(),
                    variant_name: vname.clone(),
                    expected,
                    found,
                    span,
                });
                self.check_enum_payload_fallback(payload);
            }
        }

        PatternCoverage::Variants(vec![vname])
    }

    fn check_enum_payload_fallback<'ast>(&mut self, payload: &EnumPatternPayload<'ast>) {
        match payload {
            EnumPatternPayload::None => {}
            EnumPatternPayload::Tuple(pats) => {
                for p in pats.iter() {
                    let fresh = self.fresh_var();
                    self.check_pattern(p, fresh);
                }
            }
            EnumPatternPayload::Struct(fields) => {
                for f in fields.iter() {
                    if let Some(sub_pat) = &f.pattern {
                        let fresh = self.fresh_var();
                        self.check_pattern(sub_pat, fresh);
                    } else if let Some(def_id) = self.ctx.resolutions.get(f.span) {
                        let fresh = self.fresh_var();
                        self.ctx.set_def_type(def_id, fresh);
                    }
                }
            }
        }
    }

    /// Checks match-arm coverage against an enum scrutinee's full variant
    /// set, emitting `NonExhaustiveMatch` if neither a catch-all arm nor
    /// the union of every guard-less arm's coverage accounts for all of
    /// them. Pragmatic top-level-only exhaustiveness (no nested-pattern
    /// usefulness analysis) — not Rust's full decision-tree algorithm,
    /// but a real, useful check rather than none at all.
    fn check_match_exhaustiveness<'ast>(
        &mut self,
        scrutinee_ty: TypeId,
        arms: &[(PatternCoverage, bool /* has_guard */)],
        span: Span,
    ) {
        let Some((_, shapes)) = self.enum_shapes_of(scrutinee_ty) else { return; };

        if arms.iter().any(|(cov, has_guard)| !has_guard && matches!(cov, PatternCoverage::CatchAll)) {
            return;
        }

        let mut covered: std::collections::HashSet<&str> = std::collections::HashSet::new();
        for (cov, has_guard) in arms.iter() {
            if *has_guard { continue; }
            if let PatternCoverage::Variants(vs) = cov {
                covered.extend(vs.iter().map(|s| s.as_str()));
            }
        }

        let missing: Vec<String> = shapes.iter()
            .map(|(n, _)| n.clone())
            .filter(|n| !covered.contains(n.as_str()))
            .collect();

        if !missing.is_empty() {
            self.errors.add_type_error(TypeError::NonExhaustiveMatch {
                missing_variants: missing,
                span,
            });
        }
    }

    fn collect_const_sig<'ast>(&mut self, c: &ConstDecl<'ast>) {
        let Some(def_id) = self.ctx.top_level_def(c.name) else { return; };
        let ty = c.ty.map(|t| self.ast_type_to_sema(t))
            .unwrap_or_else(|| self.fresh_var());
        self.ctx.set_def_type(def_id, ty);
    }

    // ── Phase 2b: Body inference ──────────────────────────────────

    fn infer_bodies<'ast>(&mut self, program: &Program<'ast>) {
        for item in program.items {
            match item {
                Item::Function(f) => self.infer_function_body(f),
                Item::Struct(s)   => self.infer_struct_bodies(s),
                Item::Const(c)    => self.infer_const_body(c),
                Item::Impl(i) => {
                    for m in i.methods { self.infer_method_body(m); }
                }
                Item::Extend(x) => {
                    for m in x.methods { self.infer_method_body(m); }
                }
                Item::Trait(t) => {
                    for it in t.items {
                        if let TraitItem::DefaultMethod(m) = it {
                            self.infer_method_body(m);
                        }
                    }
                }
                Item::Enum(_) | Item::TypeAlias(_) => {}
            }
        }
    }

    fn infer_function_body<'ast>(&mut self, f: &FunctionDecl<'ast>) {
        for param in f.params { self.seed_param(param); }
        let (ret_ty, is_fallible) = self.return_type_to_sema(f.return_type.as_ref());
        let prev_ret      = self.current_return.replace(ret_ty);
        let prev_fallible = self.current_fallible;
        self.current_fallible = is_fallible;
        let prev_tier = self.current_tier;
        self.current_tier = f.tier;
        self.infer_block(&f.body);
        self.current_return   = prev_ret;
        self.current_fallible = prev_fallible;
        self.current_tier     = prev_tier;
    }

    fn infer_method_body<'ast>(&mut self, m: &MethodDecl<'ast>) {
        for param in m.params { self.seed_param(param); }
        let (ret_ty, is_fallible) = self.return_type_to_sema(m.return_type.as_ref());
        let prev_ret      = self.current_return.replace(ret_ty);
        let prev_fallible = self.current_fallible;
        self.current_fallible = is_fallible;
        let prev_tier = self.current_tier;
        self.current_tier = m.tier;
        self.infer_block(&m.body);
        self.current_return   = prev_ret;
        self.current_fallible = prev_fallible;
        self.current_tier     = prev_tier;
    }

    fn seed_param<'ast>(&mut self, param: &Param<'ast>) {
        match param.kind {
            ParamKind::Named { ty, .. } => {
                let ty_id = ty.map(|t| self.ast_type_to_sema(t))
                    .unwrap_or_else(|| self.fresh_var());
                if let Some(def_id) = self.ctx.resolutions.get(param.span) {
                    self.ctx.set_def_type(def_id, ty_id);
                    self.ctx.set_binding_type(param.span, ty_id);
                }
            }
            _ => {} // self params — TODO when current_struct_type threaded in
        }
    }

    fn infer_struct_bodies<'ast>(&mut self, s: &StructDecl<'ast>) {
        let Some(def_id) = self.ctx.top_level_def(s.name) else { return; };
        let prev_generics = self.push_generic_scope(s.generic_params);
        // `self`'s type for the duration of this struct's own method
        // bodies — see `current_struct_type`'s own doc comment. Rebuilt
        // here (not reused from `collect_struct_sig`) since that field
        // TypeId belongs to a different `push_generic_scope` call and
        // this is a fresh one — same positional `Param` indices, so the
        // *meaning* is identical, just a different `TypeId` instance.
        let self_args: Vec<TypeId> = (0..s.generic_params.len())
            .map(|i| self.ctx.types.intern(SemaType::Param(i)))
            .collect();
        let self_ty = self.ctx.types.insert(SemaType::Named { def: def_id, args: self_args });
        let prev_self = self.current_struct_type.replace(self_ty);
        for member in s.members {
            if let StructMember::Method(m) = member {
                self.infer_method_body(m);
            }
        }
        self.current_struct_type = prev_self;
        self.pop_generic_scope(prev_generics);
    }

    fn infer_const_body<'ast>(&mut self, c: &ConstDecl<'ast>) {
        let inferred = self.infer_expr(c.value);
        if let Some(def_id) = self.ctx.top_level_def(c.name) {
            if let Some(declared) = self.ctx.def_type(def_id) {
                self.unify(declared, inferred, c.value.span);
            } else {
                self.ctx.set_def_type(def_id, inferred);
            }
        }
    }

    // ── Block ─────────────────────────────────────────────────────

    fn infer_block<'ast>(&mut self, block: &Block<'ast>) -> TypeId {
        let mut last = self.void_ty();
        for stmt in block.stmts {
            last = self.infer_stmt(stmt);
        }
        last
    }

    /// Infer an `if`/`elif`/`else` branch body's type — either a
    /// `{ block }` (type of its last statement) or the single-line
    /// `then expr` form (type of the expression). Mirrors the
    /// `MatchArmBody` dispatch used for match arms below.
    fn infer_if_branch<'ast>(&mut self, body: &IfBranchBody<'ast>) -> TypeId {
        match body {
            IfBranchBody::Expr(e)  => self.infer_expr(e),
            IfBranchBody::Block(b) => self.infer_block(b),
        }
    }

    // ── Statements ────────────────────────────────────────────────

    fn infer_stmt<'ast>(&mut self, stmt: &Stmt<'ast>) -> TypeId {
        match &stmt.kind {
            StmtKind::Let { binding, ty, value, .. } => {
                let rhs_ty = self.infer_expr(value);
                let bind_ty = match ty {
                    Some(ann) => {
                        let ann_id = self.ast_type_to_sema(ann);
                        self.unify(ann_id, rhs_ty, stmt.span);
                        ann_id
                    }
                    None => rhs_ty,
                };
                self.record_binding(binding, bind_ty, stmt.span);
                self.void_ty()
            }
            StmtKind::Expr(e) => self.infer_expr(e),
            StmtKind::Return(maybe_e) => {
                let ret = maybe_e.map(|e| self.infer_expr(e))
                    .unwrap_or_else(|| self.void_ty());
                if let Some(expected) = self.current_return {
                    let expected_inner = match self.ctx.types.get(expected) {
                        SemaType::Fallible(inner) => *inner,
                        _ => expected,
                    };
                    self.unify(expected_inner, ret, stmt.span);
                }
                // NOT void_ty(): a block's trailing statement type matters
                // for expression-bodied lambdas (`fn(x) x * 2`), which unify
                // it against the lambda's inferred return type. A `return`
                // statement diverges — it has no meaningful "value" of its
                // own here, already unified against current_return above —
                // so this must be an unconstrained fresh var (unifies with
                // anything) rather than void (which would conflict with
                // whatever `ret` actually was, exactly like the never-taken
                // `Some` branch bug this fixes: correct type computed, then
                // silently overwritten by a hardcoded wrong one).
                self.fresh_var()
            }
            StmtKind::Fail(e)        => { self.infer_expr(e); self.void_ty() }
            StmtKind::Break(maybe_e) => { maybe_e.map(|e| self.infer_expr(e)); self.void_ty() }
            StmtKind::Continue       => self.void_ty(),
            StmtKind::Defer(e)       => { self.infer_expr(e); self.void_ty() }

            StmtKind::If(if_node) => {
                let cond    = self.infer_expr(if_node.condition);
                let bool_ty = self.bool_ty();
                self.unify(bool_ty, cond, if_node.condition.span);
                let then_ty = self.infer_if_branch(&if_node.then_body);
                for elif in if_node.elif_branches {
                    let ct = self.infer_expr(elif.condition);
                    self.unify(bool_ty, ct, elif.condition.span);
                    self.infer_if_branch(&elif.body);
                }
                if let Some(else_b) = &if_node.else_body {
                    let else_ty = self.infer_if_branch(else_b);
                    let void_ty = self.void_ty();
                    if then_ty != void_ty && else_ty != void_ty {
                        self.unify(then_ty, else_ty, stmt.span);
                    }
                }
                self.void_ty()
            }
            StmtKind::Match { scrutinee, arms } => {
                let scrutinee_ty = self.infer_expr(scrutinee);
                let mut coverage = Vec::with_capacity(arms.len());
                for arm in arms.iter() {
                    let cov = self.check_pattern(&arm.pattern, scrutinee_ty);
                    if let Some(g) = arm.guard { self.infer_expr(g); }
                    match &arm.body {
                        MatchArmBody::Expr(e)  => { self.infer_expr(e); }
                        MatchArmBody::Block(b) => { self.infer_block(b); }
                    }
                    coverage.push((cov, arm.guard.is_some()));
                }
                self.check_match_exhaustiveness(scrutinee_ty, &coverage, stmt.span);
                self.void_ty()
            }
            StmtKind::For { binding, iter, body } => {
                let iter_ty = self.infer_expr(iter);
                let elem_ty = self.element_type_of(iter_ty);
                self.record_binding(binding, elem_ty, stmt.span);
                self.infer_block(body);
                self.void_ty()
            }
            StmtKind::While { condition, body } => {
                let cond    = self.infer_expr(condition);
                let bool_ty = self.bool_ty();
                self.unify(bool_ty, cond, condition.span);
                self.infer_block(body);
                self.void_ty()
            }
            StmtKind::Loop(body) => { self.infer_block(body); self.void_ty() }
            // BUG FIX (MEMORY_MODEL.md §11) — this arm used to ignore
            // `allocator` entirely and push an arena scope for *every*
            // `with` block, so `with pool<T>(count) { }` and even
            // `with gc { }`/`with heap { }` silently got arena's escape-
            // boundary semantics whether they wanted them or not. Now
            // dispatches on the actual allocator kind; only `Arena` and
            // `Pool` push a scope, since those are the only two kinds
            // with a lifetime narrower than their enclosing function.
            StmtKind::With { allocator, body } => {
                match allocator {
                    AllocatorKind::Arena(_) => {
                        let _arena_id = self.push_arena();
                        self.infer_block(body);
                        self.pop_arena();
                    }
                    AllocatorKind::Pool { ty, count } => {
                        let elem_ty  = self.ast_type_to_sema(ty);
                        let count_ty = self.infer_expr(count);
                        let int_ty   = self.int_ty();
                        self.unify(int_ty, count_ty, count.span);
                        let _pool_id = self.push_pool(elem_ty);
                        self.infer_block(body);
                        self.pop_pool();
                    }
                    AllocatorKind::Gc | AllocatorKind::Heap => {
                        self.infer_block(body);
                    }
                }
                self.void_ty()
            }
            StmtKind::Using { bindings, body } => {
                for b in bindings.iter() {
                    let ty = self.infer_expr(b.value);
                    if let Some(def_id) = self.ctx.resolutions.get(b.span) {
                        self.ctx.set_def_type(def_id, ty);
                        self.ctx.set_binding_type(b.span, ty);
                    }
                }
                self.infer_block(body);
                self.void_ty()
            }
            StmtKind::Extract { value, .. } => { self.infer_expr(value); self.void_ty() }
            StmtKind::Try { body, catch_body, .. } => {
                self.infer_block(body);
                if let Some(cb) = catch_body { self.infer_block(cb); }
                self.void_ty()
            }
            StmtKind::Unsafe(body) => { self.infer_block(body); self.void_ty() }
        }
    }

    fn record_binding<'ast>(&mut self, target: &BindingTarget<'ast>, ty: TypeId, span: Span) {
        match target {
            BindingTarget::Ident(_) => {
                if let Some(def_id) = self.ctx.resolutions.get(span) {
                    self.ctx.set_def_type(def_id, ty);
                    self.ctx.set_binding_type(span, ty);
                }
            }
            BindingTarget::Destructure(_) => {
                self.ctx.set_binding_type(span, ty);
            }
        }
    }

    /// DATASTRUCTURES.md §1 — `for x in pool { }` iterates a `Pool<T>`
    /// directly, same as every other collection here; no `.iter()`
    /// method call needed (none of `List`/`Queue`/`Stack` have one
    /// either). Bare `T` values, not paired with their `Handle` — see
    /// `value_to_iter_vec`'s own doc comment on why.
    fn element_type_of(&mut self, collection_ty: TypeId) -> TypeId {
        let resolved = self.apply(collection_ty);
        match self.ctx.types.get(resolved) {
            SemaType::List(e)
            | SemaType::Set(e)
            | SemaType::Queue(e)
            | SemaType::Stack(e)
            | SemaType::Slice(e)
            | SemaType::InlineList(e)
            | SemaType::Pool(e)          => *e,
            SemaType::Array { elem, .. } => *elem,
            SemaType::ArenaRef { inner, .. } => {
                let inner = *inner;
                self.element_type_of(inner)
            }
            SemaType::PoolRef { inner, .. } => {
                let inner = *inner;
                self.element_type_of(inner)
            }
            _ => self.fresh_var(),
        }
    }

    // ── Expressions ───────────────────────────────────────────────

    fn infer_expr<'ast>(&mut self, expr: &Expr<'ast>) -> TypeId {
        let ty = self.infer_expr_inner(expr);
        self.ctx.set_expr_type(expr.span, ty);
        ty
    }

    /// GAP 2 — detect an `ArenaRef`/`PoolRef` value (MEMORY_MODEL.md §11
    /// generalized this from arena-only to both) flowing into a binding,
    /// struct field, or indexed slot whose own home scope (if any)
    /// doesn't match. Returns `true` if an escape was detected and
    /// reported (the caller should skip the generic `unify` to avoid a
    /// duplicate, less-specific diagnostic on the same expression).
    ///
    /// "Home arena/pool" per target shape:
    ///   - `Ident`  — the ArenaId (if any) the binding's *original*
    ///     `def_type` carries. Reassignment never mutates `def_types`
    ///     (only the unifier's substitution table changes), so re-reading
    ///     it here always reflects the declaration site, not the current
    ///     statement — exactly the "which arena does this binding belong
    ///     to" question we need.
    ///   - `Field { target: receiver, .. }` — the receiver's own
    ///     top-level ArenaId, if the receiver itself is arena-tagged
    ///     (i.e. the whole struct instance lives in that arena, so
    ///     storing a same-arena value into one of its fields is safe).
    ///     No struct field can be *declared* `ArenaRef` (no surface
    ///     syntax exists for it), so an untagged receiver's home arena
    ///     is always `None` — any arena-tagged value assigned into a
    ///     field on it is unconditionally an escape.
    ///   - `Index { target: container, .. }` — same idea, using the
    ///     indexed container's own top-level ArenaId.
    ///   - anything else (assignment through `Try`/`OptionalChain`, etc.)
    ///     — skipped. Rare enough in practice not to risk a false
    ///     positive from a shape this function doesn't model yet.
    ///
    /// Deliberately conservative: this only compares *top-level* tags,
    /// and for `Ident` targets it flags a mismatch even on the very
    /// first assignment to an untyped, not-yet-arena-tagged binding
    /// (e.g. an untyped function parameter). Proving that case safe in
    /// general requires liveness analysis this pass doesn't do — see
    /// docs/MEMORY_MODEL.md §6 for the reasoning; over-flagging a
    /// borderline-safe pattern is the right default for a memory-safety
    /// check, the same way a real borrow checker sometimes does.
    /// Same idea as the arena case immediately above, checked first
    /// since a value can only ever be tagged with one kind of scope ref
    /// at its top level. Reuses the exact same "home scope per target
    /// shape" resolution — only the tag type and the reported diagnostic
    /// differ, so `home_ty` is computed once and both checks probe it.
    fn check_assign_arena_escape<'ast>(
        &mut self,
        target: &Expr<'ast>,
        target_ty: TypeId,
        value_ty: TypeId,
        span: Span,
    ) -> bool {
        let home_ty: TypeId = match &target.kind {
            ExprKind::Ident(_) => target_ty,
            ExprKind::Field { target: receiver, .. } => {
                self.ctx.expr_type(receiver.span).unwrap_or(target_ty)
            }
            ExprKind::Index { target: container, .. } => {
                self.ctx.expr_type(container.span).unwrap_or(target_ty)
            }
            _ => return false,
        };

        if let Some(value_pool) = self.top_level_pool(value_ty) {
            if self.top_level_pool(home_ty) == Some(value_pool) { return false; }
            self.pool_escape(value_ty, span);
            return true;
        }

        let Some(value_arena) = self.top_level_arena(value_ty) else { return false; };
        if self.top_level_arena(home_ty) == Some(value_arena) { return false; }

        self.arena_escape(value_ty, span);
        true
    }

    fn infer_expr_inner<'ast>(&mut self, expr: &Expr<'ast>) -> TypeId {
        match &expr.kind {
            ExprKind::Lit(lit) => self.infer_literal(lit),

            ExprKind::Ident(_) => {
                if let Some(def_id) = self.ctx.resolutions.get(expr.span) {
                    self.ctx.def_type(def_id).unwrap_or_else(|| self.fresh_var())
                } else {
                    self.unknown()
                }
            }

            ExprKind::SelfExpr => self.current_struct_type.unwrap_or_else(|| self.unknown()),

            ExprKind::ShortDecl { value, .. } => {
                let ty = self.infer_expr(value);
                if let Some(def_id) = self.ctx.resolutions.get(expr.span) {
                    self.ctx.set_def_type(def_id, ty);
                    self.ctx.set_binding_type(expr.span, ty);
                }
                ty
            }

            ExprKind::Assign { target, value, .. } => {
                let target_ty = self.infer_expr(target);
                let value_ty  = self.infer_expr(value);
                // GAP 2 — check for an arena escape first. If the assign
                // itself is the escape, report that specifically and skip
                // the generic structural unify below: since ArenaRef pairs
                // with mismatched arenas are never structurally compatible
                // (see `structurally_compatible`), unify would otherwise
                // immediately pile a second, less useful TypeMismatch on
                // top of the same problem.
                if !self.check_assign_arena_escape(target, target_ty, value_ty, expr.span) {
                    self.unify(target_ty, value_ty, expr.span);
                }
                self.void_ty()
            }

            ExprKind::Pipe { left, right } => {
                self.infer_expr(left);
                self.infer_expr(right)
            }

            ExprKind::BinOp { op, lhs, rhs } => {
                let lhs_ty = self.infer_expr(lhs);
                let rhs_ty = self.infer_expr(rhs);
                self.binop_result(*op, lhs_ty, rhs_ty, expr.span)
            }

            ExprKind::UnaryOp { op, operand } => {
                let op_ty = self.infer_expr(operand);
                match op {
                    UnaryOp::Not    => self.bool_ty(),
                    UnaryOp::Neg    => op_ty,
                    UnaryOp::BitNot => op_ty,
                    UnaryOp::Await  => self.unwrap_task(op_ty, expr.span),
                }
            }

            ExprKind::Call { callee, args } => {
                let callee_ty = self.infer_expr(callee);
                for arg in args.iter() {
                    match &arg.kind {
                        ArgKind::Positional(e)       => { self.infer_expr(e); }
                        ArgKind::Named { value, .. } => { self.infer_expr(value); }
                    }
                }

                // GAP 1 FIX — `<Collection>.new()` (e.g. `List.new()`) is
                // parsed as Call { callee: Field { target: Ident(ns), field:
                // "new" } }. The generic Field arm below has no field table
                // yet and always returns Unknown, so without this check
                // every builtin constructor call would type as Unknown and
                // silently skip arena coloring. Narrowly scoped to the
                // known constructor namespaces in builtin_constructor_type;
                // general field/method resolution is untouched. See
                // docs/MEMORY_MODEL.md §5.
                if let ExprKind::Field { target, field } = callee.kind {
                    if field == "new" {
                        if let ExprKind::Ident(ns) = target.kind {
                            // §11 FIX (MEMORY_MODEL.md) — `Pool.new()` is
                            // deliberately NOT in `builtin_constructor_type`:
                            // unlike `List.new()` etc. it has no generic
                            // argument of its own to infer fresh — its
                            // element type and capacity come from the
                            // innermost enclosing `with pool<T>(count) { }`
                            // block (the "Unify" design: `pool<T>(count)`
                            // is the low-level allocator, `Pool<T>` is the
                            // ergonomic handle scoped to it). Checked
                            // before the generic path below so a bare
                            // `Pool.new()` outside any pool block gets a
                            // clear, dedicated error instead of silently
                            // DATASTRUCTURES.md §5 — `InlineList.new(capacity)`:
                            // capacity must be a literal integer, checked
                            // directly against the argument's own AST
                            // node — genuine inline storage needs its
                            // size known at compile time, and this is
                            // the narrowest thing that guarantees that
                            // without building real const generics
                            // (confirmed the language has none anywhere:
                            // `parse_generic_params` only ever parses
                            // `Ident (: Bound)?`). Checked before the
                            // generic path below, same reasoning as the
                            // `Pool` special case just below this one.
                            if ns == "InlineList" {
                                let capacity_lit = args.first().and_then(|a| match a.kind {
                                    ArgKind::Positional(e) => match e.kind {
                                        ExprKind::Lit(Literal::Int(n)) if n >= 0 => Some(n as u64),
                                        _ => None,
                                    },
                                    _ => None,
                                });
                                if args.len() != 1 || capacity_lit.is_none() {
                                    self.errors.add_type_error(TypeError::InlineListCapacityNotLiteral {
                                        span: expr.span,
                                    });
                                    return self.unknown();
                                }
                                let elem = self.fresh_var();
                                let inline_list_ty = self.ctx.types.insert(SemaType::InlineList(elem));
                                return self.maybe_arena_ref(inline_list_ty);
                            }
                            // falling through to Unknown.
                            if ns == "Pool" {
                                match self.current_pool() {
                                    Some((pool_id, elem_ty)) => {
                                        if self.current_tier == TierAnnotation::Low {
                                            self.errors.add_tier_error(TierError::CollectionConstructionInLowTier {
                                                collection: "Pool".to_string(),
                                                span:       expr.span,
                                            });
                                        }
                                        let pool_ty = self.ctx.types.insert(SemaType::Pool(elem_ty));
                                        return self.ctx.types.insert(SemaType::PoolRef {
                                            pool:    pool_id,
                                            mutable: false,
                                            inner:   pool_ty,
                                        });
                                    }
                                    None => {
                                        self.errors.add_tier_error(TierError::PoolConstructedOutsideBlock {
                                            span: expr.span,
                                        });
                                        return self.unknown();
                                    }
                                }
                            }
                            // MEMORY_MODEL.md §9 — `Unique.new(value)`/
                            // `Shared.new(value)`/`SyncShared.new(value)`.
                            // Unlike `List.new()` etc. (`builtin_constructor_
                            // type`, fresh type var, resolved later by
                            // unification), these take exactly one REQUIRED
                            // argument and its own inferred type directly
                            // *is* T — no fresh var, no later unification
                            // needed. Tier-gated the opposite way collection
                            // construction is: banned everywhere except
                            // `@tier(low)`, since these three now *are* LOW
                            // tier's memory model (Open Decision #5,
                            // resolved). Checked here (not folded into
                            // `builtin_constructor_type`) for the same
                            // reason `InlineList`/`Pool` are — real logic
                            // beyond the generic fresh-var pattern.
                            if matches!(ns, "Unique" | "Shared" | "SyncShared") {
                                if args.len() != 1 {
                                    self.errors.add_type_error(TypeError::ArgumentCountMismatch {
                                        expected: 1,
                                        found:    args.len(),
                                        span:     expr.span,
                                    });
                                    return self.unknown();
                                }
                                if self.current_tier != TierAnnotation::Low {
                                    self.errors.add_tier_error(TierError::OwnershipWrapperOutsideLowTier {
                                        wrapper: ns.to_string(),
                                        actual:  self.current_tier,
                                        span:    expr.span,
                                    });
                                }
                                let arg_ty = match &args[0].kind {
                                    ArgKind::Positional(e)       => self.infer_expr(e),
                                    ArgKind::Named { value, .. } => self.infer_expr(value),
                                };
                                return self.ctx.types.insert(match ns {
                                    "Unique" => SemaType::Unique(arg_ty),
                                    "Shared" => SemaType::Shared(arg_ty),
                                    _        => SemaType::SyncShared(arg_ty), // exhaustive via the outer matches! guard
                                });
                            }
                            if let Some(ctor_ty) = self.builtin_constructor_type(ns) {
                                // §9 FIX (MEMORY_MODEL.md) — LOW tier has
                                // no memory model of its own yet
                                // (`OwnedRef` + borrow checker are Phase 4,
                                // not started). Without this check, a
                                // `@tier(low)` function calling
                                // `List.new()` etc. would fall straight
                                // through `maybe_arena_ref` below to a
                                // silent, untagged bare type — i.e.
                                // HIGH-tier (GC-default) semantics nobody
                                // has designed for LOW. Emit and continue
                                // (same non-fatal pattern as
                                // ArenaInWrongTier/MethodInWrongTier)
                                // rather than returning Unknown, so one
                                // bad constructor call doesn't cascade
                                // spurious errors through the rest of the
                                // function.
                                if self.current_tier == TierAnnotation::Low {
                                    self.errors.add_tier_error(TierError::CollectionConstructionInLowTier {
                                        collection: ns.to_string(),
                                        span:       expr.span,
                                    });
                                }
                                return self.maybe_arena_ref(ctor_ty);
                            }
                        }
                    }
                }

                // ENUM_RULES.md — tuple-payload variant construction,
                // `Result.Ok(5)`. Separate check from the constructor
                // block above (that one's gated on `field == "new"`;
                // variant names are arbitrary). Struct-payload
                // construction (`Message.Move { x = 1, y = 2 }`) goes
                // through `ExprKind::StructLit` instead — different
                // surface syntax, already routed there by the parser.
                if let ExprKind::Field { target, field } = callee.kind {
                    if let ExprKind::Ident(ns) = target.kind {
                        if let Some(def_id) = self.ctx.top_level_def(ns) {
                            if let Some(shapes) = self.enum_variants.get(&def_id) {
                                let shapes  = shapes.clone();
                                // GENERICS_RULES.md — a fresh instantiation
                                // per call site (not the raw, possibly
                                // `Param`-containing stored type): a bare
                                // constructor call has no already-known
                                // concrete args, so they're inferred from
                                // how the payload argument(s) below unify
                                // against the (substituted) element types.
                                let enum_ty  = self.instantiate(def_id);
                                let inst_args: Vec<TypeId> = match self.ctx.types.get(enum_ty).clone() {
                                    SemaType::Named { args, .. } => args,
                                    _ => Vec::new(),
                                };
                                if let Some((vname, shape)) = shapes.iter().find(|(n, _)| n == field) {
                                    if let VariantShape::Tuple(elem_tys) = shape {
                                        let elem_tys: Vec<TypeId> = elem_tys.iter()
                                            .map(|t| self.substitute(*t, &inst_args))
                                            .collect();
                                        if elem_tys.len() != args.len() {
                                            self.errors.add_type_error(TypeError::VariantArityMismatch {
                                                enum_name:    ns.to_string(),
                                                variant_name: vname.clone(),
                                                expected:     elem_tys.len(),
                                                found:        args.len(),
                                                span:         expr.span,
                                            });
                                        }
                                        for (arg, elem_ty) in args.iter().zip(elem_tys.iter()) {
                                            if let ArgKind::Positional(e) = &arg.kind {
                                                let arg_ty = self.infer_expr(e);
                                                self.unify(*elem_ty, arg_ty, e.span);
                                            }
                                        }
                                        return self.maybe_arena_ref(enum_ty);
                                    }
                                } else {
                                    self.errors.add_type_error(TypeError::UnknownVariant {
                                        enum_name:    ns.to_string(),
                                        variant_name: field.to_string(),
                                        span:         expr.span,
                                    });
                                    return self.unknown();
                                }
                            }
                        }
                    }
                }

                // GENERICS_RULES.md — struct associated/static function
                // calls: `Box.new(42)`, dispatched on the type name (the
                // matched method has no `self` param). Mirrors the enum
                // tuple-variant constructor check just above. Instance
                // calls (`boxed.unwrap()`) are handled separately, after
                // the builtin-instance-method block below, since that one
                // already needs the *receiver's* resolved type in scope
                // rather than a bare type name.
                if let ExprKind::Field { target, field } = callee.kind {
                    if let ExprKind::Ident(ns) = target.kind {
                        if let Some(def_id) = self.ctx.top_level_def(ns) {
                            if let Some(methods) = self.struct_methods.get(&def_id).cloned() {
                                if let Some((_, m)) = methods.iter().find(|(n, m)| n == field && !m.has_self) {
                                    let m = m.clone();
                                    let arity = self.generic_arity.get(&def_id).copied().unwrap_or(0);
                                    let fresh_args: Vec<TypeId> = (0..arity).map(|_| self.fresh_var()).collect();
                                    let params: Vec<TypeId> = m.params.iter()
                                        .map(|p| self.substitute(*p, &fresh_args))
                                        .collect();
                                    let return_type = self.substitute(m.return_type, &fresh_args);
                                    if params.len() != args.len() {
                                        self.errors.add_type_error(TypeError::ArgumentCountMismatch {
                                            expected: params.len(),
                                            found:    args.len(),
                                            span:     expr.span,
                                        });
                                    }
                                    for (arg, param_ty) in args.iter().zip(params.iter()) {
                                        let arg_span = match &arg.kind {
                                            ArgKind::Positional(e)       => e.span,
                                            ArgKind::Named { value, .. } => value.span,
                                        };
                                        if let Some(arg_ty) = self.ctx.expr_type(arg_span).map(|t| self.apply(t)) {
                                            self.unify(*param_ty, arg_ty, arg_span);
                                        }
                                    }
                                    return self.maybe_arena_ref(return_type);
                                }
                            }
                        }
                    }
                }

                // §8 FIX (MEMORY_MODEL.md) — builtin instance methods
                // (`myList.push(x)`, `"s".to_upper()`, ...). The generic
                // Field arm below has no field table (see that arm's own
                // TODO) and always infers Unknown, so without this every
                // builtin method call silently typed as Unknown: no
                // NoSuchMethod check, no arg-count check, no HIGH-only
                // rejection, and — the actual §8 gap — an
                // allocation-producing method's result (e.g. `to_upper`,
                // `split`, `keys`) had no receiver tier to inherit and
                // would've defaulted to un-tagged/GC regardless of whether
                // the receiver was arena- or owned-scoped, quietly
                // reopening Gap 2's escape hole from the result side
                // instead of the reference side.
                //
                // Narrowly scoped to `instance::resolve_receiver`'s six
                // builtin kinds (List/Str/Dict/Tuple/Queue/Stack); anything
                // else (user-defined struct methods, etc) falls through to
                // `call_return_type` below, unchanged. HIGH-only rejection
                // for a method flagged in `instance::is_high_only` lives
                // here too, not in tier_check.rs alongside await/LINQ's
                // otherwise-identical rule — tier_check (Pass 3) has no
                // `Unifier` of its own, and `expr_types` entries can still
                // be raw unresolved `Var`s at record-time, so resolving a
                // receiver's type correctly needs the live `apply()` this
                // pass already has.
                if let ExprKind::Field { target, field } = callee.kind {
                    let receiver_ty = self.ctx.expr_type(target.span)
                        .map(|ty| self.apply(ty));
                    if let Some(receiver_ty) = receiver_ty {
                        if let Some((wrap, kind, bare_ty)) =
                            instance::resolve_receiver(&self.ctx.types, receiver_ty)
                        {
                            let Some((ret, arity)) = instance::signature(kind, field) else {
                                let on_type = self.display_type(receiver_ty);
                                self.errors.add_type_error(TypeError::NoSuchMethod {
                                    method: field.to_string(),
                                    on_type,
                                    span: callee.span,
                                });
                                return self.unknown();
                            };
                            if args.len() != arity {
                                self.errors.add_type_error(TypeError::ArgumentCountMismatch {
                                    expected: arity,
                                    found:    args.len(),
                                    span:     expr.span,
                                });
                            }
                            if instance::is_high_only(kind, field)
                                && self.current_tier != TierAnnotation::High
                            {
                                self.errors.add_tier_error(TierError::MethodInWrongTier {
                                    method: field.to_string(),
                                    actual: self.current_tier,
                                    span:   expr.span,
                                });
                            }

                            // `Linqerizer<T>.select(selector)` and
                            // `.group_by(key_selector)` both have a return
                            // type that depends on the *argument's* own
                            // inferred type (the selector's return type),
                            // which `method_return_type`'s
                            // `(MethodReturn, ReceiverWrap, TypeId)` shape
                            // has no way to see — every other method here
                            // has a return shape that's a fixed function of
                            // the *receiver* alone. Intercept before the
                            // generic path runs, same relationship
                            // `Pool.new()`/`InlineList.new()`'s own
                            // constructor special cases already have to the
                            // generic `builtin_constructor_type` path.
                            if kind == instance::ReceiverKind::Linqerizer
                                && (field == "select" || field == "group_by")
                            {
                                let arg_span = args.first().map(|a| match &a.kind {
                                    ArgKind::Positional(e)       => e.span,
                                    ArgKind::Named { value, .. } => value.span,
                                });
                                let selector_ret = arg_span
                                    .and_then(|span| self.ctx.expr_type(span))
                                    .map(|ty| self.apply(ty))
                                    .and_then(|ty| match self.ctx.types.get(ty) {
                                        SemaType::Function { return_type, .. } => Some(*return_type),
                                        _ => None,
                                    })
                                    .unwrap_or_else(|| self.fresh_var());

                                if field == "select" {
                                    // Linqerizer<T> -> Linqerizer<U>, U = selector's return type.
                                    let linq = self.ctx.types.insert(SemaType::Linqerizer(selector_ret));
                                    return self.apply_receiver_wrap(wrap, linq);
                                } else {
                                    // group_by: Linqerizer<T> -> Dictionary<U, List<T>>,
                                    // U = key_selector's return type, T = this
                                    // Linqerizer's own (unchanged) element type.
                                    let elem = match self.ctx.types.get(bare_ty) {
                                        SemaType::Linqerizer(e) => *e,
                                        _ => self.fresh_var(),
                                    };
                                    let value_list = self.ctx.types.insert(SemaType::List(elem));
                                    let dict = self.ctx.types.insert(SemaType::Dictionary(selector_ret, value_list));
                                    return self.apply_receiver_wrap(wrap, dict);
                                }
                            }

                            return self.method_return_type(ret, wrap, bare_ty);
                        }

                        // GENERICS_RULES.md — user struct instance
                        // methods: `boxed.unwrap()`. `receiver_ty`'s own
                        // `args` are already the concrete instantiation
                        // (inferred back when `boxed` was constructed),
                        // so — unlike the associated-call case above —
                        // no fresh `Var`s are needed here, just substitute
                        // the receiver's own args straight in.
                        if let SemaType::Named { def, args: recv_args } =
                            self.ctx.types.get(receiver_ty).clone()
                        {
                            if let Some(methods) = self.struct_methods.get(&def).cloned() {
                                return match methods.iter().find(|(n, m)| n == field && m.has_self) {
                                    Some((_, m)) => {
                                        let m = m.clone();
                                        let params: Vec<TypeId> = m.params.iter()
                                            .map(|p| self.substitute(*p, &recv_args))
                                            .collect();
                                        let return_type = self.substitute(m.return_type, &recv_args);
                                        if params.len() != args.len() {
                                            self.errors.add_type_error(TypeError::ArgumentCountMismatch {
                                                expected: params.len(),
                                                found:    args.len(),
                                                span:     expr.span,
                                            });
                                        }
                                        for (arg, param_ty) in args.iter().zip(params.iter()) {
                                            let arg_span = match &arg.kind {
                                                ArgKind::Positional(e)       => e.span,
                                                ArgKind::Named { value, .. } => value.span,
                                            };
                                            if let Some(arg_ty) = self.ctx.expr_type(arg_span).map(|t| self.apply(t)) {
                                                self.unify(*param_ty, arg_ty, arg_span);
                                            }
                                        }
                                        self.maybe_arena_ref(return_type)
                                    }
                                    None => {
                                        // `.clone()` isn't a real entry in
                                        // `struct_methods` (nothing in
                                        // `StructDecl::members` produced
                                        // one). It's a derive-gated
                                        // pseudo-method, recognized here
                                        // directly instead. Mirrors the
                                        // interpreter's own dispatch order
                                        // (`eval_method_call`,
                                        // `interpreter/eval/expr.rs`):
                                        // a real user-defined method
                                        // named `clone` would already have
                                        // been found above and returned,
                                        // so this only ever fires for the
                                        // derived case.
                                        if field == "clone"
                                            && self.struct_derives.get(&def)
                                                .is_some_and(|d| d.contains("Clone"))
                                        {
                                            if !args.is_empty() {
                                                self.errors.add_type_error(TypeError::ArgumentCountMismatch {
                                                    expected: 0,
                                                    found:    args.len(),
                                                    span:     expr.span,
                                                });
                                            }
                                            return self.maybe_arena_ref(receiver_ty);
                                        }
                                        let on_type = self.display_type(receiver_ty);
                                        self.errors.add_type_error(TypeError::NoSuchMethod {
                                            method: field.to_string(),
                                            on_type,
                                            span:   callee.span,
                                        });
                                        self.unknown()
                                    }
                                };
                            }
                        }
                    }
                }

                self.call_return_type(callee_ty, args, expr.span)
            }

            ExprKind::Field { target, field } => {
                // `EnumName.Variant` (fieldless/discriminant only — a
                // payload-carrying variant referenced bare with no call
                // isn't a valid value on its own; falls through to the
                // generic Unknown below same as any other unhandled
                // shape). Mirrors `Pool.new()`'s constructor special-
                // case: recognized syntactically here rather than via
                // the general field table, which doesn't exist yet.
                if let ExprKind::Ident(name) = &target.kind {
                    if let Some(def_id) = self.ctx.top_level_def(name) {
                        if let Some(shapes) = self.enum_variants.get(&def_id) {
                            let shapes = shapes.clone();
                            if let Some((_, shape)) = shapes.iter().find(|(n, _)| n == field) {
                                if matches!(shape, VariantShape::None | VariantShape::Discriminant) {
                                    // GENERICS_RULES.md — fresh per-use
                                    // instantiation, same reasoning as the
                                    // tuple-variant constructor above:
                                    // `Option.None` has no argument to
                                    // infer T from here, so it starts as
                                    // an unresolved Var and picks up a
                                    // concrete type from wherever the
                                    // result is used (an annotation, a
                                    // return type, a later unify).
                                    let enum_ty = self.instantiate(def_id);
                                    return self.maybe_arena_ref(enum_ty);
                                }
                            } else {
                                self.errors.add_type_error(TypeError::UnknownVariant {
                                    enum_name:    name.to_string(),
                                    variant_name: field.to_string(),
                                    span:         expr.span,
                                });
                                return self.unknown();
                            }
                        }
                    }
                }

                // GENERICS_RULES.md / struct field access — `foo.field`
                // on a plain struct instance. `Named { def, args }`'s
                // `args` are already the receiver's *own* concrete
                // instantiation (inferred back when the receiver itself
                // was constructed), so no fresh-var allocation is needed
                // here — just substitute into the field's raw stored type.
                let receiver_ty = self.infer_expr(target);
                let receiver_ty = self.apply(receiver_ty);
                if let SemaType::Named { def, args } = self.ctx.types.get(receiver_ty).clone() {
                    if let Some(fields) = self.struct_fields.get(&def).cloned() {
                        if let Some((_, raw_ty)) = fields.iter().find(|(n, _)| n == field) {
                            return self.substitute(*raw_ty, &args);
                        }
                        // Not a field — this `ExprKind::Field` may be
                        // getting pre-inferred as the *callee* of an
                        // enclosing `Call` (`Box.new(..)`, `boxed.unwrap()`
                        // — the Call arm unconditionally infers its
                        // callee before running its own static/instance
                        // method dispatch, same as it already did for
                        // enum tuple-variant construction pre-dating this
                        // change). A real method name here is expected
                        // and not an error; only a name that's neither a
                        // field nor a method is genuinely unknown.
                        let is_method = self.struct_methods.get(&def)
                            .map(|ms| ms.iter().any(|(n, _)| n == field))
                            .unwrap_or(false)
                            // `.clone()` is a derive-gated pseudo-method,
                            // not a real `struct_methods` entry. This
                            // `Field` node is only ever the pre-inferred
                            // *callee* of an enclosing `Call` when it's
                            // "clone" (there's no real field of that
                            // name either), so this only needs to stop
                            // `NoSuchField` from firing here; the actual
                            // return-type computation happens in the
                            // `Call` arm's own instance-method dispatch
                            // below, which already knows about `Clone`.
                            || (*field == "clone" && self.struct_derives.get(&def)
                                .is_some_and(|d| d.contains("Clone")));
                        if !is_method {
                            let on_type = self.display_type(receiver_ty);
                            self.errors.add_type_error(TypeError::NoSuchField {
                                field: field.to_string(),
                                on_type,
                                span:  expr.span,
                            });
                        }
                        return self.unknown();
                    }
                }
                self.unknown() // not a known struct/enum receiver
            }
            ExprKind::OptionalChain { target, .. } => {
                self.infer_expr(target);
                self.unknown() // TODO: field table
            }

            ExprKind::Index { target, index } => {
                let coll = self.infer_expr(target);
                self.infer_expr(index);
                self.element_type_of(coll)
            }

            ExprKind::Try(inner)   => {
                let inner_ty = self.infer_expr(inner);
                self.unwrap_fallible(inner_ty)
            }
            ExprKind::Borrow { mutable, place } => {
                let place_ty = self.infer_expr(place);
                self.ctx.types.insert(SemaType::Reference {
                    mutable:  *mutable,
                    lifetime: None, // no surface label at an expression site
                    inner:    place_ty,
                })
            }
            ExprKind::Deref(inner) => {
                let inner_ty = self.infer_expr(inner);
                self.unwrap_reference(inner_ty, expr.span)
            }
            ExprKind::Await(inner) => {
                let inner_ty = self.infer_expr(inner);
                self.unwrap_task(inner_ty, expr.span)
            }
            ExprKind::As { expr: inner, ty } => {
                self.infer_expr(inner);
                self.ast_type_to_sema(ty)
            }

            ExprKind::Array(elems) => {
                let elem_ty = if elems.is_empty() {
                    self.fresh_var()
                } else {
                    let first = self.infer_expr(elems[0]);
                    for e in &elems[1..] {
                        let ty = self.infer_expr(e);
                        self.unify(first, ty, e.span);
                    }
                    first
                };
                let list_ty = self.ctx.types.intern(SemaType::List(elem_ty));
                self.maybe_arena_ref(list_ty)
            }

            ExprKind::Tuple(elems) => {
                let ids: Vec<TypeId> = elems.iter().map(|e| self.infer_expr(e)).collect();
                let tuple_ty = self.ctx.types.insert(SemaType::Tuple(ids));
                self.maybe_arena_ref(tuple_ty)
            }

            ExprKind::Dict(entries) => {
                let (k0, v0) = if entries.is_empty() {
                    (self.fresh_var(), self.fresh_var())
                } else {
                    (self.infer_expr(entries[0].key), self.infer_expr(entries[0].value))
                };
                for entry in entries.iter().skip(1) {
                    let kt = self.infer_expr(entry.key);
                    let vt = self.infer_expr(entry.value);
                    self.unify(k0, kt, entry.span);
                    self.unify(v0, vt, entry.span);
                }
                let dict_ty = self.ctx.types.insert(SemaType::Dictionary(k0, v0));
                self.maybe_arena_ref(dict_ty)
            }

            ExprKind::AnonObject(fields) => {
                for f in fields.iter() { self.infer_expr(f.value); }
                self.unknown() // no structural record type yet
            }

            ExprKind::StructLit { path, fields } => {
                // ENUM_RULES.md — `Message.Move { x = 1, y = 2 }`, a
                // struct-payload variant construction, arrives here as a
                // 2-segment path (parser routes it the same as a plain
                // struct literal; the only signal it's an enum variant
                // is the extra path segment). Plain struct literals
                // (`Holder { data = ... }`, 1-segment) keep their
                // existing, unvalidated behavior below — general struct-
                // field checking isn't part of this section's scope.
                if path.len() >= 2 {
                    if let Some(def_id) = self.ctx.top_level_def(path[0]) {
                        if let Some(shapes) = self.enum_variants.get(&def_id) {
                            let shapes    = shapes.clone();
                            // GENERICS_RULES.md — same fresh-per-call-site
                            // reasoning as the tuple-variant constructor:
                            // the struct-payload fields' values below are
                            // what T gets inferred from via unification.
                            let enum_ty   = self.instantiate(def_id);
                            let inst_args: Vec<TypeId> = match self.ctx.types.get(enum_ty).clone() {
                                SemaType::Named { args, .. } => args,
                                _ => Vec::new(),
                            };
                            let variant   = path[path.len() - 1];
                            match shapes.iter().find(|(n, _)| n == variant) {
                                Some((vname, VariantShape::Struct(field_tys))) => {
                                    let vname     = vname.clone();
                                    let field_tys: Vec<(String, TypeId)> = field_tys.iter()
                                        .map(|(n, t)| (n.clone(), self.substitute(*t, &inst_args)))
                                        .collect();
                                    for f in fields.iter() {
                                        match field_tys.iter().find(|(n, _)| n == f.name) {
                                            Some((_, t)) => {
                                                let val_ty = self.infer_expr(f.value);
                                                self.unify(*t, val_ty, f.span);
                                            }
                                            None => {
                                                self.infer_expr(f.value);
                                                self.errors.add_type_error(TypeError::UnknownVariant {
                                                    enum_name:    path[0].to_string(),
                                                    variant_name: format!("{}.{{{}}}", vname, f.name),
                                                    span:         f.span,
                                                });
                                            }
                                        }
                                    }
                                    return self.maybe_arena_ref(enum_ty);
                                }
                                Some(_) => {
                                    // Real variant, wrong payload shape
                                    // (fieldless/tuple constructed with
                                    // `{ }` syntax) — walk fields once so
                                    // their own errors surface, but don't
                                    // fall through to the generic path
                                    // below (which would infer them
                                    // again and double-report).
                                    for f in fields.iter() { self.infer_expr(f.value); }
                                    return self.maybe_arena_ref(enum_ty);
                                }
                                None => {
                                    for f in fields.iter() { self.infer_expr(f.value); }
                                    self.errors.add_type_error(TypeError::UnknownVariant {
                                        enum_name:    path[0].to_string(),
                                        variant_name: variant.to_string(),
                                        span:         expr.span,
                                    });
                                    return self.unknown();
                                }
                            }
                        }
                    }
                }
                // GENERICS_RULES.md / plain struct literal field
                // validation — `Rectangle { width = w, height = h }`.
                // Fresh per-call-site instantiation, same as a struct's
                // `TypeName.method(..)` associated-function call: each
                // provided field's value is what the struct's own generic
                // args (if any) get inferred from.
                let root = path.first().copied().unwrap_or("");
                if let Some(def_id) = self.ctx.top_level_def(root) {
                    if let Some(decl_fields) = self.struct_fields.get(&def_id).cloned() {
                        let struct_ty  = self.instantiate(def_id);
                        let inst_args: Vec<TypeId> = match self.ctx.types.get(struct_ty).clone() {
                            SemaType::Named { args, .. } => args,
                            _ => Vec::new(),
                        };
                        for f in fields.iter() {
                            let val_ty = self.infer_expr(f.value);
                            match decl_fields.iter().find(|(n, _)| n == f.name) {
                                Some((_, raw_ty)) => {
                                    let field_ty = self.substitute(*raw_ty, &inst_args);
                                    self.unify(field_ty, val_ty, f.span);
                                }
                                None => {
                                    self.errors.add_type_error(TypeError::NoSuchField {
                                        field:   f.name.to_string(),
                                        on_type: root.to_string(),
                                        span:    f.span,
                                    });
                                }
                            }
                        }
                        return self.maybe_arena_ref(struct_ty);
                    }
                }
                for f in fields.iter() { self.infer_expr(f.value); }
                let struct_ty = self.ctx.top_level_def(root)
                    .and_then(|id| self.ctx.def_type(id))
                    .unwrap_or_else(|| self.unknown());
                self.maybe_arena_ref(struct_ty)
            }

            ExprKind::Lambda(lambda) => {
                let param_tys: Vec<TypeId> = lambda.params.iter().map(|p| {
                    let ty = p.ty.map(|t| self.ast_type_to_sema(t))
                        .unwrap_or_else(|| self.fresh_var());
                    if let Some(def_id) = self.ctx.resolutions.get(p.span) {
                        self.ctx.set_def_type(def_id, ty);
                        self.ctx.set_binding_type(p.span, ty);
                    }
                    ty
                }).collect();
                let ret_var       = self.fresh_var();
                let prev_ret      = self.current_return.replace(ret_var);
                let prev_fallible = self.current_fallible;
                self.current_fallible = false;
                let body_ty = match &lambda.body {
                    LambdaBody::Block(b) => self.infer_block(b),
                    LambdaBody::Expr(e)  => self.infer_expr(e),
                };
                self.unify(ret_var, body_ty, lambda.span);
                self.current_return   = prev_ret;
                self.current_fallible = prev_fallible;
                let ret_resolved = self.apply(ret_var);
                let fn_ty = self.ctx.types.insert(SemaType::Function {
                    params:      param_tys,
                    return_type: ret_resolved,
                    is_fallible: false,
                    generic_arity: 0, // lambdas are never generic
                });
                // GAP 2 — closure capture. A lambda built inside a
                // `with arena(…)` block may capture arena-scoped locals
                // from the enclosing scope; lexical scoping guarantees
                // any such capture can only happen from inside that same
                // block. Tag every lambda constructed here the same way
                // array/tuple/dict/struct literals already are, so that
                // if *this lambda value* is later assigned outward,
                // returned, or stored in a struct field, the existing
                // escape checks catch it — no separate free-variable
                // capture analysis required. See module doc comment.
                self.maybe_arena_ref(fn_ty)
            }

            ExprKind::Block(b) => self.infer_block(b),

            ExprKind::If(if_node) => {
                let cond    = self.infer_expr(if_node.condition);
                let bool_ty = self.bool_ty();
                self.unify(bool_ty, cond, if_node.condition.span);
                let then_ty = self.infer_if_branch(&if_node.then_body);
                for elif in if_node.elif_branches {
                    let ct = self.infer_expr(elif.condition);
                    self.unify(bool_ty, ct, elif.condition.span);
                    self.infer_if_branch(&elif.body);
                }
                if let Some(else_b) = &if_node.else_body {
                    let else_ty = self.infer_if_branch(else_b);
                    let void_ty = self.void_ty();
                    if then_ty != void_ty { self.unify(then_ty, else_ty, expr.span); }
                    then_ty
                } else {
                    self.void_ty()
                }
            }

            ExprKind::Match(m) => {
                let scrutinee_ty = self.infer_expr(m.scrutinee);
                let void_ty = self.void_ty();
                let mut result = void_ty;
                let mut coverage = Vec::with_capacity(m.arms.len());
                for arm in m.arms.iter() {
                    let cov = self.check_pattern(&arm.pattern, scrutinee_ty);
                    if let Some(g) = arm.guard { self.infer_expr(g); }
                    let arm_ty = match &arm.body {
                        MatchArmBody::Expr(e)  => self.infer_expr(e),
                        MatchArmBody::Block(b) => self.infer_block(b),
                    };
                    if arm_ty != void_ty { result = arm_ty; }
                    coverage.push((cov, arm.guard.is_some()));
                }
                self.check_match_exhaustiveness(scrutinee_ty, &coverage, m.span);
                result
            }

            ExprKind::OrElse { expr: inner, fallback } => {
                let inner_ty = self.infer_expr(inner);
                if let OrElseFallback::Expr(fb) = fallback {
                    let fb_ty = self.infer_expr(fb);
                    self.unify(inner_ty, fb_ty, expr.span);
                }
                self.unwrap_optional(inner_ty)
            }
        }
    }

    // ── Literal typing ────────────────────────────────────────────

    fn infer_literal<'ast>(&mut self, lit: &Literal<'ast>) -> TypeId {
        match lit {
            Literal::Int(_)   => self.ctx.types.intern(SemaType::Int),
            Literal::Float(_) => self.ctx.types.intern(SemaType::Float),
            Literal::Double(_)=> self.ctx.types.intern(SemaType::Double),
            Literal::Bool(_)  => self.ctx.types.intern(SemaType::Bool),
            Literal::Char(_)  => self.ctx.types.intern(SemaType::Char),
            Literal::Null     => self.ctx.types.intern(SemaType::Null),
            Literal::Str(_) | Literal::VerbatimStr(_) => self.ctx.types.intern(SemaType::Str),
            // Real, pre-existing gap found while wiring format specs
            // through this exact literal: interpolation holes were
            // PARSED (parse_interp emits real parse errors for a broken
            // hole) but never actually TYPE-CHECKED — `$"{undefined_
            // name}"` sailed through sema undetected, only failing (or
            // not) at interpret time. Walking each hole's expression
            // here fixes that as a side effect of needing each hole's
            // type anyway, to validate its format spec against it.
            Literal::InterpolatedStr(parts)
            | Literal::InterpolatedVerbatimStr(parts) => {
                for part in parts.iter() {
                    if let InterpolationPart::Expr { expr, spec } = part {
                        let hole_ty = self.infer_expr(expr);
                        if let Some(spec) = spec {
                            self.check_format_spec(spec, hole_ty, expr.span);
                        }
                    }
                }
                self.ctx.types.intern(SemaType::Str)
            }
        }
    }

    /// `TYPE-116`/`TYPE-117`: `@derive(...)` recognizes six trait names
    /// today: `PartialEq`, `Eq`, `Hash`, `Ord`, `PartialOrd`, `Clone`,
    /// all struct-only for now (`on_enum`: an `enum`'s `==` is already
    /// structural, `Value::equals`'s Enum arm, so `@derive(PartialEq)`
    /// there is exactly as redundant as `@derive(Debug)` is everywhere;
    /// the other five simply aren't implemented for enum declarations
    /// yet, no derived ordering/hash/clone exists for an enum at all,
    /// so a request for one is treated the same as any other name this
    /// function doesn't recognize *in this context*, same as
    /// `UnknownDeriveTrait`'s own doc comment already frames the
    /// variant: "recognized, supported... in this context").
    ///
    /// Two passes: the first accepts or rejects each individual name
    /// (unrecognized, `Debug`/`Display`, `PartialEq`-on-enum, or a
    /// struct-only name used on an enum all report `UnknownDeriveTrait`
    /// here) and separately records which of the six were accepted; the
    /// second checks prerequisite chains against that full set,
    /// regardless of what order the names were written in (`@derive(Eq,
    /// PartialEq)` and `@derive(PartialEq, Eq)` are both fine): `Eq`
    /// needs `PartialEq`, `PartialOrd` needs `PartialEq`, `Ord` needs
    /// both `PartialOrd` and `Eq` (checked directly, not just
    /// transitively through `PartialOrd`, so `@derive(Ord)` alone
    /// reports both gaps rather than only the first one found), `Hash`
    /// needs `Eq` (not a real Rust supertrait bound for `Hash`, but the
    /// bound every actual hash-map API uses, and this project's whole
    /// reason for wanting `Hash`, a future `Dict` key. `Clone` has no
    /// prerequisite.
    ///
    /// Walks `attribute.args` directly rather than going through
    /// `ast::common::derive_trait_names` specifically so a non-`Ident`
    /// arg (`@derive("PartialEq")`, `@derive(x = 1)`) gets reported
    /// instead of silently vanishing. That helper exists for a
    /// different consumer (the interpreter, post-validation) that
    /// deliberately doesn't want to see those.
    fn check_derive_attrs(&mut self, attributes: &[Attribute], on_enum: bool) {
        const RECOGNIZED: &[&str] = &["PartialEq", "Eq", "Hash", "Ord", "PartialOrd", "Clone"];
        let mut present: HashSet<&str> = HashSet::new();
        let mut derive_span: Option<Span> = None;

        for attr in attributes.iter().filter(|a| a.name == "derive") {
            derive_span.get_or_insert(attr.span);
            for arg in attr.args.iter() {
                let bad_name = match arg {
                    AttrArg::Ident(name) if RECOGNIZED.contains(name) && !on_enum => {
                        present.insert(name);
                        None
                    }
                    AttrArg::Ident(name)         => Some(name.to_string()),
                    AttrArg::Str(s)              => Some(format!("\"{}\"", s)),
                    AttrArg::Int(n)              => Some(n.to_string()),
                    AttrArg::Bool(b)             => Some(b.to_string()),
                    AttrArg::Named { key, .. }   => Some(format!("{} = ...", key)),
                    AttrArg::Nested { name, .. } => Some(format!("{}(...)", name)),
                };
                if let Some(trait_name) = bad_name {
                    self.errors.add_type_error(TypeError::UnknownDeriveTrait {
                        trait_name,
                        span: attr.span,
                    });
                }
            }
        }

        let Some(span) = derive_span else { return; };
        let mut requires = |this: &mut Self, name: &str, needs: &str| {
            if present.contains(name) && !present.contains(needs) {
                this.errors.add_type_error(TypeError::DeriveRequiresOther {
                    trait_name: name.to_string(),
                    requires:   needs.to_string(),
                    span,
                });
            }
        };
        requires(self, "Eq", "PartialEq");
        requires(self, "PartialOrd", "PartialEq");
        requires(self, "Ord", "PartialOrd");
        requires(self, "Ord", "Eq");
        requires(self, "Hash", "Eq");
    }

    /// `.precision` only means something for Float/Double (decimal
    /// places) or Str (max length, truncating); `+`/zero-pad only mean
    /// something for Int/Float/Double; a numeric base only means
    /// something for Int; `#` only means something alongside a base.
    /// Anything else is a real error (`TYPE-115`/`TYPE-119`), not a
    /// silent no-op. Width/align/fill/`?` apply to any type (padding/
    /// truncation work on the rendered string regardless), so nothing
    /// to check for those.
    fn check_format_spec(&mut self, spec: &FormatSpec, hole_ty: TypeId, span: Span) {
        let resolved = self.ctx.types.get(hole_ty);
        let is_numeric = matches!(resolved, SemaType::Int | SemaType::Float | SemaType::Double);
        let on_type = || resolved.display(&self.ctx.types, &self.ctx.symbols);

        if spec.precision.is_some() {
            let ok = matches!(resolved, SemaType::Float | SemaType::Double | SemaType::Str);
            if !ok {
                self.errors.add_type_error(TypeError::InvalidFormatSpec {
                    spec_part: "precision".to_string(), on_type: on_type(), span,
                });
            }
        }
        if spec.sign_plus && !is_numeric {
            self.errors.add_type_error(TypeError::InvalidFormatSpec {
                spec_part: "sign (+)".to_string(), on_type: on_type(), span,
            });
        }
        if spec.zero_pad && !is_numeric {
            self.errors.add_type_error(TypeError::InvalidFormatSpec {
                spec_part: "zero-pad (0)".to_string(), on_type: on_type(), span,
            });
        }
        if spec.base.is_some() && !matches!(resolved, SemaType::Int) {
            self.errors.add_type_error(TypeError::InvalidFormatSpec {
                spec_part: "numeric base".to_string(), on_type: on_type(), span,
            });
        }
        if spec.alternate && spec.base.is_none() {
            self.errors.add_type_error(TypeError::AlternateFormatWithoutBase { span });
        }
    }

    // ── BinOp result type ─────────────────────────────────────────

    fn binop_result(&mut self, op: BinOp, lhs: TypeId, rhs: TypeId, span: Span) -> TypeId {
        match op {
            BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Rem
            | BinOp::BitAnd | BinOp::BitOr | BinOp::BitXor | BinOp::Shl | BinOp::Shr => {
                self.unify(lhs, rhs, span);
                self.apply(lhs)
            }
            BinOp::Eq | BinOp::Ne => {
                self.unify(lhs, rhs, span);
                self.bool_ty()
            }
            // TYPE-118: previously nothing checked here at all, this
            // arm used to cover Eq/Ne/Lt/Le/Gt/Ge together, unifying the
            // operand types and calling it done, so `"a" < "b"` or two
            // structs compared with `<` sailed through sema only to hit
            // a runtime panic in `eval_binop`'s `promote_numeric`
            // ("arithmetic not supported on {type}"). Orderable now:
            // Int/Float/Double (the pre-existing numeric behavior,
            // unchanged), Str (newly real), and a struct that's
            // `@derive`d `PartialOrd`/`Ord` (newly real). Everything
            // else (`Bool` included, which used to reach the same
            // runtime panic Str did) now fails here instead, with a
            // real message pointing at the missing derive instead of an
            // opaque runtime crash.
            BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
                self.unify(lhs, rhs, span);
                let resolved = self.apply(lhs);
                let orderable = match self.ctx.types.get(resolved) {
                    SemaType::Int | SemaType::Float | SemaType::Double | SemaType::Str => true,
                    SemaType::Named { def, .. } => self.struct_derives.get(def)
                        .is_some_and(|d| d.contains("PartialOrd") || d.contains("Ord")),
                    // Genuinely unresolved (e.g. a Linqerizer lambda
                    // parameter's type, still pending unification with
                    // the pipeline's element type at the point this
                    // particular comparison gets visited). "Don't know
                    // yet" isn't "known to be wrong". Not flagging this
                    // matches every other type check in this file: none
                    // of them treat `SemaType::Unknown` as a positive
                    // finding of its own.
                    SemaType::Unknown => true,
                    _ => false,
                };
                if !orderable {
                    let on_type = self.display_type(resolved);
                    self.errors.add_type_error(TypeError::TypeNotOrderable { on_type, span });
                }
                self.bool_ty()
            }
            BinOp::And | BinOp::Or => {
                let bool_ty = self.bool_ty();
                self.unify(bool_ty, lhs, span);
                self.unify(bool_ty, rhs, span);
                bool_ty
            }
            BinOp::Range | BinOp::RangeIncl => self.unknown(),
        }
    }

    // ── Type unwrapping helpers ───────────────────────────────────

    fn unwrap_task(&mut self, ty: TypeId, span: Span) -> TypeId {
        let resolved = self.apply(ty);
        match self.ctx.types.get(resolved) {
            SemaType::Task(inner) => *inner,
            SemaType::Var(_) => {
                let inner = self.fresh_var();
                let task  = self.ctx.types.intern(SemaType::Task(inner));
                self.unify(resolved, task, span);
                inner
            }
            _ => {
                self.errors.add_type_error(TypeError::AwaitOnNonTask {
                    found: self.display_type(ty),
                    span,
                });
                TypeId::ERROR
            }
        }
    }

    fn unwrap_reference(&mut self, ty: TypeId, span: Span) -> TypeId {
        let resolved = self.apply(ty);
        match self.ctx.types.get(resolved) {
            SemaType::Reference { inner, .. } => *inner,
            SemaType::Var(_) => {
                let inner = self.fresh_var();
                let reference = self.ctx.types.intern(SemaType::Reference {
                    mutable: false, lifetime: None, inner,
                });
                self.unify(resolved, reference, span);
                inner
            }
            _ => {
                self.errors.add_type_error(TypeError::DerefOnNonReference {
                    found: self.display_type(ty),
                    span,
                });
                TypeId::ERROR
            }
        }
    }

    fn unwrap_fallible(&mut self, ty: TypeId) -> TypeId {
        let resolved = self.apply(ty);
        match self.ctx.types.get(resolved) {
            SemaType::Fallible(inner) => *inner,
            _ => resolved,
        }
    }

    fn unwrap_optional(&mut self, ty: TypeId) -> TypeId {
        let resolved = self.apply(ty);
        match self.ctx.types.get(resolved) {
            SemaType::Optional(inner) => *inner,
            _ => resolved,
        }
    }

    /// GENERICS_RULES.md — was previously "just return the declared
    /// return type," with `args` inferred (each individually, for their
    /// own diagnostics) but never unified against the fn's own param
    /// types at all — not for generic functions, not for *any* plain
    /// function call. `identity(5)` and `identity(5, 5, 5)` type-checked
    /// identically. Fixed uniformly rather than special-cased to only
    /// generic callees: a non-generic fn's `generic_arity` is 0, so the
    /// fresh-Var/substitute step below is a no-op for it and this is
    /// just real argument checking that never existed, generic or not.
    fn call_return_type<'ast>(&mut self, callee_ty: TypeId, args: &[Arg<'ast>], span: Span) -> TypeId {
        let resolved = self.apply(callee_ty);
        match self.ctx.types.get(resolved).clone() {
            SemaType::Function { params, return_type, generic_arity, .. } => {
                let fresh_args: Vec<TypeId> = (0..generic_arity).map(|_| self.fresh_var()).collect();
                let params: Vec<TypeId> = params.iter()
                    .map(|p| self.substitute(*p, &fresh_args))
                    .collect();
                let return_type = self.substitute(return_type, &fresh_args);

                if params.len() != args.len() {
                    self.errors.add_type_error(TypeError::ArgumentCountMismatch {
                        expected: params.len(),
                        found:    args.len(),
                        span,
                    });
                }
                for (arg, param_ty) in args.iter().zip(params.iter()) {
                    let arg_span = match &arg.kind {
                        ArgKind::Positional(e)       => e.span,
                        ArgKind::Named { value, .. } => value.span,
                    };
                    if let Some(arg_ty) = self.ctx.expr_type(arg_span).map(|t| self.apply(t)) {
                        self.unify(*param_ty, arg_ty, arg_span);
                    }
                }
                return_type
            }
            SemaType::Var(_)   => self.fresh_var(),
            SemaType::Unknown  => self.unknown(),
            _ => {
                self.errors.add_type_error(TypeError::NoSuchMethod {
                    method:  "<call>".into(),
                    on_type: self.display_type(callee_ty),
                    span,
                });
                TypeId::ERROR
            }
        }
    }

    // ── Unification ───────────────────────────────────────────────

    fn unify(&mut self, a: TypeId, b: TypeId, span: Span) {
        let a = self.apply(a);
        let b = self.apply(b);
        if a == b { return; }

        // FIX: use reference patterns so we don't try to move out of &SemaType.
        // u32 is Copy so binding through &SemaType::Var(v) gives v: u32.
        let a_is_var = matches!(self.ctx.types.get(a), SemaType::Var(_));
        let b_is_var = matches!(self.ctx.types.get(b), SemaType::Var(_));

        if a_is_var {
            if let &SemaType::Var(v) = self.ctx.types.get(a) {
                self.unifier.bind(v, b);
                return;
            }
        }
        if b_is_var {
            if let &SemaType::Var(v) = self.ctx.types.get(b) {
                self.unifier.bind(v, a);
                return;
            }
        }

        // Unknown / ERROR absorb without cascading.
        let a_unk = matches!(self.ctx.types.get(a), SemaType::Unknown);
        let b_unk = matches!(self.ctx.types.get(b), SemaType::Unknown);
        if a_unk || b_unk { return; }
        if a == TypeId::ERROR || b == TypeId::ERROR { return; }

        if self.structurally_compatible(a, b) { return; }

        // GAP 2 — an ArenaRef/PoolRef meeting an incompatible context (a
        // different scope, or a context with no scope at all — e.g. a
        // MID-tier function's declared return type, which can never
        // itself express either tag) is specifically a scope escape, not
        // a generic shape mismatch. Prefer the dedicated diagnostic
        // whenever either side carries a tag.
        if let Some((escaped, is_pool)) = self.scope_mismatch_side(a, b) {
            if is_pool { self.pool_escape(escaped, span); } else { self.arena_escape(escaped, span); }
            return;
        }

        self.errors.add_type_error(TypeError::TypeMismatch {
            expected:   self.display_type(a),
            found:      self.display_type(b),
            span,
            because_of: None,
        });
    }

    /// GAP 2 — if either resolved type is `SemaType::ArenaRef` or
    /// `SemaType::PoolRef`, return that side's `TypeId` plus which kind
    /// it was (preferring `b`, conventionally the "actual" / "found"
    /// side in an `expected, found` unify call, which reads better in
    /// the "Value of type `X` ... cannot cross" message). Returns `None`
    /// when neither side carries either tag, i.e. this truly is an
    /// unrelated shape mismatch. Checked pool-first for the same reason
    /// as `check_assign_arena_escape` — a value only ever carries one.
    fn scope_mismatch_side(&mut self, a: TypeId, b: TypeId) -> Option<(TypeId, bool)> {
        let a_is_pool = matches!(self.ctx.types.get(a), SemaType::PoolRef { .. });
        let b_is_pool = matches!(self.ctx.types.get(b), SemaType::PoolRef { .. });
        if b_is_pool { return Some((b, true)); }
        if a_is_pool { return Some((a, true)); }

        let a_is_arena = matches!(self.ctx.types.get(a), SemaType::ArenaRef { .. });
        let b_is_arena = matches!(self.ctx.types.get(b), SemaType::ArenaRef { .. });
        if b_is_arena { Some((b, false)) } else if a_is_arena { Some((a, false)) } else { None }
    }

    /// Returns true if two concrete types are structurally compatible,
    /// recursively unifying their type arguments. Extracts inner TypeIds
    /// (which are Copy) before calling unify to avoid borrow conflicts.
    fn structurally_compatible(&mut self, a: TypeId, b: TypeId) -> bool {
        // GENERICS_RULES.md — `fn(..)` type compatibility, e.g. checking
        // a lambda/closure argument's inferred type against a declared
        // `fn(int) int`-shaped param. Multi-pair (each param position,
        // plus the return type), so it can't reuse the single-inner-pair
        // pattern the rest of this function is built around — handled
        // separately, up front. Never exercised before this file's own
        // `call_return_type` started actually unifying call arguments
        // against declared param types at all (previously: computed and
        // discarded) — a plain shape mismatch (param count) is a real
        // difference; a nested unresolved `Var` (an untyped lambda
        // param's type, inferred from how it's *used* inside the lambda
        // body) is exactly what a normal call argument commonly looks
        // like and must bind through here, not hard-fail.
        let fn_pair: Option<(Vec<TypeId>, TypeId, Vec<TypeId>, TypeId)> = {
            match (self.ctx.types.get(a), self.ctx.types.get(b)) {
                (SemaType::Function { params: pa, return_type: ra, .. },
                 SemaType::Function { params: pb, return_type: rb, .. }) =>
                    Some((pa.clone(), *ra, pb.clone(), *rb)),
                _ => None,
            }
        };
        if let Some((pa, ra, pb, rb)) = fn_pair {
            if pa.len() != pb.len() { return false; }
            for (ia, ib) in pa.iter().zip(pb.iter()) {
                self.unify(*ia, *ib, Span::at(0));
            }
            self.unify(ra, rb, Span::at(0));
            return true;
        }

        // GENERICS_RULES.md — `Named { def, args }` compatibility. Was
        // previously `def_a == def_b` alone, with `args` completely
        // ignored — meaning two independently-built instantiations of
        // the same generic type (the canonical case: a `let` binding's
        // own type annotation, built fresh via `ast_type_to_sema`,
        // reconciled against its initializer's own separately-built
        // `Named`, e.g. `let x: Option<int> = Option.None`) were treated
        // as compatible without ever binding the initializer's
        // unresolved `Var` args to the annotation's concrete ones. Two
        // non-generic types (`args` empty on both sides) still take the
        // cheap `def_a == def_b` path with nothing further to unify.
        let named_pair: Option<(DefId, Vec<TypeId>, DefId, Vec<TypeId>)> = {
            match (self.ctx.types.get(a), self.ctx.types.get(b)) {
                (SemaType::Named { def: da, args: aa }, SemaType::Named { def: db, args: ab }) =>
                    Some((*da, aa.clone(), *db, ab.clone())),
                _ => None,
            }
        };
        if let Some((da, aa, db, ab)) = named_pair {
            if da != db { return false; }
            if aa.len() != ab.len() { return false; } // same def => should never differ; defensive
            for (ia, ib) in aa.iter().zip(ab.iter()) {
                self.unify(*ia, *ib, Span::at(0));
            }
            return true;
        }

        // Extract inner id pairs first, releasing the immutable borrow on
        // self.ctx.types before we call self.unify (which needs &mut self).
        let inner: Option<(TypeId, TypeId)> = {
            let at = self.ctx.types.get(a);
            let bt = self.ctx.types.get(b);
            match (at, bt) {
                (SemaType::List(ia),     SemaType::List(ib))     => Some((*ia, *ib)),
                // Set/Queue/Stack/Pool/Handle were missing here entirely
                // until now — found while adding InlineList alongside
                // them. Same bug class as the Function/Named fix
                // (GENERICS_RULES.md §2): two separately-constructed
                // instances of the same wrapping type never actually
                // unified their inner element type — e.g. `Set<int>` and
                // `Set<int>` built independently were "compatible" by
                // shape alone, with the element types never reconciled.
                // Only `List` had ever been wired up. Fixed all of them
                // together since the fix is mechanically identical and
                // this exact spot was already being touched for
                // `InlineList` — leaving five known-identical instances
                // of the same bug sitting right next to the fix would've
                // been an odd thing to knowingly walk past.
                (SemaType::Set(ia),        SemaType::Set(ib))        => Some((*ia, *ib)),
                (SemaType::Queue(ia),      SemaType::Queue(ib))      => Some((*ia, *ib)),
                (SemaType::Stack(ia),      SemaType::Stack(ib))      => Some((*ia, *ib)),
                (SemaType::InlineList(ia), SemaType::InlineList(ib)) => Some((*ia, *ib)),
                (SemaType::Linqerizer(ia), SemaType::Linqerizer(ib)) => Some((*ia, *ib)),
                (SemaType::Pool(ia),       SemaType::Pool(ib))       => Some((*ia, *ib)),
                (SemaType::Handle(ia),     SemaType::Handle(ib))     => Some((*ia, *ib)),
                // Unique/Shared/SyncShared (MEMORY_MODEL.md §9,
                // DATASTRUCTURES.md) — each only matches its own kind.
                // No cross-variant arm (e.g. `(Unique, Shared)`) exists on
                // purpose: that's the actual enforcement of "new
                // orthogonal axis" from the design decision this landed
                // under — a `Unique<Node>` and a `Shared<Node>` must stay
                // structurally incompatible even though `Node == Node`,
                // the same way `Reference`'s mutable/shared split above
                // already refuses to unify `&T` with `&mut T`.
                (SemaType::Unique(ia),     SemaType::Unique(ib))     => Some((*ia, *ib)),
                (SemaType::Shared(ia),     SemaType::Shared(ib))     => Some((*ia, *ib)),
                (SemaType::SyncShared(ia), SemaType::SyncShared(ib)) => Some((*ia, *ib)),
                (SemaType::Optional(ia), SemaType::Optional(ib)) => Some((*ia, *ib)),
                (SemaType::Fallible(ia), SemaType::Fallible(ib)) => Some((*ia, *ib)),
                (SemaType::Task(ia),     SemaType::Task(ib))     => Some((*ia, *ib)),
                (SemaType::Slice(ia),    SemaType::Slice(ib))    => Some((*ia, *ib)),
                (SemaType::GcRef(ia),    SemaType::GcRef(ib))    => Some((*ia, *ib)),
                // A `&mut T` and a `&T` are NOT structurally compatible —
                // mutability is a real capability difference. Lifetime
                // labels are deliberately not compared here: the actual
                // outlives/subset fixed point is the borrow checker's
                // job (unbuilt — MEMORY_MODEL.md §9), not ordinary
                // structural unification's. Same checklist item as
                // Set/Queue/Stack/Pool/Handle/InlineList before it —
                // added proactively this time instead of reactively.
                (SemaType::Reference { mutable: ma, inner: ia, .. },
                 SemaType::Reference { mutable: mb, inner: ib, .. }) => {
                    if ma != mb { return false; }
                    Some((*ia, *ib))
                }
                // GAP 2 — two ArenaRefs are only structurally compatible
                // when they carry the *same* ArenaId (MEMORY_MODEL.md §3:
                // arena membership must always be compared by specific
                // id, never just "is/isn't tagged ArenaRef"). A mismatch
                // here is a real escape, not an ordinary shape mismatch —
                // `unify`'s caller reports it via `scope_mismatch_side`
                // rather than falling through to a generic TypeMismatch.
                (SemaType::ArenaRef { arena: aa, inner: ia, .. },
                 SemaType::ArenaRef { arena: ab, inner: ib, .. }) => {
                    if aa != ab { return false; }
                    Some((*ia, *ib))
                }
                // Same rule as ArenaRef immediately above, for the same
                // reason — MEMORY_MODEL.md §11: pool membership compares
                // by specific PoolId, never just "is/isn't tagged
                // PoolRef." A mismatch is a real escape, reported via
                // `scope_mismatch_side` rather than a generic TypeMismatch.
                (SemaType::PoolRef { pool: pa, inner: ia, .. },
                 SemaType::PoolRef { pool: pb, inner: ib, .. }) => {
                    if pa != pb { return false; }
                    Some((*ia, *ib))
                }
                // `null` also compares against a PoolRef/ArenaRef-wrapped
                // Optional — `AcquireHandle`'s result (MEMORY_MODEL.md
                // §11) needs the escape-boundary wrap applied around the
                // *whole* `Optional<Handle<T>>` (so the handle itself
                // can't escape), which leaves the `Optional` one layer
                // inside the ref wrapper instead of at the top level the
                // way List/Queue/Stack/Dictionary's bare-`Elem`-shaped
                // methods leave it. Peel the wrapper first so
                // `pool.acquire(x) == null` still type-checks the same
                // documented way `list.pop() == null` does. Must come
                // before the bare-`Optional` arm below for the same
                // "unconditional case, not a recursive unify" reason
                // that arm's own comment already explains.
                (SemaType::Null, SemaType::PoolRef { inner, .. })
                | (SemaType::PoolRef { inner, .. }, SemaType::Null) => {
                    let resolved = self.apply(*inner);
                    return matches!(self.ctx.types.get(resolved), SemaType::Optional(_));
                }
                (SemaType::Null, SemaType::ArenaRef { inner, .. })
                | (SemaType::ArenaRef { inner, .. }, SemaType::Null) => {
                    let resolved = self.apply(*inner);
                    return matches!(self.ctx.types.get(resolved), SemaType::Optional(_));
                }
                // `null` is a valid value of any `Optional<T>` — needed
                // for `x.pop() == null` (§8's Optional(elem) shape) to
                // type-check as the documented way to test for "empty."
                (SemaType::Null, SemaType::Optional(_))
                | (SemaType::Optional(_), SemaType::Null) => return true,
                // `Optional<T>` also compares directly against a bare
                // `T`, not just `Optional<T>`/`Null` — matches how most
                // languages treat nullable equality (`maybeInt == 5`
                // doesn't require wrapping the `5`). Must come after the
                // `Null` arm above so `x.pop() == null` hits that
                // specific, unconditional case rather than recursing
                // into `unify(elem, Null)`, which nothing makes true.
                (SemaType::Optional(ia), _) => Some((*ia, b)),
                (_, SemaType::Optional(ib)) => Some((a, *ib)),
                _ => None,
            }
        }; // immutable borrow released here
        if let Some((ia, ib)) = inner {
            self.unify(ia, ib, Span::at(0));
            true
        } else {
            false
        }
    }

    // ── Display helper ────────────────────────────────────────────

    fn display_type(&self, id: TypeId) -> String {
        if id == TypeId::ERROR { return "<error>".into(); }
        self.ctx.types.get(id).display(&self.ctx.types, &self.ctx.symbols)
    }
    }
