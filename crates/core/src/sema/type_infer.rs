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
//! - **Crash, found while building `docs/OUTLIVES_RULES.md`'s §8
//!   arena-escape connection, not caused by it** — confirmed by
//!   reproducing with `unify_struct_field`'s own deferral inert (a
//!   non-`edge` struct, so it falls straight through to the original
//!   `unify` call unchanged): a `return` statement inside a `with
//!   arena(...)` block, in the same function as an `ArenaRefEscapesBoundary`
//!   report, panics in `type_table.rs` on an out-of-bounds `TypeId`
//!   index (`u64::MAX`-shaped) rather than reporting a diagnostic.
//!   Narrowed one step further: an ordinary `TypeMismatch` alongside a
//!   `return` in the same block does *not* crash, so this is specific
//!   to the arena-escape path, not `return`-inside-`with` generally.
//!   Not chased further — real root-causing needs stepping through
//!   `scope_mismatch_side`/`arena_escape`'s interaction with whatever
//!   resolves a function's return type, unrelated to this delivery's
//!   actual scope. Flagged here rather than silently worked around so
//!   it isn't rediscovered from scratch; every fixture this delivery
//!   added avoids the trigger shape on purpose (confirmed each one
//!   individually, not assumed safe from the general pattern).
//! - No occurs-check in unification.
//! - A value derived from `self` (directly, or via `let x = self.field`)
//!   inside an `impl`-block method body reports `TYPE-115
//!   InvalidFormatSpec` on `on_type: "<unknown>"` for *any* format spec
//!   in a string interpolation hole, even though the same value
//!   type-checks and runs fine for ordinary (non-formatting) use.
//!   Confirmed narrow, not assumed: a plain local unrelated to `self`
//!   inside the same method body formats fine; `self.field` accessed
//!   *outside* any `impl` block (a plain struct field in `main()`, say)
//!   formats fine; extracting to a local first (`let x = self.field`
//!   before formatting `x`) does **not** fix it, despite looking like
//!   it should follow the same shape as the case that does work. Most
//!   likely the same root cause as the "impl-block methods don't
//!   dispatch" gap (`PARKED_IDEAS.md`, "Traits / interface system") —
//!   `self`'s type isn't fully wired through inside an `impl`-block
//!   body the way an ordinary parameter's is, and this is one more
//!   symptom of that, not an independent format-spec bug — but not
//!   chased to a proven common fix here.
//! - Multi-element destructuring shares one Span; all get collection elem type.
//! - An unknown method name called on an `enum` value is not reported by
//!   sema (`NoSuchMethod` is only raised for struct receivers) and
//!   panics at runtime instead; see PARKED_IDEAS.md.
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

use crate::ast::common::{BinOp, GenericParam, Span, TierAnnotation, TraitBound, UnaryOp, Attribute, AttrArg};
use crate::ast::declarations::{
    ConstDecl, EnumDecl, EnumVariantPayload, ExtendDecl, FunctionDecl, ImplBlock, StaticDecl,
    MethodDecl, Param, ParamKind, ReturnType, StructDecl, StructMember,
    TraitDecl, TraitItem, TypeAlias, MethodSig,
};
use crate::ast::expressions::{
    Arg, ArgKind, Expr, ExprKind, IfBranchBody, LambdaBody,
    MatchArmBody, OrElseFallback,
};
use crate::ast::literals::{FormatSpec, InterpolationPart, IntSuffix, Literal};
use crate::ast::patterns::{EnumPatternPayload, Pattern, PatternKind};
use crate::ast::root::{Item, Program};
use crate::ast::statements::{AllocatorKind, BindingTarget, Block, Stmt, StmtKind};
use crate::ast::types::{Type, TypeKind};
use crate::builtins::instance::{self, MethodReturn, ReceiverWrap};
use crate::error_management::{ErrorManager, errors::{TypeError, TierError}};
use crate::sema::sema_context::SemaContext;
use crate::sema::tier_check;
use crate::ast::common::Visibility;
use crate::sema::symbol_table::{DefId, DefKind};
use crate::sema::type_table::{ArenaId, PoolId, SemaType, TypeId};

mod impl_patterns;
use self::impl_patterns::{ImplGuard, ImplPattern};
use std::rc::Rc;

// ── Integer literal suffix ranges ───────────────────────────────────

/// Whether `raw` -- a literal's non-negative magnitude exactly as
/// written; there's no sign in the token itself, unary `-` is a separate
/// prefix operator applied afterward -- fits the suffix's declared type.
/// Signed suffixes only ever accept the non-negative half of their
/// range here, matching Rust's own literal-suffix behavior (`200i8` is
/// `error: literal out of range for i8` in Rust too, even though the
/// bit pattern -56 would fit, because nothing about the token itself
/// makes -56 the intended value).
/// The magnitude of `suffix`'s most-negative value (`128` for `i8`,
/// since `i8::MIN == -128`), for the `-128i8` special case in
/// `infer_expr_inner`'s `UnaryOp::Neg` arm above. `None` for unsigned
/// suffixes, which have no most-negative value to special-case at all.
fn int_suffix_signed_min_magnitude(suffix: IntSuffix) -> Option<u64> {
    match suffix {
        IntSuffix::I8    => Some(i8::MIN.unsigned_abs() as u64),
        IntSuffix::I16   => Some(i16::MIN.unsigned_abs() as u64),
        IntSuffix::I32   => Some(i32::MIN.unsigned_abs() as u64),
        IntSuffix::I64   => Some(i64::MIN.unsigned_abs()),
        IntSuffix::Isize => Some(i64::MIN.unsigned_abs()),
        IntSuffix::U8 | IntSuffix::U16 | IntSuffix::U32
        | IntSuffix::U64 | IntSuffix::Usize => None,
    }
}

/// One unsuffixed integer literal whose type is still open: its value
/// (negative when it sits directly under a unary `-`) and where it was
/// written, kept so a range check can name it once its type is known.
#[derive(Clone, Copy)]
struct IntLitUse {
    value: i128,
    span:  Span,
}

/// The inclusive value range and display name of a concrete integer
/// type, or `None` for anything that is not one. `int`/`long`/`i64`/
/// `isize` share the runtime's 64-bit signed representation, and
/// `uint`/`u32` and `ulong`/`u64`/`usize` share theirs, exactly as the
/// suffix table above does. `float`/`double` are deliberately not
/// integers here: an integer literal never becomes a float implicitly
/// (`let f: float = 5` stays a mismatch, the same as in Rust).
fn int_type_range(t: &SemaType) -> Option<(i128, i128, &'static str)> {
    Some(match t {
        SemaType::Int   => (i64::MIN as i128, i64::MAX as i128, "int"),
        SemaType::Long  => (i64::MIN as i128, i64::MAX as i128, "long"),
        SemaType::I64   => (i64::MIN as i128, i64::MAX as i128, "i64"),
        SemaType::Isize => (i64::MIN as i128, i64::MAX as i128, "isize"),
        SemaType::I8    => (i8::MIN as i128,  i8::MAX as i128,  "i8"),
        SemaType::I16   => (i16::MIN as i128, i16::MAX as i128, "i16"),
        SemaType::I32   => (i32::MIN as i128, i32::MAX as i128, "i32"),
        SemaType::U8    => (0, u8::MAX as i128,  "u8"),
        SemaType::U16   => (0, u16::MAX as i128, "u16"),
        SemaType::U32   => (0, u32::MAX as i128, "u32"),
        SemaType::Uint  => (0, u32::MAX as i128, "uint"),
        SemaType::U64   => (0, u64::MAX as i128, "u64"),
        SemaType::Ulong => (0, u64::MAX as i128, "ulong"),
        SemaType::Usize => (0, u64::MAX as i128, "usize"),
        _ => return None,
    })
}

fn int_suffix_fits(raw: u64, suffix: IntSuffix) -> bool {
    match suffix {
        IntSuffix::I8    => raw <= i8::MAX as u64,
        IntSuffix::I16   => raw <= i16::MAX as u64,
        IntSuffix::I32   => raw <= i32::MAX as u64,
        IntSuffix::I64   => raw <= i64::MAX as u64,
        // Modeled as i64-width at runtime (`Value::Int`) -- see
        // `interpreter::value::Value`'s own doc comment for why.
        IntSuffix::Isize => raw <= i64::MAX as u64,
        IntSuffix::U8    => raw <= u8::MAX as u64,
        IntSuffix::U16   => raw <= u16::MAX as u64,
        IntSuffix::U32   => raw <= u32::MAX as u64,
        // `raw` is already a `u64`; nothing wider for it to overflow.
        IntSuffix::U64   => true,
        // Modeled as u64-width at runtime (`Value::UInt`).
        IntSuffix::Usize => true,
    }
}

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

