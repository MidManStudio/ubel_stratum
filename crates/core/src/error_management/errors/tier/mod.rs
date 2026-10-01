// src/error_management/errors/tier/mod.rs
//! Errors produced during tier and arena-boundary enforcement.
//!
//! Physically split out of `TypeError` (see `../types/mod.rs`) per
//! docs/DIAGNOSTICS_RULES.md §9. Variant names and their meaning are
//! unchanged from when they lived as `TypeError`'s TYPE-2xx range —
//! only the enum they belong to and their code prefix changed
//! (`TYPE-201..207` → `TIER-001..007`, same relative order, tail
//! renumbered from 1 per §9's own "keep the tail stable" promise not
//! applying here since the whole range moved to a new prefix).

use crate::lexer::Span;
use crate::ast::common::TierAnnotation;
use std::fmt;

/// Every error that can be raised while enforcing tier rules
/// (`@tier(high|mid|low)`) and arena-lifetime boundaries.
#[derive(Debug, Clone)]
pub enum TierError {
    /// `with arena` or arena allocator used outside `@tier(mid)`.
    ArenaInWrongTier {
        actual: TierAnnotation,
        span:   Span,
    },

    /// `await` used outside `@tier(high)`.
    AwaitInWrongTier {
        actual: TierAnnotation,
        span:   Span,
    },

    /// An async function is not `@tier(high)`.
    AsyncFunctionNotHigh {
        actual: TierAnnotation,
        span:   Span,
    },

    /// A value that carries an arena lifetime crossed a boundary where
    /// it would outlive its arena: assigned to a binding whose home
    /// arena doesn't match (including a binding declared outside any
    /// arena at all), stored into a struct field or indexed slot on a
    /// receiver from a different-or-no arena, captured by a closure that
    /// then escapes the block, or returned across a tier boundary whose
    /// declared type can't express the tag. See MEMORY_MODEL.md §6.
    ArenaRefEscapesBoundary {
        /// The type that contains the arena reference.
        escaped_type: String,
        span:         Span,
    },

    /// `@tier(high)` code tried to call `@tier(low)` code directly.
    /// (HIGH may only call MID; MID may call HIGH or LOW.)
    IllegalTierCall {
        caller_tier: TierAnnotation,
        callee_tier: TierAnnotation,
        callee_name: String,
        span:        Span,
    },

    /// A MID-tier function's return type contains an arena-lifetime type.
    /// This would make it impossible for HIGH-tier to call it safely.
    MidReturnContainsArenaRef {
        return_type: String,
        span:        Span,
    },

    // ── Instance method tier violation (MEMORY_MODEL.md §8) ────────
    /// A builtin instance method flagged HIGH-only in
    /// `builtins::instance::is_high_only` was called outside
    /// `@tier(high)`. Same shape as `AwaitInWrongTier`
    /// — the method isn't a keyword, so it carries its own name.
    MethodInWrongTier {
        method: String,
        actual: TierAnnotation,
        span:   Span,
    },

    // ── LOW tier — explicit non-support (MEMORY_MODEL.md §9) ────────
    /// A `@tier(low)` function constructed a builtin collection
    /// (`List`, `Dictionary`, `Queue`, `Stack`). LOW tier's own memory
    /// model — `OwnedRef` + a borrow checker — is Phase 4 and has not
    /// been built yet. Without this check the constructor call would
    /// fall through to a silent, untagged bare type: HIGH-tier
    /// (GC-default) semantics nobody has actually designed for LOW,
    /// compiling cleanly today and surfacing as a confusing runtime
    /// bug later instead of an honest compile-time message now.
    CollectionConstructionInLowTier {
        collection: String,
        span:       Span,
    },

    // ── Pool tier violations (MEMORY_MODEL.md §11) ──────────────────
    /// `with pool<T>(count)` used outside `@tier(mid)` — same rule as
    /// `ArenaInWrongTier`, extended to pool per §11's "same relationship
    /// `with arena(...)` already has."
    PoolInWrongTier {
        actual: TierAnnotation,
        span:   Span,
    },

