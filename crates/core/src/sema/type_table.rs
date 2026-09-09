// src/sema/type_table.rs
//! Type representation used during semantic analysis.
//!
//! This is distinct from `ast::types::Type<'ast>` (which is the *syntactic*
//! type written by the programmer). `SemaType` is the *semantic* type after
//! resolution: type variables have been eliminated, aliases expanded,
//! and arena lifetimes tracked.
//!
//! # Why two type representations?
//!
//! `ast::types::Type<'ast>` is exactly what the parser produced — it may
//! contain unresolved names (`TypeKind::Named { path: ["Foo"], .. }`),
//! infer placeholders (`TypeKind::Infer`), and unevaluated generics.
//!
//! `SemaType` is what the type checker produces after it has resolved all of
//! those. A `TypeId` is a cheap integer handle into the `TypeTable`.

#![allow(dead_code)]

use std::collections::HashMap;
use crate::sema::symbol_table::{DefId, SymbolTable};

// ── TypeId ────────────────────────────────────────────────────────

/// A stable integer handle for a semantic type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TypeId(pub usize);

impl TypeId {
    /// Sentinel for "we don't know yet" or "this expression had an error."
    pub const ERROR: TypeId = TypeId(usize::MAX);

    pub fn is_error(self) -> bool { self.0 == usize::MAX }
}

// ── Arena lifetime tracking ───────────────────────────────────────

/// A unique identity for one arena created by a `with arena` block.
/// Used to track which values carry arena references.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ArenaId(pub usize);

/// A unique identity for one pool created by a `with pool<T>(count)` block
/// — MEMORY_MODEL.md §10/§11. Structurally identical to `ArenaId` (both
/// are opaque scope identities compared for equality only), kept as a
/// distinct type rather than reusing `ArenaId` outright so `PoolRef`
/// values render and report as "&pool"/"with pool<...>(...)" instead of
/// silently mislabeling every pool diagnostic as an arena one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PoolId(pub usize);

/// Which kind of scope a value is bound to — used only by
/// `scope_ref_kind` to pick the right diagnostic (arena vs pool) without
/// duplicating the recursive walk twice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScopeKind { Arena, Pool }

// ── Semantic type ─────────────────────────────────────────────────

/// A fully-resolved semantic type.
///
/// Note: this is an owned, heap-allocated structure (not arena-allocated)
/// because it lives in the `TypeTable` which outlives any individual AST.
#[derive(Debug, Clone, PartialEq)]
pub enum SemaType {
    // ── Primitives ──────────────────────────────────────────────
    Int, Uint, Long, Ulong,
    Float, Double,
    Bool, Char, Str, Void,
    I8, I16, I32, I64,
    U8, U16, U32, U64,
    F32, F64, Isize, Usize,

    // ── Composite ───────────────────────────────────────────────
    List(TypeId),
    Dictionary(TypeId, TypeId),
    Set(TypeId),
    Queue(TypeId),
    Stack(TypeId),
    /// `InlineList<T>` — DATASTRUCTURES.md: fixed-capacity, stack/inline
    /// storage, genuinely separate from `List<T>`/`Pool<T>`. Capacity is
    /// deliberately not part of the type (same reasoning as `List<T>`'s
    /// current length not being part of its type either) — it's a
    /// checked, literal-int-only constructor argument instead.
    InlineList(TypeId),
    /// `Linqerizer<T>` — HIGH-tier-only lazy query builder
    /// (`docs/DATASTRUCTURES.md` §6). Obtained via `List<T>.query()`
    /// (`HIGH_ONLY`-gated, same real, tested infrastructure
    /// `MethodInWrongTier`/`TIER-008` already had sitting unused). Never
    /// arena/pool-tagged — the only way to construct one is already
    /// restricted to HIGH tier, so there's no MID-tier tagging case to
    /// handle, unlike every other builtin collection here.
    Linqerizer(TypeId),
    Tuple(Vec<TypeId>),
    Array { len: u64, elem: TypeId },
    Slice(TypeId),
    /// `Pool<T>` — the manager/handle-issuing collection itself, as
    /// opposed to `PoolRef` below (the scope-tag wrapper indicating a
    /// value's lifetime is bound to a specific `with pool<T>(count) { }`
    /// block). MEMORY_MODEL.md §10/§11.
    Pool(TypeId),
    /// `Handle<T>` — a generational `(index, generation)` handle returned
    /// by `Pool<T>.acquire()`. Not itself the stored `T`; `.get(handle)`
    /// on the owning `Pool<T>` resolves it, checked against the slot's
    /// current generation so a stale handle fails safely instead of
    /// silently reading whatever's been written into a reused slot.
    Handle(TypeId),

