// crates/core/src/builtins/instance.rs
//! Instance methods — `receiver.method(args)` — grouped by the runtime
//! `Value` kind they apply to. Actual dispatch still lives in
//! `interpreter::eval::expr::eval_method_call` (it needs the receiver's
//! `Rc`/inner data, which doesn't fit the same `fn(&[Value]) -> EvalResult`
//! shape as global builtins) — these modules are the single implementation
//! that dispatch calls into, and the name lists below are what sema
//! consults to know a method name is valid for a given receiver kind.
//!
//! `resolve_receiver`/`signature`/`is_high_only` below are the sema-side
//! wiring — see `type_infer.rs`'s `ExprKind::Call` handling (return type +
//! arena preservation, MEMORY_MODEL.md §8) and `tier_check.rs` (HIGH-only
//! rejection, same section).

use crate::sema::type_table::{ArenaId, PoolId, SemaType, TypeId, TypeTable};

pub mod list_methods;
pub mod string_methods;
pub mod dict_methods;
pub mod tuple_methods;
pub mod queue_methods;
pub mod stack_methods;
pub mod pool_methods;
pub mod inline_list_methods;
pub mod linqerizer_methods;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReceiverKind {
    List,
    Str,
    Dict,
    Tuple,
    Queue,
    Stack,
    Pool,
    InlineList,
    Linqerizer,
}

/// Returns the valid method names for a given receiver kind — consulted
/// by sema (`type_infer.rs`) to flag `myList.frobnicate()` as `NoSuchMethod`
/// before runtime, the same way undefined global names are caught today.
pub fn method_names(kind: ReceiverKind) -> &'static [&'static str] {
    match kind {
        ReceiverKind::List  => list_methods::METHOD_NAMES,
        ReceiverKind::Str   => string_methods::METHOD_NAMES,
        ReceiverKind::Dict  => dict_methods::METHOD_NAMES,
        ReceiverKind::Tuple => tuple_methods::METHOD_NAMES,
        ReceiverKind::Queue => queue_methods::METHOD_NAMES,
        ReceiverKind::Stack => stack_methods::METHOD_NAMES,
        ReceiverKind::Pool  => pool_methods::METHOD_NAMES,
        ReceiverKind::InlineList => inline_list_methods::METHOD_NAMES,
        ReceiverKind::Linqerizer => linqerizer_methods::METHOD_NAMES,
    }
}

/// Is `name` a known builtin instance method, on any receiver kind at
/// all (not narrowed to one)? Consulted by `sema/move_facts.rs`, which
/// works purely syntactically and has no type information to know which
/// kind a given call site's receiver actually is: every builtin
/// instance method takes its receiver by reference in spirit (`&self`/
/// `&mut self`, never a consuming `self`/`mut self` the way a user-
/// declared method's own self-kind might), so a call to one is never a
/// move of its receiver, unlike a user-declared method call, which this
/// deliberately does not attempt to resolve yet (see move_facts.rs's
/// own module doc for the full scope note). A name collision with an
/// unrelated user-declared method of the same name (someone's own
/// struct with a consuming `push(self, ...)`, say) is a known, narrow,
/// accepted gap, not solved here.
pub fn is_builtin_instance_method_name(name: &str) -> bool {
    [
        ReceiverKind::List, ReceiverKind::Str, ReceiverKind::Dict, ReceiverKind::Tuple,
        ReceiverKind::Queue, ReceiverKind::Stack, ReceiverKind::Pool,
        ReceiverKind::InlineList, ReceiverKind::Linqerizer,
    ].iter().any(|&kind| method_names(kind).contains(&name))
}

/// What ref kind wrapped the receiver — carried through so an
/// allocation-producing method's result can be re-wrapped the same way
/// (MEMORY_MODEL.md §8's "allocate into the same arena as the receiver"
/// rule) instead of defaulting to GC regardless of where the receiver
/// actually lives.
#[derive(Debug, Clone, Copy)]
pub enum ReceiverWrap {
    Gc,
    Arena { arena: ArenaId, mutable: bool },
    Owned { mutable: bool },
    /// MEMORY_MODEL.md §11 — mirrors `Arena` exactly, for `Pool<T>`
    /// receivers. Kept distinct so `apply_receiver_wrap` re-wraps an
    /// `.acquire()` result in `PoolRef` (not `ArenaRef`), matching
    /// whichever kind the receiver actually carried.
    Pool { pool: PoolId, mutable: bool },
}