    /// A value scoped to a `with pool<T>(count) { }` block crossed a
    /// boundary where it would outlive its pool. Same shape and same
    /// four escape routes as `ArenaRefEscapesBoundary` (assignment,
    /// struct field, closure capture, return) — kept as its own variant
    /// purely so the message says "pool" accurately instead of "arena."
    PoolRefEscapesBoundary {
        escaped_type: String,
        span:         Span,
    },

    /// A MID-tier function's return type contains a pool-scoped type.
    /// Same reasoning as `MidReturnContainsArenaRef`: HIGH-tier callers
    /// can't safely receive something bound to a pool that may already
    /// be torn down by the time the call returns.
    MidReturnContainsPoolRef {
        return_type: String,
        span:        Span,
    },

    /// `Pool.new()` called with no enclosing `with pool<T>(count) { }`
    /// block to supply its element type and capacity from. Unlike
    /// `List.new()` etc., `Pool.new()` takes no generic argument of its
    /// own — it has nothing to construct without one.
    PoolConstructedOutsideBlock {
        span: Span,
    },

    // ── Ownership-model wrappers (MEMORY_MODEL.md §9) ────────────────
    /// `Unique.new(...)`/`Shared.new(...)`/`SyncShared.new(...)` called
    /// outside `@tier(low)`. The inverse of
    /// `CollectionConstructionInLowTier`: those three collections are
    /// HIGH-tier's (GC-default) story and are banned *inside* LOW tier
    /// because LOW has no memory model of its own to fall back to yet;
    /// `Unique`/`Shared`/`SyncShared` now *are* that memory model, so by
    /// the same "each tier's own allocation strategy stays within that
    /// tier by default" logic, construction is banned everywhere
    /// *except* LOW tier. A HIGH/MID-tier function may still receive a
    /// `Unique<T>` value as a parameter (same as it may already receive
    /// an `ArenaRef`-wrapped value to read) — this only restricts where
    /// a *new* one may be constructed.
    OwnershipWrapperOutsideLowTier {
        wrapper: String,
        actual:  TierAnnotation,
        span:    Span,
    },

    // ── Mutable globals ──────────────────────────────────────────────
    /// A `static` was read or written from a function that is not
    /// `@tier(high)`. A static is a GC-tier mutable global shared by every
    /// function, which `@tier(mid)` (arena) and `@tier(low)` (manual
    /// ownership) code has no sound way to hold a reference into, so the
    /// access is rejected there rather than left to corrupt the tier's own
    /// guarantees. Pass the value in as a parameter instead.
    StaticAccessOutsideHigh {
        name:   String,
        actual: TierAnnotation,
        span:   Span,
    },
}

impl TierError {
    pub fn span(&self) -> Span {
        match self {
            TierError::ArenaInWrongTier           { span, .. } => *span,
            TierError::AwaitInWrongTier           { span, .. } => *span,
            TierError::AsyncFunctionNotHigh       { span, .. } => *span,
            TierError::ArenaRefEscapesBoundary    { span, .. } => *span,
            TierError::IllegalTierCall            { span, .. } => *span,
            TierError::MidReturnContainsArenaRef  { span, .. } => *span,
            TierError::MethodInWrongTier          { span, .. } => *span,
            TierError::CollectionConstructionInLowTier { span, .. } => *span,
            TierError::PoolInWrongTier            { span, .. } => *span,
            TierError::PoolRefEscapesBoundary     { span, .. } => *span,
            TierError::MidReturnContainsPoolRef   { span, .. } => *span,
            TierError::PoolConstructedOutsideBlock { span, .. } => *span,
            TierError::OwnershipWrapperOutsideLowTier { span, .. } => *span,
            TierError::StaticAccessOutsideHigh { span, .. } => *span,
        }
    }

