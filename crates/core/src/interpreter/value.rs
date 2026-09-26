// ============================================================================
// NOTICE: Full documentation, design decisions, and fix history for this file
// live in docs/ubel_stratum.md, section "interpreter/value.rs"
// ============================================================================
// src/interpreter/value.rs
//! Runtime value representation.

#![allow(dead_code)]

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::rc::Rc;

/// Stable index into `Interpreter::functions`.
/// Stored inside `Value::Function` so Value carries no AST lifetimes.
pub type FunctionId = usize;

/// Every runtime value an Ubel program can produce.
///
/// Scalars are stored inline. Heap values (`List`, `Dict`, `Struct`) are
/// `Rc<RefCell<…>>` so:
///   - Assignment shares (does not deep-copy) — consistent with HIGH-tier GC semantics.
///   - Mutation inside a shared structure is visible through all aliases.
///   - `Clone` on a `Value` is always O(1) (bumps an Rc, never allocates).
#[derive(Debug, Clone)]
pub enum Value {
    Null,
    Void,
    Bool(bool),
    /// Ubel's default integer — i64 matches AST `Literal::Int(i64)`. Also
    /// backs `SemaType::Long`/`I64`/`Isize` -- all bit-identical 64-bit
    /// signed widths, so they share this one representation and (via
    /// `eval_cast`) genuinely lossless, truncation-free conversion
    /// between each other; only their *sema* types differ, not their
    /// runtime shape. `SemaType::Int` itself is documented as 64-bit
    /// (`docs/PRINT_FORMAT_RULES.md` §4) and used pervasively as the
    /// default numeric type, so this variant, its wrapping-on-overflow
    /// arithmetic (`eval_binop`), and its unlimited-within-i64 range are
    /// all unchanged by the sized-integer work below -- zero risk to the
    /// large existing body of code that already relies on plain `int`.
    Int(i64),
    /// Explicitly narrower or differently-signed integers: `u8`/`byte`,
    /// `i8`/`sbyte`, `u16`, `i16`, `u32`/`uint`, `i32`. Each carries its
    /// own real Rust primitive so arithmetic (`eval_binop`) and casts
    /// (`eval_cast`) get genuine width-correct wrapping for free from
    /// Rust's own `wrapping_*` methods and `as` operator, rather than
    /// this interpreter re-deriving modular arithmetic by hand for each
    /// width. `Uint`/`Ushort` (C#) share `U32` -- see `I32`'s sibling
    /// note on `Int` above for why sharing by bit-width rather than by
    /// sema-type name is correct here.
    I8(i8), I16(i16), I32(i32),
    U8(u8), U16(u16), U32(u32),
    /// Backs `u64`/`ulong`/`usize` -- the one width `Int(i64)` genuinely
    /// cannot hold (anything above `i64::MAX`). The reason this feature
    /// needed a new variant at all rather than just teaching `Int`
    /// better arithmetic: no `i64`-based representation can losslessly
    /// hold `u64::MAX`.
    UInt(u64),
    Float(f32),
    Double(f64),
    Char(char),
    /// Immutable string — Rc makes cloning O(1).
    Str(Rc<String>),
    /// Mutable ordered list — shared between aliases.
    List(Rc<RefCell<Vec<Value>>>),
    /// Ordered key-value store — Vec<pair> because Value doesn't impl Hash.
    /// O(n) lookup is fine for the tree-walking interpreter.
    Dict(Rc<RefCell<Vec<(Value, Value)>>>),
    /// FIFO queue.
    Queue(Rc<RefCell<VecDeque<Value>>>),
    /// LIFO stack.
    Stack(Rc<RefCell<Vec<Value>>>),
    /// Immutable fixed-size tuple.
    Tuple(Vec<Value>),
    /// Named struct instance with mutable fields.
    Struct {
        type_name: String,
        fields:    Rc<RefCell<HashMap<String, Value>>>,
        /// Set once at construction (`ExprKind::StructLit` eval) from
        /// `Interpreter::struct_derives`, itself built from
        /// `@derive(PartialEq)` on this type's declaration — never
        /// re-derived per comparison. `false` for `<anon>` object
        /// literals (no declaration to have derived anything from).
        /// Read by `equals()` to pick structural comparison over the
        /// `Rc::ptr_eq` default; nothing else consults it (`Display`/
        /// `debug_string` print identically either way).
        derives_partial_eq: bool,
        /// Same construction-time story as `derives_partial_eq`, from
        /// `@derive(PartialOrd)` or `@derive(Ord)` (either one turns
        /// this on — `Ord` requires `PartialOrd` be present too, see
        /// `check_derive_attrs`, so checking just one derived name would
        /// miss nothing, but this flag is the runtime fact that matters:
        /// "does this type have a derived ordering at all"). Read by
        /// `partial_cmp()` to pick field-by-field comparison over the
        /// default "not comparable" (`None`). `false` for `<anon>`.
        derives_ord: bool,
        /// Same story again, from `@derive(Hash)`. Read by
        /// `compute_hash()` to pick structural hashing over the default
        /// identity (`Rc` pointer address) hash. `false` for `<anon>`.
        derives_hash: bool,
        /// Same story again, from `@derive(Clone)`. Read by
        /// `eval_method_call`'s struct dispatch (`interpreter/eval/
        /// expr.rs`) to decide whether `.clone()` is a real, callable
        /// deep-copy method on this instance — not consulted by
        /// `deep_clone()` itself, which is only ever reached once that
        /// check has already passed. `false` for `<anon>`.
        derives_clone: bool,
        /// Field names in declaration order — empty for `<anon>` object
        /// literals. `fields` is a `HashMap`, which has no defined
        /// iteration order and isn't guaranteed to agree between two
        /// separately-constructed instances of the same type, so a
        /// well-defined `partial_cmp()` (where field order changes the
        /// result, not just the iteration) and a *consistent*
        /// `compute_hash()` (where two structurally-equal instances must
        /// hash equal, which an arbitrary per-`HashMap` bucket order
        /// can't promise) both need a real, shared, declaration-derived
        /// order to work from instead of `fields`'s own iteration.
        /// Populated once at construction the same way `derives_*` are,
        /// for every named struct — not gated on the derives themselves,
        /// since it costs one `Rc<Vec<String>>` clone either way and
        /// keeping population unconditional means one code path instead
        /// of two.
        field_order: Rc<Vec<String>>,
    },
    /// Enum variant with optional payload.
    Enum {
        type_name: String,
        variant:   String,
        payload:   Box<EnumPayload>,
    },
    /// Index into the interpreter's function table.
    /// Both named functions and lambdas (closures) are represented this way.
    Function(FunctionId),
    /// `Pool<T>` — fixed-capacity slot table with a LIFO free list and a
    /// generation counter per slot (MEMORY_MODEL.md §11). Capacity comes
    /// from the innermost enclosing `with pool<T>(count) { }` at
    /// construction time — see `Interpreter::pool_capacity_stack`.
    Pool(Rc<RefCell<PoolData>>),
    /// A generational handle returned by `Pool<T>.acquire()`. Small and
    /// `Copy`-cheap by construction (no `Rc`); deliberately not
    /// constructible from a tuple literal or any other user-facing
    /// value, so a handle can only ever come from a real `acquire()`.
    Handle { index: usize, generation: u64 },
    /// `InlineList<T>` — fixed-capacity collection (DATASTRUCTURES.md
    /// §5). `items.len() <= capacity` always; `.push()` is checked
    /// (`bool` return, never grows past capacity, no silent
    /// truncation). Rust-heap-backed like every other `Value` variant
    /// here — there's no real codegen yet, so "inline/stack" is
    /// currently a language-level *contract* (bounded, checked) rather
    /// than a literal memory-placement guarantee, same honesty already
    /// applied to how Pool/Arena/GC tiers all share Rust-heap-backed
    /// representations today.
    InlineList(Rc<RefCell<InlineListData>>),
    /// `Linqerizer<T>` (`docs/DATASTRUCTURES.md` §6) — a lazy query
    /// pipeline. `source` is a snapshot taken once, at `.query()` time;
    /// `ops` accumulates as the chain grows (`.where()`/`.select()`/
    /// `.order_by()`/`.order_by_desc()` each return a *new* `Linqerizer`
    /// with one more op appended, never mutating the one they were
    /// called on — matching how every other LINQ-shaped thing works,
    /// C#/Rust iterators/JS included; `let a = q.where(f); let b =
    /// q.select(g);` would be surprising if `b` silently inherited `a`'s
    /// filter). Nothing in `ops` actually runs until a terminal method
    /// (`.to_list()`/`.first()`/`.count()`/`.group_by()`) walks `source`
    /// applying every pending op in order — that deferred-until-terminal
    /// property is the actual point, and the thing the old `eval_linq`
    /// (fully eager, one-pass) never had.
    Linqerizer(Rc<LinqPipeline>),