/// Strip a `GcRef`/`ArenaRef`/`OwnedRef` wrapper off `ty` and, if what's
/// underneath is one of the six builtin instance-method receivers,
/// return the wrapper plus the *bare* `TypeId` (the `List(_)` /
/// `Dictionary(_,_)` / `Str` / etc itself, for the caller to pull element
/// type(s) out of). Returns `None` for anything else — user-defined
/// structs, primitives, etc — so callers know to fall through to
/// whatever already handles those.
///
/// A receiver with no ref wrapper at all defaults to `ReceiverWrap::Gc`,
/// matching the rest of the type system's "GcRef is the default when
/// nothing says otherwise" convention.
pub fn resolve_receiver(table: &TypeTable, ty: TypeId) -> Option<(ReceiverWrap, ReceiverKind, TypeId)> {
    // Peel off at most one ownership-model wrapper (Unique/Shared/
    // SyncShared) before doing the existing tier-wrap resolution on
    // whatever's underneath. Orthogonal axes (DATASTRUCTURES.md,
    // MEMORY_MODEL.md §9's Open Decision #5): a value's tier answers
    // "where does this live", ownership answers "who owns it", and a
    // method call needs to see through ownership to reach whatever tier
    // wrap (if any) is on the inner value, same as it already sees
    // through tier wraps to reach the bare collection underneath. Not
    // folded into the `wrap`/`bare` match below because `ReceiverWrap`
    // exists specifically to remember which *tier* wrap to reapply to an
    // allocating method's result (`method_return_type` in
    // `type_infer.rs`); ownership doesn't need that: nothing here
    // returns a value that itself needs `Unique`/`Shared`/`SyncShared`
    // re-applied, only the tier the method's own receiver already had.
    let ty = match table.get(ty) {
        SemaType::Unique(inner) | SemaType::Shared(inner) | SemaType::SyncShared(inner) => *inner,
        _ => ty,
    };
    let (wrap, bare) = match table.get(ty) {
        SemaType::GcRef(inner) => (ReceiverWrap::Gc, *inner),
        SemaType::ArenaRef { arena, mutable, inner } =>
            (ReceiverWrap::Arena { arena: *arena, mutable: *mutable }, *inner),
        SemaType::PoolRef { pool, mutable, inner } =>
            (ReceiverWrap::Pool { pool: *pool, mutable: *mutable }, *inner),
        SemaType::OwnedRef { mutable, inner } =>
            (ReceiverWrap::Owned { mutable: *mutable }, *inner),
        _ => (ReceiverWrap::Gc, ty),
    };
    let kind = match table.get(bare) {
        SemaType::List(_)        => ReceiverKind::List,
        SemaType::Str            => ReceiverKind::Str,
        SemaType::Dictionary(..) => ReceiverKind::Dict,
        SemaType::Tuple(_)       => ReceiverKind::Tuple,
        SemaType::Queue(_)       => ReceiverKind::Queue,
        SemaType::Stack(_)       => ReceiverKind::Stack,
        SemaType::Pool(_)        => ReceiverKind::Pool,
        SemaType::InlineList(_)  => ReceiverKind::InlineList,
        SemaType::Linqerizer(_)  => ReceiverKind::Linqerizer,
        _ => return None,
    };
    Some((wrap, kind, bare))
}