    pub fn message(&self) -> String {
        match self {
            TierError::ArenaInWrongTier { actual, .. } =>
                format!(
                    "`with arena` is only valid in `@tier(mid)`; this function is `@tier({})`",
                    tier_name(*actual)
                ),

            TierError::AwaitInWrongTier { actual, .. } =>
                format!(
                    "`await` is only valid in `@tier(high)`; this function is `@tier({})`",
                    tier_name(*actual)
                ),

            TierError::AsyncFunctionNotHigh { actual, .. } =>
                format!(
                    "async functions must be `@tier(high)`; this function is `@tier({})`",
                    tier_name(*actual)
                ),

            TierError::ArenaRefEscapesBoundary { escaped_type, .. } =>
                format!(
                    "value of type `{}` is scoped to a `with arena(...)` block and cannot \
                     outlive it — check the binding, struct field, or closure it's flowing into",
                    escaped_type
                ),

            TierError::IllegalTierCall { caller_tier, callee_tier, callee_name, .. } =>
                format!(
                    "`@tier({})` code cannot call `@tier({})` function `{}`",
                    tier_name(*caller_tier), tier_name(*callee_tier), callee_name
                ),

            TierError::MidReturnContainsArenaRef { return_type, .. } =>
                format!(
                    "return type `{}` contains an arena-lifetime reference; \
                     this makes the function uncallable from `@tier(high)`",
                    return_type
                ),

            TierError::MethodInWrongTier { method, actual, .. } =>
                format!(
                    "`.{}()` is only valid in `@tier(high)`; this function is `@tier({})`",
                    method, tier_name(*actual)
                ),

            TierError::CollectionConstructionInLowTier { collection, .. } =>
                format!(
                    "`{}.new()` is not yet supported in `@tier(low)` — LOW tier's memory model \
                     (move semantics + borrow checker) hasn't been built yet",
                    collection
                ),

            TierError::PoolInWrongTier { actual, .. } =>
                format!(
                    "`with pool` is only valid in `@tier(mid)`; this function is `@tier({})`",
                    tier_name(*actual)
                ),

            TierError::PoolRefEscapesBoundary { escaped_type, .. } =>
                format!(
                    "value of type `{}` is scoped to a `with pool<...>(...)` block and cannot \
                     outlive it — check the binding, struct field, or closure it's flowing into",
                    escaped_type
                ),

            TierError::MidReturnContainsPoolRef { return_type, .. } =>
                format!(
                    "return type `{}` contains a pool-lifetime reference; \
                     this makes the function uncallable from `@tier(high)`",
                    return_type
                ),

            TierError::PoolConstructedOutsideBlock { .. } =>
                "`Pool.new()` requires an enclosing `with pool<T>(count) { }` block to supply \
                 its element type and capacity".to_string(),

            TierError::StaticAccessOutsideHigh { name, actual, .. } =>
                format!(
                    "the static `{}` can only be used from `@tier(high)` code; this function is `@tier({})`",
                    name, tier_name(*actual)
                ),

            TierError::OwnershipWrapperOutsideLowTier { wrapper, actual, .. } =>
                format!(
                    "`{}.new()` is only valid in `@tier(low)`; this function is `@tier({})`",
                    wrapper, tier_name(*actual)
                ),
        }
    }