    // ── Ownership model (MEMORY_MODEL.md §9) ────────────────────────
    /// `Unique<T>` — single-owner, move-only (`Box<T>`-shaped). Plain
    /// `Box`, not `Rc`: Rust's `Box` is itself non-aliasable, so
    /// "cloning" a `Value::Unique` deep-clones the payload rather than
    /// sharing it — the semantically honest behavior for a strict
    /// single-owner type, and correct even before move *enforcement*
    /// exists (still ahead; this only lands construction).
    Unique(Box<Value>),
    /// `Shared<T>` — reference-counted, clone-based (`Rc<T>`-shaped).
    /// Same `Rc<RefCell<_>>` backing `List`/`Dict`/`Queue`/`Stack`/
    /// `Struct` already use — genuine aliasing, genuine shared mutation.
    Shared(Rc<RefCell<Value>>),
    /// `SyncShared<T>` — nominally `Arc<T>`-shaped (`Send`+`Sync`), but
    /// backed by the same `Rc<RefCell<_>>` as `Shared` for now, not a
    /// real `Arc<Mutex<_>>`. `Value` overall isn't `Send` (`List`/`Dict`/
    /// etc. use `Rc` internally), so an `Arc<Mutex<_>>` wrapper here
    /// would be structurally real but semantically hollow — nothing
    /// could actually send one across a thread today regardless of this
    /// variant's own backing. Kept as its own `Value` variant (not
    /// unified with `Shared`) so diagnostics and any future real
    /// thread-safety story have a distinct thing to point at, same
    /// reasoning `Handle` is kept distinct from a plain tuple. See
    /// `builtins::constructors::sync_shared_new`.
    SyncShared(Rc<RefCell<Value>>),
}

/// Backing storage for `Value::Linqerizer`. Immutable once built (see
/// the `Value::Linqerizer` doc comment on why chaining allocates a new
/// one rather than mutating) — plain `Rc`, no `RefCell`, nothing here
/// is ever mutated in place.
#[derive(Debug, Clone)]
pub struct LinqPipeline {
    /// Snapshot of the source, taken once at `.query()` time. `Rc` so
    /// every step in a chain shares it instead of re-cloning the whole
    /// `Vec` on every `.where()`/`.select()` call — only `ops` grows.
    pub source: Rc<Vec<Value>>,
    pub ops:    Vec<LinqOp>,
}

#[derive(Debug, Clone)]
pub enum LinqOp {
    Where(FunctionId),
    Select(FunctionId),
    /// `bool` = descending.
    OrderBy(FunctionId, bool),
}

/// Backing storage for `Value::InlineList`.
#[derive(Debug, Clone)]
pub struct InlineListData {
    pub items:    Vec<Value>,
    pub capacity: usize,
}

/// Backing storage for `Value::Pool`. Block-chained (DATASTRUCTURES.md —
/// Pool absorbs Hive's growability rather than a separate `Hive<T>` type):
/// `blocks[i]`/`generations[i]` are always the same length (`block_size`,
/// fixed at construction, from the enclosing `with pool<T>(count) { }`).
/// A full pool with `growable == true` appends a brand-new block rather
/// than reallocating/copying the existing ones — existing `Handle`s
/// (flat, cross-block indices) stay valid, and no live slot's data ever
/// moves. `free_list` is a `VecDeque` so both LIFO (default, pop from the
/// back — matches §10 item 3's "most-recently-freed reused first") and
/// FIFO (`.fifo()` opt-in, pop from the front) are O(1); `.push_back` on
/// release either way, only the acquire-side pop direction differs.
#[derive(Debug, Clone)]
pub struct PoolData {
    pub blocks:      Vec<Vec<Option<Value>>>,
    pub generations: Vec<Vec<u64>>,
    pub free_list:   std::collections::VecDeque<usize>,
    pub block_size:  usize,
    pub growable:    bool,
    pub fifo:        bool,
}

impl PoolData {
    pub fn with_capacity(capacity: usize) -> Self {
        PoolData {
            blocks:      vec![vec![None; capacity]],
            generations: vec![vec![0; capacity]],
            free_list:   (0..capacity).collect(),
            block_size:  capacity,
            growable:    false,
            fifo:        false,
        }
    }

    #[inline]
    fn locate(&self, flat_index: usize) -> (usize, usize) {
        (flat_index / self.block_size, flat_index % self.block_size)
    }

    pub fn total_capacity(&self) -> usize {
        self.blocks.len() * self.block_size
    }

    pub fn slot(&self, flat_index: usize) -> Option<&Value> {
        let (b, o) = self.locate(flat_index);
        self.blocks.get(b)?.get(o)?.as_ref()
    }

    pub fn slot_mut(&mut self, flat_index: usize) -> Option<&mut Option<Value>> {
        let (b, o) = self.locate(flat_index);
        self.blocks.get_mut(b)?.get_mut(o)
    }

    pub fn generation(&self, flat_index: usize) -> Option<u64> {
        let (b, o) = self.locate(flat_index);
        self.generations.get(b)?.get(o).copied()
    }

    pub fn generation_mut(&mut self, flat_index: usize) -> Option<&mut u64> {
        let (b, o) = self.locate(flat_index);
        self.generations.get_mut(b)?.get_mut(o)
    }

    /// Append one new block (`block_size` fresh slots) if `growable`,
    /// returning whether it did. Existing blocks are untouched — this is
    /// the actual "no reallocation-copy" property that makes it Hive-
    /// shaped rather than just `Vec::resize` wearing a new name; the
    /// latter would've been just as `Handle`-safe (indices, not raw
    /// pointers, so nothing would dangle) but would pay an O(n) copy on
    /// every grow and rule out a raw-pointer-stable FFI escape hatch
    /// later (DATASTRUCTURES.md §1).
    pub fn try_grow(&mut self) -> bool {
        if !self.growable { return false; }
        let start = self.blocks.len() * self.block_size;
        self.blocks.push(vec![None; self.block_size]);
        self.generations.push(vec![0; self.block_size]);
        for i in start..start + self.block_size {
            self.free_list.push_back(i);
        }
        true
    }

    /// LIFO (default) pops the back — most-recently-freed reused first.
    /// FIFO (`.fifo()`) pops the front. Release always pushes to the
    /// back either way; only the acquire-side end differs.
    pub fn free_pop(&mut self) -> Option<usize> {
        if self.fifo { self.free_list.pop_front() } else { self.free_list.pop_back() }
    }

    pub fn free_push(&mut self, index: usize) {
        self.free_list.push_back(index);
    }

    /// Every currently-occupied slot, in index order, holes skipped —
    /// the actual skipfield behavior (`for x in pool { }`,
    /// DATASTRUCTURES.md §1's ".iter()"). Bare values only, not paired
    /// with their `Handle` — see `value_to_iter_vec`'s own doc comment
    /// on why that pairing is deliberately not done yet.
    pub fn iter_occupied(&self) -> impl Iterator<Item = &Value> + '_ {
        self.blocks.iter().flatten().filter_map(|s| s.as_ref())
    }
}

/// Payload carried by an enum variant at runtime.
#[derive(Debug, Clone, PartialEq)]
pub enum EnumPayload {
    None,
    Tuple(Vec<Value>),
    Struct(HashMap<String, Value>),
}

/// Control-flow signal. Returned as `Err(Signal)` to propagate non-local
/// exits through the evaluation call stack without unwinding.
#[derive(Debug, Clone)]
pub enum Signal {
    /// `return expr` — propagates to enclosing function call.
    Return(Value),
    /// `break expr?` — exits the enclosing loop.
    Break(Option<Value>),
    /// `continue` — skips to the next loop iteration.
    Continue,
    /// `fail expr` — propagates until caught by `try { } catch`.
    Fail(Value),
    /// Unrecoverable error (`panic()`, failed assertion, interpreter bug).
    Panic(String),
}