/// What a method returns, described independent of any specific
/// `TypeTable` — the caller (`type_infer.rs`) turns this into a concrete
/// `TypeId`, since only it holds `&mut TypeTable` to intern new composite
/// types like `List(Char)`.
///
/// The `New*` variants are the only ones that construct a value that
/// didn't already exist — see `allocates()`. Everything else either
/// returns a bare primitive (no tier concept applies) or an element that
/// was already stored in the receiver (already correctly tiered from
/// whenever it was inserted, nothing new to tag).
///
/// Only goes one level deep: `NewListOfStr`'s element type is plain
/// `Str`, not itself re-wrapped in the receiver's tier. Matching the rest
/// of this module, that's a stated simplification, not an oversight —
/// see the doc comment on `method_return_type` in `type_infer.rs`.
#[derive(Debug, Clone, Copy)]
pub enum MethodReturn {
    Void,
    Int,
    Bool,
    /// The receiver's own element type, as already stored — `List<T>`/
    /// `Queue<T>`/`Stack<T>`'s `T`, or `Dictionary<K,V>`'s `V` for `at`.
    /// Not a new allocation.
    Elem,
    /// A brand-new value of the receiver's own bare shape. Allocates.
    NewSelf,
    /// A brand-new `List<Char>`. Allocates.
    NewListOfChar,
    /// A brand-new `List<Str>`. Allocates.
    NewListOfStr,
    /// A brand-new `List<K>`. Allocates.
    NewListOfKey,
    /// A brand-new `List<V>`. Allocates.
    NewListOfValue,
    /// `Pool<T>.acquire(value)` — `Optional<Handle<T>>`. Allocates in
    /// the sense that matters here: the returned `Handle<T>` needs the
    /// receiver's own wrap applied (MEMORY_MODEL.md §11's "must not
    /// escape" answer applies to acquired handles too, same as the pool
    /// itself), even though no new storage is created the way `NewSelf`
    /// etc. actually allocate.
    AcquireHandle,
    /// `List<T>.query()` — a brand-new `Linqerizer<T>`, `T` pulled from
    /// the receiver's own element type. Allocates.
    NewLinqerizerOfElem,
    /// `Linqerizer<T>.to_list()` — a brand-new `List<T>`, `T` pulled
    /// from the receiver `Linqerizer<T>`'s own element type (not a
    /// fixed type the way `NewListOfChar`/`NewListOfStr` are). Allocates.
    NewListOfLinqElem,
}

impl MethodReturn {
    pub fn allocates(self) -> bool {
        matches!(
            self,
            MethodReturn::NewSelf
                | MethodReturn::NewListOfChar
                | MethodReturn::NewListOfStr
                | MethodReturn::NewListOfKey
                | MethodReturn::NewListOfValue
                | MethodReturn::AcquireHandle
        )
    }
}

/// `(return shape, arity)` for a method name under a given receiver
/// kind, or `None` if `name` isn't one of `method_names(kind)`.
pub fn signature(kind: ReceiverKind, name: &str) -> Option<(MethodReturn, usize)> {
    match kind {
        ReceiverKind::List  => list_methods::signature(name),
        ReceiverKind::Str   => string_methods::signature(name),
        ReceiverKind::Dict  => dict_methods::signature(name),
        ReceiverKind::Tuple => tuple_methods::signature(name),
        ReceiverKind::Queue => queue_methods::signature(name),
        ReceiverKind::Stack => stack_methods::signature(name),
        ReceiverKind::Pool  => pool_methods::signature(name),
        ReceiverKind::InlineList => inline_list_methods::signature(name),
        ReceiverKind::Linqerizer => linqerizer_methods::signature(name),
    }
}

/// Whether calling this method requires the enclosing function to be
/// `@tier(high)` — same rule shape as `await`/LINQ (MEMORY_MODEL.md §8),
/// just keyed by method name instead of being a dedicated keyword.
/// Every list is empty today: nothing currently implemented needs this.
/// Real infrastructure, not a stub — add a name to the relevant
/// module's `HIGH_ONLY` const and this starts enforcing immediately,
/// no other wiring needed.
pub fn is_high_only(kind: ReceiverKind, name: &str) -> bool {
    let list: &[&str] = match kind {
        ReceiverKind::List  => list_methods::HIGH_ONLY,
        ReceiverKind::Str   => string_methods::HIGH_ONLY,
        ReceiverKind::Dict  => dict_methods::HIGH_ONLY,
        ReceiverKind::Tuple => tuple_methods::HIGH_ONLY,
        ReceiverKind::Queue => queue_methods::HIGH_ONLY,
        ReceiverKind::Stack => stack_methods::HIGH_ONLY,
        ReceiverKind::Pool  => pool_methods::HIGH_ONLY,
        ReceiverKind::InlineList => inline_list_methods::HIGH_ONLY,
        ReceiverKind::Linqerizer => linqerizer_methods::HIGH_ONLY,
    };
    list.contains(&name)
}