    // ── User-defined ─────────────────────────────────────────────
    /// A resolved named type with generic arguments substituted in.
    Named {
        def:  DefId,
        args: Vec<TypeId>,
    },

    /// A reference to the Nth generic parameter of the struct/enum/fn
    /// currently being declared — e.g. inside `struct Box<T> { value: T }`,
    /// `T`'s field type resolves to `Param(0)`, not a concrete type. Only
    /// ever appears in the *raw* stored signature of a generic
    /// struct/enum/fn (`struct_fields`/`struct_methods`/`enum_variants`/
    /// `SemaType::Function.generic_arity` in `type_infer.rs`); every real
    /// use site substitutes `Param(i) -> args[i]` before a value of this
    /// shape is unified, displayed, or otherwise treated as a concrete
    /// type. See `type_infer.rs`'s `substitute`.
    Param(usize),

    // ── Modifiers ────────────────────────────────────────────────
    /// `T!` — may fail.
    Fallible(TypeId),
    /// `Task<T>` — async result.
    Task(TypeId),
    /// `T?` — optional / nullable.
    Optional(TypeId),

    // ── References ───────────────────────────────────────────────
    /// A GC-managed reference (HIGH tier).  No lifetime needed.
    GcRef(TypeId),

    /// A reference into an arena.  Carries the `ArenaId` of its origin.
    /// The tier checker uses this to detect cross-boundary escapes.
    ArenaRef {
        arena:   ArenaId,
        mutable: bool,
        inner:   TypeId,
    },

    /// A reference scoped to a `with pool<T>(count) { }` block. Carries
    /// the `PoolId` of its origin — structurally the same escape-boundary
    /// mechanism as `ArenaRef` (MEMORY_MODEL.md §11: "same relationship
    /// `with arena(...)` already has"), kept as its own variant purely so
    /// diagnostics say "&pool"/"with pool<...>(...)" accurately instead
    /// of reusing arena's wording for a different block kind.
    PoolRef {
        pool:    PoolId,
        mutable: bool,
        inner:   TypeId,
    },

    /// A LOW-tier owned reference (borrow-checked in Phase 4).
    OwnedRef {
        mutable: bool,
        inner:   TypeId,
    },

    /// An explicit, user-written reference: `&T`/`ref T` (shared) or
    /// `&mut T`/`ref mut T` (mutable), with an optional named lifetime
    /// (`&L T`/`ref L T`) — surface syntax from `TypeKind::Reference`,
    /// or the result of a `Borrow` expression (`&x`/`ref x`). Orthogonal
    /// to `OwnedRef`/`ArenaRef`/`GcRef`: this is "an alias to a place,"
    /// not "how the pointee's own memory is managed" — `inner` is
    /// whatever `TypeId` the borrowed place already had, tier wrapper
    /// and all. `lifetime` is carried through and (as of the well-
    /// formedness pass, sema/lifetime_check.rs) checked to be a real
    /// declared name, but not yet checked against real usage: the
    /// actual outlives/subset fixed point is still the borrow checker's
    /// job, still unbuilt (see docs/MEMORY_MODEL.md §9).
    Reference {
        mutable:  bool,
        lifetime: Option<String>,
        inner:    TypeId,
    },