/// Every eval function returns this. `Ok(Value)` is the normal path;
/// `Err(Signal)` carries control flow or errors.
pub type EvalResult = Result<Value, Signal>;

// ── Value methods ─────────────────────────────────────────────────

impl Value {
    /// Short human-readable type name for errors and `typeof()`.
    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Null          => "null",
            Value::Void          => "void",
            Value::Bool(_)       => "bool",
            Value::Int(_)        => "int",
            Value::I8(_)         => "i8",
            Value::I16(_)        => "i16",
            Value::I32(_)        => "i32",
            Value::U8(_)         => "u8",
            Value::U16(_)        => "u16",
            Value::U32(_)        => "u32",
            Value::UInt(_)       => "u64",
            Value::Float(_)      => "float",
            Value::Double(_)     => "double",
            Value::Char(_)       => "char",
            Value::Str(_)        => "string",
            Value::List(_)       => "List",
            Value::Dict(_)       => "Dictionary",
            Value::Queue(_)      => "Queue",
            Value::Stack(_)      => "Stack",
            Value::Tuple(_)      => "tuple",
            Value::Struct { .. } => "struct",
            Value::Enum { .. }   => "enum",
            Value::Function(_)   => "function",
            Value::Pool(_)       => "Pool",
            Value::Handle { .. } => "Handle",
            Value::InlineList(_) => "InlineList",
            Value::Linqerizer(_) => "Linqerizer",
            Value::Unique(_)     => "Unique",
            Value::Shared(_)     => "Shared",
            Value::SyncShared(_) => "SyncShared",
        }
    }

    /// Ubel conditions must be `bool`; anything else is a runtime panic.
    pub fn is_truthy(&self) -> Result<bool, Signal> {
        match self {
            Value::Bool(b) => Ok(*b),
            other => Err(Signal::Panic(format!(
                "condition must be bool, got {}",
                other.type_name()
            ))),
        }
    }

    /// Structural equality that doesn't require `Hash`.
    /// Heap types (`List`, `Dict`, `Struct`) use referential equality
    /// (same `Rc` pointer) which is consistent with HIGH-tier shared semantics.
    pub fn equals(&self, other: &Value) -> bool {
        match (self, other) {
            (Value::Null,  Value::Null)  => true,
            (Value::Void,  Value::Void)  => true,
            (Value::Bool(a), Value::Bool(b)) => a == b,
            (Value::Int(a),  Value::Int(b))  => a == b,
            (Value::I8(a),   Value::I8(b))   => a == b,
            (Value::I16(a),  Value::I16(b))  => a == b,
            (Value::I32(a),  Value::I32(b))  => a == b,
            (Value::U8(a),   Value::U8(b))   => a == b,
            (Value::U16(a),  Value::U16(b))  => a == b,
            (Value::U32(a),  Value::U32(b))  => a == b,
            (Value::UInt(a), Value::UInt(b)) => a == b,
            (Value::Float(a),  Value::Float(b))  => a == b,
            (Value::Double(a), Value::Double(b)) => a == b,
            (Value::Char(a), Value::Char(b)) => a == b,
            (Value::Str(a),  Value::Str(b))  => a == b,
            (Value::Tuple(a), Value::Tuple(b)) => {
                a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.equals(y))
            }
            (Value::Enum { type_name: ta, variant: va, payload: pa },
             Value::Enum { type_name: tb, variant: vb, payload: pb }) => {
                ta == tb && va == vb && pa == pb
            }
            // Heap types: pointer equality.
            (Value::List(a),   Value::List(b))   => Rc::ptr_eq(a, b),
            (Value::Dict(a),   Value::Dict(b))   => Rc::ptr_eq(a, b),
            (Value::Queue(a),  Value::Queue(b))  => Rc::ptr_eq(a, b),
            (Value::Stack(a),  Value::Stack(b))  => Rc::ptr_eq(a, b),
            (Value::Struct { type_name: ta, fields: a, derives_partial_eq: da, .. },
             Value::Struct { type_name: tb, fields: b, derives_partial_eq: db, .. }) => {
                if *da || *db {
                    // Structural: same declared type, same field set,
                    // each field equal by its own `.equals()` — recurses
                    // correctly through Unique (by value) / Shared /
                    // SyncShared (by pointer) fields using whatever each
                    // of *their* variants already does, no special-casing
                    // needed here.
                    let (fa, fb) = (a.borrow(), b.borrow());
                    ta == tb && fa.len() == fb.len()
                        && fa.iter().all(|(k, v)| fb.get(k).is_some_and(|ov| v.equals(ov)))
                } else {
                    Rc::ptr_eq(a, b)
                }
            }
            (Value::Function(a), Value::Function(b)) => a == b,
            (Value::Pool(a), Value::Pool(b)) => Rc::ptr_eq(a, b),
            (Value::Handle { index: ia, generation: ga },
             Value::Handle { index: ib, generation: gb }) => ia == ib && ga == gb,
            (Value::InlineList(a), Value::InlineList(b)) => Rc::ptr_eq(a, b),
            (Value::Linqerizer(a), Value::Linqerizer(b)) => Rc::ptr_eq(a, b),
            // `Unique` isn't `Rc`-backed (never aliased by construction),
            // so pointer identity isn't meaningful — compare by value
            // instead, matching real Rust's own `Box<T>: PartialEq`
            // (`Box::new(5) == Box::new(5)` is `true`), not the "heap
            // types: pointer equality" convention above (that convention
            // exists *because* those types are aliased/shared; `Unique`
            // is deliberately the one wrapper here that isn't).
            (Value::Unique(a), Value::Unique(b)) => a.equals(b),
            (Value::Shared(a), Value::Shared(b)) => Rc::ptr_eq(a, b),
            (Value::SyncShared(a), Value::SyncShared(b)) => Rc::ptr_eq(a, b),
            _ => false,
        }
    }

    /// Ordering comparison — backs the real `<`/`<=`/`>`/`>=` operators
    /// (`eval_binop`, `interpreter/eval/expr.rs`) and is the single
    /// implementation `linqerizer_methods::materialize`'s `OrderBy` case
    /// now calls too, retiring that module's own narrower, private
    /// `compare_values` (same single-source-of-truth relationship
    /// `equals()` already has with every other comparison here).
    ///
    /// `None` means "not comparable" — either a genuinely incomparable
    /// pair (different types, a non-derived struct, or a type with no
    /// ordering concept at all: `List`/`Dict`/`Enum`/etc, out of scope
    /// here the same way they're out of scope for `@derive(Ord)`/
    /// `@derive(PartialOrd)`) or a comparable *shape* whose values
    /// happen to be incomparable right now (`Float`/`Double` `NaN`, via
    /// Rust's own `f32`/`f64::partial_cmp`) — the two cases aren't
    /// distinguished, matching how `PartialOrd::partial_cmp` never
    /// distinguishes them either.
    ///
    /// `Unique`/`Shared`/`SyncShared` all delegate to the *inner*
    /// value's ordering — a deliberate, confirmed divergence from
    /// `equals()` for `Shared`/`SyncShared` specifically (which compare
    /// by `Rc::ptr_eq`, not content): ordering two `Shared<T>` values by
    /// raw pointer address would be legal but meaningless to whoever
    /// wrote `a < b` (and non-deterministic run-to-run besides), so
    /// content is the only choice that's actually useful for sorting —
    /// unlike equality, where "same object" is a perfectly meaningful
    /// question on its own.
    pub fn partial_cmp(&self, other: &Value) -> Option<std::cmp::Ordering> {
        use std::cmp::Ordering;
        match (self, other) {
            (Value::Int(a),    Value::Int(b))    => Some(a.cmp(b)),
            (Value::I8(a),     Value::I8(b))     => Some(a.cmp(b)),
            (Value::I16(a),    Value::I16(b))    => Some(a.cmp(b)),
            (Value::I32(a),    Value::I32(b))    => Some(a.cmp(b)),
            (Value::U8(a),     Value::U8(b))     => Some(a.cmp(b)),
            (Value::U16(a),    Value::U16(b))    => Some(a.cmp(b)),
            (Value::U32(a),    Value::U32(b))    => Some(a.cmp(b)),
            (Value::UInt(a),   Value::UInt(b))   => Some(a.cmp(b)),
            (Value::Float(a),  Value::Float(b))  => a.partial_cmp(b),
            (Value::Double(a), Value::Double(b)) => a.partial_cmp(b),
            (Value::Str(a),    Value::Str(b))    => Some(a.cmp(b)),
            (Value::Bool(a),   Value::Bool(b))   => Some(a.cmp(b)),
            (Value::Struct { type_name: ta, fields: fa, derives_ord: true, field_order, .. },
             Value::Struct { type_name: tb, fields: fb, derives_ord: true, .. }) if ta == tb => {
                let (fa, fb) = (fa.borrow(), fb.borrow());
                for name in field_order.iter() {
                    match (fa.get(name), fb.get(name)) {
                        (Some(va), Some(vb)) => match va.partial_cmp(vb) {
                            Some(Ordering::Equal) => continue,
                            other => return other,
                        },
                        // Sema has already validated field sets match by
                        // this point (same declared type) — reachable
                        // only for an `<anon>`-shaped mismatch, treated
                        // as not comparable rather than panicking.
                        _ => return None,
                    }
                }
                Some(Ordering::Equal)
            }
            (Value::Unique(a), Value::Unique(b)) => a.partial_cmp(b),
            (Value::Shared(a), Value::Shared(b)) => a.borrow().partial_cmp(&b.borrow()),
            (Value::SyncShared(a), Value::SyncShared(b)) => a.borrow().partial_cmp(&b.borrow()),
            _ => None,
        }
    }

    /// Structural hash — `Hash`'s half of the `Eq`+`Hash` contract every
    /// real hash-map API actually requires (not just `Hash` alone — see
    /// `check_derive_attrs`'s prerequisite chain, TYPE-117). No consumer
    /// yet (`Dict` is still `Vec<pair>`; `Value` having no `Hash` is
    /// *why*, per its own doc comment) — shipped ahead of one anyway,
    /// same precedent `move_facts.rs` set: real behavior, real unit
    /// tests, nothing user-observable different until something
    /// downstream actually consumes it.
    ///
    /// Must agree with `equals()`: `a.equals(b)` implies
    /// `a.compute_hash() == b.compute_hash()`. Checked variant by
    /// variant against `equals()`'s own rule for that variant, not
    /// assumed:
    ///   - `Struct` (when derived)/`Tuple`/`Unique`/`Enum` compare
    ///     structurally in `equals()` (or always, for `Tuple`/`Unique`/
    ///     `Enum`), so they hash structurally here too.
    ///   - `List`/`Dict`/`Queue`/`Stack`/`Pool`/`InlineList`/
    ///     `Linqerizer`/`Shared`/`SyncShared`, and a non-derived
    ///     `Struct`, all compare by `Rc::ptr_eq` in `equals()`, so they
    ///     hash the pointer address here, not their contents — hashing
    ///     contents would let two *unequal* (by `equals()`) but
    ///     same-valued instances collide into looking interchangeable,
    ///     and would break the contract outright the moment either
    ///     mutated after insertion (a `RefCell`'s content isn't stable
    ///     the way its address is).
    ///   - `Float`/`Double` hash by bit pattern (`to_bits`), with `-0.0`
    ///     normalized to `0.0`'s bits first — `equals()` uses plain
    ///     `==`, under which `-0.0 == 0.0`, so without normalizing,
    ///     equal-by-`equals()` values could still hash unequal. `NaN`
    ///     gets no such normalization: `equals()` already says `NaN !=
    ///     NaN`, so two `NaN`s aren't required to hash equal, and in
    ///     fact generally won't (differing `NaN` bit patterns are
    ///     common) — consistent with `equals()`, not a new
    ///     inconsistency.
    ///   - `EnumPayload::Struct`'s field map has the same `HashMap`
    ///     ordering problem `Value::Struct` does, with no declaration to
    ///     derive a canonical order from (enum `@derive` isn't in scope
    ///     this delivery) — sorted by field name at hash time instead,
    ///     a simpler fix that only needs to be *consistent*, not
    ///     meaningful to a reader the way `Struct`'s declaration-order
    ///     `field_order` needs to be for `partial_cmp`.
    pub fn compute_hash(&self) -> u64 {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::Hasher;
        let mut hasher = DefaultHasher::new();
        self.hash_into(&mut hasher);
        hasher.finish()
    }

    fn hash_into(&self, hasher: &mut impl std::hash::Hasher) {
        use std::hash::Hash;
        // Discriminant first so e.g. `Int(0)` and `Bool(false)` (or an
        // empty `Str` and `Null`) never collide just because their
        // payload-hashing happens to agree.
        std::mem::discriminant(self).hash(hasher);
        match self {
            Value::Null | Value::Void => {}
            Value::Bool(b) => b.hash(hasher),
            Value::Int(n)  => n.hash(hasher),
            Value::I8(n)   => n.hash(hasher),
            Value::I16(n)  => n.hash(hasher),
            Value::I32(n)  => n.hash(hasher),
            Value::U8(n)   => n.hash(hasher),
            Value::U16(n)  => n.hash(hasher),
            Value::U32(n)  => n.hash(hasher),
            Value::UInt(n) => n.hash(hasher),
            Value::Char(c) => c.hash(hasher),
            Value::Float(f) => {
                let f = if *f == 0.0 { 0.0f32 } else { *f };
                f.to_bits().hash(hasher);
            }
            Value::Double(d) => {
                let d = if *d == 0.0 { 0.0f64 } else { *d };
                d.to_bits().hash(hasher);
            }
            Value::Str(s) => s.hash(hasher),
            Value::Tuple(items) => {
                items.len().hash(hasher);
                for item in items { item.hash_into(hasher); }
            }
            Value::Struct { type_name, fields, derives_hash: true, field_order, .. } => {
                type_name.hash(hasher);
                let fields = fields.borrow();
                for name in field_order.iter() {
                    name.hash(hasher);
                    if let Some(v) = fields.get(name) { v.hash_into(hasher); }
                }
            }
            Value::Struct { fields, .. } => {
                // Not derived — same identity fallback `equals()` uses.
                Rc::as_ptr(fields).hash(hasher);
            }
            Value::Enum { type_name, variant, payload } => {
                type_name.hash(hasher);
                variant.hash(hasher);
                match payload.as_ref() {
                    EnumPayload::None => {}
                    EnumPayload::Tuple(items) => {
                        for item in items { item.hash_into(hasher); }
                    }
                    EnumPayload::Struct(map) => {
                        let mut keys: Vec<&String> = map.keys().collect();
                        keys.sort();
                        for k in keys {
                            k.hash(hasher);
                            map[k].hash_into(hasher);
                        }
                    }
                }
            }
            Value::Function(id) => id.hash(hasher),
            Value::Pool(rc) => Rc::as_ptr(rc).hash(hasher),
            Value::Handle { index, generation } => { index.hash(hasher); generation.hash(hasher); }
            Value::InlineList(rc) => Rc::as_ptr(rc).hash(hasher),
            Value::Linqerizer(rc) => Rc::as_ptr(rc).hash(hasher),
            Value::List(rc) => Rc::as_ptr(rc).hash(hasher),
            Value::Dict(rc) => Rc::as_ptr(rc).hash(hasher),
            Value::Queue(rc) => Rc::as_ptr(rc).hash(hasher),
            Value::Stack(rc) => Rc::as_ptr(rc).hash(hasher),
            Value::Unique(inner) => inner.hash_into(hasher),
            Value::Shared(rc) => Rc::as_ptr(rc).hash(hasher),
            Value::SyncShared(rc) => Rc::as_ptr(rc).hash(hasher),
        }
    }

    /// `.clone()`'s real implementation once `@derive(Clone)` gates it
    /// on for a struct (`eval_method_call`, `interpreter/eval/expr.rs`)
    /// — genuinely independent, not the cheap `Rc`-bump the derived
    /// Rust `Clone` on `Value` itself already gives every value for
    /// free (this file's own top doc comment). Recurses through
    /// everything a struct might hold *except* `Shared`/`SyncShared`,
    /// which alias (bump the `Rc`) instead — the whole reason those two
    /// wrappers exist is deliberate, explicit shared ownership (their
    /// own doc comments: "genuine aliasing, genuine shared mutation"),
    /// so recursing through one here would silently undo the one thing
    /// the person wrote `Shared<T>` to ask for. Matches
    /// `Rc<RefCell<T>>::clone()` in real Rust for exactly the same
    /// reason.
    ///
    /// `Pool`/`InlineList`/`Linqerizer` are a known, stated limitation:
    /// deep-cloning a generational slot table (or a lazy pipeline's
    /// snapshot) correctly is real, separate design work with no
    /// motivating use case yet (a struct holding one of these directly
    /// isn't the pattern `@derive(Clone)` exists for) — they fall back
    /// to the same shallow `Rc`-bump the derived Rust `Clone` already
    /// does, same as `Function`'s bare index (nothing to deep-copy) and
    /// `Handle`'s two integers (nothing `Rc`-backed to alias in the
    /// first place — a plain copy already *is* independent).
    pub fn deep_clone(&self) -> Value {
        match self {
            Value::Null | Value::Void
            | Value::Bool(_) | Value::Int(_) | Value::Float(_) | Value::Double(_)
            | Value::I8(_) | Value::I16(_) | Value::I32(_)
            | Value::U8(_) | Value::U16(_) | Value::U32(_) | Value::UInt(_)
            | Value::Char(_) | Value::Str(_)
            | Value::Function(_) | Value::Handle { .. }
            | Value::Pool(_) | Value::InlineList(_) | Value::Linqerizer(_) => self.clone(),
            Value::List(rc) => {
                let items: Vec<Value> = rc.borrow().iter().map(Value::deep_clone).collect();
                Value::List(Rc::new(RefCell::new(items)))
            }
            Value::Dict(rc) => {
                let pairs: Vec<(Value, Value)> = rc.borrow().iter()
                    .map(|(k, v)| (k.deep_clone(), v.deep_clone()))
                    .collect();
                Value::Dict(Rc::new(RefCell::new(pairs)))
            }
            Value::Queue(rc) => {
                let items: VecDeque<Value> = rc.borrow().iter().map(Value::deep_clone).collect();
                Value::Queue(Rc::new(RefCell::new(items)))
            }
            Value::Stack(rc) => {
                let items: Vec<Value> = rc.borrow().iter().map(Value::deep_clone).collect();
                Value::Stack(Rc::new(RefCell::new(items)))
            }
            Value::Tuple(items) => Value::Tuple(items.iter().map(Value::deep_clone).collect()),
            Value::Struct { type_name, fields, derives_partial_eq, derives_ord, derives_hash, derives_clone, field_order } => {
                let cloned: HashMap<String, Value> = fields.borrow().iter()
                    .map(|(k, v)| (k.clone(), v.deep_clone()))
                    .collect();
                Value::Struct {
                    type_name:          type_name.clone(),
                    fields:             Rc::new(RefCell::new(cloned)),
                    derives_partial_eq: *derives_partial_eq,
                    derives_ord:        *derives_ord,
                    derives_hash:       *derives_hash,
                    derives_clone:      *derives_clone,
                    field_order:        Rc::clone(field_order),
                }
            }
            Value::Enum { type_name, variant, payload } => {
                let cloned_payload = match payload.as_ref() {
                    EnumPayload::None => EnumPayload::None,
                    EnumPayload::Tuple(items) =>
                        EnumPayload::Tuple(items.iter().map(Value::deep_clone).collect()),
                    EnumPayload::Struct(map) => EnumPayload::Struct(
                        map.iter().map(|(k, v)| (k.clone(), v.deep_clone())).collect()
                    ),
                };
                Value::Enum {
                    type_name: type_name.clone(),
                    variant:   variant.clone(),
                    payload:   Box::new(cloned_payload),
                }
            }
            Value::Unique(inner) => Value::Unique(Box::new(inner.deep_clone())),
            // Deliberate divergence from every arm above: alias, don't
            // recurse — see the doc comment on this method.
            Value::Shared(rc)     => Value::Shared(Rc::clone(rc)),
            Value::SyncShared(rc) => Value::SyncShared(Rc::clone(rc)),
        }
    }

    /// Ubel-language-level `{x:?}` formatter — distinct from `Display`
    /// (`{x}`) in exactly two ways, both chosen because they're real
    /// information available *right now*, not fabricated for the
    /// occasion (docs/PRINT_FORMAT_RULES.md §4 explicitly rejected a
    /// fabricated tier/arena tag here — no `Value` carries one at
    /// runtime, on purpose, per MEMORY_MODEL.md §7):
    ///
    ///   1. `Str`/`Char` are quoted and escaped (Rust's own `{:?}` on
    ///      `&str`/`char` does exactly this) — `hello` (Display) vs
    ///      `"hello"` (Debug) — so embedded whitespace/quotes are
    ///      visible instead of blending into surrounding text.
    ///   2. `Shared`/`SyncShared` show their live `Rc::strong_count` —
    ///      genuinely new info `Display` shouldn't clutter but a
    ///      systems debugger printing an aliased value legitimately
    ///      wants: how many places currently point at this.
    ///
    /// Everything else delegates straight to `Display` — nothing else
    /// real to add yet (`PartialEq`/`Eq`/`Hash`/`Ord`/`Clone` as their
    /// own derivable concepts remain a separate, deferred slice).
    pub fn debug_string(&self) -> String {
        match self {
            Value::Str(s)  => format!("{:?}", s.as_str()),
            Value::Char(c) => format!("{:?}", c),

            Value::Tuple(elems) => {
                let mut out = String::from("(");
                for (i, e) in elems.iter().enumerate() {
                    if i > 0 { out.push_str(", "); }
                    out.push_str(&e.debug_string());
                }
                out.push(')');
                out
            }
            Value::List(rc) => {
                let items = rc.borrow();
                let mut out = String::from("[");
                for (i, v) in items.iter().enumerate() {
                    if i > 0 { out.push_str(", "); }
                    out.push_str(&v.debug_string());
                }
                out.push(']');
                out
            }
            Value::Dict(rc) => {
                let entries = rc.borrow();
                let mut out = String::from("{");
                for (i, (k, v)) in entries.iter().enumerate() {
                    if i > 0 { out.push_str(", "); }
                    out.push_str(&k.debug_string());
                    out.push_str(": ");
                    out.push_str(&v.debug_string());
                }
                out.push('}');
                out
            }
            Value::Queue(rc) => {
                let items = rc.borrow();
                let mut out = String::from("Queue[");
                for (i, v) in items.iter().enumerate() {
                    if i > 0 { out.push_str(", "); }
                    out.push_str(&v.debug_string());
                }
                out.push(']');
                out
            }
            Value::Stack(rc) => {
                let items = rc.borrow();
                let mut out = String::from("Stack[");
                for (i, v) in items.iter().enumerate() {
                    if i > 0 { out.push_str(", "); }
                    out.push_str(&v.debug_string());
                }
                out.push(']');
                out
            }
            Value::InlineList(rc) => {
                let data = rc.borrow();
                let mut out = String::from("InlineList[");
                for (i, v) in data.items.iter().enumerate() {
                    if i > 0 { out.push_str(", "); }
                    out.push_str(&v.debug_string());
                }
                out.push_str(&format!("] (len={}, capacity={})", data.items.len(), data.capacity));
                out
            }
            Value::Unique(v) => format!("Unique({})", v.debug_string()),
            Value::Shared(rc) => format!(
                "Shared(refs={}, {})", Rc::strong_count(rc), rc.borrow().debug_string()
            ),
            Value::SyncShared(rc) => format!(
                "SyncShared(refs={}, {})", Rc::strong_count(rc), rc.borrow().debug_string()
            ),
            Value::Struct { type_name, fields, .. } => {
                let fields = fields.borrow();
                let mut out = format!("{} {{", type_name);
                for (i, (k, v)) in fields.iter().enumerate() {
                    if i > 0 { out.push_str(", "); }
                    out.push_str(k);
                    out.push_str(": ");
                    out.push_str(&v.debug_string());
                }
                out.push('}');
                out
            }
            Value::Enum { type_name, variant, payload } => match payload.as_ref() {
                EnumPayload::None => format!("{}.{}", type_name, variant),
                EnumPayload::Tuple(items) => {
                    let mut out = format!("{}.{}(", type_name, variant);
                    for (i, v) in items.iter().enumerate() {
                        if i > 0 { out.push_str(", "); }
                        out.push_str(&v.debug_string());
                    }
                    out.push(')');
                    out
                }
                EnumPayload::Struct(fields) => {
                    let mut out = format!("{}.{} {{", type_name, variant);
                    for (i, (k, v)) in fields.iter().enumerate() {
                        if i > 0 { out.push_str(", "); }
                        out.push_str(k);
                        out.push_str(": ");
                        out.push_str(&v.debug_string());
                    }
                    out.push('}');
                    out
                }
            },
            // Null/Void/Bool/Int/Float/Double/Function/Pool/Handle/
            // Linqerizer: nothing debug-specific to add over Display yet.
            other => other.to_string(),
        }
    }

    /// Convenience: make an empty `Value::Pool` with the given capacity.
    pub fn new_pool(capacity: usize) -> Self {
        Value::Pool(Rc::new(RefCell::new(PoolData::with_capacity(capacity))))
    }

    /// Convenience: make an empty `Value::InlineList` with the given
    /// (sema-checked-literal) capacity.
    pub fn new_inline_list(capacity: usize) -> Self {
        Value::InlineList(Rc::new(RefCell::new(InlineListData {
            items: Vec::with_capacity(capacity),
            capacity,
        })))
    }

    /// Convenience: make a `Value::Str` from a `&str`.
    pub fn str_from(s: impl Into<String>) -> Self {
        Value::Str(Rc::new(s.into()))
    }

    /// Convenience: make an empty `Value::List`.
    pub fn new_list() -> Self {
        Value::List(Rc::new(RefCell::new(Vec::new())))
    }

    /// Convenience: make an empty `Value::Dict`.
    pub fn new_dict() -> Self {
        Value::Dict(Rc::new(RefCell::new(Vec::new())))
    }

    pub fn new_queue() -> Self {
        Value::Queue(Rc::new(RefCell::new(VecDeque::new())))
    }

    pub fn new_stack() -> Self {
        Value::Stack(Rc::new(RefCell::new(Vec::new())))
    }
}

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool { self.equals(other) }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Null        => write!(f, "null"),
            Value::Void        => Ok(()),
            Value::Bool(b)     => write!(f, "{}", b),
            Value::Int(n)      => write!(f, "{}", n),
            Value::I8(n)       => write!(f, "{}", n),
            Value::I16(n)      => write!(f, "{}", n),
            Value::I32(n)      => write!(f, "{}", n),
            Value::U8(n)       => write!(f, "{}", n),
            Value::U16(n)      => write!(f, "{}", n),
            Value::U32(n)      => write!(f, "{}", n),
            Value::UInt(n)     => write!(f, "{}", n),
            Value::Float(v)    => write!(f, "{}", v),
            Value::Double(v)   => write!(f, "{}", v),
            Value::Char(c)     => write!(f, "{}", c),
            Value::Str(s)      => write!(f, "{}", s),
            Value::Function(i) => write!(f, "<fn #{}>", i),
            Value::Tuple(elems) => {
                write!(f, "(")?;
                for (i, e) in elems.iter().enumerate() {
                    if i > 0 { write!(f, ", ")?; }
                    write!(f, "{}", e)?;
                }
                write!(f, ")")
            }
            Value::List(rc) => {
                let items = rc.borrow();
                write!(f, "[")?;
                for (i, v) in items.iter().enumerate() {
                    if i > 0 { write!(f, ", ")?; }
                    write!(f, "{}", v)?;
                }
                write!(f, "]")
            }
            Value::Dict(rc) => {
                let entries = rc.borrow();
                write!(f, "{{")?;
                for (i, (k, v)) in entries.iter().enumerate() {
                    if i > 0 { write!(f, ", ")?; }
                    write!(f, "{}: {}", k, v)?;
                }
                write!(f, "}}")
            }
            Value::Queue(rc) => {
                let items = rc.borrow();
                write!(f, "Queue[")?;
                for (i, v) in items.iter().enumerate() {
                    if i > 0 { write!(f, ", ")?; }
                    write!(f, "{}", v)?;
                }
                write!(f, "]")
            }
            Value::Stack(rc) => {
                let items = rc.borrow();
                write!(f, "Stack[")?;
                for (i, v) in items.iter().enumerate() {
                    if i > 0 { write!(f, ", ")?; }
                    write!(f, "{}", v)?;
                }
                write!(f, "]")
            }
            Value::InlineList(rc) => {
                let data = rc.borrow();
                write!(f, "InlineList[")?;
                for (i, v) in data.items.iter().enumerate() {
                    if i > 0 { write!(f, ", ")?; }
                    write!(f, "{}", v)?;
                }
                write!(f, "] (len={}, capacity={})", data.items.len(), data.capacity)
            }
            Value::Linqerizer(pipeline) => {
                // Lazy and not yet materialized — showing the pending
                // pipeline shape rather than pretending to show results
                // that haven't been computed (no `Interpreter` access
                // here to actually run the ops even if we wanted to).
                write!(f, "Linqerizer(source_len={}, pending_ops={})",
                    pipeline.source.len(), pipeline.ops.len())
            }
            Value::Unique(v) => write!(f, "Unique({})", v),
            Value::Shared(rc) => write!(f, "Shared({})", rc.borrow()),
            Value::SyncShared(rc) => write!(f, "SyncShared({})", rc.borrow()),
            Value::Struct { type_name, fields, .. } => {
                let fields = fields.borrow();
                write!(f, "{} {{", type_name)?;
                for (i, (k, v)) in fields.iter().enumerate() {
                    if i > 0 { write!(f, ", ")?; }
                    write!(f, "{}: {}", k, v)?;
                }
                write!(f, "}}")
            }
            Value::Pool(rc) => {
                let pool = rc.borrow();
                write!(f, "Pool(capacity={}, free={})", pool.total_capacity(), pool.free_list.len())
            }
            Value::Handle { index, generation } => write!(f, "Handle(#{}/{})", index, generation),
            Value::Enum { type_name, variant, payload } => {
                match payload.as_ref() {
                    EnumPayload::None => write!(f, "{}.{}", type_name, variant),
                    EnumPayload::Tuple(items) => {
                        write!(f, "{}.{}(", type_name, variant)?;
                        for (i, v) in items.iter().enumerate() {
                            if i > 0 { write!(f, ", ")?; }
                            write!(f, "{}", v)?;
                        }
                        write!(f, ")")
                    }
                    EnumPayload::Struct(fields) => {
                        write!(f, "{}.{} {{", type_name, variant)?;
                        for (i, (k, v)) in fields.iter().enumerate() {
                            if i > 0 { write!(f, ", ")?; }
                            write!(f, "{}: {}", k, v)?;
                        }
                        write!(f, "}}")
                    }
                }
            }
        }
    }
        }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_debug_string_quotes_str_display_does_not() {
        let v = Value::str_from("hello");
        assert_eq!(v.to_string(), "hello");
        assert_eq!(v.debug_string(), "\"hello\"");
    }

    #[test]
    fn test_debug_string_quotes_and_escapes_char() {
        let v = Value::Char('x');
        assert_eq!(v.to_string(), "x");
        assert_eq!(v.debug_string(), "'x'");

        let newline = Value::str_from("a\nb");
        assert_eq!(newline.debug_string(), "\"a\\nb\"");
        assert_eq!(newline.to_string(), "a\nb");
    }

    #[test]
    fn test_debug_string_recurses_into_struct_fields() {
        let mut fields = HashMap::new();
        fields.insert("name".to_string(), Value::str_from("Ada"));
        fields.insert("hp".to_string(), Value::Int(100));
        let v = Value::Struct {
            type_name: "Player".to_string(),
            fields:    Rc::new(RefCell::new(fields)),
            derives_partial_eq: false, derives_ord: false, derives_hash: false, derives_clone: false, field_order: Rc::new(Vec::new()),
        };
        let debug = v.debug_string();
        assert!(debug.contains("\"Ada\""), "expected quoted name in debug output, got: {debug}");
        assert!(!v.to_string().contains('"'), "display should not quote strings, got: {}", v.to_string());
    }

    #[test]
    fn test_debug_string_recurses_into_list_elements() {
        let items = vec![Value::str_from("a"), Value::str_from("b")];
        let v = Value::List(Rc::new(RefCell::new(items)));
        assert_eq!(v.debug_string(), "[\"a\", \"b\"]");
        assert_eq!(v.to_string(), "[a, b]");
    }

    #[test]
    fn test_debug_string_shows_strong_count_for_shared_display_does_not() {
        let rc = Rc::new(RefCell::new(Value::Int(42)));
        let a  = Value::Shared(Rc::clone(&rc));
        let b  = Value::Shared(Rc::clone(&rc));
        // rc itself + a's clone + b's clone = 3 live owners right now.
        assert_eq!(Rc::strong_count(&rc), 3);
        let debug = a.debug_string();
        assert!(debug.contains("refs=3"), "expected refs=3 in debug output, got: {debug}");
        assert!(!a.to_string().contains("refs"), "display should not show ref count, got: {}", a.to_string());
        drop(b);
        assert_eq!(Rc::strong_count(&rc), 2);
    }

    #[test]
    fn test_debug_string_equals_display_when_nothing_new_to_show() {
        // Regression guard for the design principle itself: debug_string
        // only ever diverges from Display for Str/Char/Shared/SyncShared.
        // Everything else must produce byte-identical output.
        let cases = vec![
            Value::Null,
            Value::Void,
            Value::Bool(true),
            Value::Int(42),
            Value::Float(3.14),
            Value::Double(2.718),
            Value::Function(0),
        ];
        for v in cases {
            assert_eq!(
                v.debug_string(), v.to_string(),
                "debug_string should equal Display for {}", v.type_name()
            );
        }
    }

    #[test]
    fn test_struct_equals_is_ptr_eq_by_default() {
        let mut fa = HashMap::new();
        fa.insert("x".to_string(), Value::Int(1));
        let mut fb = HashMap::new();
        fb.insert("x".to_string(), Value::Int(1));
        let a = Value::Struct {
            type_name: "Point".to_string(),
            fields:    Rc::new(RefCell::new(fa)),
            derives_partial_eq: false, derives_ord: false, derives_hash: false, derives_clone: false, field_order: Rc::new(Vec::new()),
        };
        let b = Value::Struct {
            type_name: "Point".to_string(),
            fields:    Rc::new(RefCell::new(fb)),
            derives_partial_eq: false, derives_ord: false, derives_hash: false, derives_clone: false, field_order: Rc::new(Vec::new()),
        };
        // Same shape, same values, different instances, no derive ->
        // today's tier-consistent default (Rc::ptr_eq) still applies.
        assert!(!a.equals(&b));
        let c = a.clone(); // clones the Rc -> same instance
        assert!(a.equals(&c));
    }

    #[test]
    fn test_struct_equals_is_structural_when_derived() {
        let mut fa = HashMap::new();
        fa.insert("x".to_string(), Value::Int(1));
        fa.insert("y".to_string(), Value::str_from("hi"));
        let mut fb = HashMap::new();
        fb.insert("x".to_string(), Value::Int(1));
        fb.insert("y".to_string(), Value::str_from("hi"));
        let a = Value::Struct {
            type_name: "Point".to_string(),
            fields:    Rc::new(RefCell::new(fa)),
            derives_partial_eq: true, derives_ord: false, derives_hash: false, derives_clone: false, field_order: Rc::new(Vec::new()),
        };
        let b = Value::Struct {
            type_name: "Point".to_string(),
            fields:    Rc::new(RefCell::new(fb)),
            derives_partial_eq: true, derives_ord: false, derives_hash: false, derives_clone: false, field_order: Rc::new(Vec::new()),
        };
        assert!(a.equals(&b), "derived PartialEq should compare structurally, not by pointer");

        let mut fc = HashMap::new();
        fc.insert("x".to_string(), Value::Int(2)); // different value
        fc.insert("y".to_string(), Value::str_from("hi"));
        let c = Value::Struct {
            type_name: "Point".to_string(),
            fields:    Rc::new(RefCell::new(fc)),
            derives_partial_eq: true, derives_ord: false, derives_hash: false, derives_clone: false, field_order: Rc::new(Vec::new()),
        };
        assert!(!a.equals(&c), "structural comparison must still catch a real field difference");
    }

    #[test]
    fn test_struct_equals_recurses_through_shared_fields_by_pointer() {
        // A derived struct containing a Shared<T> field: the OUTER
        // comparison is structural, but each field still uses its OWN
        // established equals() -- Shared compares by Rc::ptr_eq (already
        // shipped, unrelated to this delivery), so two structurally-
        // derived structs holding two DIFFERENT Shared instances with
        // the same inner value are still unequal at that field.
        let shared_rc = Rc::new(RefCell::new(Value::Int(5)));
        let mut fa = HashMap::new();
        fa.insert("inner".to_string(), Value::Shared(Rc::clone(&shared_rc)));
        let mut fb = HashMap::new();
        fb.insert("inner".to_string(), Value::Shared(Rc::clone(&shared_rc)));
        let a = Value::Struct {
            type_name: "Holder".to_string(), fields: Rc::new(RefCell::new(fa)), derives_partial_eq: true, derives_ord: false, derives_hash: false, derives_clone: false, field_order: Rc::new(Vec::new()),
        };
        let b = Value::Struct {
            type_name: "Holder".to_string(), fields: Rc::new(RefCell::new(fb)), derives_partial_eq: true, derives_ord: false, derives_hash: false, derives_clone: false, field_order: Rc::new(Vec::new()),
        };
        assert!(a.equals(&b), "same underlying Rc -> Shared fields should compare equal");

        let other_rc = Value::Shared(Rc::new(RefCell::new(Value::Int(5)))); // same value, different Rc
        let mut fc = HashMap::new();
        fc.insert("inner".to_string(), other_rc);
        let c = Value::Struct {
            type_name: "Holder".to_string(), fields: Rc::new(RefCell::new(fc)), derives_partial_eq: true, derives_ord: false, derives_hash: false, derives_clone: false, field_order: Rc::new(Vec::new()),
        };
        assert!(!a.equals(&c), "different Rc instance -> Shared fields should NOT compare equal, even with the same inner value");
    }

    #[test]
    fn test_debug_string_and_display_ignore_derives_partial_eq() {
        // derives_partial_eq only affects equals() -- it must not change
        // what a struct prints as, either way.
        let mut fields = HashMap::new();
        fields.insert("x".to_string(), Value::Int(1));
        let derived = Value::Struct {
            type_name: "Point".to_string(), fields: Rc::new(RefCell::new(fields.clone())), derives_partial_eq: true, derives_ord: false, derives_hash: false, derives_clone: false, field_order: Rc::new(Vec::new()),
        };
        let not_derived = Value::Struct {
            type_name: "Point".to_string(), fields: Rc::new(RefCell::new(fields)), derives_partial_eq: false, derives_ord: false, derives_hash: false, derives_clone: false, field_order: Rc::new(Vec::new()),
        };
        assert_eq!(derived.to_string(), not_derived.to_string());
        assert_eq!(derived.debug_string(), not_derived.debug_string());
    }

    // ── partial_cmp ──────────────────────────────────────────────

    #[test]
    fn test_partial_cmp_numeric_and_str() {
        use std::cmp::Ordering;
        assert_eq!(Value::Int(1).partial_cmp(&Value::Int(2)), Some(Ordering::Less));
        assert_eq!(Value::Double(2.5).partial_cmp(&Value::Double(2.5)), Some(Ordering::Equal));
        assert_eq!(
            Value::str_from("apple").partial_cmp(&Value::str_from("banana")),
            Some(Ordering::Less),
        );
        assert_eq!(Value::Bool(true).partial_cmp(&Value::Bool(false)), Some(Ordering::Greater));
    }

    #[test]
    fn test_partial_cmp_float_nan_is_none() {
        // Matches `equals()`: `NaN != NaN` there via plain `==`, so
        // `NaN` isn't comparable here either -- consistent, not a new
        // special case invented for `partial_cmp` alone.
        assert_eq!(Value::Double(f64::NAN).partial_cmp(&Value::Double(1.0)), None);
        assert_eq!(Value::Double(f64::NAN).partial_cmp(&Value::Double(f64::NAN)), None);
    }

    #[test]
    fn test_partial_cmp_struct_respects_declaration_order() {
        use std::cmp::Ordering;
        // Point { x, y } -- field_order says x decides first. Two
        // instances that disagree on BOTH fields must be ordered by x,
        // not y, proving declaration order (not, say, alphabetical --
        // which would coincidentally also be x-then-y here, so the
        // fields are deliberately named so alphabetical would give the
        // SAME answer only by accident; the real proof is
        // `field_order`'s content, exercised directly below).
        let order = Rc::new(vec!["x".to_string(), "y".to_string()]);
        let make = |x: i64, y: i64| {
            let mut f = HashMap::new();
            f.insert("x".to_string(), Value::Int(x));
            f.insert("y".to_string(), Value::Int(y));
            Value::Struct {
                type_name: "Point".to_string(),
                fields: Rc::new(RefCell::new(f)),
                derives_partial_eq: false, derives_ord: true, derives_hash: false, derives_clone: false,
                field_order: Rc::clone(&order),
            }
        };
        // a.x < b.x but a.y > b.y -- x must win.
        let a = make(1, 9);
        let b = make(2, 0);
        assert_eq!(a.partial_cmp(&b), Some(Ordering::Less));

        // Tied on x -- y breaks the tie.
        let c = make(5, 1);
        let d = make(5, 2);
        assert_eq!(c.partial_cmp(&d), Some(Ordering::Less));
    }

    #[test]
    fn test_partial_cmp_struct_not_derived_is_none() {
        let mut f = HashMap::new();
        f.insert("x".to_string(), Value::Int(1));
        let a = Value::Struct {
            type_name: "Point".to_string(), fields: Rc::new(RefCell::new(f.clone())),
            derives_partial_eq: false, derives_ord: false, derives_hash: false, derives_clone: false,
            field_order: Rc::new(vec!["x".to_string()]),
        };
        let b = Value::Struct {
            type_name: "Point".to_string(), fields: Rc::new(RefCell::new(f)),
            derives_partial_eq: false, derives_ord: false, derives_hash: false, derives_clone: false,
            field_order: Rc::new(vec!["x".to_string()]),
        };
        assert_eq!(a.partial_cmp(&b), None, "no @derive(Ord)/@derive(PartialOrd) -> not comparable");
    }

    #[test]
    fn test_partial_cmp_unique_shared_syncshared_delegate_to_inner() {
        use std::cmp::Ordering;
        // Confirmed design decision: unlike `equals()` (ptr-eq for
        // Shared/SyncShared), ordering delegates to the INNER value for
        // all three wrappers -- proven here with deliberately DIFFERENT
        // Rc instances holding 1 and 2, so a ptr-address-based ordering
        // would give a meaningless, non-deterministic answer instead of
        // this one.
        let unique_a = Value::Unique(Box::new(Value::Int(1)));
        let unique_b = Value::Unique(Box::new(Value::Int(2)));
        assert_eq!(unique_a.partial_cmp(&unique_b), Some(Ordering::Less));

        let shared_a = Value::Shared(Rc::new(RefCell::new(Value::Int(1))));
        let shared_b = Value::Shared(Rc::new(RefCell::new(Value::Int(2))));
        assert_eq!(shared_a.partial_cmp(&shared_b), Some(Ordering::Less));

        let sync_a = Value::SyncShared(Rc::new(RefCell::new(Value::Int(1))));
        let sync_b = Value::SyncShared(Rc::new(RefCell::new(Value::Int(2))));
        assert_eq!(sync_a.partial_cmp(&sync_b), Some(Ordering::Less));
    }

    // ── compute_hash ─────────────────────────────────────────────

    #[test]
    fn test_compute_hash_consistent_with_equals_for_derived_struct() {
        // The actual contract: a.equals(b) implies matching hashes.
        // Two SEPARATELY-constructed (different Rc, different HashMap
        // instances -- so this can't pass by coincidentally sharing one
        // map's own iteration order) but content-equal derived-Hash
        // structs must hash the same.
        let order = Rc::new(vec!["name".to_string()]);
        let make = || {
            let mut f = HashMap::new();
            f.insert("name".to_string(), Value::str_from("Ada"));
            Value::Struct {
                type_name: "Tag".to_string(), fields: Rc::new(RefCell::new(f)),
                derives_partial_eq: true, derives_ord: false, derives_hash: true, derives_clone: false,
                field_order: Rc::clone(&order),
            }
        };
        let (a, b) = (make(), make());
        assert!(a.equals(&b), "sanity: these should already be equal");
        assert_eq!(a.compute_hash(), b.compute_hash());
    }

    #[test]
    fn test_compute_hash_negative_zero_matches_positive_zero() {
        // `equals()` uses plain `==`, under which -0.0 == 0.0 -- hashing
        // must agree, or the Hash contract breaks for this pair alone.
        assert!(Value::Double(-0.0).equals(&Value::Double(0.0)));
        assert_eq!(Value::Double(-0.0).compute_hash(), Value::Double(0.0).compute_hash());
        assert!(Value::Float(-0.0).equals(&Value::Float(0.0)));
        assert_eq!(Value::Float(-0.0).compute_hash(), Value::Float(0.0).compute_hash());
    }

    #[test]
    fn test_compute_hash_non_derived_struct_is_identity_based() {
        // Matches equals()'s own fallback: two separate instances with
        // identical content but NO @derive(Hash) are not equal (ptr-eq)
        // and must not be required to hash equal either -- same
        // `Rc::as_ptr` fallback `equals()` already implies.
        let mut f = HashMap::new();
        f.insert("name".to_string(), Value::str_from("Ada"));
        let a = Value::Struct {
            type_name: "Tag".to_string(), fields: Rc::new(RefCell::new(f.clone())),
            derives_partial_eq: false, derives_ord: false, derives_hash: false, derives_clone: false,
            field_order: Rc::new(Vec::new()),
        };
        let b = Value::Struct {
            type_name: "Tag".to_string(), fields: Rc::new(RefCell::new(f)),
            derives_partial_eq: false, derives_ord: false, derives_hash: false, derives_clone: false,
            field_order: Rc::new(Vec::new()),
        };
        assert!(!a.equals(&b), "sanity: not derived -> ptr-eq -> not equal");
        assert_ne!(a.compute_hash(), b.compute_hash());
    }

    // ── deep_clone ───────────────────────────────────────────────

    #[test]
    fn test_deep_clone_struct_list_field_is_independent() {
        let mut f = HashMap::new();
        f.insert("items".to_string(), Value::List(Rc::new(RefCell::new(vec![Value::Int(1)]))));
        let original = Value::Struct {
            type_name: "Bag".to_string(), fields: Rc::new(RefCell::new(f)),
            derives_partial_eq: false, derives_ord: false, derives_hash: false, derives_clone: true,
            field_order: Rc::new(vec!["items".to_string()]),
        };
        let cloned = original.deep_clone();

        // Mutate the CLONE's list; the original's must be untouched.
        if let Value::Struct { fields, .. } = &cloned {
            if let Some(Value::List(items)) = fields.borrow().get("items") {
                items.borrow_mut().push(Value::Int(2));
            }
        }
        let original_len = match &original {
            Value::Struct { fields, .. } => match fields.borrow().get("items") {
                Some(Value::List(items)) => items.borrow().len(),
                _ => panic!("expected a List field"),
            },
            _ => panic!("expected a Struct"),
        };
        assert_eq!(original_len, 1, "deep_clone: mutating the clone's List must not affect the original's");
    }

    #[test]
    fn test_deep_clone_shared_field_aliases_not_copies() {
        // The confirmed, deliberate divergence from every OTHER field:
        // deep_clone recurses through a List (see above) but a Shared
        // field must come out the SAME Rc, not a fresh one holding an
        // equal-looking value.
        let shared = Rc::new(RefCell::new(Value::Int(0)));
        let mut f = HashMap::new();
        f.insert("counter".to_string(), Value::Shared(Rc::clone(&shared)));
        let original = Value::Struct {
            type_name: "Session".to_string(), fields: Rc::new(RefCell::new(f)),
            derives_partial_eq: false, derives_ord: false, derives_hash: false, derives_clone: true,
            field_order: Rc::new(vec!["counter".to_string()]),
        };
        let cloned = original.deep_clone();

        let cloned_rc = match &cloned {
            Value::Struct { fields, .. } => match fields.borrow().get("counter") {
                Some(Value::Shared(rc)) => Rc::clone(rc),
                _ => panic!("expected a Shared field"),
            },
            _ => panic!("expected a Struct"),
        };
        assert!(Rc::ptr_eq(&shared, &cloned_rc), "deep_clone: Shared field must alias, not copy");
    }
}