/// Whether a parameter list starts with `self`, `mut self`, `&self` or
/// `&mut self`.
/// Whether an attribute list writes a `@tier(...)` at all, valid or not.
fn writes_tier(attrs: &[Attribute<'_>]) -> bool {
    attrs.iter().any(|a| a.name == "tier")
}

fn has_self_param(params: &[Param<'_>]) -> bool {
    params.first().is_some_and(|p| matches!(p.kind,
        ParamKind::SelfVal | ParamKind::SelfMut | ParamKind::SelfRef | ParamKind::SelfRefMut))
}

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
    /// The method's own generic parameters (`fn pick<U: Show>(self, x: U)`).
    generics:    MethodGenerics,
    /// What the `impl` or `extend` block that supplied the method asks of the
    /// receiver's type arguments (`impl<T: Show> Show for Box<T>`, `extend
    /// Box<int>`). `None` for a method with nothing to ask.
    guard:       Option<Rc<ImplGuard>>,
    /// The tier a call to the method is checked against (the cross-tier
    /// rule, `TIER-005`). `Some` for a method written in source: an inherent
    /// method of a struct, an `impl` or an `extend`, a trait's own signatures
    /// and default methods (the tier the trait declares), and a method an
    /// `impl Trait for Type` writes (which has to equal the trait's). `None`
    /// for every method with no source declaration (a built-in method, a
    /// derived or native prelude method).
    tier:        Option<TierAnnotation>,
}

/// One generic parameter a method declares itself, with the bounds written
/// on it. The bounds are as written (not widened by supertraits) and are in
/// terms of the same `Param` numbering as the method's signature.
#[derive(Clone)]
struct OwnParam {
    name:   String,
    bounds: Vec<BoundRef>,
}

/// The generic parameters a method declares itself. `base` is the index of
/// the first one: the parameters of the declaration around the method come
/// first (a struct's, a trait's followed by its abstract `Self`), so the
/// method's own `U` is `Param(base)`, then `Param(base + 1)`, and so on. A
/// method without parameters of its own has an empty `own`.
#[derive(Clone, Default)]
struct MethodGenerics {
    base: usize,
    own:  Vec<OwnParam>,
}

/// A method signature instantiated for one call: the parameter and return
/// types with the receiver's arguments substituted and a fresh inference
/// variable for each of the method's own parameters. `env` is what was
/// substituted, in `Param` order, so the method's bounds can be applied.
struct MethodInst {
    params:      Vec<TypeId>,
    return_type: TypeId,
    env:         Vec<TypeId>,
}

/// What a `type Name<T...> = Target` expands to. `target` is written in
/// terms of `Param(i)` for the alias's own generic parameters, exactly
/// like a generic struct's field types, and a use site substitutes its
/// arguments in with `InferCtx::substitute`.
#[derive(Clone, Copy)]
struct AliasExpansion {
    arity:  usize,
    target: TypeId,
}

/// One method of a trait as sema sees it. The shape is written in terms of
/// the abstract `Self` (`Param(n)`, n = the trait's generic arity, so `0`
/// until generic traits exist) and is instantiated per implementing type.
#[derive(Clone)]
struct TraitMethodInfo {
    name:        String,
    shape:       MethodShape,
    /// A default body exists, so an impl may omit the method.
    has_default: bool,
}

/// A trait collected by `collect_trait_info`. See docs/TRAITS_DESIGN.md.
#[derive(Clone)]
struct TraitInfo {
    name:    String,
    /// How many type parameters the trait declares (`trait Convert<T>` is 1).
    /// The abstract `Self` in `methods` is `Param(arity)` after them.
    arity:   usize,
    methods: Vec<TraitMethodInfo>,
}

/// A trait applied to its type arguments: `Eq`, or `Convert<int>`. A bound
/// written on a declaration's parameter keeps its arguments in terms of that
/// declaration's own `Param(i)`, and a supertrait's are in terms of the
/// trait's own parameters and its abstract `Self`; a use site substitutes
/// (`InferCtx::substitute`). `args` is empty for a trait without parameters,
/// and also before a bound's arguments have been converted (they wait for the
/// type aliases to be expanded, see `fill_generic_bound_args`).
#[derive(Clone, PartialEq, Eq)]
struct BoundRef {
    trait_def: DefId,
    args:      Vec<TypeId>,
}

/// A requirement that `ty` implement `trait_def`, raised where a bounded
/// generic parameter is instantiated (a call to a generic function, a
/// struct or enum used with arguments). Checked once the arguments are
/// known: see `require_bound` and `check_obligations` (slice S1b).
struct Obligation {
    ty:        TypeId,
    trait_def: DefId,
    /// The type arguments the trait is applied to (`Convert<int>`); empty
    /// for a trait without parameters. Checked against the impl's own.
    args:      Vec<TypeId>,
    span:      Span,
    /// A `Dictionary` key requirement: `let d: Dictionary<float, int> =
    /// Dictionary.new()` names the key type twice on one line, and one
    /// mistake is one diagnostic (see `reported_dict_keys`).
    dict_key:  bool,
    /// Already known to fail (decided while the scope it was written in was
    /// live); kept so it is reported in source order with the rest.
    failed:    bool,
}

/// The generic scope `push_generic_scope` replaced, handed back to
/// `pop_generic_scope`: the parameter names and the trait bounds that
/// were in force before.
struct GenericScope {
    names:  HashMap<String, TypeId>,
    bounds: HashMap<TypeId, Vec<BoundRef>>,
}

/// The bounds (and display names) of a declaration's type parameters,
/// installed for a body that does not make those names resolvable as
/// types (a generic function body, an `extend` body on a generic struct)
/// and handed back to `restore_def_scope`.
struct BoundsScope {
    bounds: HashMap<TypeId, Vec<BoundRef>>,
    names:  HashMap<TypeId, String>,
}

/// The names of the prelude traits (declared in `rd_parser::prelude`). What
/// each one builds on is its declared supertrait list (`trait Ord:
/// PartialOrd + Eq`), read into `trait_supers` like any other trait's.
const PRELUDE_TRAIT_NAMES: [&str; 6] = ["PartialEq", "Eq", "PartialOrd", "Ord", "Clone", "Hash"];

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
    /// The expansion of every `type` alias, filled by `collect_alias_sigs`
    /// before any other signature is collected. An alias is transparent:
    /// `ast_type_to_sema` replaces a use of it by its target, so `Score`
    /// from `type Score = int` is simply `int` everywhere.
    alias_expansions: HashMap<DefId, AliasExpansion>,
    /// True only while `collect_alias_sigs` runs. A use of an alias that
    /// is not expanded yet (it is declared later, or is part of a cycle)
    /// sets `alias_blocked` so the caller retries it on the next round
    /// instead of caching a half-resolved target.
    alias_prepass: bool,
    alias_blocked: bool,
    /// Every trait, by definition. Filled by `collect_trait_info` before any
    /// impl is checked.
    traits:           HashMap<DefId, TraitInfo>,
    /// `(trait, implementing type)` -> the span of its `impl`. Exactly one
    /// impl per pair is allowed (`TYPE-128`).
    trait_impls:      HashMap<(DefId, DefId), Span>,
    /// `(type, method name)` -> the traits that supply a method of that
    /// name to the type. Only trait-supplied methods are listed; an
    /// inherent method is recognised by being in `struct_methods` with no
    /// entry here.
    method_origins:   HashMap<(DefId, String), Vec<DefId>>,
    /// `(type, method name)` pairs two or more traits supply with no
    /// inherent method to win. An unqualified call of one is `TYPE-127`.
    ambiguous_methods: HashSet<(DefId, String)>,
    /// `(type, method name)` pairs whose entry in `struct_methods` was put
    /// there by a TRAIT impl. Anything else in the table is inherent, which
    /// is how `add_trait_method` tells an inherent method from the first
    /// trait's entry.
    trait_entries:    HashSet<(DefId, String)>,
    /// What the written type `Self` means right now: the implementing type
    /// inside an `impl`/`extend`/struct, the abstract `Self` parameter
    /// inside a trait. `None` outside all of them.
    self_type:        Option<TypeId>,
    /// Trait bounds of the type parameters in scope, by the parameter's
    /// `TypeId`. Inside a trait's default method the abstract `Self` is
    /// bounded by the trait itself, so `self.other()` resolves through it.
    current_bounds:   HashMap<TypeId, Vec<BoundRef>>,
    /// The declared bounds of each generic function, struct, enum and trait,
    /// by the declaration's definition and then the parameter's position.
    /// Only declarations that have at least one bound are present.
    generic_bounds:   HashMap<DefId, Vec<Vec<BoundRef>>>,
    /// The written parameter names of each generic function, struct and
    /// enum, by position, so a diagnostic in a body that cannot see the
    /// declaration's own scope still says `T` rather than a type id.
    generic_param_names: HashMap<DefId, Vec<String>>,
    /// The function type of each bounded generic function, back to its
    /// definition. Every function gets a type of its own, so a call
    /// through `let f = total` still finds `total`'s bounds.
    fn_type_defs:     HashMap<TypeId, DefId>,
    /// D1 (docs/ARENA_BORROW_RULES.md): the lifetimes each non-async top-level function names in
    /// a `where` clause. Rule R2 refuses to lend an arena reference through any of them.
    fn_where_lifetimes: HashMap<DefId, HashSet<String>>,
    /// Display names for the type parameters of the body being inferred
    /// (installed by `install_def_scope`).
    param_names:      HashMap<TypeId, String>,
    /// Bound requirements not decided yet (the type is still an inference
    /// variable, or trait impls are not registered yet).
    pending_obligations: Vec<Obligation>,
    /// Obligations already reported, so one mistake reached twice (a call
    /// inferred more than once) is one diagnostic.
    reported_obligations: HashSet<(Span, DefId)>,
    /// Dictionary key mistakes already reported, by line and key type.
    reported_dict_keys: HashSet<(usize, TypeId)>,
    /// The prelude traits (`PartialEq`, `Eq`, `PartialOrd`, `Ord`, `Clone`),
    /// by definition, and back by name. Present only for a prelude item the
    /// program did not shadow with its own.
    prelude_traits:   HashMap<DefId, &'static str>,
    prelude_defs:     HashMap<&'static str, DefId>,
    /// Each trait's declared supertraits (`trait B: A + C`), resolved, in
    /// the order written and without repeats. Filled by
    /// `register_trait_supertraits` before any bound is registered. A name
    /// that is not a trait is reported there and left out.
    trait_supers:     HashMap<DefId, Vec<DefId>>,
    /// The type arguments of each supertrait edge, `(trait, supertrait)`,
    /// written in the trait's own scope: its parameters, then `Self`
    /// (`trait B<T>: A<T>` stores `[Param(0)]` for `(B, A)`). Absent for a
    /// supertrait without arguments.
    trait_super_args: HashMap<(DefId, DefId), Vec<TypeId>>,
    /// How many type parameters each trait declares.
    trait_arity:      HashMap<DefId, usize>,
    /// The type arguments of each impl, by `(trait, implementing type)`:
    /// `[int]` for `impl Convert<int> for Celsius`. A type has one impl of a
    /// trait, so the arguments are a property of the pair.
    trait_impl_args:  HashMap<(DefId, DefId), Vec<TypeId>>,
    /// Set once `register_trait_impls` has run: before that a named type
    /// cannot be said to lack an impl.
    impls_registered: bool,
    /// `self`'s type while inside a struct's own method bodies — the
    /// abstract `Named { def, args: [Param(0), Param(1), ...] }` for a
    /// generic struct, or `Named { def, args: [] }` for a non-generic
    /// one. Method bodies are checked once, generically — the same way
    /// Rust checks a generic impl's body once rather than per
    /// instantiation — with concrete substitution happening at each call
    /// site instead. `None` outside any method body.
    current_struct_type: Option<TypeId>,
    /// Set by the `Call` arm just before it pre-infers a `Field` callee,
    /// and consumed (reset) by the `Field` arm's first line. While set,
    /// that one `Field` node does not report `NoSuchField` for a name
    /// that is not a field: the `Call` arm's own dispatch reports the
    /// call-shaped `NoSuchMethod` for the same name, so reporting both
    /// gave two diagnostics for one typo. Nested field accesses inside
    /// the callee's target (`a.b.c()`) still report normally, since the
    /// flag is taken before the target is inferred.
    callee_field_pending: bool,
    /// Populated once per struct by `collect_struct_sig`, mirrors
    /// `enum_variants`'s role. Field types may contain `Param`
    /// placeholders for a generic struct.
    struct_fields:    HashMap<DefId, Vec<(String, TypeId)>>,
    /// Populated once per struct by `collect_struct_sig`. Same `Param`-
    /// placeholder treatment as `struct_fields`.
    struct_methods:   HashMap<DefId, Vec<(String, MethodShape)>>,
    /// The generic parameters of every method that declares some, by the
    /// method's own definition. Filled by `collect_method_sig`; read where a
    /// shape is rebuilt from the method's function type.
    method_generics:  HashMap<DefId, MethodGenerics>,
    /// The definition standing in for each built-in type (a primitive, a
    /// tuple, an array) an `extend` or `impl` block is written for, by the
    /// type as a diagnostic writes it (`int`, `(int, string)`, `[3]int`). The
    /// tables keyed by a definition (`struct_methods`, `trait_impls`,
    /// `trait_impl_args`) then serve built-in types as they serve structs.
    builtin_defs:      HashMap<String, DefId>,
    /// The type each of those definitions stands for.
    builtin_def_types: HashMap<DefId, TypeId>,
    /// The definition for the target of each `extend` or `impl` block, by the
    /// target type's span, so the target is converted once.
    builtin_targets:   HashMap<Span, (DefId, TypeId)>,
    /// The analysis of each `impl` or `extend` block for a generic head or
    /// with parameters of its own, by the block's span (docs/TRAITS_DESIGN.md,
    /// slice S3e). Blocks for anything else do not have one.
    impl_patterns:     HashMap<Span, Rc<ImplPattern>>,
    /// Blocks whose target was reported as unusable. Their methods keep
    /// signatures but are not registered, implemented or read.
    impl_failed:       HashSet<Span>,
    /// What each trait impl asks of the type arguments of its head, for the
    /// impls that ask anything (`impl<T: Show> Show for Box<T>`).
    trait_impl_guards: HashMap<(DefId, DefId), Rc<ImplGuard>>,
    /// `(run-time key, method name)` -> the built-in definition that already
    /// gave that name to a type of that kind, so `extend int` and `extend
    /// long` cannot both define `tag`.
    inherent_key_methods: HashMap<(String, String), DefId>,
    /// `(run-time key, trait)` -> the definition that implements the trait
    /// for that key. Types that are one kind of value at run time (`int`,
    /// `long`, `i64`, `isize`) cannot be told apart by a call, so each trait
    /// can be implemented for only one of them.
    builtin_key_impls: HashMap<(String, DefId), DefId>,
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
    /// Unsuffixed integer literals whose type is still open, keyed by the
    /// number of the (root) `Var` standing for them. Several literals
    /// share one entry once their vars have been unified with each other
    /// (`1 + 2`, `[1, 2, 3]`). An entry disappears the moment its var is
    /// bound to a concrete type, which is also when the range check runs.
    /// See `infer_int_literal`.
    int_lit_vars:     HashMap<u32, Vec<IntLitUse>>,
    /// Every integer-literal site seen since the last `finish_int_literals`
    /// (its span and its var), so the final width of each one can be
    /// recorded in `SemaContext::int_literal_types` for the interpreter.
    int_lit_sites:    Vec<(Span, TypeId)>,
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
            alias_expansions: HashMap::new(),
            alias_prepass:    false,
            alias_blocked:    false,
            traits:            HashMap::new(),
            trait_impls:       HashMap::new(),
            method_origins:    HashMap::new(),
            ambiguous_methods: HashSet::new(),
            trait_entries:     HashSet::new(),
            self_type:         None,
            current_bounds:    HashMap::new(),
            generic_bounds:    HashMap::new(),
            generic_param_names: HashMap::new(),
            fn_type_defs:      HashMap::new(),
            fn_where_lifetimes: HashMap::new(),
            param_names:       HashMap::new(),
            pending_obligations: Vec::new(),
            reported_obligations: HashSet::new(),
            reported_dict_keys: HashSet::new(),
            prelude_traits:    HashMap::new(),
            prelude_defs:      HashMap::new(),
            trait_supers:      HashMap::new(),
            trait_super_args:  HashMap::new(),
            trait_arity:       HashMap::new(),
            trait_impl_args:   HashMap::new(),
            impls_registered:  false,
            current_struct_type:    None,
            callee_field_pending:   false,
            struct_fields:    HashMap::new(),
            struct_methods:   HashMap::new(),
            method_generics:  HashMap::new(),
            builtin_defs:      HashMap::new(),
            builtin_def_types: HashMap::new(),
            builtin_targets:   HashMap::new(),
            builtin_key_impls: HashMap::new(),
            impl_patterns:     HashMap::new(),
            impl_failed:       HashSet::new(),
            trait_impl_guards: HashMap::new(),
            inherent_key_methods: HashMap::new(),
            struct_derives:   HashMap::new(),
            generic_arity:    HashMap::new(),
            current_tier:     TierAnnotation::High,
            int_lit_vars:     HashMap::new(),
            int_lit_sites:    Vec::new(),
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
    ///
    /// The scope also carries each parameter's declared trait bounds
    /// (`current_bounds`), so a method call on a value of that parameter's
    /// type resolves through them. Bound names are resolved silently here;
    /// `register_generic_bounds` reports the invalid ones once per
    /// declaration. A caller that needs an extra bound in scope (a trait's
    /// abstract `Self`) adds it after the push.
    fn push_generic_scope(&mut self, params: &[GenericParam]) -> GenericScope {
        let mut new_map = HashMap::with_capacity(params.len());
        for (i, gp) in params.iter().enumerate() {
            let ty = self.ctx.types.intern(SemaType::Param(i));
            new_map.insert(gp.name.to_string(), ty);
        }
        // The names go in first: a bound's arguments may mention any
        // parameter of the declaration (`U: Convert<T>`).
        let names = std::mem::replace(&mut self.current_generic_params, new_map);
        let mut new_bounds: HashMap<TypeId, Vec<BoundRef>> = HashMap::new();
        for (i, gp) in params.iter().enumerate() {
            if gp.bounds.is_empty() { continue; }
            let ty = self.ctx.types.intern(SemaType::Param(i));
            let declared = self.declared_bounds(gp.bounds);
            if !declared.is_empty() {
                let widened = self.expand_supertraits(declared, ty);
                new_bounds.insert(ty, widened);
            }
        }
        GenericScope {
            names,
            bounds: std::mem::replace(&mut self.current_bounds, new_bounds),
        }
    }

    fn pop_generic_scope(&mut self, prev: GenericScope) {
        self.current_generic_params = prev.names;
        self.current_bounds = prev.bounds;
    }

    /// Bring a method's own generic parameters into scope on top of the ones
    /// already there (the struct's, the trait's): `U` is `Param(base)`, the
    /// next is `Param(base + 1)`. Each parameter's declared bounds, with the
    /// traits they build on, are in force for its type. A method without
    /// parameters of its own changes nothing and returns `None`. Pairs with
    /// `pop_own_generic_scope`.
    fn push_own_generic_scope(&mut self, params: &[GenericParam], base: usize) -> Option<GenericScope> {
        if params.is_empty() { return None; }
        let prev = GenericScope {
            names:  self.current_generic_params.clone(),
            bounds: self.current_bounds.clone(),
        };
        // The names go in first: a bound's arguments may mention any
        // parameter in scope (`fn m<U: Convert<V>, V>`).
        for (j, gp) in params.iter().enumerate() {
            let ty = self.ctx.types.intern(SemaType::Param(base + j));
            self.current_generic_params.insert(gp.name.to_string(), ty);
        }
        for (j, gp) in params.iter().enumerate() {
            if gp.bounds.is_empty() { continue; }
            let ty = self.ctx.types.intern(SemaType::Param(base + j));
            let declared = self.declared_bounds(gp.bounds);
            if !declared.is_empty() {
                let widened = self.expand_supertraits(declared, ty);
                self.current_bounds.insert(ty, widened);
            }
        }
        Some(prev)
    }

    fn pop_own_generic_scope(&mut self, prev: Option<GenericScope>) {
        if let Some(prev) = prev { self.pop_generic_scope(prev); }
    }

    /// The generic parameters a method declares, with their bounds checked
    /// and recorded. Runs in the method's own scope, once per declaration:
    /// the bound names, the argument counts and conflicting bounds are
    /// reported here, the same checks a generic function's bounds get.
    fn method_generics_of<'ast>(&mut self, params: &[GenericParam<'ast>], base: usize) -> MethodGenerics {
        let mut own: Vec<OwnParam> = Vec::with_capacity(params.len());
        for gp in params {
            let bounds = self.check_bound_list(gp, true);
            own.push(OwnParam { name: gp.name.to_string(), bounds });
        }
        MethodGenerics { base, own }
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
    ///
    /// `span` is where the instantiation happens: each declared bound on
    /// the struct or enum's parameters becomes an obligation there (see
    /// `require_def_bounds`).
    fn instantiate(&mut self, def_id: DefId, span: Span) -> TypeId {
        let arity = self.generic_arity.get(&def_id).copied().unwrap_or(0);
        if arity == 0 {
            return self.ctx.def_type(def_id).unwrap_or_else(|| self.unknown());
        }
        let args: Vec<TypeId> = (0..arity).map(|_| self.fresh_var()).collect();
        self.require_def_bounds(def_id, &args, span);
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

    /// D3b (docs/ARENA_STRINGS.md): if `ty` is an arena-tagged string (`&arena string`), the plain
    /// string type; any other type comes back unchanged. For an operator that only reads its string
    /// operands (`==`, `<`, `+`): the result is a `bool`, or a new string that is always allocated in
    /// the GC heap (a copy), so it carries no borrow of the operand.
    fn peel_arena_str(&mut self, ty: TypeId) -> TypeId {
        let resolved = self.apply(ty);
        if let SemaType::ArenaRef { inner, .. } = self.ctx.types.get(resolved).clone() {
            let inner = self.apply(inner);
            if matches!(self.ctx.types.get(inner), SemaType::Str) {
                return inner;
            }
        }
        ty
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
    fn builtin_constructor_type(&mut self, namespace: &str, span: Span) -> Option<TypeId> {
        match namespace {
            "List" => {
                let elem = self.fresh_var();
                Some(self.ctx.types.intern(SemaType::List(elem)))
            }
            "Dictionary" => {
                let k = self.fresh_var();
                let v = self.fresh_var();
                self.require_dict_key(k, span);
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
                self.require_dict_key(k, ty.span);
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
                // `Self`: the implementing type inside an `impl`, `extend` or
                // struct, the abstract `Self` parameter inside a trait.
                // Outside all of them it falls through to the ordinary
                // lookup and stays unresolved, as before.
                if path.len() == 1 && args.is_empty() && root == "Self" {
                    if let Some(t) = self.self_type { return t; }
                }
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
                        // A `type` alias is transparent: it is replaced by
                        // its target, not kept as a type of its own.
                        if matches!(self.ctx.symbols.lookup(def_id).kind, DefKind::TypeAlias) {
                            return self.expand_alias(def_id, root, arg_ids, ty.span);
                        }
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
                        self.require_def_bounds(def_id, &arg_ids, ty.span);
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

    // ── Traits (docs/TRAITS_DESIGN.md, slice S1a) ────────────────────
    //
    // A trait is collected once (`collect_trait_info`), each `impl Trait for
    // Type` is then checked against it and its methods are registered into
    // the type's dispatch table (`register_trait_impls`), and a call is
    // resolved through that table, through a bound on a type parameter, or
    // through an explicit `Trait.method(value)`.

    /// Register every struct and enum's generic arity up front, so a
    /// signature that mentions one (an impl's `Self`, an alias target) is
    /// built correctly whatever order the declarations appear in.
    fn register_generic_arities<'ast>(&mut self, program: &Program<'ast>) {
        for item in program.items {
            let (name, arity) = match item {
                Item::Struct(s) => (s.name, s.generic_params.len()),
                Item::Enum(e)   => (e.name, e.generic_params.len()),
                _ => continue,
            };
            if let Some(id) = self.ctx.top_level_def(name) {
                self.generic_arity.insert(id, arity);
            }
        }
    }

    /// `Self` for an `impl`/`extend` block: the target type with its own
    /// generic parameters as `Param`s, or `None` when the target is not a
    /// plain named type.
    fn self_type_for_target<'ast>(&mut self, target: &Type<'ast>) -> Option<TypeId> {
        let Some(def) = self.target_type_def_id(target) else {
            // A primitive, a tuple or an array is its own `Self`.
            return self.builtin_target(target).map(|(_, ty)| ty);
        };
        let arity = self.generic_arity.get(&def).copied().unwrap_or(0);
        let args: Vec<TypeId> = (0..arity)
            .map(|i| self.ctx.types.intern(SemaType::Param(i)))
            .collect();
        Some(self.ctx.types.insert(SemaType::Named { def, args }))
    }

    /// How a name used where a trait is required resolves: the trait's
    /// definition, or what the name actually is (`None` = nothing by that
    /// name).
    fn resolve_trait_name(&self, name: &str) -> Result<DefId, Option<&'static str>> {
        let Some(id) = self.ctx.top_level_def(name) else { return Err(None); };
        Err(Some(match self.ctx.symbols.lookup(id).kind {
            DefKind::Trait             => return Ok(id),
            DefKind::Struct { .. }     => "struct",
            DefKind::Enum              => "enum",
            DefKind::Function { .. }   => "function",
            DefKind::Const             => "constant",
            DefKind::Static            => "static",
            DefKind::TypeAlias         => "type alias",
            _                          => "name that is not a trait",
        }))
    }

    /// An impl needs the traits its trait builds on: `impl Ord` needs
    /// `PartialOrd` and `Eq` (so `PartialEq`), `impl Hash` needs `Eq`, and
    /// `impl B for T` with `trait B: A` needs `A`. Each impl is reported at
    /// most once, for the first missing trait in the chain.
    fn check_impl_supertraits(&mut self) {
        let mut impls: Vec<(DefId, DefId, Span)> = self.trait_impls.iter()
            .map(|((t, d), s)| (*t, *d, *s))
            .collect();
        impls.sort_by_key(|(_, _, s)| s.start);
        for (trait_def, target, span) in impls {
            let own_args = self.trait_impl_args.get(&(trait_def, target)).cloned().unwrap_or_default();
            // An impl for a generic head is read in its own scope, with the
            // bounds it wrote in force: `impl<T: Show> Loud for Box<T>` has
            // `Show for Box<T>` when that impl asks no more than `T: Show`.
            let pattern = self.impl_patterns.get(&span).cloned();
            let scope = pattern.as_ref().map(|p| self.enter_impl_scope(p));
            let ty = match &pattern {
                Some(p) => p.self_ty,
                None => match self.builtin_def_types.get(&target) {
                    Some(t) => *t,
                    None    => self.ctx.types.insert(SemaType::Named { def: target, args: Vec::new() }),
                },
            };
            // The impl's own trait comes first in the closure, then what it
            // builds on, with the impl's arguments carried through
            // (`impl B<int> for T` with `trait B<X>: A<X>` needs `A<int>`).
            // On a cycle the closure holds the trait only once, and its impl
            // exists (it is being checked).
            let closure = self.supertrait_closure(&[BoundRef { trait_def, args: own_args }], ty);
            // One missing supertrait per impl, the first in the chain: the rest
            // follow from it (`PartialOrd` itself needs `PartialEq`), and one
            // mistake is one diagnostic.
            let mut missing: Option<BoundRef> = None;
            for b in closure.into_iter().skip(1) {
                if self.bound_args_status(ty, b.trait_def, &b.args, span) != Some(true) {
                    missing = Some(b);
                    break;
                }
            }
            let supertrait = missing.as_ref().map(|b| self.trait_ref_text(b.trait_def, &b.args));
            let type_name = match &pattern {
                Some(p) => self.display_type(p.self_ty),
                None    => self.ctx.symbols.lookup(target).name.clone(),
            };
            if let Some(scope) = scope { self.leave_impl_scope(scope); }
            let (Some(b), Some(supertrait)) = (missing, supertrait) else { continue; };
            let trait_name = self.trait_display_name(trait_def);
            let derivable = self.prelude_traits.contains_key(&b.trait_def);
            self.errors.add_type_error(TypeError::MissingSupertraitImpl {
                trait_name, supertrait, derivable, type_name, span,
            });
        }
    }

    /// A trait's name as a diagnostic writes it.
    fn trait_display_name(&self, trait_def: DefId) -> String {
        self.traits.get(&trait_def)
            .map(|t| t.name.clone())
            .unwrap_or_else(|| self.ctx.symbols.lookup(trait_def).name.clone())
    }

    /// Resolve every trait's supertrait list into `trait_supers`, and report
    /// a name that is not a trait (`TYPE-122`) and a trait that is its own
    /// supertrait, directly or through others (`TYPE-133`). Runs before any
    /// bound is registered: a bound's meaning includes what its trait builds
    /// on (`expand_supertraits`).
    fn register_trait_supertraits<'ast>(&mut self, program: &Program<'ast>) {
        let mut order: Vec<DefId> = Vec::new();
        let mut edges: HashMap<DefId, Vec<(DefId, Span)>> = HashMap::new();
        for item in program.items {
            let Item::Trait(t) = item else { continue; };
            let Some(def) = self.ctx.top_level_def(t.name) else { continue; };
            self.trait_arity.insert(def, t.generic_params.len());
            let mut list: Vec<(DefId, Span)> = Vec::with_capacity(t.supertraits.len());
            for s in t.supertraits {
                match self.resolve_trait_name(s.name) {
                    Ok(id) => {
                        if !list.iter().any(|(d, _)| *d == id) { list.push((id, s.span)); }
                    }
                    Err(found) => {
                        self.errors.add_type_error(TypeError::NotATrait {
                            name: s.name.to_string(), found, span: s.span,
                        });
                    }
                }
            }
            order.push(def);
            self.trait_supers.insert(def, list.iter().map(|(d, _)| *d).collect());
            edges.insert(def, list);
        }
        let mut state: HashMap<DefId, bool> = HashMap::new();
        let mut stack: Vec<DefId> = Vec::new();
        for root in order {
            self.walk_supertraits(root, &edges, &mut state, &mut stack);
        }
    }

    /// Depth-first walk over the supertrait edges. `state` holds `false` for
    /// a trait on the current path and `true` once it is finished; an edge
    /// back to a trait on the path closes a cycle, reported once at that
    /// edge's name.
    fn walk_supertraits(
        &mut self,
        id:    DefId,
        edges: &HashMap<DefId, Vec<(DefId, Span)>>,
        state: &mut HashMap<DefId, bool>,
        stack: &mut Vec<DefId>,
    ) {
        if state.contains_key(&id) { return; }
        state.insert(id, false);
        stack.push(id);
        if let Some(out) = edges.get(&id) {
            for &(to, span) in out {
                match state.get(&to).copied() {
                    None => self.walk_supertraits(to, edges, state, stack),
                    Some(false) => {
                        let from = stack.iter().position(|d| *d == to).unwrap_or(0);
                        let mut names: Vec<String> = stack[from..].iter()
                            .map(|d| self.trait_display_name(*d))
                            .collect();
                        names.push(self.trait_display_name(to));
                        self.errors.add_type_error(TypeError::SupertraitCycle {
                            trait_name: names[0].clone(),
                            path:       names.join(" -> "),
                            span,
                        });
                    }
                    Some(true) => {}
                }
            }
        }
        stack.pop();
        state.insert(id, true);
    }

    /// `start` and every trait they build on, each once: first `start` in
    /// the order given, then their supertraits level by level (so `Ord`
    /// gives `Ord`, `PartialOrd`, `Eq`, `PartialEq`). A supertrait's
    /// arguments are the ones the trait's declaration writes, with the
    /// trait's own parameters replaced by the arguments it was given and
    /// `Self` by `self_ty` (`T: B<int>` with `trait B<X>: A<X>` gives
    /// `A<int>`). Safe on a cycle; when a trait is reached twice with
    /// different arguments the first is kept.
    fn supertrait_closure(&mut self, start: &[BoundRef], self_ty: TypeId) -> Vec<BoundRef> {
        let mut out: Vec<BoundRef> = Vec::with_capacity(start.len() + 2);
        let mut frontier: Vec<BoundRef> = start.to_vec();
        while !frontier.is_empty() {
            let mut next: Vec<BoundRef> = Vec::new();
            for b in frontier {
                if out.iter().any(|o| o.trait_def == b.trait_def) { continue; }
                let supers = self.trait_supers.get(&b.trait_def).cloned().unwrap_or_default();
                for sup in supers {
                    let written = self.trait_super_args.get(&(b.trait_def, sup)).cloned().unwrap_or_default();
                    let args: Vec<TypeId> = if written.is_empty() {
                        Vec::new()
                    } else {
                        let arity = self.trait_arity.get(&b.trait_def).copied().unwrap_or(b.args.len());
                        let mut env: Vec<TypeId> = b.args.iter().take(arity).copied().collect();
                        while env.len() < arity { env.push(self.unknown()); }
                        env.push(self_ty);
                        written.iter().map(|t| self.substitute(*t, &env)).collect()
                    };
                    next.push(BoundRef { trait_def: sup, args });
                }
                out.push(b);
            }
            frontier = next;
        }
        out
    }

    /// Record which traits are the prelude's, before any signature is
    /// collected: an annotation such as `Dictionary<K, V>` needs to know
    /// `Hash` while the signature that contains it is being built.
    fn register_prelude_traits<'ast>(&mut self, program: &Program<'ast>) {
        for item in program.items {
            let Item::Trait(t) = item else { continue; };
            if !t.span.is_prelude() { continue; }
            let Some(name) = PRELUDE_TRAIT_NAMES.iter().find(|n| **n == t.name).copied() else { continue; };
            if let Some(def_id) = self.ctx.top_level_def(t.name) {
                self.prelude_traits.insert(def_id, name);
                self.prelude_defs.insert(name, def_id);
            }
        }
    }

    /// A `Dictionary` key type must implement `Hash` (which implies `Eq`).
    /// Raised where a dictionary type comes into being: `Dictionary.new()`
    /// and a `Dictionary<K, V>` annotation. A key that is still unresolved
    /// at the end of the body is not an error.
    ///
    /// Not raised while a `type` alias is being expanded: an alias such as
    /// `type Pairs<K, V> = Dictionary<K, V>` has no way to declare `K: Hash`
    /// (bounds on alias parameters are not built), so the requirement is
    /// checked where the alias is used (`require_dict_keys_in`).
    fn require_dict_key(&mut self, key: TypeId, span: Span) {
        if self.alias_prepass { return; }
        if let Some(hash) = self.prelude_defs.get("Hash").copied() {
            self.require_bound_with(key, hash, span, true);
        }
    }

    /// Every `Dictionary` inside `ty` (an alias's expansion at a use site)
    /// has a key type that must be `Hash`.
    fn require_dict_keys_in(&mut self, ty: TypeId, span: Span) {
        match self.ctx.types.get(ty).clone() {
            SemaType::Dictionary(k, v) => {
                self.require_dict_key(k, span);
                self.require_dict_keys_in(k, span);
                self.require_dict_keys_in(v, span);
            }
            SemaType::List(e) | SemaType::Queue(e) | SemaType::Stack(e) | SemaType::Optional(e) =>
                self.require_dict_keys_in(e, span),
            SemaType::Tuple(elems) => {
                for e in elems { self.require_dict_keys_in(e, span); }
            }
            _ => {}
        }
    }

    // ── Bounds (docs/TRAITS_DESIGN.md, slice S1b) ───────────────────
    //
    // `fn f<T: Shape>(x: T)`: the bound names are validated once per
    // declaration (`register_generic_bounds`), a body sees them through
    // `current_bounds`, and every place a bounded parameter is
    // instantiated records an `Obligation` that the argument type
    // implements the trait (`require_bound`, `check_obligations`).

    /// The traits a parameter's written bounds name, with their arguments
    /// converted in the generic scope now in force. Silent: a name that is
    /// not a trait, a wrong argument count and an argument that does not
    /// resolve are reported once at the declaration (`fill_generic_bound_args`),
    /// not again by every scope built from it. The first bound on a trait
    /// wins, so `T: A + A` is one bound.
    fn declared_bounds(&mut self, bounds: &[TraitBound]) -> Vec<BoundRef> {
        let before = self.errors.type_error_count();
        let mut out: Vec<BoundRef> = Vec::with_capacity(bounds.len());
        for b in bounds {
            let Ok(id) = self.resolve_trait_name(b.name) else { continue; };
            let arity = self.trait_arity.get(&id).copied().unwrap_or(b.args.len());
            let args = self.trait_arg_types(b.args, arity);
            out.push(BoundRef { trait_def: id, args });
        }
        self.discard_type_errors_since(before);
        out
    }

    /// The written type arguments of a trait, converted in the scope now in
    /// force and made to have exactly `arity` entries (extra ones are
    /// dropped, missing ones become unknown) so a count mistake, reported
    /// where the trait is named, does not cascade.
    fn trait_arg_types<'ast>(&mut self, args: &[&'ast Type<'ast>], arity: usize) -> Vec<TypeId> {
        let mut out: Vec<TypeId> = args.iter().take(arity).map(|t| self.ast_type_to_sema(t)).collect();
        while out.len() < arity { out.push(self.unknown()); }
        out
    }

    /// Drop the type errors added since the count was `before`.
    fn discard_type_errors_since(&mut self, before: usize) {
        if self.errors.type_error_count() <= before { return; }
        let mut all = self.errors.take_type_errors();
        all.truncate(before);
        for e in all { self.errors.add_type_error(e); }
    }

    /// `bounds` plus the traits each one builds on, however deep, with their
    /// arguments worked out: a body with `T: Ord` may also use what
    /// `PartialOrd`, `Eq` and `PartialEq` provide, and one with `T: B<int>`
    /// where `trait B<X>: A<X>` may use what `A<int>` provides. `self_ty` is
    /// what a supertrait's `Self` means (the bounded parameter, or the
    /// implementing type). Declared bounds stay as written (an obligation for
    /// `Ord` is one obligation); only the bounds in force inside a body widen.
    fn expand_supertraits(&mut self, bounds: Vec<BoundRef>, self_ty: TypeId) -> Vec<BoundRef> {
        self.supertrait_closure(&bounds, self_ty)
    }

    /// Validate the bounds written on every generic function, struct and
    /// enum and record them by declaration. Runs before any signature is
    /// collected so a bounded parameter is known everywhere it is used.
    fn register_generic_bounds<'ast>(&mut self, program: &Program<'ast>) {
        for item in program.items {
            match item {
                Item::Function(f) => {
                    if let Some(def) = self.ctx.top_level_def(f.name) {
                        self.register_decl_bounds(def, f.generic_params);
                    }
                }
                Item::Struct(s) => {
                    if let Some(def) = self.ctx.top_level_def(s.name) {
                        self.register_decl_bounds(def, s.generic_params);
                    }
                }
                Item::Trait(t) => {
                    if let Some(def) = self.ctx.top_level_def(t.name) {
                        self.register_decl_bounds(def, t.generic_params);
                    }
                }
                Item::Enum(e) => {
                    if let Some(def) = self.ctx.top_level_def(e.name) {
                        self.register_decl_bounds(def, e.generic_params);
                    }
                }
                Item::TypeAlias(a) => {
                    if let Some(def) = self.ctx.top_level_def(a.name) {
                        self.register_decl_bounds(def, a.generic_params);
                    }
                }
                _ => {}
            }
        }
    }

    /// Install the declared bounds (and names) of `def`'s parameters for a
    /// body that does not push the declaration's generic scope. Pairs with
    /// `restore_def_scope`.
    fn install_def_scope(&mut self, def: Option<DefId>) -> BoundsScope {
        let mut bounds: HashMap<TypeId, Vec<BoundRef>> = HashMap::new();
        let mut names:  HashMap<TypeId, String>        = HashMap::new();
        if let Some(def) = def {
            let per_param = self.generic_bounds.get(&def).cloned().unwrap_or_default();
            let written   = self.generic_param_names.get(&def).cloned().unwrap_or_default();
            for (i, name) in written.iter().enumerate() {
                let ty = self.ctx.types.intern(SemaType::Param(i));
                names.insert(ty, name.clone());
                if let Some(bs) = per_param.get(i) {
                    if !bs.is_empty() {
                        let widened = self.expand_supertraits(bs.clone(), ty);
                        bounds.insert(ty, widened);
                    }
                }
            }
        }
        BoundsScope {
            bounds: std::mem::replace(&mut self.current_bounds, bounds),
            names:  std::mem::replace(&mut self.param_names, names),
        }
    }

    fn restore_def_scope(&mut self, prev: BoundsScope) {
        self.current_bounds = prev.bounds;
        self.param_names    = prev.names;
    }

    /// Validate the bound names written on `def`'s parameters and record them
    /// by declaration. The arguments of each bound wait for
    /// `fill_generic_bound_args`: converting a type needs the aliases expanded.
    fn register_decl_bounds(&mut self, def: DefId, params: &[GenericParam]) {
        if !params.is_empty() {
            self.generic_param_names.insert(def, params.iter().map(|g| g.name.to_string()).collect());
        }
        let mut per_param: Vec<Vec<BoundRef>> = Vec::with_capacity(params.len());
        for gp in params {
            let mut refs: Vec<BoundRef> = Vec::with_capacity(gp.bounds.len());
            for b in gp.bounds {
                match self.resolve_trait_name(b.name) {
                    Ok(id) => {
                        if !refs.iter().any(|r| r.trait_def == id) {
                            refs.push(BoundRef { trait_def: id, args: Vec::new() });
                        }
                    }
                    Err(found) => {
                        self.errors.add_type_error(TypeError::NotATrait {
                            name: b.name.to_string(), found, span: gp.span,
                        });
                    }
                }
            }
            per_param.push(refs);
        }
        if per_param.iter().any(|b| !b.is_empty()) {
            self.generic_bounds.insert(def, per_param);
        }
    }

    /// Convert the arguments of every written bound and supertrait, now that
    /// the aliases are expanded, and report what is wrong with them once: a
    /// wrong argument count (`GenericArgCountMismatch`), an argument that does
    /// not resolve, and two bounds on one parameter that name the same trait
    /// with different arguments (`ConflictingBounds`).
    fn fill_generic_bound_args<'ast>(&mut self, program: &Program<'ast>) {
        for item in program.items {
            match item {
                Item::Function(f) => {
                    if let Some(def) = self.ctx.top_level_def(f.name) {
                        self.fill_decl_bound_args(def, f.generic_params);
                    }
                }
                Item::Struct(s) => {
                    if let Some(def) = self.ctx.top_level_def(s.name) {
                        self.fill_decl_bound_args(def, s.generic_params);
                    }
                }
                Item::Enum(e) => {
                    if let Some(def) = self.ctx.top_level_def(e.name) {
                        self.fill_decl_bound_args(def, e.generic_params);
                    }
                }
                Item::Trait(t) => self.fill_trait_bound_args(t),
                Item::TypeAlias(a) => {
                    if let Some(def) = self.ctx.top_level_def(a.name) {
                        self.fill_decl_bound_args(def, a.generic_params);
                    }
                }
                _ => {}
            }
        }
    }

    /// The bounds on `def`'s own parameters, with arguments.
    fn fill_decl_bound_args(&mut self, def: DefId, params: &[GenericParam]) {
        if !self.generic_bounds.contains_key(&def) { return; }
        let prev = self.push_generic_scope(params);
        self.fill_param_bound_args(def, params);
        self.pop_generic_scope(prev);
    }

    /// A trait's own parameter bounds (`trait Convert<T: Eq>`) and its
    /// supertraits' arguments (`trait B<T>: A<T>`), written in the trait's
    /// scope where `Self` is the abstract parameter after its own.
    fn fill_trait_bound_args<'ast>(&mut self, t: &TraitDecl<'ast>) {
        let Some(def) = self.ctx.top_level_def(t.name) else { return; };
        let self_param = self.ctx.types.intern(SemaType::Param(t.generic_params.len()));
        let prev_self = self.self_type.replace(self_param);
        let prev = self.push_generic_scope(t.generic_params);
        if self.generic_bounds.contains_key(&def) {
            self.fill_param_bound_args(def, t.generic_params);
        }
        let mut seen: Vec<DefId> = Vec::with_capacity(t.supertraits.len());
        for sup in t.supertraits {
            let Ok(id) = self.resolve_trait_name(sup.name) else { continue; };
            if seen.contains(&id) { continue; }
            seen.push(id);
            let arity = self.trait_arity.get(&id).copied().unwrap_or(sup.args.len());
            self.check_trait_arg_count(sup, arity);
            let args = self.trait_arg_types(sup.args, arity);
            if !args.is_empty() { self.trait_super_args.insert((def, id), args); }
        }
        self.pop_generic_scope(prev);
        self.self_type = prev_self;
    }

    /// Convert and check the written bounds of each parameter of `def`, in
    /// the scope the caller has pushed, and store the arguments.
    fn fill_param_bound_args(&mut self, def: DefId, params: &[GenericParam]) {
        for (i, gp) in params.iter().enumerate() {
            let kept = self.check_bound_list(gp, false);
            if let Some(per_param) = self.generic_bounds.get_mut(&def) {
                if let Some(slot) = per_param.get_mut(i) { *slot = kept; }
            }
        }
    }

    /// Convert and check the bounds written on one generic parameter, in
    /// the scope the caller has pushed: a wrong argument count
    /// (`GenericArgCountMismatch`) and two bounds that name the same trait
    /// with different arguments (`ConflictingBounds`) are reported, and the
    /// bounds that stand are returned. The same trait with the same
    /// arguments twice is one bound. A name that is not a trait is reported
    /// only when `report_names` is set: a declaration's bound names are
    /// reported earlier, by `register_decl_bounds`.
    fn check_bound_list(&mut self, gp: &GenericParam, report_names: bool) -> Vec<BoundRef> {
        let mut kept: Vec<BoundRef> = Vec::with_capacity(gp.bounds.len());
        for b in gp.bounds {
            let id = match self.resolve_trait_name(b.name) {
                Ok(id) => id,
                Err(found) => {
                    if report_names {
                        self.errors.add_type_error(TypeError::NotATrait {
                            name: b.name.to_string(), found, span: gp.span,
                        });
                    }
                    continue;
                }
            };
            let arity = self.trait_arity.get(&id).copied().unwrap_or(b.args.len());
            self.check_trait_arg_count(b, arity);
            let args = self.trait_arg_types(b.args, arity);
            if let Some(first) = kept.iter().find(|k| k.trait_def == id) {
                let first_args = first.args.clone();
                let same = first_args.len() == args.len()
                    && first_args.iter().zip(&args).all(|(x, y)| self.same_type(*x, *y));
                if !same {
                    let name = b.name.to_string();
                    let first_text = self.trait_ref_text(id, &first_args);
                    let this_text  = self.trait_ref_text(id, &args);
                    self.errors.add_type_error(TypeError::ConflictingBounds {
                        param: gp.name.to_string(), trait_name: name,
                        first: first_text, second: this_text, span: b.span,
                    });
                }
                continue;
            }
            kept.push(BoundRef { trait_def: id, args });
        }
        kept
    }

    /// Report a trait written with the wrong number of arguments.
    fn check_trait_arg_count(&mut self, b: &TraitBound, arity: usize) {
        if b.args.len() != arity {
            self.errors.add_type_error(TypeError::GenericArgCountMismatch {
                type_name: b.name.to_string(),
                expected:  arity,
                found:     b.args.len(),
                span:      b.span,
            });
        }
    }

    /// A trait with its arguments as a diagnostic writes it: `Eq`,
    /// `Convert<int>`.
    fn trait_ref_text(&self, trait_def: DefId, args: &[TypeId]) -> String {
        let name = self.trait_display_name(trait_def);
        if args.is_empty() { return name; }
        let shown: Vec<String> = args.iter().map(|a| self.display_type(*a)).collect();
        format!("{}<{}>", name, shown.join(", "))
    }

    /// `ty` with every reference and ownership wrapper removed, down to the
    /// value's own type. A bound is about the value, not where it lives or
    /// who owns it, so a struct reached through `ArenaRef` or `Unique`
    /// satisfies the same bounds as the bare struct (this can only
    /// under-report, never reject a program that is fine).
    fn peel_wrappers(&mut self, ty: TypeId) -> TypeId {
        let mut cur = self.apply(ty);
        loop {
            let inner = match self.ctx.types.get(cur) {
                SemaType::GcRef(inner)
                | SemaType::Unique(inner)
                | SemaType::Shared(inner)
                | SemaType::SyncShared(inner)
                | SemaType::ArenaRef { inner, .. }
                | SemaType::PoolRef { inner, .. }
                | SemaType::OwnedRef { inner, .. }
                | SemaType::Reference { inner, .. } => *inner,
                _ => return cur,
            };
            cur = self.apply(inner);
        }
    }

    /// Whether a type implements a prelude trait, from what the runtime does
    /// with that kind of value today (docs/TRAITS_DESIGN.md, "What S2
    /// settled"). `None` while the type is an unresolved variable.
    ///
    /// - Integers and `string`: all six. `float` and `double` (and `f32`,
    ///   `f64`): `PartialEq`, `PartialOrd` and `Clone`, not `Eq`, `Hash` or
    ///   `Ord`, because of NaN. `bool` and `char`: `PartialEq`, `Eq`, `Hash`
    ///   and `Clone` (`<` is not defined on them, so neither is ordering).
    /// - A tuple or an optional: `PartialEq`, `Eq`, `Hash` and `Clone` when
    ///   its elements have them, never ordering.
    /// - A struct: the traits it derives. An enum: `PartialEq`, `Eq`, `Hash`.
    /// - Every other built-in type: none (`==` on a collection is reference
    ///   identity, so it is not `PartialEq` here).
    fn prelude_status(&mut self, name: &str, ty: TypeId) -> Option<bool> {
        let native = self.prelude_status_native(name, ty);
        if native != Some(false) { return native; }
        // A built-in type that lacks the trait natively has it when an `impl`
        // wrote it (`Hash` for `double`), and may still get one until the
        // impls are registered.
        let ty = self.peel_wrappers(ty);
        if self.runtime_impl_key(ty).is_none() { return native; }
        let written = self.prelude_defs.get(name).copied()
            .is_some_and(|t| self.builtin_implements(ty, t));
        if written { Some(true) }
        else if self.impls_registered { native }
        else { None }
    }

    /// What a type has of a prelude trait without any `impl` of the user's:
    /// the traits a struct derives, the fixed set of a primitive, and so on
    /// (the list above). `prelude_status` adds the hand-written impls.
    fn prelude_status_native(&mut self, name: &str, ty: TypeId) -> Option<bool> {
        let ty = self.peel_wrappers(ty);
        if ty == TypeId::ERROR { return Some(true); }
        let eq_like  = matches!(name, "PartialEq" | "Eq" | "Clone" | "Hash");
        let all_five = true;
        match self.ctx.types.get(ty).clone() {
            SemaType::Unknown => Some(true),
            SemaType::Var(n)  => if self.int_lit_vars.contains_key(&n) { Some(all_five) } else { None },
            SemaType::Int | SemaType::Uint | SemaType::Long | SemaType::Ulong
            | SemaType::I8 | SemaType::I16 | SemaType::I32 | SemaType::I64
            | SemaType::U8 | SemaType::U16 | SemaType::U32 | SemaType::U64
            | SemaType::Isize | SemaType::Usize | SemaType::Str => Some(all_five),
            SemaType::Float | SemaType::Double | SemaType::F32 | SemaType::F64 =>
                Some(matches!(name, "PartialEq" | "PartialOrd" | "Clone")),
            SemaType::Bool | SemaType::Char => Some(eq_like),
            SemaType::Tuple(elems) => {
                if !eq_like { return Some(false); }
                let mut all = Some(true);
                for e in elems {
                    match self.prelude_status(name, e) {
                        Some(false) => return Some(false),
                        None        => all = None,
                        Some(true)  => {}
                    }
                }
                all
            }
            SemaType::Optional(inner) => if eq_like { self.prelude_status(name, inner) } else { Some(false) },
            SemaType::Named { def, args } => {
                let kind = &self.ctx.symbols.lookup(def).kind;
                let by_default = match kind {
                    DefKind::Struct { .. } => self.struct_derives.get(&def).is_some_and(|d| d.contains(name)),
                    DefKind::Enum          => matches!(name, "PartialEq" | "Eq" | "Hash"),
                    _                      => return Some(false),
                };
                // A hand-written impl is the other way to have the trait; an
                // impl for a generic type may ask something of the arguments
                // (`impl<T: PartialEq> PartialEq for Box<T>`).
                let written = match self.prelude_defs.get(name).copied() {
                    Some(t) if self.trait_impls.contains_key(&(t, def)) => self.impl_status(t, def, &args),
                    _ => Some(false),
                };
                if by_default || written == Some(true) { Some(true) }
                else if written.is_none() { None }
                else if self.impls_registered { Some(false) }
                else { None }
            }
            SemaType::Param(_) => Some(self.prelude_defs.get(name)
                .and_then(|d| self.current_bounds.get(&ty)
                    .map(|bs| bs.iter().any(|b| b.trait_def == *d)))
                .unwrap_or(false)),
            _ => Some(false),
        }
    }

    /// Whether `ty` implements `trait_def`: `Some(true)` or `Some(false)`
    /// once that can be told, `None` while it cannot (an unresolved
    /// inference variable, or a named type before impls are registered).
    /// An unknown or error type counts as implementing it, so an earlier
    /// error does not cascade into this one.
    fn bound_status(&mut self, ty: TypeId, trait_def: DefId) -> Option<bool> {
        let ty = self.peel_wrappers(ty);
        if ty == TypeId::ERROR { return Some(true); }
        if let Some(name) = self.prelude_traits.get(&trait_def).copied() {
            return self.prelude_status(name, ty);
        }
        match self.ctx.types.get(ty).clone() {
            SemaType::Unknown => Some(true),
            SemaType::Var(_)  => None,
            SemaType::Param(_) => Some(self.current_bounds.get(&ty)
                .is_some_and(|bs| bs.iter().any(|b| b.trait_def == trait_def))),
            _ if self.runtime_impl_key(ty).is_some() => {
                // A primitive, a tuple, an array or a collection has the trait
                // when an `impl` says so, which may not be registered yet.
                if self.builtin_implements(ty, trait_def) {
                    match self.builtin_def_of(ty) {
                        Some(def) => {
                            let args = self.builtin_head_args(ty);
                            self.impl_status(trait_def, def, &args)
                        }
                        None => Some(true),
                    }
                }
                else if self.impls_registered { Some(false) }
                else { None }
            }
            SemaType::Named { def, args } => {
                if self.trait_impls.contains_key(&(trait_def, def)) {
                    self.impl_status(trait_def, def, &args)
                }
                else if self.impls_registered { Some(false) }
                else { None }
            }
            // Built-in types have no impls until slice S2.
            _ => Some(false),
        }
    }

    /// Record that `ty` must implement `trait_def`. While signatures are
    /// being collected the generic scope in force is the one the type was
    /// written in, so what can be decided is decided now and the rest waits
    /// for the impls to be registered. In a body everything waits for the
    /// end of the body, where the diagnostics come out in source order.
    fn require_bound(&mut self, ty: TypeId, trait_def: DefId, span: Span) {
        self.require_bound_with(ty, trait_def, span, false);
    }

    fn require_bound_with(&mut self, ty: TypeId, trait_def: DefId, span: Span, dict_key: bool) {
        let mut failed = false;
        if !self.impls_registered {
            match self.bound_status(ty, trait_def) {
                Some(true)  => return,
                Some(false) => failed = true,
                None        => {}
            }
        }
        self.pending_obligations.push(Obligation { ty, trait_def, args: Vec::new(), span, dict_key, failed });
    }

    /// Record that `ty` must implement `trait_def` applied to `args`
    /// (`T: Convert<int>` after substitution). Besides the impl existing, the
    /// impl's own arguments must be these; a type has one impl of a trait, so
    /// that is a comparison, and where an argument is still an inference
    /// variable the impl decides it (`f(celsius)` for `fn f<T: Convert<U>, U>`
    /// learns `U` from `impl Convert<int> for Celsius`).
    fn require_bound_args(&mut self, ty: TypeId, trait_def: DefId, args: Vec<TypeId>, span: Span) {
        if args.is_empty() {
            self.require_bound(ty, trait_def, span);
            return;
        }
        let mut failed = false;
        let status = self.bound_args_status(ty, trait_def, &args, span);
        if !self.impls_registered {
            match status {
                Some(true)  => return,
                Some(false) => failed = true,
                None        => {}
            }
        }
        self.pending_obligations.push(Obligation { ty, trait_def, args, span, dict_key: false, failed });
    }

    /// `bound_status` for a trait applied to arguments: the trait must be
    /// implemented (or bounded) and the arguments must agree with the ones the
    /// impl (or the bound in scope) has. Agreeing binds inference variables
    /// in `args`. A trait the type satisfies without recording arguments
    /// (a prelude trait) has nothing to compare.
    fn bound_args_status(&mut self, ty: TypeId, trait_def: DefId, args: &[TypeId], span: Span) -> Option<bool> {
        match self.bound_status(ty, trait_def) {
            Some(true) => {}
            other      => return other,
        }
        if args.is_empty() { return Some(true); }
        let ty = self.peel_wrappers(ty);
        let have: Vec<TypeId> = match self.ctx.types.get(ty).clone() {
            // An impl for a generic head writes its trait arguments in the
            // head's numbering, so the receiver's own arguments go in.
            SemaType::Named { def, args: recv } => match self.trait_impl_args.get(&(trait_def, def)).cloned() {
                Some(a) => a.iter().map(|t| self.substitute(*t, &recv)).collect(),
                None    => return Some(true),
            },
            SemaType::Param(_) => match self.current_bounds.get(&ty)
                .and_then(|bs| bs.iter().find(|b| b.trait_def == trait_def)) {
                Some(b) => b.args.clone(),
                None    => return Some(true),
            },
            _ => {
                let recv = self.builtin_head_args(ty);
                match self.builtin_def_of(ty).and_then(|d| self.trait_impl_args.get(&(trait_def, d)).cloned()) {
                    Some(a) => a.iter().map(|t| self.substitute(*t, &recv)).collect(),
                    None    => return Some(true),
                }
            }
        };
        if have.len() != args.len() { return Some(true); }
        let mut agree = true;
        for (want, got) in args.iter().zip(have.iter()) {
            if !self.types_agree(*want, *got, span) { agree = false; }
        }
        Some(agree)
    }

    /// Unify two types without reporting: whether they can be the same, with
    /// inference variables bound along the way. A mismatch leaves no
    /// diagnostic behind (the caller says what is wrong in its own words).
    fn types_agree(&mut self, a: TypeId, b: TypeId, span: Span) -> bool {
        let before = self.errors.type_error_count();
        self.unify(a, b, span);
        if self.errors.type_error_count() > before {
            self.discard_type_errors_since(before);
            return false;
        }
        true
    }

    fn report_unsatisfied(&mut self, ty: TypeId, trait_def: DefId, args: &[TypeId], span: Span, dict_key: bool) {
        if !self.reported_obligations.insert((span, trait_def)) { return; }
        let ty = self.peel_wrappers(ty);
        if dict_key && !self.reported_dict_keys.insert((span.line, ty)) { return; }
        let applied: Vec<TypeId> = args.iter().map(|a| self.deep_apply(*a)).collect();
        let trait_name = self.trait_ref_text(trait_def, &applied);
        let is_param = matches!(self.ctx.types.get(ty), SemaType::Param(_));
        let type_name = if is_param {
            self.param_display_name(ty)
        } else {
            let shown = self.deep_apply(ty);
            self.display_type(shown)
        };
        self.errors.add_type_error(TypeError::UnsatisfiedBound {
            type_name, trait_name, is_param, span,
        });
    }

    /// Settle every pending obligation. Called at the end of a body (after
    /// literal types are settled, while the body's own bounds are still in
    /// scope) and once more at the very end. One whose type is still
    /// unresolved is dropped: nothing says it is wrong.
    fn check_obligations(&mut self) {
        let mut pending = std::mem::take(&mut self.pending_obligations);
        pending.sort_by_key(|o| o.span.start);
        for o in pending {
            let status = if o.args.is_empty() {
                self.bound_status(o.ty, o.trait_def)
            } else {
                self.bound_args_status(o.ty, o.trait_def, &o.args, o.span)
            };
            if o.failed || status == Some(false) {
                self.report_unsatisfied(o.ty, o.trait_def, &o.args, o.span, o.dict_key);
            }
        }
    }

    /// The bounds of a generic struct or enum `def` applied to its
    /// arguments `args` (one obligation per bounded parameter).
    fn require_def_bounds(&mut self, def: DefId, args: &[TypeId], span: Span) {
        let Some(per_param) = self.generic_bounds.get(&def).cloned() else { return; };
        for (i, bounds) in per_param.iter().enumerate() {
            let Some(&arg) = args.get(i) else { break; };
            for b in bounds {
                let applied: Vec<TypeId> = b.args.iter().map(|t| self.substitute(*t, args)).collect();
                self.require_bound_args(arg, b.trait_def, applied, span);
            }
        }
    }

    fn method_shape_of_fn_type(&self, has_self: bool, fn_ty: TypeId, generics: MethodGenerics) -> Option<MethodShape> {
        match self.ctx.types.get(fn_ty).clone() {
            SemaType::Function { params, return_type, is_fallible, .. } =>
                Some(MethodShape { has_self, params, return_type, is_fallible, generics, guard: None, tier: None }),
            _ => None,
        }
    }

    /// The generic parameters recorded for the method declared at `span`
    /// (none when it declares none).
    fn generics_at(&self, span: Span) -> MethodGenerics {
        self.ctx.resolutions.get(span)
            .and_then(|d| self.method_generics.get(&d))
            .cloned()
            .unwrap_or_default()
    }

    /// The shape of a required method signature (no body), with `Self`
    /// already resolved to the trait's abstract `Self` parameter.
    fn shape_of_sig<'ast>(&mut self, sig: &MethodSig<'ast>, base: usize) -> MethodShape {
        let scope = self.push_own_generic_scope(sig.generic_params, base);
        let params: Vec<TypeId> = sig.params.iter()
            .filter_map(|p| match p.kind {
                ParamKind::Named { ty, .. } | ParamKind::Discard { ty } =>
                    ty.map(|t| self.ast_type_to_sema(t)),
                _ => None,
            })
            .collect();
        let (return_type, is_fallible) = self.return_type_to_sema(sig.return_type.as_ref());
        let generics = self.method_generics_of(sig.generic_params, base);
        self.pop_own_generic_scope(scope);
        MethodShape { has_self: has_self_param(sig.params), params, return_type, is_fallible, generics, guard: None, tier: Some(sig.tier) }
    }

    /// Collect a trait: its required signatures and its default methods,
    /// all written in terms of the abstract `Self`.
    fn collect_trait_info<'ast>(&mut self, t: &TraitDecl<'ast>) {
        let Some(def_id) = self.ctx.top_level_def(t.name) else { return; };
        let arity = t.generic_params.len();
        let self_param = self.ctx.types.intern(SemaType::Param(arity));
        let prev_self = self.self_type.replace(self_param);
        let prev_generics = self.push_generic_scope(t.generic_params);
        // Inside the trait `Self` implements the trait applied to its own
        // parameters (`Convert<T>` for `trait Convert<T>`).
        let own_args: Vec<TypeId> = (0..arity).map(|i| self.ctx.types.intern(SemaType::Param(i))).collect();
        self.current_bounds.insert(self_param, vec![BoundRef { trait_def: def_id, args: own_args }]);

        let mut methods: Vec<TraitMethodInfo> = Vec::with_capacity(t.items.len());
        for it in t.items {
            match it {
                TraitItem::MethodSig(sig) => {
                    let shape = self.shape_of_sig(sig, arity + 1);
                    methods.push(TraitMethodInfo {
                        name: sig.name.to_string(), shape, has_default: false,
                    });
                }
                TraitItem::DefaultMethod(m) => {
                    let (fn_ty, generics) = self.collect_method_sig(m, arity + 1);
                    if let Some(mut shape) = self.method_shape_of_fn_type(has_self_param(m.params), fn_ty, generics) {
                        shape.tier = Some(m.tier);
                        methods.push(TraitMethodInfo {
                            name: m.name.to_string(), shape, has_default: true,
                        });
                    }
                }
                TraitItem::AssociatedType { span, .. } => {
                    self.errors.add_type_error(TypeError::UnsupportedTraitFeature {
                        feature: "an associated type",
                        span:    *span,
                    });
                }
            }
        }

        self.pop_generic_scope(prev_generics);
        self.self_type = prev_self;
        self.traits.insert(def_id, TraitInfo { name: t.name.to_string(), arity, methods });
    }

    /// A trait's default method bodies. Inside one, `Self` is the trait's
    /// abstract `Self` parameter and `self` has that type, bounded by the
    /// trait itself and by the traits it builds on, so `self.other_method()`
    /// resolves through the trait or one of its supertraits.
    fn infer_trait_default_bodies<'ast>(&mut self, t: &TraitDecl<'ast>) {
        let Some(trait_def) = self.ctx.top_level_def(t.name) else { return; };
        let arity = t.generic_params.len();
        let self_param = self.ctx.types.intern(SemaType::Param(arity));
        let prev_self_type = self.self_type.replace(self_param);
        let prev_struct = self.current_struct_type.replace(self_param);
        let prev_generics = self.push_generic_scope(t.generic_params);
        let own_args: Vec<TypeId> = (0..arity).map(|i| self.ctx.types.intern(SemaType::Param(i))).collect();
        let self_bounds = self.expand_supertraits(
            vec![BoundRef { trait_def, args: own_args }], self_param);
        self.current_bounds.insert(self_param, self_bounds);
        for it in t.items {
            if let TraitItem::DefaultMethod(m) = it {
                self.infer_method_body(m, arity + 1);
            }
        }
        self.pop_generic_scope(prev_generics);
        self.current_struct_type = prev_struct;
        self.self_type = prev_self_type;
    }

    /// A trait method's shape for one implementing type: the trait's own
    /// parameters replaced by `trait_args` (`[int]` for `Convert<int>`) and
    /// the abstract `Self` by `self_ty`. A trait without parameters passes no
    /// arguments.
    fn instantiate_trait_shape(&mut self, shape: &MethodShape, self_ty: TypeId, trait_args: &[TypeId]) -> MethodShape {
        let mut env: Vec<TypeId> = trait_args.to_vec();
        env.push(self_ty);
        // The method's own parameters follow the trait's parameters and
        // `Self`. Once those are replaced the own ones are renumbered from
        // zero, which is where an implementing type's method (or a call)
        // expects them.
        if !shape.generics.own.is_empty() {
            // Every trait argument list has the trait's arity (a count
            // mistake is reported where the trait is named and the list is
            // made to fit), so `Self` is exactly at the method's base.
            debug_assert_eq!(env.len(), shape.generics.base);
            for j in 0..shape.generics.own.len() {
                env.push(self.ctx.types.intern(SemaType::Param(j)));
            }
        }
        let params: Vec<TypeId> = shape.params.iter()
            .map(|p| self.substitute(*p, &env))
            .collect();
        let return_type = self.substitute(shape.return_type, &env);
        let own: Vec<OwnParam> = shape.generics.own.iter()
            .map(|o| OwnParam {
                name:   o.name.clone(),
                bounds: o.bounds.iter()
                    .map(|b| BoundRef {
                        trait_def: b.trait_def,
                        args:      b.args.iter().map(|a| self.substitute(*a, &env)).collect(),
                    })
                    .collect(),
            })
            .collect();
        MethodShape {
            has_self: shape.has_self, params, return_type, is_fallible: shape.is_fallible,
            generics: MethodGenerics { base: 0, own },
            guard: None,
            tier: shape.tier,
        }
    }

    /// Two types are the same when they print the same; an unknown on
    /// either side matches anything so one earlier error does not cascade.
    fn same_type(&self, a: TypeId, b: TypeId) -> bool {
        let (sa, sb) = (self.display_type(a), self.display_type(b));
        sa == sb || [&sa, &sb].iter().any(|s| s.contains("<unknown>") || s.contains("<e>"))
    }

    /// Two signatures are the same when everything in them is: the shape of
    /// the parameters and the result and, for a generic method, how many
    /// parameters it declares of its own and the bounds on each. An impl
    /// repeats its trait's method exactly (the own parameters are compared by
    /// position, so their names may differ).
    fn shapes_match(&self, a: &MethodShape, b: &MethodShape) -> bool {
        a.has_self == b.has_self
            && a.is_fallible == b.is_fallible
            && a.params.len() == b.params.len()
            && a.params.iter().zip(&b.params).all(|(x, y)| self.same_type(*x, *y))
            && self.same_type(a.return_type, b.return_type)
            && a.generics.own.len() == b.generics.own.len()
            && a.generics.own.iter().zip(&b.generics.own)
                .all(|(x, y)| self.same_bounds(&x.bounds, &y.bounds))
    }

    /// The same set of bounds, in any order: each trait once, with the same
    /// arguments.
    fn same_bounds(&self, a: &[BoundRef], b: &[BoundRef]) -> bool {
        a.len() == b.len()
            && a.iter().all(|x| b.iter().any(|y| {
                x.trait_def == y.trait_def
                    && x.args.len() == y.args.len()
                    && x.args.iter().zip(&y.args).all(|(p, q)| self.same_type(*p, *q))
            }))
    }

    /// A signature as written in a diagnostic: `fn(self, int) string`, with
    /// the method's own parameters ahead of the list when it has any:
    /// `fn<U: Show>(self, U) U`.
    fn shape_text(&self, s: &MethodShape) -> String {
        let show = |ty: TypeId| -> String {
            let mut text = self.display_type(ty);
            for (j, o) in s.generics.own.iter().enumerate() {
                text = text.replace(&format!("<generic param #{}>", s.generics.base + j), &o.name);
            }
            text
        };
        let mut parts: Vec<String> = Vec::with_capacity(s.params.len() + 1);
        if s.has_self { parts.push("self".to_string()); }
        parts.extend(s.params.iter().map(|p| show(*p)));
        let fallible = if s.is_fallible { " (fallible)" } else { "" };
        let header = if s.generics.own.is_empty() {
            String::new()
        } else {
            let own: Vec<String> = s.generics.own.iter().map(|o| {
                if o.bounds.is_empty() { return o.name.clone(); }
                let bs: Vec<String> = o.bounds.iter()
                    .map(|b| self.trait_ref_text(b.trait_def, &b.args))
                    .collect();
                format!("{}: {}", o.name, bs.join(" + "))
            }).collect();
            format!("<{}>", own.join(", "))
        };
        format!("fn{}({}) {}{}", header, parts.join(", "), show(s.return_type), fallible)
    }

    /// Put one trait-supplied method into `target`'s dispatch table. An
    /// inherent method of the same name wins and the trait method is then
    /// reachable only as `Trait.method(value)`. Two traits supplying the
    /// same name keep the first in the table and mark the name ambiguous.
    fn add_trait_method(&mut self, target: DefId, trait_def: DefId, name: &str, shape: MethodShape) {
        let key = (target, name.to_string());
        let in_table = self.struct_methods.get(&target)
            .is_some_and(|ms| ms.iter().any(|(n, _)| n == name));
        // In the table but not put there by a trait: an inherent method.
        let inherent = in_table && !self.trait_entries.contains(&key);
        let origins = self.method_origins.entry(key.clone()).or_default();
        origins.push(trait_def);
        if inherent { return; }
        if origins.len() > 1 {
            self.ambiguous_methods.insert(key);
            return;
        }
        self.trait_entries.insert(key);
        self.struct_methods.entry(target).or_default().push((name.to_string(), shape));
    }

    /// Check every `impl Trait for Type` against its trait and register its
    /// methods (and the trait's default methods it does not override) into
    /// the type's dispatch table. Runs after the inherent registration so an
    /// inherent method is already there to win.
    fn register_trait_impls<'ast>(&mut self, program: &Program<'ast>) {
        for item in program.items {
            let Item::Impl(i) = item else { continue; };
            if i.trait_path.is_none() || self.impl_failed.contains(&i.span) { continue; }
            // A block for a generic head or with parameters of its own is
            // checked in its own scope: its parameters, bounds and `Self`.
            let pattern = self.impl_patterns.get(&i.span).cloned();
            let scope = pattern.as_ref().map(|p| self.enter_impl_scope(p));
            self.register_one_trait_impl(i, pattern.as_deref());
            if let Some(scope) = scope { self.leave_impl_scope(scope); }
        }
    }

    /// One `impl Trait for Type` block: checked against its trait, and its
    /// methods (with the trait's defaults it does not override) put in the
    /// type's dispatch table. `pattern` is the analysis of a block for a
    /// generic head, whose scope is already in force.
    fn register_one_trait_impl<'ast>(&mut self, i: &ImplBlock<'ast>, pattern: Option<&ImplPattern>) {
        let Some(path) = i.trait_path else { return; };
        let trait_name = path.last().copied().unwrap_or("");

        let trait_def = match self.resolve_trait_name(trait_name) {
            Ok(id) => id,
            Err(found) => {
                self.errors.add_type_error(TypeError::NotATrait {
                    name: trait_name.to_string(), found, span: i.span,
                });
                return;
            }
        };
        // `@derive(X)` and `impl X for T` are two sources of the same impl.
        if let Some(pname) = self.prelude_traits.get(&trait_def).copied() {
            if let Some(td) = self.target_type_def_id(i.target_type) {
                if self.struct_derives.get(&td).is_some_and(|d| d.contains(pname)) {
                    let type_name = match pattern {
                        Some(p) => self.display_type(p.self_ty),
                        None    => self.ctx.symbols.lookup(td).name.clone(),
                    };
                    self.errors.add_type_error(TypeError::OverlappingImpl {
                        trait_name: pname.to_string(), type_name, note: None, span: i.span,
                    });
                    return;
                }
            }
        }
        // A collection is compared, hashed and cloned by identity at run
        // time, so a hand-written prelude impl for one would say something
        // the operators do not do.
        if pattern.is_some_and(|p| p.builtin) && self.prelude_traits.contains_key(&trait_def) {
            self.report_unsupported(impl_patterns::PRELUDE_ON_COLLECTION, i.span);
            return;
        }
        let builtin = match pattern {
            Some(p) if p.builtin => Some((p.head, p.self_ty)),
            Some(_)              => None,
            None                 => self.builtin_target(i.target_type),
        };
        let target_def = match (pattern, builtin) {
            (Some(p), _)       => Some(p.head),
            (None, Some((d, _))) => Some(d),
            (None, None) => self.target_type_def_id(i.target_type).filter(|d|
                matches!(self.ctx.symbols.lookup(*d).kind, DefKind::Struct { .. } | DefKind::Enum)),
        };
        let Some(target_def) = target_def else {
            self.report_unsupported(impl_patterns::TARGET_FOR_TRAIT, i.span);
            return;
        };
        let Some(info) = self.traits.get(&trait_def).cloned() else { return; };
        // The target as a diagnostic writes it: `Box<T>`, `List<int>`.
        let type_name = match pattern {
            Some(p) => self.display_type(p.self_ty),
            None    => self.ctx.symbols.lookup(target_def).name.clone(),
        };
        let written = TraitBound { name: trait_name, args: i.trait_args, span: i.span };
        self.check_trait_arg_count(&written, info.arity);
        let impl_args = self.trait_arg_types(i.trait_args, info.arity);

        if self.trait_impls.contains_key(&(trait_def, target_def)) {
            // Generics are erased at run time, so one impl covers every
            // `Box<..>`, and a second one is the same mistake as ever.
            let note = pattern.map(|_| "generics are erased at run time, so one impl covers every instance of the type".to_string());
            self.errors.add_type_error(TypeError::OverlappingImpl {
                trait_name: info.name.clone(), type_name, note, span: i.span,
            });
            return;
        }
        if let Some((_, builtin_ty)) = builtin.filter(|_| pattern.is_none()) {
            // A prelude trait the type already has natively cannot be
            // implemented again; one it lacks (`Hash` for `double`) can.
            if let Some(pname) = self.prelude_traits.get(&trait_def).copied() {
                if self.prelude_status_native(pname, builtin_ty) == Some(true) {
                    self.errors.add_type_error(TypeError::OverlappingImpl {
                        trait_name: info.name.clone(),
                        type_name,
                        note: Some("the built-in type already implements it".to_string()),
                        span: i.span,
                    });
                    return;
                }
            }
        }
        // Types that are one kind of value at run time cannot be told apart
        // by a call, so a trait has one impl for all of them.
        if let Some((_, builtin_ty)) = builtin {
            if let Some(key) = self.runtime_impl_key(builtin_ty) {
                if let Some(first) = self.builtin_key_impls.get(&(key.clone(), trait_def)).copied() {
                    let first_name = self.ctx.symbols.lookup(first).name.clone();
                    let kind = if key.starts_with("tuple") {
                        "tuples with the same number of elements"
                    } else if key == "List" {
                        let arrays = [first, target_def].iter().all(|d|
                            self.builtin_def_types.get(d).is_some_and(|t|
                                matches!(self.ctx.types.get(*t), SemaType::Array { .. })));
                        if arrays {
                            "arrays of every length and element type"
                        } else {
                            "arrays and lists of every length and element type"
                        }
                    } else {
                        "types of the same kind"
                    };
                    self.errors.add_type_error(TypeError::OverlappingImpl {
                        trait_name: info.name.clone(),
                        type_name: type_name.clone(),
                        note: Some(format!(
                            "`{}` and `{}` are one kind of value at run time, and a call cannot tell {} apart",
                            first_name, self.ctx.symbols.lookup(target_def).name, kind)),
                        span: i.span,
                    });
                    return;
                }
                self.builtin_key_impls.insert((key, trait_def), target_def);
            }
        }
        self.trait_impls.insert((trait_def, target_def), i.span);
        self.trait_impl_args.insert((trait_def, target_def), impl_args.clone());
        if let Some(g) = pattern.and_then(|p| p.guard.clone()) {
            self.trait_impl_guards.insert((trait_def, target_def), g);
        }
        self.require_def_bounds(trait_def, &impl_args, i.span);
        let self_ty = match (pattern, builtin) {
            (Some(p), _)        => p.self_ty,
            (None, Some((_, t))) => t,
            (None, None) => self.ctx.types.insert(SemaType::Named { def: target_def, args: Vec::new() }),
        };
        let guard = pattern.and_then(|p| p.guard.clone());

        for tm in &info.methods {
            let impl_m = i.methods.iter().find(|m| m.name == tm.name.as_str());
            let mut expected = self.instantiate_trait_shape(&tm.shape, self_ty, &impl_args);
            match impl_m {
                Some(m) => {
                    let fn_ty = self.ctx.resolutions.get(m.span)
                        .and_then(|d| self.ctx.def_type(d));
                    let found = fn_ty.and_then(|t|
                        self.method_shape_of_fn_type(has_self_param(m.params), t, self.generics_at(m.span)));
                    if let Some(found) = found {
                        if !self.shapes_match(&expected, &found) {
                            self.errors.add_type_error(TypeError::TraitMethodSignatureMismatch {
                                trait_name: info.name.clone(),
                                method:     tm.name.clone(),
                                expected:   self.shape_text(&expected),
                                found:      self.shape_text(&found),
                                span:       m.span,
                            });
                        }
                    }
                    expected.guard = guard.clone();
                    let trait_tier = tm.shape.tier.unwrap_or(TierAnnotation::High);
                    // A tier written for the method (on it, on its `impl`, or on
                    // a tier block around that) has to equal the trait's. One
                    // that is not written is the trait's.
                    // (A tier block around the impl merges its own `@tier` into the
                    // impl's attributes, so `writes_tier` covers that case too.)
                    let written = writes_tier(m.attributes) || writes_tier(i.attributes);
                    let impl_tier = if written { m.tier } else { trait_tier };
                    if !written {
                        self.ctx.method_tiers.insert(m.span, trait_tier);
                    } else if impl_tier != trait_tier {
                        self.errors.add_tier_error(TierError::TraitMethodTierMismatch {
                            trait_name: info.name.clone(),
                            type_name:  type_name.clone(),
                            method:     tm.name.clone(),
                            impl_tier,
                            trait_tier,
                            span:       m.span,
                        });
                    }
                    expected.tier = Some(impl_tier);
                    self.add_trait_method(target_def, trait_def, &tm.name, expected);
                }
                None if tm.has_default => {
                    expected.guard = guard.clone();
                    self.add_trait_method(target_def, trait_def, &tm.name, expected);
                }
                None => {
                    self.errors.add_type_error(TypeError::TraitMethodMissing {
                        trait_name: info.name.clone(),
                        method:     tm.name.clone(),
                        type_name:  type_name.clone(),
                        span:       i.span,
                    });
                }
            }
        }
        for m in i.methods {
            if !info.methods.iter().any(|tm| tm.name == m.name) {
                self.errors.add_type_error(TypeError::UnknownTraitMethod {
                    trait_name: info.name.clone(),
                    method:     m.name.to_string(),
                    span:       m.span,
                });
            }
        }
    }

    /// The cross-tier rule for a call to a method (`TIER-005`): the caller is
    /// the function whose body is being inferred. A method without a tier to
    /// enforce (`MethodShape::tier` is `None`) is never reported.
    fn check_method_tier(&mut self, m: &MethodShape, name: &str, span: Span) {
        let Some(callee) = m.tier else { return; };
        if tier_check::tier_call_forbidden(self.current_tier, callee) {
            self.errors.add_tier_error(TierError::IllegalTierCall {
                caller_tier: self.current_tier,
                callee_tier: callee,
                callee_name: name.to_string(),
                span,
            });
        }
    }

    /// A method's signature for one call. The receiver's type arguments
    /// (`recv_args`, as many as the declaration around the method has) come
    /// first in `Param` order, then a fresh inference variable for each
    /// parameter the method declares itself, so every call chooses its own.
    fn instantiate_method(&mut self, m: &MethodShape, recv_args: &[TypeId]) -> MethodInst {
        let mut env: Vec<TypeId> = recv_args.to_vec();
        if !m.generics.own.is_empty() {
            while env.len() < m.generics.base { env.push(self.unknown()); }
            env.truncate(m.generics.base);
            for _ in 0..m.generics.own.len() { env.push(self.fresh_var()); }
        }
        let params: Vec<TypeId> = m.params.iter().map(|p| self.substitute(*p, &env)).collect();
        let return_type = self.substitute(m.return_type, &env);
        MethodInst { params, return_type, env }
    }

    /// The bounds a method puts on its own generic parameters, as one
    /// obligation each for this call. Reported at the first argument whose
    /// declared type is exactly that parameter, at the call itself when
    /// there is none, as for a generic function. `args` are the call's
    /// arguments without the receiver. Called after the arguments are
    /// unified, so a parameter they decide is known.
    fn require_method_bounds<'ast>(&mut self, m: &MethodShape, inst: &MethodInst, args: &[Arg<'ast>], call_span: Span) {
        for (j, own) in m.generics.own.iter().enumerate() {
            let idx = m.generics.base + j;
            let Some(&fresh) = inst.env.get(idx) else { continue; };
            let at = m.params.iter()
                .position(|p| matches!(self.ctx.types.get(*p), SemaType::Param(k) if *k == idx))
                .and_then(|pos| args.get(pos))
                .map(|a| match &a.kind {
                    ArgKind::Positional(e)       => e.span,
                    ArgKind::Named { value, .. } => value.span,
                })
                .unwrap_or(call_span);
            for b in &own.bounds {
                let applied: Vec<TypeId> = b.args.iter()
                    .map(|t| self.substitute(*t, &inst.env))
                    .collect();
                self.require_bound_args(fresh, b.trait_def, applied, at);
            }
        }
    }

    /// Unify a call's arguments with a method's parameter types and report
    /// a wrong argument count. `args` are already inferred.
    fn check_method_args<'ast>(&mut self, params: &[TypeId], args: &[Arg<'ast>], call_span: Span) {
        if params.len() != args.len() {
            self.errors.add_type_error(TypeError::ArgumentCountMismatch {
                expected: params.len(),
                found:    args.len(),
                span:     call_span,
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
    }

    /// A type parameter as a diagnostic names it: its written name (`T`),
    /// or `Self` for a trait's abstract `Self`.
    fn param_display_name(&self, ty: TypeId) -> String {
        if self.self_type == Some(ty) { return "Self".to_string(); }
        self.current_generic_params.iter()
            .find(|(_, id)| **id == ty)
            .map(|(name, _)| name.clone())
            .or_else(|| self.param_names.get(&ty).cloned())
            .unwrap_or_else(|| self.display_type(ty))
    }

    /// `x.method(..)` where `x` has a type-parameter type bounded by
    /// traits (inside a trait default body, the abstract `Self` bounded by
    /// the trait itself): the method must come from one of the bounds.
    fn call_through_bounds<'ast>(
        &mut self,
        receiver_ty: TypeId,
        bounds:      &[BoundRef],
        method:      &str,
        args:        &[Arg<'ast>],
        callee_span: Span,
        call_span:   Span,
    ) -> TypeId {
        let mut found: Vec<(String, MethodShape, Vec<TypeId>)> = Vec::new();
        for b in bounds {
            if let Some(info) = self.traits.get(&b.trait_def) {
                if let Some(m) = info.methods.iter().find(|m| m.name == method && m.shape.has_self) {
                    found.push((info.name.clone(), m.shape.clone(), b.args.clone()));
                }
            }
        }
        match found.len() {
            0 => {
                self.errors.add_type_error(TypeError::NoSuchMethod {
                    method:  method.to_string(),
                    on_type: self.param_display_name(receiver_ty),
                    span:    callee_span,
                });
                self.unknown()
            }
            1 => {
                let shape = self.instantiate_trait_shape(&found[0].1, receiver_ty, &found[0].2);
                self.check_method_tier(&shape, method, callee_span);
                let inst = self.instantiate_method(&shape, &[]);
                self.check_method_args(&inst.params, args, call_span);
                self.require_method_bounds(&shape, &inst, args, call_span);
                // The interpreter must run THIS trait's method for this call,
                // not an inherent method the concrete type may also have.
                self.ctx.trait_call_sites.insert(callee_span, found[0].0.clone());
                inst.return_type
            }
            _ => {
                self.errors.add_type_error(TypeError::AmbiguousTraitMethod {
                    method:    method.to_string(),
                    traits:    found.into_iter().map(|(n, _, _)| n).collect(),
                    type_name: self.param_display_name(receiver_ty),
                    span:      callee_span,
                });
                self.unknown()
            }
        }
    }

    /// `Trait.method(value, args..)`: the explicit form, which names the
    /// trait and so never has to choose between two traits supplying the
    /// same method. The first argument is the receiver and its type must
    /// implement the trait.
    fn infer_qualified_trait_call<'ast>(
        &mut self,
        trait_def: DefId,
        method:    &str,
        args:      &[Arg<'ast>],
        span:      Span,
    ) -> TypeId {
        let Some(info) = self.traits.get(&trait_def).cloned() else { return self.unknown(); };
        let Some(tm) = info.methods.iter().find(|m| m.name == method && m.shape.has_self) else {
            self.errors.add_type_error(TypeError::NoSuchMethod {
                method:  method.to_string(),
                on_type: format!("trait {}", info.name),
                span,
            });
            return self.unknown();
        };
        let Some(first) = args.first() else {
            self.errors.add_type_error(TypeError::ArgumentCountMismatch {
                expected: tm.shape.params.len() + 1,
                found:    0,
                span,
            });
            return self.unknown();
        };
        let first_span = match &first.kind {
            ArgKind::Positional(e)       => e.span,
            ArgKind::Named { value, .. } => value.span,
        };
        let Some(recv_ty) = self.ctx.expr_type(first_span).map(|t| self.apply(t)) else {
            return self.unknown();
        };
        if !self.type_implements(recv_ty, trait_def) {
            let shown = self.deep_apply(recv_ty);
            self.errors.add_type_error(TypeError::UnsatisfiedBound {
                type_name:  self.display_type(shown),
                trait_name: info.name.clone(),
                is_param:   matches!(self.ctx.types.get(recv_ty), SemaType::Param(_)),
                span:       first_span,
            });
            return self.unknown();
        }
        let trait_args = self.trait_args_of(recv_ty, trait_def);
        let shape = self.instantiate_trait_shape(&tm.shape.clone(), recv_ty, &trait_args);
        self.check_method_tier(&shape, method, span);
        let inst = self.instantiate_method(&shape, &[]);
        self.check_method_args(&inst.params, &args[1..], span);
        self.require_method_bounds(&shape, &inst, &args[1..], span);
        inst.return_type
    }

    /// The arguments `ty` has for `trait_def`: the impl's, for a user type,
    /// or the bound's, for a type parameter. Unknown where nothing records
    /// them (so a call is checked no further than the types allow).
    fn trait_args_of(&mut self, ty: TypeId, trait_def: DefId) -> Vec<TypeId> {
        let arity = self.trait_arity.get(&trait_def).copied().unwrap_or(0);
        if arity == 0 { return Vec::new(); }
        let ty = self.peel_wrappers(ty);
        let known: Option<Vec<TypeId>> = match self.ctx.types.get(ty).clone() {
            SemaType::Named { def, args: recv } => self.trait_impl_args.get(&(trait_def, def)).cloned()
                .map(|a| a.iter().map(|t| self.substitute(*t, &recv)).collect()),
            SemaType::Param(_) => self.current_bounds.get(&ty)
                .and_then(|bs| bs.iter().find(|b| b.trait_def == trait_def))
                .map(|b| b.args.clone()),
            _ => {
                let recv = self.builtin_head_args(ty);
                self.builtin_def_of(ty)
                    .and_then(|d| self.trait_impl_args.get(&(trait_def, d)).cloned())
                    .map(|a| a.iter().map(|t| self.substitute(*t, &recv)).collect())
            }
        };
        match known {
            Some(a) if a.len() == arity => a,
            _ => (0..arity).map(|_| self.unknown()).collect(),
        }
    }

    /// Whether `ty` implements `trait_def`: a user struct or enum with an
    /// `impl`, or a type parameter whose bounds include the trait. Anything
    /// unresolved or unknown counts as implementing it, so an earlier error
    /// does not cascade into this one.
    fn type_implements(&mut self, ty: TypeId, trait_def: DefId) -> bool {
        if ty == TypeId::ERROR { return true; }
        if let Some(name) = self.prelude_traits.get(&trait_def).copied() {
            return self.prelude_status(name, ty).unwrap_or(true);
        }
        match self.ctx.types.get(ty).clone() {
            SemaType::Named { def, args } => self.trait_impls.contains_key(&(trait_def, def))
                && self.impl_status(trait_def, def, &args) != Some(false),
            SemaType::Param(_) => self.current_bounds.get(&ty)
                .is_some_and(|bs| bs.iter().any(|b| b.trait_def == trait_def)),
            SemaType::Unknown | SemaType::Var(_) => true,
            _ => match self.builtin_def_of(ty) {
                Some(def) if self.builtin_implements(ty, trait_def) => {
                    let args = self.builtin_head_args(ty);
                    self.impl_status(trait_def, def, &args) != Some(false)
                }
                _ => false,
            },
        }
    }

    /// Expand every `type` alias to a fixpoint. Aliases may refer to each
    /// other in any declaration order, so each round converts every alias
    /// not yet done; one that mentions a not-yet-expanded alias is skipped
    /// (`alias_blocked`) and retried next round. A round with no progress
    /// means the remaining aliases can never finish: a cycle
    /// (`TYPE-121`). They get an `Unknown` target so uses of them do not
    /// cascade into further errors.
    ///
    /// Runs before any other signature is collected, so a struct field, a
    /// function parameter or a const written with an alias already sees
    /// the real type. Generic arities of structs and enums are registered
    /// first so an argument-count mistake inside an alias target
    /// (`type X = Box<int, int>`) is still reported.
    fn collect_alias_sigs<'ast>(&mut self, program: &Program<'ast>) {
        let aliases: Vec<&TypeAlias<'ast>> = program.items.iter()
            .filter_map(|item| match item { Item::TypeAlias(a) => Some(a), _ => None })
            .collect();
        if aliases.is_empty() { return; }

        for item in program.items {
            match item {
                Item::Struct(s) => {
                    if let Some(id) = self.ctx.top_level_def(s.name) {
                        self.generic_arity.insert(id, s.generic_params.len());
                    }
                }
                Item::Enum(e) => {
                    if let Some(id) = self.ctx.top_level_def(e.name) {
                        self.generic_arity.insert(id, e.generic_params.len());
                    }
                }
                _ => {}
            }
        }

        self.alias_prepass = true;
        let mut pending: Vec<&TypeAlias<'ast>> = aliases;
        while !pending.is_empty() {
            let mut still: Vec<&TypeAlias<'ast>> = Vec::with_capacity(pending.len());
            for a in pending.iter().copied() {
                let Some(def_id) = self.ctx.top_level_def(a.name) else { continue; };
                let prev_generics = self.push_generic_scope(a.generic_params);
                self.alias_blocked = false;
                let target = self.ast_type_to_sema(a.ty);
                let blocked = self.alias_blocked;
                self.pop_generic_scope(prev_generics);
                if blocked {
                    still.push(a);
                } else {
                    self.alias_expansions.insert(
                        def_id,
                        AliasExpansion { arity: a.generic_params.len(), target },
                    );
                    self.ctx.set_def_type(def_id, target);
                }
            }
            if still.len() == pending.len() {
                for a in still {
                    if let Some(def_id) = self.ctx.top_level_def(a.name) {
                        self.errors.add_type_error(TypeError::TypeAliasCycle {
                            name: a.name.to_string(),
                            span: a.span,
                        });
                        let unknown = self.unknown();
                        self.alias_expansions.insert(
                            def_id,
                            AliasExpansion { arity: a.generic_params.len(), target: unknown },
                        );
                        self.ctx.set_def_type(def_id, unknown);
                    }
                }
                break;
            }
            pending = still;
        }
        self.alias_prepass = false;
        self.alias_blocked = false;
    }

    /// Whether the struct or enum `def` declares an associated function
    /// (a method with no `self`) called `name`.
    fn has_static_method(&self, def: DefId, name: &str) -> bool {
        self.struct_methods.get(&def)
            .is_some_and(|ms| ms.iter().any(|(n, m)| n == name && !m.has_self))
    }

    /// The definition a type NAME written in an expression or pattern
    /// (`P { x = 1 }`, `P.origin()`, `C.Red`, `Message.Move { .. }`) stands
    /// for. An alias to a struct or enum is transparent there too, so it
    /// yields the struct or enum's own definition. An alias to anything
    /// else (an `int`, a `List<T>`) keeps the alias's definition, which
    /// has no struct fields, methods or variants, so every lookup keyed by
    /// it misses exactly as it would for any other non-struct name.
    fn type_def_of(&self, name: &str) -> Option<DefId> {
        let id = self.ctx.top_level_def(name)?;
        if matches!(self.ctx.symbols.lookup(id).kind, DefKind::TypeAlias) {
            if let Some(exp) = self.alias_expansions.get(&id) {
                if let SemaType::Named { def, .. } = self.ctx.types.get(exp.target) {
                    return Some(*def);
                }
            }
        }
        Some(id)
    }

    /// A use of a `type` alias: its target with the use site's generic
    /// arguments substituted in. During the alias pre-pass an alias that
    /// is not expanded yet blocks the alias being converted.
    fn expand_alias(&mut self, def_id: DefId, name: &str, args: Vec<TypeId>, span: Span) -> TypeId {
        let Some(exp) = self.alias_expansions.get(&def_id).copied() else {
            if self.alias_prepass { self.alias_blocked = true; }
            return self.unknown();
        };
        if exp.arity != args.len() {
            self.errors.add_type_error(TypeError::GenericArgCountMismatch {
                type_name: name.to_string(),
                expected:  exp.arity,
                found:     args.len(),
                span,
            });
            return self.unknown();
        }
        // A bound written on the alias's parameter holds at every use, like
        // the bound of a struct: `type Wrapped<T: Show> = W<T>` makes
        // `Wrapped<int>` an error even though `W` itself asks nothing of `T`.
        self.require_def_bounds(def_id, &args, span);
        let expanded = self.substitute(exp.target, &args);
        self.require_dict_keys_in(expanded, span);
        expanded
    }

    fn collect_signatures<'ast>(&mut self, program: &Program<'ast>) {
        self.register_prelude_traits(program);
        self.register_trait_supertraits(program);
        self.register_generic_arities(program);
        self.register_generic_bounds(program);
        self.collect_alias_sigs(program);
        self.fill_generic_bound_args(program);
        for item in program.items {
            match item {
                Item::Function(f) => { self.collect_fn_sig(f); }
                Item::Struct(s)   => { self.collect_struct_sig(s); }
                Item::Enum(e)     => { self.collect_enum_sig(e); }
                Item::Const(c)    => { self.collect_const_sig(c); }
                Item::Static(s)   => { self.collect_static_sig(s); }
                // `collect_method_sig` alone (below) records each method as
                // its own standalone symbol (a real, pre-existing, and
                // still-needed step — it's what makes `self.ctx.resolutions
                // .get(m.span)` resolve at all) but was never told which
                // struct the method actually belongs to. `TYPE-103`/
                // `TYPE-104` ("type X has no field/method Y") on any
                // `extend`/`impl`-block method — confirmed the actual,
                // reported bug — trace straight back to that: nothing here
                // ever touched `i.target_type`/`x.target_type`, so
                // `struct_methods` (the table `v.method()` dispatch
                // actually looks up, see `collect_struct_sig` below) never
                // got an entry for these methods at all. Fixed by
                // `register_extend_impl_methods`, a deliberately separate
                // pass run after this one finishes (not folded into this
                // loop) so every struct's own `collect_struct_sig` entry
                // already exists to append onto, regardless of whether the
                // `extend`/`impl` block appears before or after the
                // `struct` it targets in the source file.
                Item::Impl(i)     => self.collect_block_sigs(
                    i.generic_params, i.target_type, i.methods, i.span, i.span, i.trait_path.is_some(),
                ),
                Item::Extend(x) => self.collect_block_sigs(
                    x.generic_params, x.target_type, x.methods, x.span, x.target_type.span, false,
                ),
                Item::Trait(t) => self.collect_trait_info(t),
                Item::TypeAlias(_) => {} // expanded up front by `collect_alias_sigs`
            }
        }
        self.register_extend_impl_methods(program);
        self.register_trait_impls(program);
        self.impls_registered = true;
        self.check_impl_supertraits();
        // Requirements raised by signatures that had to wait for the impls.
        self.check_obligations();
    }

    /// The root name of a `target_type` written as a plain (possibly
    /// generic) path — `Vector2` out of `extend Vector2 { .. }`, `Box` out
    /// of `impl<T> Box<T> { .. }` — resolved to its declaring `DefId` the
    /// same way `ast_type_to_sema`'s own `TypeKind::Named` arm does for an
    /// ordinary type reference. `None` for anything that isn't a plain
    /// named path (there's nothing sensible for `extend`/`impl` to target
    /// otherwise) or that doesn't resolve to a known top-level type —
    /// silently skipped by the caller, the same lenient handling
    /// `ast_type_to_sema` gives an unresolved type name anywhere else in
    /// this file (not diagnosed as its own error yet).
    fn target_type_def_id<'ast>(&self, ty: &Type<'ast>) -> Option<DefId> {
        match ty.kind {
            TypeKind::Named { path, .. } => {
                let root = path.first().copied()?;
                // An alias target (`extend P { .. }` with `type P = Point`)
                // is the aliased struct or enum. A name that is anything else
                // (an alias of `int`, a trait) is no user type.
                self.type_def_of(root).filter(|d| matches!(
                    self.ctx.symbols.lookup(*d).kind,
                    DefKind::Struct { .. } | DefKind::Enum,
                ))
            }
            _ => None,
        }
    }

    /// The key a built-in type's methods are called under at run time
    /// (`ast::types::builtin_impl_key` writes the same strings from the type
    /// as written, `Value::impl_key` from a value): the kind of value, so
    /// `int`, `long`, `i64` and `isize` share one, a tuple is keyed by its
    /// arity and an array shares `"List"` with `List<T>`. `None` for a type
    /// nothing can be implemented for.
    fn runtime_impl_key(&self, ty: TypeId) -> Option<String> {
        let key: &str = match self.ctx.types.get(ty) {
            SemaType::Int | SemaType::Long | SemaType::I64 | SemaType::Isize => "int",
            SemaType::I8   => "i8",
            SemaType::I16  => "i16",
            SemaType::I32  => "i32",
            SemaType::U8   => "u8",
            SemaType::U16  => "u16",
            SemaType::Uint | SemaType::U32 => "u32",
            SemaType::Ulong | SemaType::U64 | SemaType::Usize => "u64",
            SemaType::Float | SemaType::F32 => "float",
            SemaType::Double | SemaType::F64 => "double",
            SemaType::Bool => "bool",
            SemaType::Char => "char",
            SemaType::Str  => "string",
            SemaType::Tuple(elems) => return Some(format!("tuple{}", elems.len())),
            // An array shares the kind with `List`: one value at run time.
            SemaType::Array { .. } | SemaType::List(_) => "List",
            SemaType::Dictionary(..) => "Dictionary",
            SemaType::Queue(_)       => "Queue",
            SemaType::Stack(_)       => "Stack",
            SemaType::InlineList(_)  => "InlineList",
            _ => return None,
        };
        Some(key.to_string())
    }

    /// The definition for the built-in type an `extend` or `impl` block is
    /// written for, made on first use, with the type it stands for. `None`
    /// for a struct or an enum (which have a definition of their own) and for
    /// any type nothing can be implemented for.
    fn builtin_target<'ast>(&mut self, ty: &Type<'ast>) -> Option<(DefId, TypeId)> {
        if let Some(hit) = self.builtin_targets.get(&ty.span) { return Some(*hit); }
        if self.target_type_def_id(ty).is_some() { return None; }
        let sty = self.ast_type_to_sema(ty);
        self.runtime_impl_key(sty)?;
        let text = self.builtin_text(sty);
        let def = match self.builtin_defs.get(&text) {
            Some(d) => *d,
            None => {
                let d = self.ctx.symbols.insert(
                    text.clone(), DefKind::BuiltinType, ty.span, Visibility::Private,
                );
                self.builtin_defs.insert(text, d);
                self.builtin_def_types.insert(d, sty);
                d
            }
        };
        self.builtin_targets.insert(ty.span, (def, sty));
        Some((def, sty))
    }

    /// The definition of a built-in type that has an `extend` or `impl`
    /// block, if it has one. `ty` has its ownership wrappers peeled.
    fn builtin_def_of(&self, ty: TypeId) -> Option<DefId> {
        if self.builtin_defs.is_empty() { return None; }
        self.runtime_impl_key(ty)?;
        // A generic head has one definition whatever its arguments.
        if let Some(head) = self.builtin_head_name(ty) {
            return self.builtin_defs.get(head).copied();
        }
        self.builtin_defs.get(&self.builtin_text(ty)).copied()
    }

    /// A built-in type as the table of definitions names it. An integer
    /// literal that is still undecided inside a tuple or an array counts as
    /// `int`, the type it defaults to, so `let p = (1, "a")` finds the impl
    /// written for `(int, string)`.
    fn builtin_text(&self, ty: TypeId) -> String {
        let ty = self.apply(ty);
        if self.int_lit_var_of(ty).is_some() { return "int".to_string(); }
        match self.ctx.types.get(ty) {
            SemaType::Tuple(elems) => format!("({})", elems.iter()
                .map(|e| self.builtin_text(*e))
                .collect::<Vec<_>>()
                .join(", ")),
            SemaType::Array { len, elem } => format!("[{}]{}", len, self.builtin_text(*elem)),
            _ => self.display_type(ty),
        }
    }

    /// Whether the built-in type `ty` has an `impl` of `trait_def`.
    fn builtin_implements(&self, ty: TypeId, trait_def: DefId) -> bool {
        self.builtin_def_of(ty)
            .is_some_and(|d| self.trait_impls.contains_key(&(trait_def, d)))
    }

    /// A method an `extend` or `impl` block gave a built-in type, called on a
    /// value of it. `None` when the receiver has no such method, so the
    /// caller goes on as before. The callers try the native methods first, so
    /// a native one wins over a user's of the same name, as an inherent
    /// method wins over a trait's.
    fn try_builtin_user_method<'ast>(
        &mut self,
        receiver_ty: TypeId,
        field:       &str,
        args:        &[Arg<'ast>],
        callee_span: Span,
        call_span:   Span,
    ) -> Option<TypeId> {
        if self.builtin_defs.is_empty() { return None; }
        let bare = self.peel_wrappers(receiver_ty);
        let def = self.builtin_def_of(bare)?;
        if self.ambiguous_methods.contains(&(def, field.to_string())) {
            let traits: Vec<String> = self.method_origins
                .get(&(def, field.to_string()))
                .map(|ts| ts.iter()
                    .filter_map(|t| self.traits.get(t).map(|i| i.name.clone()))
                    .collect())
                .unwrap_or_default();
            self.errors.add_type_error(TypeError::AmbiguousTraitMethod {
                method:    field.to_string(),
                traits,
                type_name: self.display_type(bare),
                span:      callee_span,
            });
            return Some(self.unknown());
        }
        let m = self.struct_methods.get(&def)?
            .iter()
            .find(|(n, m)| n == field && m.has_self)
            .map(|(_, m)| m.clone())?;
        // A collection's own type arguments are the head's (`List<int>` is the
        // head `List` applied to `int`); other built-in types have none.
        let recv_args = self.builtin_head_args(bare);
        // A method an impl for `List<int>` gave is not a method of `List<string>`.
        if !self.apply_impl_guard(&m.guard, &recv_args, callee_span) { return None; }
        self.check_method_tier(&m, field, callee_span);
        let inst = self.instantiate_method(&m, &recv_args);
        self.check_method_args(&inst.params, args, call_span);
        self.require_method_bounds(&m, &inst, args, call_span);
        Some(self.maybe_arena_ref(inst.return_type))
    }

    /// How many generic parameters the type an `extend` or `impl` block is
    /// written for declares: the number of parameters in scope before a
    /// method's own (zero for a target that does not resolve).
    fn target_arity<'ast>(&self, ty: &Type<'ast>) -> usize {
        self.target_type_def_id(ty)
            .and_then(|d| self.generic_arity.get(&d).copied())
            .unwrap_or(0)
    }

    /// Phase 2a continued: append every `extend`/`impl`-block method's
    /// shape onto its target struct's `struct_methods` entry, the same
    /// dispatch table an in-struct `struct S { fn f(self) {..} }` method
    /// already populates in `collect_struct_sig` below. Deliberately a
    /// second, later pass over `program.items` (not folded into the loop
    /// above) rather than an `.insert()` at each `Item::Impl`/`Item::Extend`
    /// site: `collect_struct_sig` uses a plain `.insert()` that would wipe
    /// out anything an earlier-processed `extend` block had already added
    /// for that struct, so this has to run once every struct's own entry
    /// already exists, appending via `.entry(..).or_default()` rather than
    /// ever inserting fresh.
    ///
    /// Trait `impl`s (`i.trait_path.is_some()`) are skipped here on
    /// purpose — traits themselves aren't dispatched through yet
    /// (`docs/PARKED_IDEAS.md`), so registering their methods into
    /// `struct_methods` would let `v.method()` resolve to a method whose
    /// trait requirement was never actually checked. An inherent `impl`
    /// (`impl Type { .. }`, `trait_path: None`) is registered exactly like
    /// `extend` — the two are equivalent for dispatch purposes today.
    ///
    /// Generic targets (`extend Box<T> { .. }`) reuse the target struct's
    /// own generic arity (`self.generic_arity`) to build the same `Self`
    /// shape `collect_struct_sig`/`infer_struct_bodies` use, but don't
    /// bind the `extend`/`impl` block's own generic parameter *names* —
    /// neither `ImplBlock` nor `ExtendDecl` carries its own
    /// `generic_params` list the way `StructDecl`/`FunctionDecl` do, so
    /// there's nothing here to bind them from. Out of scope for now
    /// (untested, unrequested); the common, non-generic case this was
    /// actually asked to fix is unaffected.
    fn register_extend_impl_methods<'ast>(&mut self, program: &Program<'ast>) {
        for item in program.items {
            let (target_type, methods, key) = match item {
                Item::Extend(x) => (x.target_type, x.methods, x.span),
                Item::Impl(i) if i.trait_path.is_none() => (i.target_type, i.methods, i.span),
                _ => continue,
            };
            if self.impl_failed.contains(&key) { continue; }
            let pattern = self.impl_patterns.get(&key).cloned();
            let def_id = match &pattern {
                Some(p) => p.head,
                None => match self.target_type_def_id(target_type) {
                    Some(d) => d,
                    None => match self.builtin_target(target_type) {
                        Some((d, _)) => d,
                        None => {
                            // Anything else has no definition to hang methods
                            // on. Saying so beats a call that panics at run
                            // time.
                            if matches!(target_type.kind,
                                TypeKind::List(..) | TypeKind::Dictionary(..) | TypeKind::Set(..)
                                | TypeKind::Queue(..) | TypeKind::Stack(..) | TypeKind::Optional(..)
                                | TypeKind::Slice(..) | TypeKind::Function(..))
                            {
                                self.report_unsupported(impl_patterns::TARGET_FOR_EXTEND, target_type.span);
                            }
                            continue;
                        }
                    },
                },
            };
            let guard = pattern.as_ref().and_then(|p| p.guard.clone());
            // The run-time kind a built-in type's methods are called under.
            // Two definitions of one kind (`int` and `long`, an array and a
            // `List<T>`) share a method table at run time.
            let kind_key = self.builtin_def_types.get(&def_id).copied()
                .and_then(|t| self.runtime_impl_key(t));
            let generic_head = pattern.is_some()
                || self.generic_arity.get(&def_id).copied().unwrap_or(0) > 0;
            for m in methods {
                let Some(fn_ty) = self.ctx.def_type(
                    self.ctx.resolutions.get(m.span).unwrap_or(DefId::INVALID)
                ) else { continue; };
                let has_self = m.params.first()
                    .map(|p| matches!(p.kind,
                        ParamKind::SelfVal | ParamKind::SelfMut
                        | ParamKind::SelfRef | ParamKind::SelfRefMut))
                    .unwrap_or(false);
                // One inherent method of a name per generic head, and one per
                // built-in kind: a call cannot tell the instances apart.
                let taken_here = generic_head && self.struct_methods.get(&def_id)
                    .is_some_and(|ms| ms.iter().any(|(n, _)| n == m.name)
                        && !self.trait_entries.contains(&(def_id, m.name.to_string())));
                let taken_by_kind = kind_key.as_ref().is_some_and(|k|
                    self.inherent_key_methods
                        .get(&(k.clone(), m.name.to_string()))
                        .is_some_and(|other| *other != def_id));
                if taken_here || taken_by_kind {
                    self.report_unsupported(impl_patterns::DUPLICATE_METHOD, m.span);
                    continue;
                }
                if let Some(k) = &kind_key {
                    self.inherent_key_methods.insert((k.clone(), m.name.to_string()), def_id);
                }
                if let SemaType::Function { params, return_type, is_fallible, .. } =
                    self.ctx.types.get(fn_ty).clone()
                {
                    let generics = self.generics_at(m.span);
                    self.struct_methods.entry(def_id).or_default().push((
                        m.name.to_string(),
                        MethodShape { has_self, params, return_type, is_fallible, generics, guard: guard.clone(), tier: Some(m.tier) },
                    ));
                }
            }
        }
    }

    fn collect_fn_sig<'ast>(&mut self, f: &FunctionDecl<'ast>) -> TypeId {
        let prev_generics = self.push_generic_scope(f.generic_params);
        let mut param_tys: Vec<TypeId> = Vec::with_capacity(f.params.len());
        for p in f.params {
            if let ParamKind::Named { ty, .. } | ParamKind::Discard { ty } = p.kind {
                if let Some(t) = ty {
                    let ty_id = self.ast_type_to_sema(t);
                    param_tys.push(ty_id);
                    self.seed_param_type(p, ty_id);
                }
            }
        }
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
            self.record_where_lifetimes(def_id, f);
            if self.generic_bounds.contains_key(&def_id) {
                self.fn_type_defs.insert(fn_ty, def_id);
            }
        }
        fn_ty
    }

    /// A method's signature, with its own generic parameters (if any) in
    /// scope on top of the declaration's: `base` is how many parameters that
    /// declaration has, so the method's first own parameter is `Param(base)`.
    /// Returns the function type and the generic parameters the method
    /// declares; the latter are also recorded by the method's definition for
    /// the pass that rebuilds shapes from function types.
    fn collect_method_sig<'ast>(&mut self, m: &MethodDecl<'ast>, base: usize) -> (TypeId, MethodGenerics) {
        let scope = self.push_own_generic_scope(m.generic_params, base);
        let mut param_tys: Vec<TypeId> = Vec::with_capacity(m.params.len());
        for p in m.params {
            if let ParamKind::Named { ty, .. } | ParamKind::Discard { ty } = p.kind {
                if let Some(t) = ty {
                    let ty_id = self.ast_type_to_sema(t);
                    param_tys.push(ty_id);
                    self.seed_param_type(p, ty_id);
                }
            }
        }
        let (ret_ty, is_fallible) = self.return_type_to_sema(m.return_type.as_ref());
        let generics = self.method_generics_of(m.generic_params, base);
        self.pop_own_generic_scope(scope);
        let fn_ty = self.ctx.types.insert(SemaType::Function {
            params: param_tys,
            return_type: ret_ty,
            is_fallible,
            generic_arity: m.generic_params.len(),
        });
        if let Some(def_id) = self.ctx.resolutions.get(m.span) {
            self.ctx.set_def_type(def_id, fn_ty);
            if !generics.own.is_empty() {
                self.method_generics.insert(def_id, generics.clone());
            }
        }
        (fn_ty, generics)
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
        let prev_self_type = self.self_type.replace(struct_ty);

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
                    let (fn_ty, generics) = self.collect_method_sig(m, s.generic_params.len());
                    let has_self = m.params.first()
                        .map(|p| matches!(p.kind,
                            ParamKind::SelfVal | ParamKind::SelfMut
                            | ParamKind::SelfRef | ParamKind::SelfRefMut))
                        .unwrap_or(false);
                    if let SemaType::Function { params, return_type, is_fallible, .. } =
                        self.ctx.types.get(fn_ty).clone()
                    {
                        methods.push((m.name.to_string(), MethodShape {
                            has_self, params, return_type, is_fallible, generics, guard: None,
                            tier: Some(m.tier),
                        }));
                    }
                }
                StructMember::Property(_) => {}
            }
        }

        self.struct_fields.insert(def_id, fields);
        self.struct_methods.insert(def_id, methods);
        self.self_type = prev_self_type;
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
                let lit_ty = self.infer_literal(lit, pat.span);
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

    /// A static's type is always written out (the parser requires the
    /// annotation), so it is known before any body is inferred.
    fn collect_static_sig<'ast>(&mut self, s: &StaticDecl<'ast>) {
        let Some(def_id) = self.ctx.top_level_def(s.name) else { return; };
        let ty = self.ast_type_to_sema(s.ty);
        self.ctx.set_def_type(def_id, ty);
    }

    // ── Phase 2b: Body inference ──────────────────────────────────

    fn infer_bodies<'ast>(&mut self, program: &Program<'ast>) {
        for item in program.items {
            match item {
                Item::Function(f) => self.infer_function_body(f),
                Item::Struct(s)   => self.infer_struct_bodies(s),
                Item::Const(c)    => self.infer_const_body(c),
                Item::Static(s)   => self.infer_static_body(s),
                Item::Impl(i) => self.infer_block_bodies(i.span, i.target_type, i.methods),
                Item::Extend(x) => self.infer_block_bodies(x.span, x.target_type, x.methods),
                Item::Trait(t) => self.infer_trait_default_bodies(t),
                Item::Enum(_) | Item::TypeAlias(_) => {}
            }
        }
        self.finish_body_checks();
    }

    fn infer_function_body<'ast>(&mut self, f: &FunctionDecl<'ast>) {
        for param in f.params { self.seed_param(param); }
        let (ret_ty, is_fallible) = self.return_type_to_sema(f.return_type.as_ref());
        let prev_ret      = self.current_return.replace(ret_ty);
        let prev_fallible = self.current_fallible;
        self.current_fallible = is_fallible;
        let prev_tier = self.current_tier;
        self.current_tier = f.tier;
        let fn_def = self.ctx.top_level_def(f.name);
        let prev_scope = self.install_def_scope(fn_def);
        self.infer_block(&f.body);
        self.finish_body_checks();
        self.restore_def_scope(prev_scope);
        self.current_return   = prev_ret;
        self.current_fallible = prev_fallible;
        self.current_tier     = prev_tier;
    }

    fn infer_method_body<'ast>(&mut self, m: &MethodDecl<'ast>, base: usize) {
        let scope = self.push_own_generic_scope(m.generic_params, base);
        for param in m.params { self.seed_param(param); }
        let (ret_ty, is_fallible) = self.return_type_to_sema(m.return_type.as_ref());
        let prev_ret      = self.current_return.replace(ret_ty);
        let prev_fallible = self.current_fallible;
        self.current_fallible = is_fallible;
        let prev_tier = self.current_tier;
        self.current_tier = self.ctx.method_tier(m);
        self.infer_block(&m.body);
        self.finish_body_checks();
        self.pop_own_generic_scope(scope);
        self.current_return   = prev_ret;
        self.current_fallible = prev_fallible;
        self.current_tier     = prev_tier;
    }

    /// Records a param's already-resolved type into the same side tables
    /// `seed_param` used to compute independently: the definition's own
    /// type (via `resolutions`, populated earlier by name_resolution) and
    /// its binding type (keyed by the param's own span). Called from
    /// `collect_fn_sig`/`collect_method_sig`, under whatever generic scope
    /// is active there, so a function's own generic params resolve
    /// correctly. `seed_param` below reads `binding_type` back instead of
    /// resolving `ast_type_to_sema` a second time with no generic scope of
    /// its own. See Open Decision #6, docs/MEMORY_MODEL.md.
    fn seed_param_type<'ast>(&mut self, param: &Param<'ast>, ty_id: TypeId) {
        if let Some(def_id) = self.ctx.resolutions.get(param.span) {
            self.ctx.set_def_type(def_id, ty_id);
            self.ctx.set_binding_type(param.span, ty_id);
        }
    }

    fn seed_param<'ast>(&mut self, param: &Param<'ast>) {
        match param.kind {
            ParamKind::Named { ty, .. } => {
                if self.ctx.binding_type(param.span).is_some() {
                    // Already resolved by collect_fn_sig/collect_method_sig,
                    // under the correct generic scope. Resolving again here
                    // used to both waste the work and, for a bad annotation,
                    // double-report the exact same diagnostic (see
                    // tests/fixtures/err_unique_missing_type_argument.ubl).
                    return;
                }
                let ty_id = ty.map(|t| self.ast_type_to_sema(t))
                    .unwrap_or_else(|| self.fresh_var());
                self.seed_param_type(param, ty_id);
            }
            _ => {} // self params (TODO when current_struct_type threaded in)
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
        let prev_self_type = self.self_type.replace(self_ty);
        for member in s.members {
            if let StructMember::Method(m) = member {
                self.infer_method_body(m, s.generic_params.len());
            }
        }
        self.self_type = prev_self_type;
        self.current_struct_type = prev_self;
        self.pop_generic_scope(prev_generics);
    }

    /// The bodies of one `impl` or `extend` block. A block for a generic head
    /// or with parameters of its own is read in its own scope (its parameters,
    /// their bounds, `Self`); a block whose target was reported is not read.
    fn infer_block_bodies<'ast>(
        &mut self,
        key:         Span,
        target_type: &'ast Type<'ast>,
        methods:     &'ast [MethodDecl<'ast>],
    ) {
        if self.impl_failed.contains(&key) { return; }
        if let Some(p) = self.impl_patterns.get(&key).cloned() {
            let scope = self.enter_impl_scope(&p);
            for m in methods { self.infer_method_body(m, p.arity); }
            self.leave_impl_scope(scope);
            return;
        }
        self.infer_extend_impl_bodies(target_type, methods);
    }

    /// The `extend`/`impl` twin of `infer_struct_bodies` just above: sets
    /// `current_struct_type` around each method body the same way, so
    /// `self.x` inside an `extend`/`impl` method actually type-checks
    /// against the real target struct instead of silently inferring as
    /// `Unknown` — previously the case for *every* `extend`/`impl` method
    /// (confirmed: `self.x + "not a number"` inside one raised no error at
    /// all before this), since nothing set `current_struct_type` for this
    /// path and `seed_param` explicitly does nothing for `self` params of
    /// its own (see that function's own `TODO` comment). Applies to every
    /// `impl`, trait or inherent — unlike dispatch registration
    /// (`register_extend_impl_methods`), which skips trait impls since
    /// traits aren't dispatched through yet, a trait impl's method body
    /// still deserves correct `self` typing for its own sake. Silently
    /// does nothing for a `target_type` that isn't a plain named path or
    /// doesn't resolve — same lenient handling as `target_type_def_id`'s
    /// other caller.
    fn infer_extend_impl_bodies<'ast>(
        &mut self,
        target_type: &'ast Type<'ast>,
        methods:     &'ast [MethodDecl<'ast>],
    ) {
        let Some(def_id) = self.target_type_def_id(target_type) else {
            // A primitive, a tuple or an array is its own `Self`, so `self`
            // in the body has that type.
            let builtin = self.builtin_target(target_type).map(|(_, t)| t);
            let prev_self = self.current_struct_type;
            let prev_self_type = self.self_type;
            if let Some(t) = builtin {
                self.current_struct_type = Some(t);
                self.self_type = Some(t);
            }
            for m in methods { self.infer_method_body(m, 0); }
            self.self_type = prev_self_type;
            self.current_struct_type = prev_self;
            return;
        };
        let arity = self.generic_arity.get(&def_id).copied().unwrap_or(0);
        let self_args: Vec<TypeId> = (0..arity)
            .map(|i| self.ctx.types.intern(SemaType::Param(i)))
            .collect();
        let self_ty = self.ctx.types.insert(SemaType::Named { def: def_id, args: self_args });
        let prev_self = self.current_struct_type.replace(self_ty);
        let prev_self_type = self.self_type.replace(self_ty);
        let prev_scope = self.install_def_scope(Some(def_id));
        for m in methods {
            self.infer_method_body(m, arity);
        }
        self.restore_def_scope(prev_scope);
        self.self_type = prev_self_type;
        self.current_struct_type = prev_self;
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
        // A const's type is shared by every function that reads it, so
        // whatever its initializer left open is settled here, not left for
        // one unrelated caller to decide.
        self.finish_body_checks();
    }

    /// The initializer must match the declared type. As with a const, a
    /// literal in it is typed by that declaration (`static HP: u8 = 200`)
    /// and settled here, since the type is shared by every reader.
    fn infer_static_body<'ast>(&mut self, s: &StaticDecl<'ast>) {
        let inferred = self.infer_expr(s.value);
        if let Some(def_id) = self.ctx.top_level_def(s.name) {
            if let Some(declared) = self.ctx.def_type(def_id) {
                self.unify(declared, inferred, s.value.span);
            }
        }
        self.finish_body_checks();
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
            ExprKind::Lit(lit) => self.infer_literal(lit, expr.span),

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
                // `-128i8` (and the i16/i32/i64/isize equivalents):
                // special-cased the way Rust's own compiler conventionally
                // treats a negated integer literal, so the type's full
                // signed range — including its most-negative value — is
                // available to a literal written this way. Without this,
                // `TYPE-120` only ever sees the positive magnitude `128`
                // (see `int_suffix_fits`'s own doc comment for why that's
                // the check's default), which overflows `i8` even though
                // the actual value `-128` is `i8::MIN` and perfectly
                // valid. Scoped exactly like Rust's own special case: only
                // a literal *directly* under the `-` gets it (`-x` for a
                // variable `x` doesn't, only `-` immediately followed by
                // a suffixed literal does), and only signed suffixes have
                // a most-negative value to special-case in the first
                // place — this deliberately does not touch (or excuse)
                // `-5u8`, which stays exactly what it already was: sema
                // accepts it (there's no compile-time negation-validity
                // check for *any* type here, signed or not — matches
                // this codebase's existing convention of leaving
                // operator-validity to the interpreter's own runtime
                // panic, same as e.g. `"hello" + 5`), and the interpreter
                // panics on it at runtime ("cannot negate u8") the same
                // way it already did before this feature existed.
                if let (UnaryOp::Neg, ExprKind::Lit(Literal::TypedInt { raw, suffix })) =
                    (op, &operand.kind)
                {
                    if let Some(min_magnitude) = int_suffix_signed_min_magnitude(*suffix) {
                        if *raw <= min_magnitude {
                            return self.int_suffix_sema_type(*suffix);
                        }
                        // Falls through to the generic path below on
                        // purpose: `-200i8` really is out of range even
                        // negated, and that path is what raises `TYPE-120`
                        // for it.
                    }
                }
                // `-128` for an `i8` binding: an unsuffixed literal directly
                // under `-` is checked as the negative value it denotes, so
                // the type's most-negative value is reachable, and `-1` for
                // a `u8` is rejected instead of wrapping. The literal and the
                // negation share one type, so the operand records the same
                // var the whole expression returns.
                if let (UnaryOp::Neg, ExprKind::Lit(Literal::Int(n))) = (op, &operand.kind) {
                    let ty = self.infer_int_literal(-(*n as i128), operand.span);
                    self.ctx.set_expr_type(operand.span, ty);
                    return ty;
                }
                let op_ty = self.infer_expr(operand);
                match op {
                    UnaryOp::Not    => self.bool_ty(),
                    UnaryOp::Neg    => op_ty,
                    UnaryOp::BitNot => op_ty,
                    UnaryOp::Await  => self.unwrap_task(op_ty, expr.span),
                }
            }

            ExprKind::Call { callee, args } => {
                self.callee_field_pending = matches!(callee.kind, ExprKind::Field { .. });
                let callee_ty = self.infer_expr(callee);
                self.callee_field_pending = false;
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
                            if let Some(ctor_ty) = self.builtin_constructor_type(ns, expr.span) {
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

                // `Trait.method(value, ..)`: the explicit trait-qualified call.
                if let ExprKind::Field { target, field } = callee.kind {
                    if let ExprKind::Ident(ns) = target.kind {
                        if let Some(def_id) = self.ctx.top_level_def(ns) {
                            if matches!(self.ctx.symbols.lookup(def_id).kind, DefKind::Trait) {
                                return self.infer_qualified_trait_call(def_id, field, args, expr.span);
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
                        if let Some(def_id) = self.type_def_of(ns) {
                            if let Some(shapes) = self.enum_variants.get(&def_id) {
                                let shapes  = shapes.clone();
                                // GENERICS_RULES.md — a fresh instantiation
                                // per call site (not the raw, possibly
                                // `Param`-containing stored type): a bare
                                // constructor call has no already-known
                                // concrete args, so they're inferred from
                                // how the payload argument(s) below unify
                                // against the (substituted) element types.
                                let enum_ty  = self.instantiate(def_id, expr.span);
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
                                } else if !self.has_static_method(def_id, field) {
                                    // Not a variant, and not an associated
                                    // fn declared in an `extend`/`impl`
                                    // block either (`Color.make()`): that
                                    // case falls through to the
                                    // associated-call branch below.
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
                        if let Some(def_id) = self.type_def_of(ns) {
                            if let Some(methods) = self.struct_methods.get(&def_id).cloned() {
                                if let Some((_, m)) = methods.iter().find(|(n, m)| n == field && !m.has_self) {
                                    let m = m.clone();
                                    self.check_method_tier(&m, field, callee.span);
                                    let arity = self.generic_arity.get(&def_id).copied().unwrap_or(0);
                                    let fresh_args: Vec<TypeId> = (0..arity).map(|_| self.fresh_var()).collect();
                                    self.require_def_bounds(def_id, &fresh_args, expr.span);
                                    let inst = self.instantiate_method(&m, &fresh_args);
                                    // What the block that gave the function asks of the
                                    // type's arguments (`extend Box<int> { fn make() .. }`).
                                    self.apply_impl_guard(&m.guard, &fresh_args, expr.span);
                                    self.check_method_args(&inst.params, args, expr.span);
                                    self.require_method_bounds(&m, &inst, args, expr.span);
                                    return self.maybe_arena_ref(inst.return_type);
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
                        .map(|ty| { let ty = self.default_if_int_lit(ty); self.apply(ty) });
                    if let Some(receiver_ty) = receiver_ty {
                        if let Some((wrap, kind, bare_ty)) =
                            instance::resolve_receiver(&self.ctx.types, receiver_ty)
                        {
                            let Some((ret, arity)) = instance::signature(kind, field) else {
                                // A method an `extend` or `impl` block gave this
                                // kind (a string, a tuple, an array).
                                if let Some(ret) = self.try_builtin_user_method(
                                    receiver_ty, field, args, callee.span, expr.span,
                                ) {
                                    return ret;
                                }
                                // `s.lt(t)` on a string: a prelude method on a
                                // built-in kind that has the trait.
                                if let Some(ret) = self.try_prelude_method(
                                    receiver_ty, field, args, callee.span, expr.span,
                                ) {
                                    return ret;
                                }
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

                            // A key handed to a dictionary whose own key type is
                            // not written anywhere (`let d = Dictionary.new()`)
                            // must be `Hash`: the annotation check cannot see
                            // it. A written key type was checked where it is
                            // written.
                            if kind == instance::ReceiverKind::Dict
                                && matches!(field, "set" | "get" | "contains_key")
                            {
                                if let SemaType::Dictionary(k, _) = self.ctx.types.get(bare_ty).clone() {
                                    let k = self.apply(k);
                                    if matches!(self.ctx.types.get(k), SemaType::Var(_) | SemaType::Unknown) {
                                        let key_span = args.first().map(|a| match &a.kind {
                                            ArgKind::Positional(e)       => e.span,
                                            ArgKind::Named { value, .. } => value.span,
                                        });
                                        if let Some(sp) = key_span {
                                            if let Some(arg_ty) = self.ctx.expr_type(sp) {
                                                self.require_dict_key(arg_ty, sp);
                                            }
                                        }
                                    }
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
                        //
                        // MEMORY_MODEL.md §9 Open Decision #5's remaining
                        // half (PARKED_IDEAS.md, "Traits / interface
                        // system"): peel at most one ownership wrapper
                        // (`Unique`/`Shared`/`SyncShared`) before this
                        // check, mirroring `instance::resolve_receiver`'s
                        // own peel for the six builtin kinds just above.
                        // `resolve_receiver` can't be reused directly for
                        // this: it returns `None` outright for a bare
                        // `Named` struct (none of its `ReceiverKind` arms
                        // match one), discarding the very `bare_ty` this
                        // branch needs, so the peel is repeated narrowly
                        // here rather than threaded through that call.
                        // Not reapplied to the return type below, for the
                        // same reason `resolve_receiver`'s own comment
                        // gives: nothing here returns a value that itself
                        // needs `Unique`/`Shared`/`SyncShared` re-applied,
                        // only the tier wrap `maybe_arena_ref` already
                        // handles. The interpreter side needed no
                        // matching change: `eval_method_call`
                        // (`interpreter/eval/expr.rs`) already peels all
                        // three unconditionally, for every receiver, not
                        // just the six builtin kinds.
                        // The receiver is a type parameter (inside a trait
                        // default body: the abstract `Self`). Its declared
                        // bounds say what it can do; with none, nothing
                        // does, and the call is an error rather than a
                        // runtime panic for an argument type that lacks the
                        // method (slice S1b, D6).
                        let bare_recv = self.peel_wrappers(receiver_ty);
                        if matches!(self.ctx.types.get(bare_recv), SemaType::Param(_)) {
                            return match self.current_bounds.get(&bare_recv).cloned() {
                                Some(bounds) => self.call_through_bounds(
                                    bare_recv, &bounds, field, args, callee.span, expr.span,
                                ),
                                None => {
                                    let param = self.param_display_name(bare_recv);
                                    self.errors.add_type_error(TypeError::MethodOnUnboundedParam {
                                        method: field.to_string(),
                                        param,
                                        span:   callee.span,
                                    });
                                    self.unknown()
                                }
                            };
                        }
                        // A method an `extend` or `impl` block gave a primitive
                        // (the other built-in kinds are handled above).
                        if let Some(ret) = self.try_builtin_user_method(
                            receiver_ty, field, args, callee.span, expr.span,
                        ) {
                            return ret;
                        }
                        // A prelude method (`eq`, `cmp`, `clone`, ...) on a value
                        // whose type has the trait: a primitive, a derived
                        // struct, an enum, a tuple. A method of the same name
                        // on the type itself wins.
                        if let Some(ret) = self.try_prelude_method(
                            receiver_ty, field, args, callee.span, expr.span,
                        ) {
                            return ret;
                        }
                        // A primitive has no native methods, so a method that
                        // is neither a user's nor a prelude's does not exist.
                        // Saying so beats a call that panics at run time.
                        let bare = self.peel_wrappers(receiver_ty);
                        if self.runtime_impl_key(bare).is_some_and(|k| k != "List") {
                            self.errors.add_type_error(TypeError::NoSuchMethod {
                                method:  field.to_string(),
                                on_type: self.display_type(bare),
                                span:    callee.span,
                            });
                            return self.unknown();
                        }
                        let struct_recv_ty = match self.ctx.types.get(receiver_ty) {
                            SemaType::Unique(inner)
                            | SemaType::Shared(inner)
                            | SemaType::SyncShared(inner) => *inner,
                            _ => receiver_ty,
                        };
                        if let SemaType::Named { def, args: recv_args } =
                            self.ctx.types.get(struct_recv_ty).clone()
                        {
                            // A struct always has a methods entry (possibly
                            // empty), but an enum only gets one when an
                            // `extend`/`impl` block adds a method. Without
                            // this an unknown method on a plain enum value
                            // fell straight through to an unchecked call and
                            // panicked at runtime. Only a user struct or enum
                            // is treated as "has a (possibly empty) method
                            // list"; any other `Named` keeps the old path.
                            let is_user_type = matches!(
                                self.ctx.symbols.lookup(def).kind,
                                DefKind::Enum | DefKind::Struct { .. }
                            );
                            let methods_opt = self.struct_methods.get(&def).cloned()
                                .or_else(|| if is_user_type { Some(Vec::new()) } else { None });
                            if let Some(methods) = methods_opt {
                                // `c.cb(4)` where `cb` is a struct FIELD of
                                // function type, not a method. A real method
                                // of the same name wins (found first), the
                                // interpreter checks in the same order.
                                // Two traits supply this name and no inherent
                                // method does: the caller has to say which.
                                if self.ambiguous_methods.contains(&(def, field.to_string())) {
                                    let traits: Vec<String> = self.method_origins
                                        .get(&(def, field.to_string()))
                                        .map(|ts| ts.iter()
                                            .filter_map(|t| self.traits.get(t).map(|i| i.name.clone()))
                                            .collect())
                                        .unwrap_or_default();
                                    self.errors.add_type_error(TypeError::AmbiguousTraitMethod {
                                        method:    field.to_string(),
                                        traits,
                                        type_name: self.display_type(struct_recv_ty),
                                        span:      callee.span,
                                    });
                                    return self.unknown();
                                }
                                let found_method = methods.iter()
                                    .find(|(n, m)| n == field && m.has_self).cloned();
                                let calls_a_field = found_method.is_none()
                                    && self.struct_fields.get(&def)
                                        .is_some_and(|fs| fs.iter().any(|(n, _)| n == field));
                                if !calls_a_field { return match found_method {
                                    Some((_, m)) => {
                                        let m = m.clone();
                                        // A method an impl for `Box<int>` gave is not a method of
                                        // `Box<string>`.
                                        if !self.apply_impl_guard(&m.guard, &recv_args, callee.span) {
                                            let shown = self.deep_apply(receiver_ty);
                                            let on_type = self.display_type(shown);
                                            self.errors.add_type_error(TypeError::NoSuchMethod {
                                                method: field.to_string(),
                                                on_type,
                                                span:   callee.span,
                                            });
                                            return self.unknown();
                                        }
                                        self.check_method_tier(&m, field, callee.span);
                                        let inst = self.instantiate_method(&m, &recv_args);
                                        self.check_method_args(&inst.params, args, expr.span);
                                        self.require_method_bounds(&m, &inst, args, expr.span);
                                        self.maybe_arena_ref(inst.return_type)
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
                                }; }
                            }
                        }
                    }
                }

                self.call_return_type(callee, callee_ty, args, expr.span)
            }

            ExprKind::Field { target, field } => {
                // Taken before anything is inferred, so it only ever
                // applies to this node; see `callee_field_pending`.
                let is_call_callee = std::mem::take(&mut self.callee_field_pending);
                // `EnumName.Variant` (fieldless/discriminant only — a
                // payload-carrying variant referenced bare with no call
                // isn't a valid value on its own; falls through to the
                // generic Unknown below same as any other unhandled
                // shape). Mirrors `Pool.new()`'s constructor special-
                // case: recognized syntactically here rather than via
                // the general field table, which doesn't exist yet.
                if let ExprKind::Ident(name) = &target.kind {
                    if let Some(def_id) = self.type_def_of(name) {
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
                                    let enum_ty = self.instantiate(def_id, expr.span);
                                    return self.maybe_arena_ref(enum_ty);
                                }
                            } else if !self.has_static_method(def_id, field) {
                                // When this is the callee of a call
                                // (`Color.nothing()`), the `Call` arm reports
                                // the same unknown name against the whole
                                // call, so reporting it here too is one typo
                                // shown twice.
                                if !is_call_callee {
                                    self.errors.add_type_error(TypeError::UnknownVariant {
                                        enum_name:    name.to_string(),
                                        variant_name: field.to_string(),
                                        span:         expr.span,
                                    });
                                }
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
                let receiver_ty = self.default_if_int_lit(receiver_ty);
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
                        if !is_method && !is_call_callee {
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
                    if let Some(def_id) = self.type_def_of(path[0]) {
                        if let Some(shapes) = self.enum_variants.get(&def_id) {
                            let shapes    = shapes.clone();
                            // GENERICS_RULES.md — same fresh-per-call-site
                            // reasoning as the tuple-variant constructor:
                            // the struct-payload fields' values below are
                            // what T gets inferred from via unification.
                            let enum_ty   = self.instantiate(def_id, expr.span);
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
                if let Some(def_id) = self.type_def_of(root) {
                    if let Some(decl_fields) = self.struct_fields.get(&def_id).cloned() {
                        let struct_ty  = self.instantiate(def_id, expr.span);
                        let inst_args: Vec<TypeId> = match self.ctx.types.get(struct_ty).clone() {
                            SemaType::Named { args, .. } => args,
                            _ => Vec::new(),
                        };
                        for f in fields.iter() {
                            let val_ty = self.infer_expr(f.value);
                            match decl_fields.iter().find(|(n, _)| n == f.name) {
                                Some((_, raw_ty)) => {
                                    let field_ty = self.substitute(*raw_ty, &inst_args);
                                    self.unify_struct_field(def_id, field_ty, val_ty, f.span);
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
                let struct_ty = self.type_def_of(root)
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

    // ── Integer-literal variables ──────────────────────────────────
    //
    // docs/PARKED_IDEAS.md, "Unsuffixed integer literals: full
    // context-driven typing (Rust style)". An unsuffixed literal starts
    // as a fresh type variable that remembers its value. Unifying that
    // variable with a concrete integer type range-checks the value
    // (`TYPE-120`) and binds it; unifying it with another literal
    // variable merges the two; unifying it with anything else (a float,
    // a struct, a string) is an ordinary type mismatch. Whatever is
    // still open when the enclosing body ends is a plain `int`.

    /// A new literal variable for one unsuffixed integer literal.
    fn infer_int_literal(&mut self, value: i128, span: Span) -> TypeId {
        let ty = self.fresh_var();
        if let &SemaType::Var(n) = self.ctx.types.get(ty) {
            self.int_lit_vars.insert(n, vec![IntLitUse { value, span }]);
        }
        self.int_lit_sites.push((span, ty));
        ty
    }

    /// The var number if `ty` (already `apply`-ed) is a still-open
    /// integer-literal variable.
    fn int_lit_var_of(&self, ty: TypeId) -> Option<u32> {
        if ty == TypeId::ERROR { return None; }
        match self.ctx.types.get(ty) {
            &SemaType::Var(n) if self.int_lit_vars.contains_key(&n) => Some(n),
            _ => None,
        }
    }

    /// Range-check every literal behind `var` against `target`, then bind
    /// the variable to it. `target_ty` must be a concrete integer type
    /// (the caller establishes that via `int_type_range`).
    fn bind_int_lit(&mut self, var: u32, target_ty: TypeId, range: (i128, i128, &'static str)) {
        let (min, max, name) = range;
        let uses = self.int_lit_vars.remove(&var).unwrap_or_default();
        for u in uses {
            if u.value < min || u.value > max {
                self.errors.add_type_error(TypeError::IntLiteralOutOfRange {
                    suffix:   name,
                    raw:      u.value.unsigned_abs() as u64,
                    negative: u.value < 0,
                    inferred: true,
                    span:     u.span,
                });
            }
        }
        self.unifier.bind(var, target_ty);
    }

    /// Settle an open literal variable as plain `int`.
    fn default_int_lit_var(&mut self, var: u32) {
        let int_ty = self.int_ty();
        let range  = int_type_range(&SemaType::Int).expect("int is an integer type");
        self.bind_int_lit(var, int_ty, range);
    }

    /// If `ty` is still an open literal variable, settle it as `int` and
    /// return `int`; otherwise return `ty` untouched. For the few places
    /// that need a concrete shape right now (a format hole, an `await`
    /// operand, a method receiver) rather than a type to unify later.
    ///
    /// The returned id is always the *resolved* type: a variable that an
    /// earlier call already settled must not come back as the stale
    /// unresolved id.
    fn default_if_int_lit(&mut self, ty: TypeId) -> TypeId {
        let applied = self.apply(ty);
        match self.int_lit_var_of(applied) {
            Some(var) => { self.default_int_lit_var(var); self.int_ty() }
            None      => applied,
        }
    }

    /// The literal-variable half of `unify`. `a` and `b` are already
    /// `apply`-ed and distinct. Returns `true` when at least one side was
    /// an open literal variable and this handled the pair completely.
    fn try_unify_int_lit(&mut self, a: TypeId, b: TypeId, span: Span) -> bool {
        let la = self.int_lit_var_of(a);
        let lb = self.int_lit_var_of(b);
        let (lit_var, lit_ty, other) = match (la, lb) {
            (None, None) => return false,
            // Two literals: one variable now stands for both.
            (Some(x), Some(y)) => {
                self.unifier.bind(x, b);
                if let Some(uses) = self.int_lit_vars.remove(&x) {
                    self.int_lit_vars.entry(y).or_default().extend(uses);
                }
                return true;
            }
            (Some(x), None) => (x, a, b),
            (None, Some(y)) => (y, b, a),
        };

        let other_ty = self.ctx.types.get(other).clone();

        // A concrete integer type: range-check and bind.
        if let Some(range) = int_type_range(&other_ty) {
            self.bind_int_lit(lit_var, other, range);
            return true;
        }
        match other_ty {
            // An ordinary variable simply becomes the literal's type.
            SemaType::Var(o) => { self.unifier.bind(o, lit_ty); true }
            // Unknown absorbs without cascading, as everywhere in `unify`.
            SemaType::Unknown => true,
            // `maybe_int == 5`: the literal meets the payload type.
            SemaType::Optional(inner) => { self.unify(inner, lit_ty, span); true }
            // Anything else cannot be an integer. Settle the literal as
            // `int` and unify again, so the mismatch (or scope-escape)
            // diagnostic reads exactly as it did before literals were
            // context-typed.
            _ => {
                self.default_int_lit_var(lit_var);
                self.unify(a, b, span);
                true
            }
        }
    }

    /// End of a body (or const initializer): record the width each
    /// literal ended up with for the interpreter, and settle every
    /// literal nothing constrained as plain `int`.
    /// Settle everything a body left open: literal types first (an
    /// obligation on `{integer}` needs the settled type), then bound
    /// obligations.
    fn finish_body_checks(&mut self) {
        self.finish_int_literals();
        self.check_obligations();
    }

    fn finish_int_literals(&mut self) {
        let sites = std::mem::take(&mut self.int_lit_sites);
        for (span, ty) in sites {
            let applied = self.apply(ty);
            if let Some(var) = self.int_lit_var_of(applied) {
                self.default_int_lit_var(var);
                continue;
            }
            if applied == TypeId::ERROR { continue; }
            // `int`, `long`, `i64` and `isize` are all the interpreter's
            // default 64-bit value, so they need no entry.
            let suffix = match self.ctx.types.get(applied) {
                SemaType::I8    => Some(IntSuffix::I8),
                SemaType::I16   => Some(IntSuffix::I16),
                SemaType::I32   => Some(IntSuffix::I32),
                SemaType::U8    => Some(IntSuffix::U8),
                SemaType::U16   => Some(IntSuffix::U16),
                SemaType::U32 | SemaType::Uint              => Some(IntSuffix::U32),
                SemaType::U64 | SemaType::Ulong             => Some(IntSuffix::U64),
                SemaType::Usize                             => Some(IntSuffix::Usize),
                _ => None,
            };
            if let Some(s) = suffix {
                self.ctx.int_literal_types.insert(span, s);
            }
        }
    }

    // ── Literal typing ────────────────────────────────────────────

    fn infer_literal<'ast>(&mut self, lit: &Literal<'ast>, span: Span) -> TypeId {
        match lit {
            // Unsuffixed: the type comes from where the literal is used
            // (docs/PARKED_IDEAS.md, "Unsuffixed integer literals"), so it
            // starts as a literal type variable, see `infer_int_literal`.
            Literal::Int(n)   => self.infer_int_literal(*n as i128, span),
            // `TYPE-120`: a suffix (explicit or the lexer's own
            // auto-promoted implicit `u64`, see `Literal::TypedInt`'s doc
            // comment) claims a specific width -- check the value
            // actually fits it now, at the one place both are known
            // together, rather than silently wrapping it at runtime.
            Literal::TypedInt { raw, suffix } => {
                if !int_suffix_fits(*raw, *suffix) {
                    self.errors.add_type_error(TypeError::IntLiteralOutOfRange {
                        suffix:   suffix.as_str(),
                        raw:      *raw,
                        negative: false,
                        inferred: false,
                        span,
                    });
                }
                self.int_suffix_sema_type(*suffix)
            }
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
                // D3b: an interpolated string is allocated where it is written (docs/ARENA_STRINGS.md).
                let str_ty = self.ctx.types.intern(SemaType::Str);
                self.maybe_arena_ref(str_ty)
            }
        }
    }

    /// The `SemaType` an explicit (or auto-promoted implicit) integer
    /// literal suffix resolves to. Always the Rust-named variant --
    /// there's no suffix syntax for the C#-named twins (`Uint`/`Long`/
    /// `Ulong`), see `IntSuffix`'s own doc comment.
    fn int_suffix_sema_type(&mut self, suffix: IntSuffix) -> TypeId {
        match suffix {
            IntSuffix::I8    => self.ctx.types.intern(SemaType::I8),
            IntSuffix::I16   => self.ctx.types.intern(SemaType::I16),
            IntSuffix::I32   => self.ctx.types.intern(SemaType::I32),
            IntSuffix::I64   => self.ctx.types.intern(SemaType::I64),
            IntSuffix::U8    => self.ctx.types.intern(SemaType::U8),
            IntSuffix::U16   => self.ctx.types.intern(SemaType::U16),
            IntSuffix::U32   => self.ctx.types.intern(SemaType::U32),
            IntSuffix::U64   => self.ctx.types.intern(SemaType::U64),
            IntSuffix::Isize => self.ctx.types.intern(SemaType::Isize),
            IntSuffix::Usize => self.ctx.types.intern(SemaType::Usize),
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
        // Precision, `+`, zero-pad and a numeric base are only implemented
        // for plain `int` (and float/double/str) at runtime, see
        // `apply_format_spec`, so a spec that depends on the type settles
        // an open literal (`{255:x}`, or `n` in `let n = 255` used as
        // `{n:+}`) as `int`. Width, fill, alignment and `?` do not depend
        // on the type and leave it open for a later use to decide.
        let type_dependent = spec.precision.is_some() || spec.sign_plus
            || spec.zero_pad || spec.base.is_some();
        let hole_ty = if type_dependent {
            self.default_if_int_lit(hole_ty)
        } else {
            self.apply(hole_ty)
        };
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
        let (lhs, rhs) = (self.peel_arena_str(lhs), self.peel_arena_str(rhs));
        match op {
            BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Rem
            | BinOp::BitAnd | BinOp::BitOr | BinOp::BitXor | BinOp::Shl | BinOp::Shr => {
                self.unify(lhs, rhs, span);
                self.apply(lhs)
            }
            BinOp::Eq | BinOp::Ne => {
                self.unify(lhs, rhs, span);
                let text = if matches!(op, BinOp::Eq) { "==" } else { "!=" };
                self.check_operator_bound(lhs, "PartialEq", text, span);
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
                let bare = self.peel_wrappers(lhs);
                if matches!(self.ctx.types.get(bare), SemaType::Param(_)) {
                    let text = match op {
                        BinOp::Lt => "<", BinOp::Le => "<=", BinOp::Gt => ">", _ => ">=",
                    };
                    self.check_operator_bound(bare, "PartialOrd", text, span);
                    return self.bool_ty();
                }
                let resolved = self.apply(lhs);
                // A hand-written `PartialOrd` or `Ord` for a user type; for a
                // generic one it may ask something of the arguments.
                let named_written = match self.ctx.types.get(resolved).clone() {
                    SemaType::Named { def, args } => {
                        let mut any = false;
                        for name in ["PartialOrd", "Ord"] {
                            let Some(t) = self.prelude_defs.get(name).copied() else { continue; };
                            if self.trait_impls.contains_key(&(t, def))
                                && self.impl_status(t, def, &args) != Some(false) {
                                any = true;
                            }
                        }
                        any
                    }
                    _ => false,
                };
                let orderable = match self.ctx.types.get(resolved) {
                    SemaType::Int | SemaType::Float | SemaType::Double | SemaType::Str => true,
                    // The sized integers and `f32`/`f64` were missing from
                    // this list, so `a < b` on two `u32`s was TYPE-118
                    // even though the interpreter's `sized_int_binop`
                    // compares them fine.
                    SemaType::Uint | SemaType::Long | SemaType::Ulong
                    | SemaType::I8 | SemaType::I16 | SemaType::I32 | SemaType::I64
                    | SemaType::U8 | SemaType::U16 | SemaType::U32 | SemaType::U64
                    | SemaType::Isize | SemaType::Usize
                    | SemaType::F32 | SemaType::F64 => true,
                    // Two unconstrained literals (`1 < 2`) are still just
                    // an integer of some width, and integers are ordered.
                    SemaType::Var(n) if self.int_lit_vars.contains_key(n) => true,
                    // A built-in type with a hand-written `PartialOrd` or `Ord`
                    // (`bool`, `char`, a tuple, an array).
                    _ if self.runtime_impl_key(resolved).is_some() => {
                        let written = |name: &str| self.prelude_defs.get(name)
                            .is_some_and(|t| self.builtin_implements(resolved, *t));
                        written("PartialOrd") || written("Ord")
                    }
                    // Derived, or hand-written (`impl PartialOrd`, or `impl Ord`
                    // which needs it).
                    SemaType::Named { def, .. } => {
                        self.struct_derives.get(def)
                            .is_some_and(|d| d.contains("PartialOrd") || d.contains("Ord"))
                            || named_written
                    }
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

    /// `x.eq(y)`, `x.cmp(y)`, `x.clone()` and the other prelude methods on a
    /// value that is not a type parameter. `Some(type)` when `method` is a
    /// prelude method and the receiver's type has that trait; the call is
    /// recorded in `trait_call_sites`, so the interpreter runs the prelude
    /// method (natively for a built-in or derived type). `None` leaves the
    /// call to the ordinary paths, which report an unknown method.
    fn try_prelude_method<'ast>(
        &mut self,
        receiver_ty: TypeId,
        method:      &str,
        args:        &[Arg<'ast>],
        callee_span: Span,
        call_span:   Span,
    ) -> Option<TypeId> {
        if self.prelude_traits.is_empty() { return None; }
        let trait_def = self.prelude_traits.keys().copied().find(|d| {
            self.traits.get(d).is_some_and(|i| i.methods.iter().any(|m| m.name == method && m.shape.has_self))
        })?;
        let name = self.prelude_traits[&trait_def];
        let bare = self.peel_wrappers(receiver_ty);
        match self.ctx.types.get(bare) {
            SemaType::Param(_) | SemaType::Var(_) | SemaType::Unknown => return None,
            SemaType::Named { def, .. } => {
                if self.struct_methods.get(def)
                    .is_some_and(|ms| ms.iter().any(|(n, _)| n == method)) {
                    return None;
                }
            }
            _ => {}
        }
        if self.prelude_status(name, bare) != Some(true) { return None; }
        let ret = self.call_through_bounds(bare, &[BoundRef { trait_def, args: Vec::new() }], method, args, callee_span, call_span);
        // `clone` hands back the receiver's own type, tier tag included.
        if method == "clone" { return Some(self.maybe_arena_ref(receiver_ty)); }
        Some(ret)
    }

    /// `a == b`, `a < b` and the like on a value of a type-parameter type:
    /// the operator needs the prelude trait in the parameter's bounds
    /// (`PartialEq` for `==` and `!=`, `PartialOrd` for ordering; `Ord` and
    /// `Eq` imply them). A concrete type is not checked here.
    fn check_operator_bound(&mut self, operand: TypeId, trait_name: &'static str, op: &'static str, span: Span) {
        let ty = self.peel_wrappers(operand);
        if !matches!(self.ctx.types.get(ty), SemaType::Param(_)) { return; }
        if self.prelude_status(trait_name, ty) == Some(true) { return; }
        // The prelude trait itself (an operator inside its own default body
        // on `Self`) is covered by the Self bound; a shadowed prelude name
        // has no prelude trait to require.
        if !self.prelude_defs.contains_key(trait_name) { return; }
        let param = self.param_display_name(ty);
        self.errors.add_type_error(TypeError::OperatorNeedsBound { op, param, trait_name, span });
    }

    // ── Type unwrapping helpers ───────────────────────────────────

    fn unwrap_task(&mut self, ty: TypeId, span: Span) -> TypeId {
        let ty = self.default_if_int_lit(ty);
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
        let ty = self.default_if_int_lit(ty);
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

    // ── D1: borrowed arena references across calls ───────────────────
    // Rules and reasoning: docs/ARENA_BORROW_RULES.md.

    /// The `where` clause lifetimes of a declared function, kept for rule R2.
    fn record_where_lifetimes<'ast>(&mut self, def_id: DefId, f: &FunctionDecl<'ast>) {
        let mut names: HashSet<String> = HashSet::new();
        for lp in f.lifetime_params.iter() {
            if let Some(c) = lp.constraint {
                names.insert(c.longer.to_string());
                names.insert(c.shorter.to_string());
            }
        }
        self.fn_where_lifetimes.insert(def_id, names);
    }

    /// The function a call names, when the callee is a plain identifier that resolves to a
    /// non-async top-level function. A closure, a function-typed variable or a method gives
    /// `None`: their bodies and captures are not visible from the call, so no arena borrow is
    /// allowed through them yet.
    fn callee_fn_def<'ast>(&self, callee: &Expr<'ast>) -> Option<DefId> {
        if !matches!(callee.kind, ExprKind::Ident(_)) { return None; }
        let def_id = self.ctx.resolutions.get(callee.span)?;
        match self.ctx.symbols.lookup(def_id).kind {
            DefKind::Function { is_async: false, .. } => Some(def_id),
            _ => None,
        }
    }

    /// How many references in `ty` carry the named lifetime `name`. Exhaustive on purpose: a
    /// new `SemaType` variant has to say here whether it can hold a reference.
    fn count_ref_lifetime(&self, ty: TypeId, name: &str) -> usize {
        match self.ctx.types.get(ty).clone() {
            SemaType::Reference { lifetime, inner, .. } => {
                let here = (lifetime.as_deref() == Some(name)) as usize;
                here + self.count_ref_lifetime(inner, name)
            }
            SemaType::List(a) | SemaType::Set(a) | SemaType::Queue(a) | SemaType::Stack(a)
            | SemaType::InlineList(a) | SemaType::Linqerizer(a) | SemaType::Slice(a)
            | SemaType::Pool(a) | SemaType::Handle(a) | SemaType::Fallible(a)
            | SemaType::Task(a) | SemaType::Optional(a) | SemaType::GcRef(a)
            | SemaType::Unique(a) | SemaType::Shared(a) | SemaType::SyncShared(a)
            | SemaType::Array { elem: a, .. }
            | SemaType::ArenaRef { inner: a, .. } | SemaType::PoolRef { inner: a, .. }
            | SemaType::OwnedRef { inner: a, .. } => self.count_ref_lifetime(a, name),
            SemaType::Dictionary(a, b) =>
                self.count_ref_lifetime(a, name) + self.count_ref_lifetime(b, name),
            SemaType::Tuple(ts) => ts.iter().map(|t| self.count_ref_lifetime(*t, name)).sum(),
            SemaType::Named { args, .. } =>
                args.iter().map(|t| self.count_ref_lifetime(*t, name)).sum(),
            SemaType::Function { params, return_type, .. } =>
                params.iter().map(|t| self.count_ref_lifetime(*t, name)).sum::<usize>()
                    + self.count_ref_lifetime(return_type, name),
            SemaType::Int | SemaType::Uint | SemaType::Long | SemaType::Ulong
            | SemaType::Float | SemaType::Double | SemaType::Bool | SemaType::Char
            | SemaType::Str | SemaType::Void
            | SemaType::I8 | SemaType::I16 | SemaType::I32 | SemaType::I64
            | SemaType::U8 | SemaType::U16 | SemaType::U32 | SemaType::U64
            | SemaType::F32 | SemaType::F64 | SemaType::Isize | SemaType::Usize
            | SemaType::Param(_) | SemaType::Var(_) | SemaType::Null | SemaType::Unknown => 0,
        }
    }

    /// Could a value of this type hold on to a reference (or a closure or task that captured
    /// one)? Struct fields and enum payloads are followed; anything not yet known counts as yes.
    fn can_carry_ref(&self, ty: TypeId, seen: &mut HashSet<DefId>) -> bool {
        match self.ctx.types.get(ty).clone() {
            SemaType::Reference { .. } | SemaType::Function { .. } | SemaType::Task(_)
            | SemaType::Param(_) | SemaType::Var(_) | SemaType::Unknown => true,
            SemaType::List(a) | SemaType::Set(a) | SemaType::Queue(a) | SemaType::Stack(a)
            | SemaType::InlineList(a) | SemaType::Linqerizer(a) | SemaType::Slice(a)
            | SemaType::Pool(a) | SemaType::Handle(a) | SemaType::Fallible(a)
            | SemaType::Optional(a) | SemaType::GcRef(a)
            | SemaType::Unique(a) | SemaType::Shared(a) | SemaType::SyncShared(a)
            | SemaType::Array { elem: a, .. }
            | SemaType::ArenaRef { inner: a, .. } | SemaType::PoolRef { inner: a, .. }
            | SemaType::OwnedRef { inner: a, .. } => self.can_carry_ref(a, seen),
            SemaType::Dictionary(a, b) =>
                self.can_carry_ref(a, seen) || self.can_carry_ref(b, seen),
            SemaType::Tuple(ts) => ts.iter().any(|t| self.can_carry_ref(*t, seen)),
            SemaType::Named { def, args } => {
                if args.iter().any(|t| self.can_carry_ref(*t, seen)) { return true; }
                if !seen.insert(def) { return false; }
                if let Some(fields) = self.struct_fields.get(&def) {
                    if fields.iter().any(|(_, t)| self.can_carry_ref(*t, seen)) { return true; }
                }
                if let Some(shapes) = self.enum_variants.get(&def) {
                    for (_, shape) in shapes.iter() {
                        let hit = match shape {
                            VariantShape::None | VariantShape::Discriminant => false,
                            VariantShape::Tuple(ts) => ts.iter().any(|t| self.can_carry_ref(*t, seen)),
                            VariantShape::Struct(fs) => fs.iter().any(|(_, t)| self.can_carry_ref(*t, seen)),
                        };
                        if hit { return true; }
                    }
                }
                false
            }
            SemaType::Int | SemaType::Uint | SemaType::Long | SemaType::Ulong
            | SemaType::Float | SemaType::Double | SemaType::Bool | SemaType::Char
            | SemaType::Str | SemaType::Void
            | SemaType::I8 | SemaType::I16 | SemaType::I32 | SemaType::I64
            | SemaType::U8 | SemaType::U16 | SemaType::U32 | SemaType::U64
            | SemaType::F32 | SemaType::F64 | SemaType::Isize | SemaType::Usize
            | SemaType::Null => false,
        }
    }

    /// Could a reference be stored INTO a value of this parameter type? Look through the
    /// reference layers to the pointee: a `Vec2` of two ints cannot take one.
    fn param_is_sink_free(&self, ty: TypeId) -> bool {
        let mut t = ty;
        loop {
            match self.ctx.types.get(t).clone() {
                SemaType::Reference { inner, .. } | SemaType::OwnedRef { inner, .. }
                | SemaType::GcRef(inner) | SemaType::ArenaRef { inner, .. }
                | SemaType::PoolRef { inner, .. } => t = inner,
                _ => break,
            }
        }
        !self.can_carry_ref(t, &mut HashSet::new())
    }

    /// Rules R2 to R4: may parameter `idx` of function `def` receive a reference into the
    /// caller's arena? Only if the callee has no way to hand it back or keep it.
    /// R2  its lifetime is not named in a `where` clause, and a named one appears nowhere else
    ///     in the signature;
    /// R3  the result cannot carry a reference (no reference, closure or task inside it);
    /// R4  no other parameter is a place a reference could be stored into.
    fn param_is_borrow_only(&self, def: DefId, fn_ty: TypeId, idx: usize) -> bool {
        let Some(where_names) = self.fn_where_lifetimes.get(&def) else { return false };
        let SemaType::Function { params, return_type, .. } = self.ctx.types.get(fn_ty).clone() else { return false };
        let Some(&p) = params.get(idx) else { return false };
        let SemaType::Reference { lifetime, .. } = self.ctx.types.get(p).clone() else { return false };
        if let Some(l) = &lifetime {
            if where_names.contains(l) { return false; }
            let uses: usize = params.iter()
                .map(|q| self.count_ref_lifetime(*q, l.as_str()))
                .sum::<usize>()
                + self.count_ref_lifetime(return_type, l.as_str());
            if uses != 1 { return false; }
        }
        if self.can_carry_ref(return_type, &mut HashSet::new()) { return false; }
        params.iter().enumerate().all(|(i, q)| i == idx || self.param_is_sink_free(*q))
    }

    /// Rule R1: `&x` for an arena-resident `x` carries its tag one layer in
    /// (`Reference { inner: ArenaRef { .. } }`, see `unify_struct_field`). For a borrow-only
    /// parameter, take the tag off so the call unifies like an ordinary borrow; the callee
    /// cannot return or store it, so nothing it produces needs the tag back.
    fn strip_arena_borrow(&mut self, arg_ty: TypeId) -> TypeId {
        let resolved = self.apply(arg_ty);
        if let SemaType::Reference { mutable, lifetime, inner } = self.ctx.types.get(resolved).clone() {
            let inner = self.apply(inner);
            if let SemaType::ArenaRef { inner: stripped, .. } = self.ctx.types.get(inner).clone() {
                return self.ctx.types.insert(SemaType::Reference { mutable, lifetime, inner: stripped });
            }
        }
        arg_ty
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
    ///
    /// The callee's declared bounds (found through its function type, so a
    /// function held in a variable keeps them) become one obligation per
    /// bounded parameter, reported at the first argument whose declared
    /// type is exactly that parameter (the call itself when there is none).
    fn call_return_type<'ast>(&mut self, callee: &Expr<'ast>, callee_ty: TypeId, args: &[Arg<'ast>], span: Span) -> TypeId {
        let resolved = self.apply(callee_ty);
        let callee_def = self.fn_type_defs.get(&resolved).copied();
        match self.ctx.types.get(resolved).clone() {
            SemaType::Function { params, return_type, generic_arity, .. } => {
                let fresh_args: Vec<TypeId> = (0..generic_arity).map(|_| self.fresh_var()).collect();
                let raw_params = params.clone();
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
                let borrow_def = self.callee_fn_def(callee);
                for (arg_idx, (arg, param_ty)) in args.iter().zip(params.iter()).enumerate() {
                    let arg_span = match &arg.kind {
                        ArgKind::Positional(e)       => e.span,
                        ArgKind::Named { value, .. } => value.span,
                    };
                    if let Some(arg_ty) = self.ctx.expr_type(arg_span).map(|t| self.apply(t)) {
                        let arg_ty = match borrow_def {
                            Some(def) if self.param_is_borrow_only(def, resolved, arg_idx) => self.strip_arena_borrow(arg_ty),
                            _ => arg_ty,
                        };
                        self.unify(*param_ty, arg_ty, arg_span);
                    }
                }
                if let Some(per_param) = callee_def.and_then(|d| self.generic_bounds.get(&d).cloned()) {
                    for (i, bounds) in per_param.iter().enumerate() {
                        let Some(&fresh) = fresh_args.get(i) else { break; };
                        let at = raw_params.iter()
                            .position(|p| matches!(self.ctx.types.get(*p), SemaType::Param(j) if *j == i))
                            .and_then(|j| args.get(j))
                            .map(|a| match &a.kind {
                                ArgKind::Positional(e)       => e.span,
                                ArgKind::Named { value, .. } => value.span,
                            })
                            .unwrap_or(span);
                        for b in bounds {
                            let applied: Vec<TypeId> = b.args.iter()
                                .map(|t| self.substitute(*t, &fresh_args))
                                .collect();
                            self.require_bound_args(fresh, b.trait_def, applied, at);
                        }
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

    /// GAP 2 / `docs/OUTLIVES_RULES.md` §8's "free win" — a plain
    /// struct-literal field normally goes through ordinary `unify`.
    /// One exception: the declaring struct is `edge` and the field's
    /// declared type is a *named*-lifetime reference (`&L T`). There,
    /// `unify`'s generic `scope_mismatch_side` rule (any `ArenaRef`-
    /// tagged value meeting a non-`ArenaRef` expected type is an
    /// escape) is too blunt — an `edge struct` field storing a
    /// reference built in the *same* arena the struct itself is being
    /// constructed in is exactly what `edge struct` exists for, not a
    /// violation. `MEMORY_MODEL.md` §9 already documented `is_edge` as
    /// fully decorative before this; this is the connection.
    ///
    /// Concretely: `&d` for an arena-resident `d` doesn't itself carry
    /// an outer `ArenaRef` tag — the tag lives one layer in, on the
    /// *pointee* (`Reference { inner: ArenaRef { inner: Data } }`, not
    /// `ArenaRef { inner: Reference { inner: Data } } }`), confirmed
    /// directly rather than assumed (an earlier version of this
    /// function checked the wrong layer and silently did nothing —
    /// caught by the probe script still failing after the "fix",
    /// not by re-reading the diff). So the tag has to be stripped from
    /// the *inner* type, reconstructing a plain `Reference` around it,
    /// before handing off to `unify` — not stripped from `val_ty`
    /// itself.
    ///
    /// Deferring doesn't mean skipping enforcement — it means routing
    /// it to `outlives_check.rs`'s own struct-literal boundary check
    /// instead (same loan-region machinery as the function-call case),
    /// which is what can actually tell "still live" from "already
    /// dead," not just "tagged with an arena at all." Every other case
    /// (non-`edge` structs, unnamed reference fields, non-arena
    /// values) falls through to plain `unify`, unchanged.
    fn unify_struct_field(&mut self, def_id: DefId, field_ty: TypeId, val_ty: TypeId, span: Span) {
        let is_edge = matches!(
            self.ctx.symbols.lookup(def_id).kind,
            DefKind::Struct { is_edge: true }
        );
        let field_is_named_ref = matches!(
            self.ctx.types.get(self.apply(field_ty)),
            SemaType::Reference { lifetime: Some(_), .. }
        );
        if is_edge && field_is_named_ref {
            let resolved_val = self.apply(val_ty);
            if let SemaType::Reference { mutable, lifetime, inner } = self.ctx.types.get(resolved_val).clone() {
                let resolved_inner = self.apply(inner);
                if let SemaType::ArenaRef { inner: stripped, .. } = self.ctx.types.get(resolved_inner).clone() {
                    let stripped_val_ty = self.ctx.types.insert(SemaType::Reference { mutable, lifetime, inner: stripped });
                    self.unify(field_ty, stripped_val_ty, span);
                    return;
                }
            }
        }
        self.unify(field_ty, val_ty, span);
    }

    fn unify(&mut self, a: TypeId, b: TypeId, span: Span) {
        let a = self.apply(a);
        let b = self.apply(b);
        if a == b { return; }

        // An integer-literal variable has its own binding rules (range
        // check, no implicit float, merging with other literals).
        if self.try_unify_int_lit(a, b, span) { return; }

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

        if self.structurally_compatible(a, b, span) { return; }

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
    ///
    /// `span` is where the outer unification happened. Every nested unify
    /// (a function's parameters and return type, a generic's arguments, a
    /// dictionary's key and value, an inner element type) reports at that
    /// span, so a mismatch deep inside `List<int>` against `List<string>`
    /// points at the offending expression instead of at offset 0.
    fn structurally_compatible(&mut self, a: TypeId, b: TypeId, span: Span) -> bool {
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
                self.unify(*ia, *ib, span);
            }
            self.unify(ra, rb, span);
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
                self.unify(*ia, *ib, span);
            }
            return true;
        }

        // `Dictionary` carries two type arguments, so it cannot go through
        // the single inner-pair extraction below, and it was missing from
        // this function entirely: `let d: Dictionary<string, int> =
        // Dictionary.new()` failed with a TypeMismatch against
        // `Dictionary<?T, ?T>` because the key and value arguments were
        // never unified. Same bug class as the `Set`/`Queue`/`Stack`
        // additions just below.
        let dict_pair: Option<(TypeId, TypeId, TypeId, TypeId)> = {
            match (self.ctx.types.get(a), self.ctx.types.get(b)) {
                (SemaType::Dictionary(ka, va), SemaType::Dictionary(kb, vb)) =>
                    Some((*ka, *va, *kb, *vb)),
                _ => None,
            }
        };
        if let Some((ka, va, kb, vb)) = dict_pair {
            self.unify(ka, kb, span);
            self.unify(va, vb, span);
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
            self.unify(ia, ib, span);
            true
        } else {
            false
        }
    }

    // ── Display helper ────────────────────────────────────────────

    fn display_type(&self, id: TypeId) -> String {
        if id == TypeId::ERROR { return "<error>".into(); }
        if self.int_lit_var_of(self.apply(id)).is_some() { return "{integer}".into(); }
        let mut text = self.ctx.types.get(id).display(&self.ctx.types, &self.ctx.symbols);
        // A type parameter is written by its name while its scope is in
        // force, and a trait's abstract `Self` as `Self`.
        if text.contains("<generic param #") {
            for (name, ty) in &self.current_generic_params {
                if let SemaType::Param(i) = self.ctx.types.get(*ty) {
                    text = text.replace(&format!("<generic param #{}>", i), name);
                }
            }
            if let Some(SemaType::Param(i)) = self.self_type.map(|t| self.ctx.types.get(t)) {
                text = text.replace(&format!("<generic param #{}>", i), "Self");
            }
        }
        text
    }
    }