    // ── Ownership model ─────────────────────────────────────────
    // `Unique<T>`/`Shared<T>`/`SyncShared<T>` — DATASTRUCTURES.md
    // "Decisions locked in"; MEMORY_MODEL.md §9. Orthogonal to the tier
    // wrappers (`GcRef`/`ArenaRef`/`OwnedRef`/`PoolRef` answer "where does
    // this value's memory live"), the same relationship `Reference` above
    // already has to them: this axis answers "who owns it and how," not
    // "where does it live." A `Unique<T>` is not itself a place-alias or a
    // tier tag — it can appear as a struct field, a return type, or be
    // wrapped in any of the tier/reference wrappers above it, same as any
    // other type. Move *enforcement* (the actual borrow-checker pass that
    // reads this to reject a used-after-moved `Unique<T>`) is still
    // unbuilt — this lands the type only, per facts.rs's own note that
    // building move tracking against an unsettled ownership-type story
    // would mean building it twice.
    //
    // No dedicated `TypeKind` variant exists for these (unlike
    // `List`/`Set`/`Queue`/etc.) — plain `Identifier<Args>` parsing
    // already produces exactly the shape needed
    // (`TypeKind::Named { path: ["Unique"], args: [T] }`), so
    // `ast_type_to_sema` special-cases the three names directly instead
    // of adding parser surface for something the grammar already parses.
    //
    // Single-owner, move-only (`Box<T>`-shaped). No `mutable` field of its
    // own — mutability of a `Unique`-owned value is a property of its
    // *binding* (`let` vs `let mut`), not of the wrapper, matching how a
    // plain (unwrapped) value's mutability already works.
    Unique(TypeId),
    /// Reference-counted, shared, clone-based (`Rc<T>`-shaped). Multiple
    /// owners are legitimate by construction, so this is never subject to
    /// move-checking the way `Unique<T>` is — cloning, not moving, is the
    /// operation that matters here.
    Shared(TypeId),
    /// Atomically reference-counted, shared, `Send`+`Sync` (`Arc<T>`-
    /// shaped). Same non-move semantics as `Shared<T>`; kept as a
    /// distinct variant (not a `Shared<T>` with a flag) purely so
    /// diagnostics and any future `Send`/`Sync` boundary check can name it
    /// accurately, same reasoning `PoolRef` was kept distinct from
    /// `ArenaRef` for.
    SyncShared(TypeId),

    // ── Function types ───────────────────────────────────────────
    Function {
        params:      Vec<TypeId>,
        return_type: TypeId,
        is_fallible: bool,
        /// How many of the *declaring* fn/method's own generic params
        /// (`fn identity<T>(x: T) T`) appear as `Param(i)` placeholders
        /// inside `params`/`return_type`. Zero for non-generic functions,
        /// closures, and bare `fn(int) bool` type annotations — those
        /// never contain a `Param`. A call site uses this to know how
        /// many fresh `Var`s to allocate before substituting, without
        /// having to re-scan `params`/`return_type` for the highest
        /// `Param` index actually used (a param declared but never
        /// referenced in a signature, e.g. only used inside the body,
        /// would otherwise be invisible to that scan).
        generic_arity: usize,
    },

    // ── Inference variables ──────────────────────────────────────
    /// An unsolved type variable — replaced by unification.
    Var(u32),

    // ── Null / unknown ───────────────────────────────────────────
    Null,
    Unknown,
}

impl SemaType {
    /// Returns which scope kind (arena or pool), if any, this type — or
    /// anything it contains — is bound to. Used by the tier checker both
    /// to detect cross-boundary escapes (formerly `contains_arena_ref`,
    /// generalized in MEMORY_MODEL.md §11 to also recognize `PoolRef`)
    /// and to pick the accurately-worded diagnostic at the call site,
    /// since arena and pool escapes are reported through different
    /// `TierError` variants.
    pub fn scope_ref_kind(&self, table: &TypeTable) -> Option<ScopeKind> {
        match self {
            SemaType::ArenaRef { .. } => Some(ScopeKind::Arena),
            SemaType::PoolRef { .. }  => Some(ScopeKind::Pool),
            SemaType::List(t)
            | SemaType::Set(t)
            | SemaType::Queue(t)
            | SemaType::Stack(t)
            | SemaType::InlineList(t)
            | SemaType::Linqerizer(t)
            | SemaType::Slice(t)
            | SemaType::Fallible(t)
            | SemaType::Task(t)
            | SemaType::Optional(t)
            | SemaType::GcRef(t)
            | SemaType::Pool(t)
            | SemaType::Handle(t)
            | SemaType::OwnedRef { inner: t, .. }
            | SemaType::Reference { inner: t, .. }
            | SemaType::Unique(t)
            | SemaType::Shared(t)
            | SemaType::SyncShared(t) => {
                table.get(*t).scope_ref_kind(table)
            }
            SemaType::Dictionary(k, v) => {
                table.get(*k).scope_ref_kind(table)
                    .or_else(|| table.get(*v).scope_ref_kind(table))
            }
            SemaType::Tuple(ts) => ts.iter().find_map(|t| table.get(*t).scope_ref_kind(table)),
            SemaType::Named { args, .. } => {
                args.iter().find_map(|t| table.get(*t).scope_ref_kind(table))
            }
            SemaType::Function { params, return_type, .. } => {
                params.iter().find_map(|t| table.get(*t).scope_ref_kind(table))
                    .or_else(|| table.get(*return_type).scope_ref_kind(table))
            }
            _ => None,
        }
    }

