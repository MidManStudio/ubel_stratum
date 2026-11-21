// crates/core/src/builtins/mod.rs
//! Native builtins: bare-call globals (`println(x)`) and instance methods
//! (`myList.push(x)`), plus builtin *static namespaces* (`Math.sqrt(x)`).
//!
//! # Why this module exists
//!
//! Before this split, sema and the interpreter each had their own idea of
//! what builtins existed — sema didn't know about them at all (`println`
//! resolved as `UndefinedName`), and the interpreter's list lived in one
//! flat file with `len`/`push`/`pop`/`contains` implemented a *second*
//! time, differently, as inline match arms for instance-method calls.
//!
//! `GLOBAL_BUILTINS` (see `global.rs`) is now the single source of truth
//! for bare-call builtins, consumed by sema, the interpreter, and — once
//! it exists — LLVM codegen. `instance.rs` plays the same role for
//! receiver methods, modulo the dispatch-shape difference noted there.

pub mod global;
pub mod instance;
pub mod validate;
pub mod constructors;
pub mod std_modules;

use crate::interpreter::value::{EvalResult, Value};

/// Signature every builtin function implementation must satisfy.
pub type BuiltinFn = fn(&[Value]) -> EvalResult;

/// A coarse type constraint for builtin signature validation. Deliberately
/// simpler than the full `SemaType` lattice in `sema::type_table` — this
/// only needs to catch real argument mistakes (wrong count, wrong kind),
/// not do full inference.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamType {
    Int,
    Float,
    Double,
    /// Accepts Int, Float, or Double — most math builtins don't care which.
    Numeric,
    Bool,
    Str,
    Char,
    List,
    Dict,
    Tuple,
    /// No constraint — used for things like `println`'s variadic args.
    Any,
}

