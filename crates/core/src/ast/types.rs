// src/ast/types.rs
//! Type expression nodes.

#![allow(dead_code)]

use crate::ast::common::Span;

/// A type expression with its source location.
#[derive(Debug, Clone, Copy)]
pub struct Type<'ast> {
    pub kind: TypeKind<'ast>,
    pub span: Span,
}

#[derive(Debug, Clone, Copy)]
pub enum TypeKind<'ast> {
    // ── Primitive types ──────────────────────────────────────────
    Int, Uint, Long, Ulong, Short, Ushort,
    Byte, Ubyte, Float, Double,
    Bool, Char, Str, Void,
    I8, I16, I32, I64,
    U8, U16, U32, U64,
    F32, F64, Isize, Usize,

    // ── Built-in generic collections ────────────────────────────
    /// `List` or `List<T>`
    List(Option<&'ast Type<'ast>>),
    /// `Dictionary<K, V>`
    Dictionary(Option<(&'ast Type<'ast>, &'ast Type<'ast>)>),
    /// `Set<T>`
    Set(Option<&'ast Type<'ast>>),
    /// `Queue<T>`
    Queue(Option<&'ast Type<'ast>>),
    /// `Stack<T>`
    Stack(Option<&'ast Type<'ast>>),
    /// `InlineList<T>` — fixed-capacity, stack/inline-storage collection
    /// (DATASTRUCTURES.md — genuinely separate from `List<T>`/`Pool<T>`,
    /// not another face of either). Capacity is deliberately *not* part
    /// of the type — same reasoning as `List<T>`'s current length not
    /// being part of *its* type — it's a checked, literal-int-only
    /// argument to `InlineList.new(capacity)` instead (checked directly
    /// against the AST node in sema, not inferred, not a general
    /// expression — see `type_infer.rs`). Sidesteps needing any const-
    /// generics grammar; confirmed the language has none anywhere
    /// (`parse_generic_params` only ever parses `Ident (: Bound)?`).
    InlineList(Option<&'ast Type<'ast>>),

    // ── User-defined / imported types ────────────────────────────
    /// A named type, possibly with generic arguments: `Foo<T, U>`
    Named {
        /// Qualified path segments, e.g. `["std", "io", "File"]`
        path: &'ast [&'ast str],
        /// Type arguments (empty = non-generic use)
        args: &'ast [&'ast Type<'ast>],
    },

    // ── Composite types ──────────────────────────────────────────
    /// `(int, string, bool)` — two or more elements
    Tuple(&'ast [&'ast Type<'ast>]),

    /// `[4]int` — fixed-size array
    Array {
        len: u64,
        elem: &'ast Type<'ast>,
    },

    /// `[]int` — dynamically-sized slice
    Slice(&'ast Type<'ast>),

    // ── Modifier types ───────────────────────────────────────────
    /// `T!` — the operation may fail and must be handled
    Fallible(&'ast Type<'ast>),

    /// `Task<T>` — an asynchronous computation
    Task(Option<&'ast Type<'ast>>),

    /// `&T`, `&mut T`, or `&L T` (with explicit lifetime)
    Reference {
        mutable: bool,
        lifetime: Option<&'ast str>,
        inner: &'ast Type<'ast>,
    },

    /// `T?` — an optional / nullable value
    Optional(&'ast Type<'ast>),

    // ── Function type ─────────────────────────────────────────────
    /// `fn(A, B) C` or `fn(A, B) C!`
    Function(FunctionType<'ast>),

    // ── Wildcard ─────────────────────────────────────────────────
    /// `_` — ask the compiler to infer the type
    Infer,
}

/// The signature shape of a function-typed value.
#[derive(Debug, Clone, Copy)]
pub struct FunctionType<'ast> {
    pub params: &'ast [&'ast Type<'ast>],
    pub return_type: Option<&'ast Type<'ast>>,
    /// Whether the return type carries `!` (may-fail).
    pub is_fallible: bool,
}

/// The run-time key under which the methods of an `extend` or `impl` block
/// written for a built-in type are registered, and under which a call finds
/// them again. It is the name of the interpreter's value kind
/// (`Value::impl_key`), so types that are one kind of value at run time share
/// a key: `int`, `long`, `i64` and `isize` are all `"int"`, `uint` and `u32`
/// are `"u32"`, `ulong`, `u64` and `usize` are `"u64"`, `float` and `f32` are
/// `"float"`, `double` and `f64` are `"double"`. A tuple is keyed by its
/// arity (`"tuple2"`) and an array by `"List"`, the kind of value it shares
/// with `List<T>`. `None` for every other type.
pub fn builtin_impl_key(kind: &TypeKind<'_>) -> Option<String> {
    let key: &str = match kind {
        TypeKind::Int | TypeKind::Long | TypeKind::I64 | TypeKind::Isize | TypeKind::Short => "int",
        TypeKind::I8 | TypeKind::Byte => "i8",
        TypeKind::I16 => "i16",
        TypeKind::I32 => "i32",
        TypeKind::U8 | TypeKind::Ubyte => "u8",
        TypeKind::U16 => "u16",
        TypeKind::Uint | TypeKind::U32 | TypeKind::Ushort => "u32",
        TypeKind::Ulong | TypeKind::U64 | TypeKind::Usize => "u64",
        TypeKind::Float | TypeKind::F32 => "float",
        TypeKind::Double | TypeKind::F64 => "double",
        TypeKind::Bool => "bool",
        TypeKind::Char => "char",
        TypeKind::Str => "string",
        TypeKind::Tuple(elems) => return Some(format!("tuple{}", elems.len())),
        TypeKind::Array { .. } => "List",
        _ => return None,
    };
    Some(key.to_string())
}