    /// A short human-readable name for error messages.
    ///
    /// Every variant is handled explicitly — no catch-all. A prior version
    /// of this function fell through a `_ => "<type>".into()` arm for
    /// anything past the handful of variants written out by hand, which
    /// silently produced the useless literal string `<type>` in real user-
    /// facing diagnostics for `Dictionary`, `Queue`, `Stack`, `Set`, `Tuple`,
    /// `Array`, `Slice`, `Named` (every struct/enum!), `GcRef`, `OwnedRef`,
    /// `Function` (every closure!), and every fixed-width numeric alias.
    /// Caught via `err_arena_escapes_closure_capture.ubl` rendering
    /// `&arena <type>` instead of a real function-type string. Composite
    /// arms recurse with `display(table, symbols)`, so nesting (e.g. a
    /// `List<Dictionary<string, Foo>>`) resolves all the way down instead
    /// of bottoming out at the first unhandled inner type.
    pub fn display(&self, table: &TypeTable, symbols: &SymbolTable) -> String {
        match self {
            SemaType::Int     => "int".into(),
            SemaType::Uint    => "uint".into(),
            SemaType::Long    => "long".into(),
            SemaType::Ulong   => "ulong".into(),
            SemaType::Float   => "float".into(),
            SemaType::Double  => "double".into(),
            SemaType::Bool    => "bool".into(),
            SemaType::Char    => "char".into(),
            SemaType::Str     => "string".into(),
            SemaType::Void    => "void".into(),
            SemaType::Null    => "null".into(),

            SemaType::I8    => "i8".into(),
            SemaType::I16   => "i16".into(),
            SemaType::I32   => "i32".into(),
            SemaType::I64   => "i64".into(),
            SemaType::U8    => "u8".into(),
            SemaType::U16   => "u16".into(),
            SemaType::U32   => "u32".into(),
            SemaType::U64   => "u64".into(),
            SemaType::F32   => "f32".into(),
            SemaType::F64   => "f64".into(),
            SemaType::Isize => "isize".into(),
            SemaType::Usize => "usize".into(),

            SemaType::List(t)  => format!("List<{}>", table.get(*t).display(table, symbols)),
            SemaType::Set(t)   => format!("Set<{}>", table.get(*t).display(table, symbols)),
            SemaType::Queue(t) => format!("Queue<{}>", table.get(*t).display(table, symbols)),
            SemaType::Stack(t) => format!("Stack<{}>", table.get(*t).display(table, symbols)),
            SemaType::InlineList(t) => format!("InlineList<{}>", table.get(*t).display(table, symbols)),
            SemaType::Linqerizer(t) => format!("Linqerizer<{}>", table.get(*t).display(table, symbols)),
            SemaType::Slice(t) => format!("[]{}", table.get(*t).display(table, symbols)),

            SemaType::Dictionary(k, v) => format!(
                "Dictionary<{}, {}>",
                table.get(*k).display(table, symbols),
                table.get(*v).display(table, symbols),
            ),

            SemaType::Tuple(ts) => format!(
                "({})",
                ts.iter()
                    .map(|t| table.get(*t).display(table, symbols))
                    .collect::<Vec<_>>()
                    .join(", "),
            ),

            SemaType::Array { len, elem } =>
                format!("[{}]{}", len, table.get(*elem).display(table, symbols)),

            SemaType::Optional(t) => format!("{}?", table.get(*t).display(table, symbols)),
            SemaType::Fallible(t) => format!("{}!", table.get(*t).display(table, symbols)),
            SemaType::Task(t)     => format!("Task<{}>", table.get(*t).display(table, symbols)),

            SemaType::ArenaRef { inner, .. } =>
                format!("&arena {}", table.get(*inner).display(table, symbols)),
            SemaType::PoolRef { inner, .. } =>
                format!("&pool {}", table.get(*inner).display(table, symbols)),
            SemaType::Pool(t) =>
                format!("Pool<{}>", table.get(*t).display(table, symbols)),
            SemaType::Handle(t) =>
                format!("Handle<{}>", table.get(*t).display(table, symbols)),

            // `apply_receiver_wrap` never actually constructs this (Gc is
            // the implicit default: "no wrapper" already means GC-tier —
            // see that function's doc comment). Previously this was also
            // the accidental landing spot for user-written `&T` (a stub
            // bug in `ast_type_to_sema` — see git history / TYPE-114's
            // introduction) — that's fixed now, `TypeKind::Reference`
            // produces a real `SemaType::Reference` (below) instead, so
            // this arm is genuinely GC-tier-only again.
            SemaType::GcRef(t) => format!("&{}", table.get(*t).display(table, symbols)),

            // No surface syntax exists yet — LOW-tier borrow *checking*
            // (the actual move/loan enforcement pass) is still unbuilt
            // (see MEMORY_MODEL.md §9). Nothing in a currently-valid
            // program can construct this yet; provisional rendering only.
            SemaType::OwnedRef { mutable: true,  inner } =>
                format!("&own mut {}", table.get(*inner).display(table, symbols)),
            SemaType::OwnedRef { mutable: false, inner } =>
                format!("&own {}", table.get(*inner).display(table, symbols)),

            // The real, live rendering for user-written `&T`/`ref T` and
            // `&mut T`/`ref mut T` — always shown in symbol form
            // regardless of which spelling the author used, same
            // convention as `and`/`&&` diagnostics staying consistent.
            SemaType::Reference { mutable: true, lifetime, inner } => match lifetime {
                Some(l) => format!("&{} mut {}", l, table.get(*inner).display(table, symbols)),
                None    => format!("&mut {}", table.get(*inner).display(table, symbols)),
            },
            SemaType::Reference { mutable: false, lifetime, inner } => match lifetime {
                Some(l) => format!("&{} {}", l, table.get(*inner).display(table, symbols)),
                None    => format!("&{}", table.get(*inner).display(table, symbols)),
            },

            SemaType::Unique(t) =>
                format!("Unique<{}>", table.get(*t).display(table, symbols)),
            SemaType::Shared(t) =>
                format!("Shared<{}>", table.get(*t).display(table, symbols)),
            SemaType::SyncShared(t) =>
                format!("SyncShared<{}>", table.get(*t).display(table, symbols)),

            SemaType::Named { def, args } => {
                let name = &symbols.lookup(*def).name;
                if args.is_empty() {
                    name.clone()
                } else {
                    format!(
                        "{}<{}>",
                        name,
                        args.iter()
                            .map(|t| table.get(*t).display(table, symbols))
                            .collect::<Vec<_>>()
                            .join(", "),
                    )
                }
            }

            SemaType::Function { params, return_type, is_fallible, .. } => format!(
                "fn({}) {}{}",
                params.iter()
                    .map(|t| table.get(*t).display(table, symbols))
                    .collect::<Vec<_>>()
                    .join(", "),
                table.get(*return_type).display(table, symbols),
                if *is_fallible { "!" } else { "" },
            ),

            // Should always be substituted away before display() sees it
            // (see the `Param` variant's own doc comment) — this arm is a
            // safety net, not an expected real-diagnostic rendering.
            SemaType::Param(i) => format!("<generic param #{}>", i),

            SemaType::Var(n) => format!("?T{}", n),
            SemaType::Unknown => "<unknown>".into(),
        }
    }
}