    pub fn suggestion(&self) -> Option<String> {
        match self {
            TierError::ArenaInWrongTier { .. } =>
                Some("annotate this function with `@tier(mid)` or remove the arena block".to_string()),

            TierError::AwaitInWrongTier { .. } =>
                Some("annotate this function with `@tier(high)` or remove the `await`".to_string()),

            TierError::AsyncFunctionNotHigh { .. } =>
                Some("add `@tier(high)` annotation, or remove `async`".to_string()),

            TierError::ArenaRefEscapesBoundary { .. } =>
                Some("copy the data out into a plain (non-arena) value before it leaves the \
                      `with arena(...)` block, or restructure with the callback pattern: pass a \
                      closure in and return a GC-owned value from inside it, instead of returning \
                      or storing the arena-scoped value directly".to_string()),

            TierError::IllegalTierCall { caller_tier: TierAnnotation::High, .. } =>
                Some("HIGH tier may only call MID-tier functions directly — wrap the LOW-tier logic in a MID-tier function".to_string()),

            TierError::MethodInWrongTier { method, .. } =>
                Some(format!(
                    "move this call to a `@tier(high)` function, or avoid `.{}()` in `@tier(mid)`/`@tier(low)` code",
                    method
                )),

            TierError::CollectionConstructionInLowTier { collection, .. } =>
                Some(format!(
                    "construct the `{}` in a `@tier(mid)` or `@tier(high)` function instead, or \
                     restructure this function to receive it as a parameter",
                    collection
                )),

            TierError::PoolInWrongTier { .. } =>
                Some("annotate this function with `@tier(mid)` or remove the pool block".to_string()),

            TierError::PoolRefEscapesBoundary { .. } =>
                Some("copy the data out into a plain (non-pool) value before it leaves the \
                      `with pool<...>(...)` block, or restructure with the callback pattern: pass a \
                      closure in and return a GC-owned value from inside it, instead of returning \
                      or storing the pool-scoped value directly".to_string()),

            TierError::MidReturnContainsPoolRef { .. } =>
                Some("copy the data out into a plain (non-pool) value before returning, or change \
                      this function's tier so it isn't callable from `@tier(high)`".to_string()),

            TierError::PoolConstructedOutsideBlock { .. } =>
                Some("wrap this call in a `with pool<T>(count) { }` block".to_string()),

            TierError::StaticAccessOutsideHigh { .. } =>
                Some("read the static in a `@tier(high)` function and pass the value in as a parameter, or return a result for a `@tier(high)` caller to store".to_string()),

            TierError::OwnershipWrapperOutsideLowTier { wrapper, .. } =>
                Some(format!(
                    "annotate this function with `@tier(low)`, or receive the `{}` as a parameter \
                     from LOW-tier code instead of constructing it here",
                    wrapper
                )),

            _ => None,
        }
    }
}

fn tier_name(tier: TierAnnotation) -> &'static str {
    match tier {
        TierAnnotation::High => "high",
        TierAnnotation::Mid  => "mid",
        TierAnnotation::Low  => "low",
    }
}

impl fmt::Display for TierError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.message())
    }
}

impl std::error::Error for TierError {}

impl crate::error_management::render::Diagnosable for TierError {
    // See docs/DIAGNOSTICS_RULES.md, "Error Code Registry" — TIER-0xx.
    // None of these variants carry a secondary span, so `secondary_spans`
    // is left at the trait's default (empty) rather than overridden here.
    fn code(&self) -> &'static str {
        match self {
            TierError::ArenaInWrongTier { .. }           => "TIER-001",
            TierError::AwaitInWrongTier { .. }           => "TIER-002",
            TierError::AsyncFunctionNotHigh { .. }       => "TIER-003",
            TierError::ArenaRefEscapesBoundary { .. }    => "TIER-004",
            TierError::IllegalTierCall { .. }            => "TIER-005",
            TierError::MidReturnContainsArenaRef { .. }  => "TIER-006",
            TierError::MethodInWrongTier { .. }          => "TIER-008",
            TierError::CollectionConstructionInLowTier { .. } => "TIER-009",
            TierError::PoolInWrongTier { .. }             => "TIER-010",
            TierError::PoolRefEscapesBoundary { .. }      => "TIER-011",
            TierError::MidReturnContainsPoolRef { .. }    => "TIER-012",
            TierError::PoolConstructedOutsideBlock { .. } => "TIER-013",
            TierError::OwnershipWrapperOutsideLowTier { .. } => "TIER-014",
            TierError::StaticAccessOutsideHigh { .. }        => "TIER-015",
        }
    }
    fn span(&self) -> Span { self.span() }
    fn message(&self) -> String { self.message() }
    fn suggestion(&self) -> Option<String> { self.suggestion() }
}