/// How a builtin will eventually lower to LLVM IR. Nothing reads this yet
/// (there's no LLVM backend), but deciding it per-function *now*, while
/// each one is fresh in mind, is cheaper than re-deriving it later:
///
/// - `Intrinsic`   — maps directly to a native LLVM intrinsic (`sqrt`,
///                   `abs`, `min`/`max`, `floor`/`ceil`). No function call
///                   at all in the emitted IR.
/// - `RuntimeCall` — needs real support code (I/O, allocation, formatting)
///                   that doesn't exist as an LLVM primitive. Lowers to a
///                   `call` against a small linked runtime library — which
///                   can genuinely be compiled Rust, just linked rather
///                   than interpreted.
/// - `ConstFoldable` — resolvable at compile time from sema's own output
///                   (e.g. `typeof(x)` once `x`'s type is known); no
///                   runtime representation needed at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lowering {
    Intrinsic(&'static str),
    RuntimeCall(&'static str),
    ConstFoldable,
}

/// One entry in `GLOBAL_BUILTINS`.
#[derive(Clone, Copy)]
pub struct BuiltinSignature {
    pub name:        &'static str,
    pub params:      &'static [ParamType],
    pub variadic:    bool,
    pub return_type: ParamType,
    pub lowering:    Lowering,
    pub run:         BuiltinFn,
}

// ── Builtin static namespaces ───────────────────────────────────────────
//
// `Namespace.method(args)` where `Namespace` is NOT a user-defined type.
// Distinct from `method_table` (which only ever holds user struct names).
// `Math.sqrt(x)` and bare `sqrt(x)` share the exact same implementation —
// see `global::math` — so there is still only one `sqrt`, just two ways
// to spell calling it.

/// Names of builtin static namespaces. Checked before falling through to
/// `method_table` in `eval_call_with_receiver`, and before sema treats a
/// `Namespace.method` call as an unresolved user type.
///
/// "Pool" is here so name resolution pre-declares it (see
/// `declare_builtins` in `sema/name_resolution.rs`) — it is deliberately
/// NOT in `resolve_namespace_member`'s dispatch table below, since
/// `Pool.new()` needs the interpreter's own ambient `pool_capacity_stack`
/// (MEMORY_MODEL.md §11) and is special-cased directly in
/// `eval_call_with_receiver` before that table is ever consulted.
pub const BUILTIN_NAMESPACES: &[&str] = &[
    "Math", "List", "Dictionary", "Queue", "Stack", "Pool", "InlineList",
    "Unique", "Shared", "SyncShared", "Task",
];

pub fn is_builtin_namespace(name: &str) -> bool {
    BUILTIN_NAMESPACES.contains(&name)
}

/// Every name available under a builtin namespace, paired with the same
/// `BuiltinFn` the bare-call form uses.
static MATH_NAMESPACE: &[(&str, BuiltinFn)] = &[
    ("sqrt",  global::math::sqrt),
    ("abs",   global::math::abs),
    ("min",   global::math::min),
    ("max",   global::math::max),
    ("floor", global::math::floor),
    ("ceil",  global::math::ceil),
    ("range", global::math::range),
];

/// Resolve `Namespace.method` to a runtime function pointer, if it names a
/// real builtin namespace member. Returns `None` for both "not a builtin
/// namespace" and "no such member" — callers that already checked
/// `is_builtin_namespace` first can treat `None` as the latter.
static LIST_NAMESPACE: &[(&str, BuiltinFn)] = &[("new", constructors::list_new)];
static DICTIONARY_NAMESPACE: &[(&str, BuiltinFn)] = &[("new", constructors::dictionary_new)];
static QUEUE_NAMESPACE: &[(&str, BuiltinFn)] = &[("new", constructors::queue_new)];
static STACK_NAMESPACE: &[(&str, BuiltinFn)] = &[("new", constructors::stack_new)];
static INLINE_LIST_NAMESPACE: &[(&str, BuiltinFn)] = &[("new", constructors::inline_list_new)];
// MEMORY_MODEL.md §9 — unlike `Pool`, these three need no interpreter-
// ambient state (no `pool_capacity_stack` equivalent), so they go through
// the standard `BuiltinFn` dispatch here rather than being special-cased
// in `eval_call_with_receiver` the way `Pool.new()` is.
static UNIQUE_NAMESPACE:      &[(&str, BuiltinFn)] = &[("new", constructors::unique_new)];
static SHARED_NAMESPACE:      &[(&str, BuiltinFn)] = &[("new", constructors::shared_new)];
static SYNC_SHARED_NAMESPACE: &[(&str, BuiltinFn)] = &[("new", constructors::sync_shared_new)];

pub fn resolve_namespace_member(namespace: &str, method: &str) -> Option<BuiltinFn> {
    let table = match namespace {
        "Math"       => MATH_NAMESPACE,
        "List"       => LIST_NAMESPACE,
        "Dictionary" => DICTIONARY_NAMESPACE,
        "Queue"      => QUEUE_NAMESPACE,
        "Stack"      => STACK_NAMESPACE,
        "InlineList" => INLINE_LIST_NAMESPACE,
        "Unique"      => UNIQUE_NAMESPACE,
        "Shared"      => SHARED_NAMESPACE,
        "SyncShared"  => SYNC_SHARED_NAMESPACE,
        _ => return None,
    };
    table.iter().find(|(n, _)| *n == method).map(|(_, f)| *f)
}

/// Every member name under a builtin namespace — used by sema to validate
/// `Math.frobnicate()` the same way an undefined global name is caught.
pub fn namespace_member_names(namespace: &str) -> &'static [&'static str] {
    match namespace {
        "Math"       => &["sqrt", "abs", "min", "max", "floor", "ceil", "range"],
        "List"       => &["new"],
        "Dictionary" => &["new"],
        "Queue"      => &["new"],
        "Stack"      => &["new"],
        "InlineList" => &["new"],
        "Unique"      => &["new"],
        "Shared"      => &["new"],
        "SyncShared"  => &["new"],
        "Task"        => &["spawn"],
        _ => &[],
    }
    }