// ── TypeTable ────────────────────────────────────────────────────

/// The flat table of all semantic types, indexed by `TypeId`.
///
/// Interning: `intern` checks whether an identical `SemaType` already exists
/// and returns the existing `TypeId` if so.  This keeps the table small and
/// makes equality checks O(1) (just compare `TypeId`s).
#[derive(Debug, Default)]
pub struct TypeTable {
    types:  Vec<SemaType>,
    /// Reverse map for interning.
    intern: HashMap<Internable, TypeId>,
    /// Counter for fresh type variables.
    next_var: u32,
}

/// The subset of `SemaType` variants that support O(1) structural interning.
/// Complex variants (Tuple, Named, …) are inserted without interning.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Internable {
    Int, Uint, Long, Ulong,
    Float, Double, Bool, Char, Str, Void,
    I8, I16, I32, I64, U8, U16, U32, U64,
    F32, F64, Isize, Usize,
    Null, Unknown,
    /// `SemaType::Param(usize)` — see that variant's own doc comment.
    /// Interned by index alone (not per-declaration), so `Param(0)`
    /// inside one generic decl's stored signature and `Param(0)`
    /// rebuilt in a later pass over the *same* decl (e.g.
    /// `collect_struct_sig`'s vs. `infer_struct_bodies`'s own
    /// independently-built placeholder set) are the identical `TypeId`
    /// rather than merely equal-by-value — required for `unify`'s `a ==
    /// b` fast path (and everything downstream of it) to recognize two
    /// references to "the enclosing decl's own Nth generic param" as
    /// trivially the same type. Safe to share globally across unrelated
    /// decls too: a `Param` placeholder never leaks into a value's real,
    /// finished type — every real use site substitutes concrete args in
    /// before the result is unified against anything from a different
    /// decl's scope (GENERICS_RULES.md).
    Param(usize),
    List(TypeId),
    Optional(TypeId),
    Fallible(TypeId),
    Task(TypeId),
    Slice(TypeId),
}

impl TypeTable {
    pub fn new() -> Self { TypeTable::default() }

    /// Insert a new type unconditionally and return its `TypeId`.
    /// Prefer `intern` for primitive and simple composite types.
    pub fn insert(&mut self, ty: SemaType) -> TypeId {
        let id = TypeId(self.types.len());
        self.types.push(ty);
        id
    }

    /// Insert a type with interning — returns the existing `TypeId` if this
    /// exact type has been seen before.
    pub fn intern(&mut self, ty: SemaType) -> TypeId {
        let key: Option<Internable> = match &ty {
            SemaType::Int     => Some(Internable::Int),
            SemaType::Uint    => Some(Internable::Uint),
            SemaType::Long    => Some(Internable::Long),
            SemaType::Float   => Some(Internable::Float),
            SemaType::Double  => Some(Internable::Double),
            SemaType::Bool    => Some(Internable::Bool),
            SemaType::Char    => Some(Internable::Char),
            SemaType::Str     => Some(Internable::Str),
            SemaType::Void    => Some(Internable::Void),
            SemaType::Null    => Some(Internable::Null),
            SemaType::Unknown => Some(Internable::Unknown),
            SemaType::Param(i) => Some(Internable::Param(*i)),
            SemaType::List(t) => Some(Internable::List(*t)),
            SemaType::Optional(t) => Some(Internable::Optional(*t)),
            SemaType::Fallible(t) => Some(Internable::Fallible(*t)),
            SemaType::Task(t)     => Some(Internable::Task(*t)),
            SemaType::Slice(t)    => Some(Internable::Slice(*t)),
            _ => None,
        };

        if let Some(k) = key {
            if let Some(&existing) = self.intern.get(&k) {
                return existing;
            }
            let id = TypeId(self.types.len());
            self.types.push(ty);
            self.intern.insert(k, id);
            return id;
        }

        self.insert(ty)
    }

    /// Retrieve a type by its `TypeId`. Panics on an invalid id.
    pub fn get(&self, id: TypeId) -> &SemaType {
        &self.types[id.0]
    }

    /// Allocate a fresh type-inference variable.
    pub fn fresh_var(&mut self) -> TypeId {
        let var_id = self.next_var;
        self.next_var += 1;
        self.insert(SemaType::Var(var_id))
    }

    /// Return the well-known `TypeId` for a primitive.
    /// Call this after the table has been seeded with `seed_builtins`.
    pub fn builtin_int(&mut self)    -> TypeId { self.intern(SemaType::Int) }
    pub fn builtin_bool(&mut self)   -> TypeId { self.intern(SemaType::Bool) }
    pub fn builtin_str(&mut self)    -> TypeId { self.intern(SemaType::Str) }
    pub fn builtin_void(&mut self)   -> TypeId { self.intern(SemaType::Void) }
    pub fn builtin_float(&mut self)  -> TypeId { self.intern(SemaType::Float) }
    pub fn builtin_double(&mut self) -> TypeId { self.intern(SemaType::Double) }
    pub fn builtin_char(&mut self)   -> TypeId { self.intern(SemaType::Char) }
    pub fn builtin_null(&mut self)   -> TypeId { self.intern(SemaType::Null) }
              }
