//! The Stage 1B emitter: a small subset of Ubel to textual LLVM IR.
//!
//! # What it handles
//!
//! * top-level functions with `int`, `bool` and `void` parameters and results, `@tier`
//!   annotations (recorded as comments, and cross-tier calls are marked);
//! * `let`, `x := e`, assignment and compound assignment, shadowing and nested scopes;
//! * arithmetic, comparison, bitwise and short-circuit logical operators;
//! * `if` / `elif` / `else` as statements and as expressions, `while`, `loop`,
//!   `for i in a..b` and `a..=b`, `break`, `continue`, `return`, implicit return;
//! * calls between user functions;
//! * `println` / `print` of ints, bools, string literals and interpolated strings;
//! * `with arena(size) { ... }`, with the arena destroyed on EVERY way out of the block:
//!   falling off the end, `return`, `break` and `continue`;
//! * plain and `edge` structs (fields of `int`, `bool`, another struct, or a `&L Struct`
//!   reference): literals, field reads,
//!   field assignment (also compound and nested), and passing/returning them. A struct
//!   value is a pointer, like the interpreter's reference semantics: `let b = a` aliases.
//!   A literal allocates from the GC heap in a HIGH function and from the innermost open
//!   arena inside `with arena`; anywhere else (MID outside an arena, LOW) it is refused.
//!   A reference to a struct (`&x`, a `&L T` field, parameter or result, with or without a
//!   `[lifetime L]` on the function or struct) is the same pointer as the struct value: the
//!   lifetime is checked by sema and erased here.
//!
//! Everything else is refused with a located "not supported yet" error instead of being
//! guessed at: enums, strings as values, collections, closures, generics, struct methods,
//! `&mut`, explicit `*` dereference, references to anything but a struct, `match`, sized
//! integers, floats.
//!
//! # Lowering model
//!
//! Every local lives in an `alloca` slot and the emitter never builds phi nodes; LLVM's
//! `mem2reg` turns the slots into SSA at -O1 and above. All tiers share one calling
//! convention and one data layout, so a cross-tier call is a plain call. The only tier
//! specific lowering so far is the arena block.
//!
//! # Known differences from the interpreter
//!
//! `int` is 64-bit and arithmetic wraps on overflow natively. Division by zero is trapped
//! (exit 1, message on stderr). Shifts by 64 or more are undefined. The test programs avoid
//! overflow and over-wide shifts; the differences are documented in `docs/LLVM_POC.md`.

use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap};
use std::fmt;
use std::fmt::Write as _;

use ubel_stratum::ast::{
    AllocatorKind, Arg, ArgKind, AssignOp, BinOp, BindingTarget, Block, Expr, ExprKind,
    EnumDecl, EnumVariantPayload, FieldInit, FunctionDecl, IfBranchBody, IfExpr, InterpolationPart, Item, Literal, ParamKind, Program,
    SizeExpr, Span, Stmt, StmtKind, StructDecl, StructMember, TierAnnotation, TypeKind, UnaryOp,
};
use ubel_stratum::ast::expressions::{Lambda, LambdaBody, MatchArm, MatchArmBody, OrElseFallback};
use ubel_stratum::ast::patterns::{EnumPatternPayload, PatternKind};
use ubel_stratum::lexer::IntSuffix;
use ubel_stratum::sema::{SemaContext, SemaType, TypeId};

// ── errors ───────────────────────────────────────────────────────

/// A failure in any stage. `messages` are already formatted as `line:col: text`.
#[derive(Debug, Clone)]
pub struct CompileError {
    pub stage: &'static str,
    pub messages: Vec<String>,
}

impl CompileError {
    pub fn stage(stage: &'static str, messages: Vec<String>) -> Self {
        CompileError { stage, messages }
    }

    fn at(span: Span, text: impl Into<String>) -> Self {
        CompileError {
            stage: "emit",
            messages: vec![format!("{}:{}: {}", span.line, span.column, text.into())],
        }
    }

    /// Multi-line text for the terminal.
    pub fn render(&self, path: &str) -> String {
        let mut s = format!("{path}: {} error", self.stage);
        for m in &self.messages {
            let _ = write!(s, "\n  {m}");
        }
        s
    }
}

impl fmt::Display for CompileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.render("<source>"))
    }
}

impl std::error::Error for CompileError {}

type R<T> = Result<T, CompileError>;

fn unsupported<T>(span: Span, what: impl AsRef<str>) -> R<T> {
    Err(CompileError::at(span, format!("not supported yet: {}", what.as_ref())))
}

/// The type of an integer literal that carries a width (`250u8`, or a literal sema gave one).
fn int_suffix_ty(suffix: IntSuffix) -> Ty {
    match suffix {
        IntSuffix::I8 => Ty::Int(IntTy::I8),
        IntSuffix::I16 => Ty::Int(IntTy::I16),
        IntSuffix::I32 => Ty::Int(IntTy::I32),
        IntSuffix::I64 | IntSuffix::Isize => Ty::I64,
        IntSuffix::U8 => Ty::Int(IntTy::U8),
        IntSuffix::U16 => Ty::Int(IntTy::U16),
        IntSuffix::U32 => Ty::Int(IntTy::U32),
        IntSuffix::U64 | IntSuffix::Usize => Ty::Int(IntTy::U64),
    }
}

/// An integer constant of type `ty`, written the way LLVM reads it: the low bits of `raw` as a signed
/// number of that width, so `200u8` is written `-56` (it is the same bit pattern).
fn int_const(ty: Ty, raw: u64) -> String {
    let (bits, _) = ty.int_info().expect("an integer type");
    if bits == 64 {
        return (raw as i64).to_string();
    }
    let shift = 64 - bits;
    (((raw << shift) as i64) >> shift).to_string()
}

// ── small types ──────────────────────────────────────────────────

/// A fixed-width integer other than the default `int` (which is `Ty::I64`, signed 64-bit): `i8 i16 i32`,
/// `u8 u16 u32 u64`. Every one is a plain LLVM integer; signedness lives in the operation chosen
/// (`sdiv` or `udiv`, `ashr` or `lshr`, `slt` or `ult`, `sext` or `zext`), not in the type.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum IntTy {
    I8,
    I16,
    I32,
    U8,
    U16,
    U32,
    U64,
}

impl IntTy {
    fn bits(self) -> u32 {
        match self {
            IntTy::I8 | IntTy::U8 => 8,
            IntTy::I16 | IntTy::U16 => 16,
            IntTy::I32 | IntTy::U32 => 32,
            IntTy::U64 => 64,
        }
    }

    fn signed(self) -> bool {
        matches!(self, IntTy::I8 | IntTy::I16 | IntTy::I32)
    }

    fn ir(self) -> &'static str {
        match self {
            IntTy::I8 | IntTy::U8 => "i8",
            IntTy::I16 | IntTy::U16 => "i16",
            IntTy::I32 | IntTy::U32 => "i32",
            IntTy::U64 => "i64",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Ty {
    I64,
    I1,
    Void,
    /// A struct value: a pointer to an object, `usize` indexes `Emitter::structs`.
    Struct(usize),
    /// An enum value: a pointer to an object (tag word, then payload slots), `usize` indexes
    /// `Emitter::enums`. Every enum value is an object, payload-free variants included.
    Enum(usize),
    /// A string value: a pointer to `{ i64 length, bytes }` (UTF-8, no terminator, immutable). A literal
    /// is a static constant outside the heap; a string built at run time lives where it was written (D3b).
    Str,
    /// A closure value (A7): a pointer to a closure object whose first word is the code pointer, `usize`
    /// indexes `Emitter::fn_tys`. A closure that captures nothing is a static constant outside the heap.
    Fn(usize),
    /// A fixed-width integer other than `int` (see `IntTy`).
    Int(IntTy),
    /// `double` / `f64`: an IEEE 754 binary64, LLVM `double` (M2b).
    F64,
    /// `float` / `f32`: an IEEE 754 binary32, LLVM `float` (M2b).
    F32,
    /// `List<T>` (M3a): a pointer to a GC list object. The payload indexes `Emitter::list_tys`, which holds the
    /// element type (lists are specialised per element type, decided 2026-11-10).
    List(usize),
    /// `T?` (M3b): a pointer, null meaning "no value". For a `T` that is itself a pointer (string, struct, enum, list)
    /// it is that pointer; for a number or `bool` it points to a GC box holding the value. The payload indexes
    /// `Emitter::opt_tys`, which holds the `T`.
    Opt(usize),
    /// The literal `null` before its context gives it an optional type: only `==` / `!=` against an optional,
    /// printing, and a place of optional type (`expr_as`) accept it.
    Null,
}

/// The shape of a function type: parameter types and result.
#[derive(Clone, PartialEq, Eq)]
struct FnTy {
    params: Vec<Ty>,
    ret: Ty,
}

impl Ty {
    fn ir(self) -> &'static str {
        match self {
            Ty::I64 => "i64",
            Ty::I1 => "i1",
            Ty::Void => "void",
            Ty::Struct(_) | Ty::Enum(_) | Ty::Str | Ty::Fn(_) | Ty::List(_) | Ty::Opt(_) | Ty::Null => "ptr",
            Ty::Int(k) => k.ir(),
            Ty::F64 => "double",
            Ty::F32 => "float",
        }
    }

    /// Width in bits and signedness, for every integer type (`int` included); `None` for the rest.
    fn int_info(self) -> Option<(u32, bool)> {
        match self {
            Ty::I64 => Some((64, true)),
            Ty::Int(k) => Some((k.bits(), k.signed())),
            _ => None,
        }
    }

    fn is_float(self) -> bool {
        matches!(self, Ty::F64 | Ty::F32)
    }

    /// Values that are pointers to objects, and so need a collector mask bit.
    fn is_ptr(self) -> bool {
        matches!(self, Ty::Struct(_) | Ty::Enum(_) | Ty::Str | Ty::Fn(_) | Ty::List(_) | Ty::Opt(_))
    }
}

/// An LLVM floating-point constant for `v`: the hexadecimal form of the double's bits, which LLVM accepts for a
/// `double`, and for a `float` when the double is exactly a float (every f32 widens exactly). It also spells
/// infinity, which a literal like `1e999` is.
fn float_const(v: f64) -> String {
    format!("0x{:016X}", v.to_bits())
}

#[derive(Clone)]
struct VariantInfo {
    name: String,
    fields: Vec<Ty>,
}

#[derive(Clone)]
struct EnumInfo {
    name: String,
    variants: Vec<VariantInfo>,
    /// Payload slots every value of this enum carries: the arity of its widest variant.
    slots: usize,
}

/// Payload fields per variant. One slot is 8 bytes, a GC object is at most 64 pointer-words
/// (256 bytes on a 32-bit target) and the tag takes one slot, so 31 always fits.
const MAX_VARIANT_FIELDS: usize = 31;

#[derive(Clone)]
struct FieldInfo {
    name: String,
    ty: Ty,
}

#[derive(Clone)]
struct StructInfo {
    name: String,
    fields: Vec<FieldInfo>,
}

/// Fields per struct. A GC object is at most 64 pointer-words, which is 256 bytes on a
/// 32-bit target, and a field is at most 8 bytes, so 32 fields always fit.
const MAX_STRUCT_FIELDS: usize = 32;

#[derive(Clone, Debug)]
struct Val {
    v: String,
    ty: Ty,
}

impl Val {
    fn void() -> Val {
        Val { v: String::new(), ty: Ty::Void }
    }
}

#[derive(Clone)]
struct Sig {
    params: Vec<Ty>,
    ret: Ty,
    tier: TierAnnotation,
}

#[derive(Clone)]
struct Local {
    slot: String,
    ty: Ty,
    /// Declared `mut`, a `for` variable or a `:=` binding: it can change after a closure copied it, so a
    /// closure may not capture it.
    mutable: bool,
    /// Bound by an immutable `let` straight to a lambda (`let f = fn(...) ...`): a closure created in this
    /// function, so its tier is this function's (decision D2). The only closure a LOW function may call.
    lambda: bool,
}

/// A value a closure copies from the frame it is written in when it is created.
struct Capture {
    ty: Ty,
    /// The stack slot of the variable in the enclosing function.
    outer_slot: String,
}

#[derive(Clone)]
struct LoopCtx {
    cont: String,
    brk: String,
    /// How many arenas were open when the loop body began. `break` and `continue`
    /// destroy the arenas opened since then, and only those.
    arena_depth: usize,
}

fn tier_name(t: TierAnnotation) -> &'static str {
    match t {
        TierAnnotation::High => "HIGH",
        TierAnnotation::Mid => "MID",
        TierAnnotation::Low => "LOW",
    }
}

fn expr_kind_name(k: &ExprKind) -> &'static str {
    match k {
        ExprKind::Lit(_) => "this literal",
        ExprKind::Ident(_) => "this name",
        ExprKind::SelfExpr => "`self`",
        ExprKind::BinOp { .. } => "this operator",
        ExprKind::UnaryOp { .. } => "this unary operator",
        ExprKind::Assign { .. } => "this assignment",
        ExprKind::Pipe { .. } => "the pipe operator",
        ExprKind::Call { .. } => "this call",
        ExprKind::Field { .. } => "field access",
        ExprKind::Index { .. } => "indexing",
        ExprKind::OptionalChain { .. } => "optional chaining",
        ExprKind::Try(_) => "`?`",
        ExprKind::Await(_) => "`await`",
        ExprKind::Borrow { .. } => "borrows",
        ExprKind::Deref(_) => "dereference",
        ExprKind::Tuple(_) => "tuples",
        ExprKind::Array(_) => "array literals",
        ExprKind::Dict(_) => "dictionary literals",
        ExprKind::AnonObject(_) => "anonymous objects",
        ExprKind::StructLit { .. } => "struct literals",
        ExprKind::Lambda(_) => "closures",
        ExprKind::Block(_) => "block expressions",
        ExprKind::If(_) => "`if` expressions",
        ExprKind::Match(_) => "`match`",
        ExprKind::OrElse { .. } => "`orelse`",
        ExprKind::As { .. } => "`as` casts",
        ExprKind::ShortDecl { .. } => "`:=` declarations",
    }
}

fn stmt_kind_name(k: &StmtKind) -> &'static str {
    match k {
        StmtKind::Let { .. } => "`let`",
        StmtKind::Expr(_) => "expression statements",
        StmtKind::Return(_) => "`return`",
        StmtKind::Fail(_) => "`fail`",
        StmtKind::If(_) => "`if`",
        StmtKind::Match { .. } => "`match`",
        StmtKind::For { .. } => "`for`",
        StmtKind::While { .. } => "`while`",
        StmtKind::Loop(_) => "`loop`",
        StmtKind::Break(_) => "`break`",
        StmtKind::Continue => "`continue`",
        StmtKind::With { .. } => "`with`",
        StmtKind::Using { .. } => "`using`",
        StmtKind::Extract { .. } => "`extract`",
        StmtKind::Defer(_) => "`defer`",
        StmtKind::Try { .. } => "`try`",
        StmtKind::Unsafe(_) => "`unsafe`",
    }
}

// ── the emitter ──────────────────────────────────────────────────

struct Emitter<'a> {
    ctx: &'a SemaContext,
    sigs: HashMap<String, Sig>,
    structs: Vec<StructInfo>,
    struct_ids: HashMap<String, usize>,
    enums: Vec<EnumInfo>,
    enum_ids: HashMap<String, usize>,
    esized: BTreeSet<usize>,
    emasked: BTreeSet<(usize, usize)>,
    /// Payload-free variants that have a static singleton object (E1): (enum, variant).
    estatics: BTreeSet<(usize, usize)>,
    /// The IR of those singletons: emitted after the enum types, which a struct initializer needs defined.
    estatic_ir: String,
    /// Which structs are allocated somewhere, so only their helper functions are emitted.
    sized: BTreeSet<usize>,
    masked: BTreeSet<usize>,
    uses_gc: bool,
    uses_bump: bool,
    /// Any string operation was emitted, so the runtime's string helpers are declared.
    uses_str: bool,
    /// Some `with arena` block was made scannable, so the runtime function that creates one is declared.
    uses_scan: bool,
    /// LLVM intrinsics used (whole `declare` lines), declared once after the runtime's functions.
    decls: BTreeSet<String>,
    /// Arenas (by SSA name) that hold, or may hold, a pointer into the GC heap: a `with arena` block whose
    /// objects have pointer fields. They are created scannable, so the collector treats what was written
    /// in them as roots instead of freeing a GC object whose only reference is an arena object's field.
    arena_scan: BTreeSet<String>,
    /// Function types seen so far (interned, so equal shapes have equal `Ty::Fn` indexes).
    fn_tys: RefCell<Vec<FnTy>>,
    /// The element type of each list type (`Ty::List(i)` is a list of `list_tys[i]`).
    list_tys: RefCell<Vec<Ty>>,
    /// The `T` of each optional type (`Ty::Opt(i)` is a `T?` with `T` = `opt_tys[i]`).
    opt_tys: RefCell<Vec<Ty>>,
    /// Set when a list is used: the list type and the runtime's list functions are declared.
    uses_list: bool,
    /// Closures emitted so far, for unique names.
    lam_n: usize,
    /// While a closure body is emitted and sema left its result type open (an integer literal not yet
    /// defaulted, say): the result type is taken from the first value the body returns.
    ret_infer: bool,
    ret_seen: Option<Ty>,
    /// While a closure body is emitted: every name visible in the enclosing code, so a use of one of
    /// them is reported as a capture instead of as an unknown name.
    outer_names: Vec<BTreeSet<String>>,
    /// While a closure body is emitted: the scopes of the frame it is written in (one per closure being
    /// emitted, innermost last), where its captures are looked up.
    enclosing: Vec<Vec<HashMap<String, Local>>>,
    /// The captures found so far in each closure being emitted.
    captures: Vec<Vec<Capture>>,
    /// Instructions that run first in the closure being emitted (they copy its captures out of the closure
    /// object); they come after the allocas and before the body.
    prologue: String,
    /// For each closure being emitted: was it written, directly or through other closures, inside `with arena`?
    /// Its body is a separate function that does not hold that arena, so nothing in it can be allocated there.
    in_arena_closure: Vec<bool>,

    funcs: String,
    strings: String,
    str_n: usize,

    // per-function state
    body: String,
    allocas: String,
    tmp: usize,
    lbl: usize,
    terminated: bool,
    scopes: Vec<HashMap<String, Local>>,
    loops: Vec<LoopCtx>,
    arenas: Vec<String>,
    /// For each arena of the current function that holds a string sema tracked: the sema `ArenaId` its `&arena`
    /// strings carry, and the arena pointer. Filled where an interpolation is allocated in the arena (sema tags
    /// it with the innermost arena there), dropped when the block ends. A string method that builds a string
    /// from an arena receiver allocates in the receiver's own arena, found here; no entry means the receiver's
    /// arena is not known in this function (a borrowed `&arena` parameter, say) and the result is a GC copy.
    arena_ids: Vec<(usize, String)>,
    ret: Ty,
    cur_tier: TierAnnotation,
}

pub(crate) fn emit_program(program: &Program, ctx: &SemaContext) -> R<String> {
    let mut e = Emitter::new(ctx);
    e.collect_structs(program)?;

    // The front end injects the prelude (Ordering, Hasher and the core traits) as ordinary items with
    // synthetic spans. Programs that do not use it should not be refused because of it.
    for item in program.items {
        if item.span().is_prelude() {
            continue;
        }
        match item {
            Item::Function(f) => {
                let sig = e.signature(f)?;
                e.sigs.insert(f.name.to_string(), sig);
            }
            Item::Struct(_) | Item::Enum(_) => {} // registered by collect_structs
            other => {
                return unsupported(other.span(), format!("{} declarations", other.kind_name()));
            }
        }
    }
    if !e.sigs.contains_key("main") {
        return Err(CompileError::stage("emit", vec!["the program has no `fn main`".to_string()]));
    }
    for item in program.items {
        if item.span().is_prelude() {
            continue;
        }
        if let Item::Function(f) = item {
            e.function(f)?;
        }
    }
    Ok(e.finish())
}

impl<'a> Emitter<'a> {
    fn new(ctx: &'a SemaContext) -> Self {
        Emitter {
            ctx,
            sigs: HashMap::new(),
            structs: Vec::new(),
            struct_ids: HashMap::new(),
            enums: Vec::new(),
            enum_ids: HashMap::new(),
            esized: BTreeSet::new(),
            emasked: BTreeSet::new(),
            estatics: BTreeSet::new(),
            estatic_ir: String::new(),
            sized: BTreeSet::new(),
            masked: BTreeSet::new(),
            uses_gc: false,
            uses_bump: false,
            uses_str: false,
            uses_scan: false,
            decls: BTreeSet::new(),
            arena_scan: BTreeSet::new(),
            fn_tys: RefCell::new(Vec::new()),
            list_tys: RefCell::new(Vec::new()),
            opt_tys: RefCell::new(Vec::new()),
            uses_list: false,
            lam_n: 0,
            ret_infer: false,
            ret_seen: None,
            outer_names: Vec::new(),
            enclosing: Vec::new(),
            captures: Vec::new(),
            prologue: String::new(),
            in_arena_closure: Vec::new(),
            funcs: String::new(),
            strings: String::new(),
            str_n: 0,
            body: String::new(),
            allocas: String::new(),
            tmp: 0,
            lbl: 0,
            terminated: false,
            scopes: Vec::new(),
            loops: Vec::new(),
            arenas: Vec::new(),
            arena_ids: Vec::new(),
            ret: Ty::Void,
            cur_tier: TierAnnotation::High,
        }
    }

    fn finish(self) -> String {
        let mut out = String::new();
        out.push_str("; Generated by ubel-llvm (Stage 1B of the LLVM proof of concept). Do not edit.\n");
        out.push_str("; Function tiers are recorded as comments; every tier shares one calling\n");
        out.push_str("; convention and data layout, so cross-tier calls are plain calls.\n\n");
        out.push_str("declare ptr  @ubel_arena_create(i64)\n");
        if self.uses_scan {
            out.push_str("declare ptr  @ubel_arena_create_scan(i64)\n");
        }
        out.push_str("declare void @ubel_arena_destroy(ptr)\n");
        out.push_str("declare void @ubel_print_i64(i64)\n");
        out.push_str("declare void @ubel_print_u64(i64)\n");
        out.push_str("declare void @ubel_print_f64(double)\n");
        out.push_str("declare void @ubel_print_f32(float)\n");
        out.push_str("declare void @ubel_panic(ptr, i64)\n");
        out.push_str("declare void @ubel_print_bool(i32)\n");
        out.push_str("declare void @ubel_print_str(ptr, i64)\n");
        out.push_str("declare void @ubel_print_nl()\n");
        out.push_str("declare void @ubel_trap_div_zero()\n");
        out.push_str("declare void @ubel_trap_no_return()\n\n");
        if self.uses_gc {
            out.push_str("declare ptr  @ubel_gc_alloc(i64, i64)\n");
        }
        if self.uses_bump {
            out.push_str("declare ptr  @ubel_arena_grow(ptr, i64)\n");
        }
        if self.uses_str {
            out.push_str("declare i32  @ubel_str_eq(ptr, ptr)\n");
            out.push_str("declare i32  @ubel_str_cmp(ptr, ptr)\n");
            out.push_str("declare i32  @ubel_str_contains(ptr, ptr)\n");
            out.push_str("declare i32  @ubel_str_starts_with(ptr, ptr)\n");
            out.push_str("declare i32  @ubel_str_ends_with(ptr, ptr)\n");
            out.push_str("declare ptr  @ubel_str_to_upper(ptr, ptr)\n");
            out.push_str("declare ptr  @ubel_str_to_lower(ptr, ptr)\n");
            out.push_str("declare ptr  @ubel_str_trim(ptr, ptr)\n");
            out.push_str("declare ptr  @ubel_str_trim_start(ptr, ptr)\n");
            out.push_str("declare ptr  @ubel_str_trim_end(ptr, ptr)\n");
            out.push_str("declare ptr  @ubel_str_replace(ptr, ptr, ptr, ptr)\n");
            out.push_str("declare i64  @ubel_i64_len(i64)\n");
            out.push_str("declare void @ubel_i64_write(ptr, i64)\n");
            out.push_str("declare i64  @ubel_u64_len(i64)\n");
            out.push_str("declare void @ubel_u64_write(ptr, i64)\n");
            out.push_str("declare i64  @ubel_f64_len(double)\n");
            out.push_str("declare void @ubel_f64_write(ptr, double)\n");
            out.push_str("declare i64  @ubel_f32_len(float)\n");
            out.push_str("declare void @ubel_f32_write(ptr, float)\n");
            out.push_str("declare void @ubel_put(ptr, ptr, i64)\n");
        }
        if self.uses_gc || self.uses_bump || self.uses_str {
            out.push('\n');
        }
        if self.uses_list {
            out.push_str("%ubel.list = type { i64, i64, ptr }\n");
            out.push_str("declare ptr  @ubel_list_new()\n");
            out.push_str("declare void @ubel_list_grow(ptr, i64, i64)\n");
            out.push_str("declare ptr  @ubel_list_snapshot(ptr, i64, i64)\n");
            out.push_str("declare void @ubel_trap_index(i64, i64)\n\n");
        }
        if !self.decls.is_empty() {
            for d in &self.decls {
                out.push_str(d);
                out.push('\n');
            }
            out.push('\n');
        }
        if !self.strings.is_empty() {
            out.push_str(&self.strings);
            out.push('\n');
        }
        out.push_str(&self.struct_defs());
        out.push_str(&self.funcs);
        if self.uses_bump {
            out.push_str("!0 = !{!\"branch_weights\", i32 1999, i32 1}\n");
        }
        out
    }

    /// Struct types, and for every struct that is allocated, the size and pointer-mask
    /// helpers. Both are computed from the TARGET's data layout with the `getelementptr
    /// null` trick, so one `.ll` file is right for 32-bit and 64-bit builds alike. Bit i of
    /// a mask means the pointer-sized word at byte offset i * sizeof(ptr) holds a pointer.
    fn struct_defs(&self) -> String {
        let mut out = String::new();
        for s in &self.structs {
            let fields: Vec<&str> = s.fields.iter().map(|f| f.ty.ir()).collect();
            let _ = writeln!(out, "%struct.{} = type {{ {} }}", s.name, fields.join(", "));
        }
        for en in &self.enums {
            if en.slots == 0 {
                let _ = writeln!(out, "%enum.{} = type {{ i64 }}", en.name);
            } else {
                let _ = writeln!(out, "%enum.{} = type {{ i64, [{} x i64] }}", en.name, en.slots);
            }
        }
        out.push_str(&self.estatic_ir);
        if self.uses_bump {
            out.push_str("%ubel_arena = type { ptr, ptr, ptr, i64 }\n");
        }
        if !self.structs.is_empty() || !self.enums.is_empty() {
            out.push('\n');
        }
        for &sid in &self.sized {
            let s = &self.structs[sid];
            let _ = writeln!(
                out,
                "define internal i64 @ubel_sz.{n}() alwaysinline nounwind readnone {{\nentry:\n  ret i64 ptrtoint (ptr getelementptr (%struct.{n}, ptr null, i32 1) to i64)\n}}",
                n = s.name
            );
        }
        for &sid in &self.masked {
            let s = &self.structs[sid];
            let ptr_fields: Vec<usize> = s
                .fields
                .iter()
                .enumerate()
                .filter(|(_, f)| f.ty.is_ptr())
                .map(|(i, _)| i)
                .collect();
            let mut body = String::from("  %psz = ptrtoint ptr getelementptr (ptr, ptr null, i32 1) to i64\n");
            let mut acc = String::from("0");
            for (k, fi) in ptr_fields.iter().enumerate() {
                let _ = writeln!(body, "  %off{k} = ptrtoint ptr getelementptr (%struct.{}, ptr null, i32 0, i32 {fi}) to i64", s.name);
                let _ = writeln!(body, "  %idx{k} = udiv i64 %off{k}, %psz");
                let _ = writeln!(body, "  %bit{k} = shl i64 1, %idx{k}");
                let _ = writeln!(body, "  %m{k} = or i64 {acc}, %bit{k}");
                acc = format!("%m{k}");
            }
            let _ = writeln!(
                out,
                "define internal i64 @ubel_mask.{}() alwaysinline nounwind readnone {{\nentry:\n{body}  ret i64 {acc}\n}}",
                s.name
            );
        }
        // Enum helpers. Every value of an enum has the size of its widest variant; the mask
        // depends on the variant, because only some payload slots hold pointers. A payload
        // slot is 8 bytes whatever it holds, a pointer sits at the start of its slot.
        for &eid in &self.esized {
            let en = &self.enums[eid];
            let _ = writeln!(
                out,
                "define internal i64 @ubel_esz.{n}() alwaysinline nounwind readnone {{\nentry:\n  ret i64 ptrtoint (ptr getelementptr (%enum.{n}, ptr null, i32 1) to i64)\n}}",
                n = en.name
            );
        }
        for &(eid, vi) in &self.emasked {
            let en = &self.enums[eid];
            let v = &en.variants[vi];
            let mut body = String::from("  %psz = ptrtoint ptr getelementptr (ptr, ptr null, i32 1) to i64\n");
            let mut acc = String::from("0");
            let mut k = 0usize;
            for (slot, f) in v.fields.iter().enumerate() {
                if !f.is_ptr() {
                    continue;
                }
                let _ = writeln!(body, "  %off{k} = ptrtoint ptr getelementptr (%enum.{}, ptr null, i32 0, i32 1, i64 {slot}) to i64", en.name);
                let _ = writeln!(body, "  %idx{k} = udiv i64 %off{k}, %psz");
                let _ = writeln!(body, "  %bit{k} = shl i64 1, %idx{k}");
                let _ = writeln!(body, "  %m{k} = or i64 {acc}, %bit{k}");
                acc = format!("%m{k}");
                k += 1;
            }
            let _ = writeln!(
                out,
                "define internal i64 @ubel_emask.{}.{}() alwaysinline nounwind readnone {{\nentry:\n{body}  ret i64 {acc}\n}}",
                en.name, v.name
            );
        }
        if self.uses_bump {
            out.push_str(concat!(
                "; MID-tier bump allocation, the fast path inlined. `size` is a multiple of 16.\n",
                "define internal ptr @ubel_arena_bump(ptr %a, i64 %size) alwaysinline nounwind {\n",
                "entry:\n",
                "  %curp = getelementptr inbounds %ubel_arena, ptr %a, i32 0, i32 0\n",
                "  %endp = getelementptr inbounds %ubel_arena, ptr %a, i32 0, i32 1\n",
                "  %cur  = load ptr, ptr %curp\n",
                "  %end  = load ptr, ptr %endp\n",
                "  %new  = getelementptr i8, ptr %cur, i64 %size\n",
                "  %ok   = icmp ule ptr %new, %end\n",
                "  br i1 %ok, label %fast, label %slow, !prof !0\n",
                "fast:\n",
                "  store ptr %new, ptr %curp\n",
                "  ret ptr %cur\n",
                "slow:\n",
                "  %p = call ptr @ubel_arena_grow(ptr %a, i64 %size)\n",
                "  ret ptr %p\n",
                "}\n",
            ));
        }
        if !out.is_empty() && !out.ends_with("\n\n") {
            out.push('\n');
        }
        out
    }

    // ── low-level emission ───────────────────────────────────────

    fn fresh(&mut self) -> String {
        self.tmp += 1;
        format!("%t{}", self.tmp)
    }

    fn label(&mut self, base: &str) -> String {
        self.lbl += 1;
        format!("{base}{}", self.lbl)
    }

    /// Write `label:` and make it the current block. Falls through with a `br` if the
    /// previous block has no terminator yet.
    fn begin(&mut self, label: &str) {
        if !self.terminated {
            let _ = writeln!(self.body, "  br label %{label}");
        }
        let _ = writeln!(self.body, "{label}:");
        self.terminated = false;
    }

    /// Code after a terminator (`return`, `break`, ...) is unreachable but still has to be
    /// valid IR, so it goes into a fresh block that nothing branches to.
    fn open_if_dead(&mut self) {
        if self.terminated {
            let l = self.label("dead");
            self.begin(&l);
        }
    }

    fn inst(&mut self, text: &str) {
        self.open_if_dead();
        let _ = writeln!(self.body, "  {text}");
    }

    fn term(&mut self, text: &str) {
        self.open_if_dead();
        let _ = writeln!(self.body, "  {text}");
        self.terminated = true;
    }

    fn alloca(&mut self, ty: Ty, hint: &str) -> String {
        self.tmp += 1;
        let name = format!("%{hint}.{}", self.tmp);
        let _ = writeln!(self.allocas, "  {name} = alloca {}", ty.ir());
        name
    }

    fn declare(&mut self, name: &str, ty: Ty) -> String {
        let slot = self.alloca(ty, name);
        if let Some(scope) = self.scopes.last_mut() {
            scope.insert(name.to_string(), Local { slot: slot.clone(), ty, mutable: false, lambda: false });
        }
        slot
    }

    /// Mark the local just declared in the innermost scope as one that can change.
    fn mark_mutable(&mut self, name: &str) {
        if let Some(l) = self.scopes.last_mut().and_then(|sc| sc.get_mut(name)) {
            l.mutable = true;
        }
    }

    /// Mark the local just declared in the innermost scope as bound straight to a lambda.
    fn mark_lambda(&mut self, name: &str) {
        if let Some(l) = self.scopes.last_mut().and_then(|sc| sc.get_mut(name)) {
            l.lambda = true;
        }
    }

    fn lookup(&self, name: &str) -> Option<Local> {
        self.scopes.iter().rev().find_map(|s| s.get(name).cloned())
    }

    /// An object with pointer fields is about to be allocated in `arena`: make that arena scannable.
    /// Conservative: the pointers may point at other objects of the same arena (then scanning is only
    /// a little work), but a field cannot say that its value is not a GC object, and a plain arena
    /// holding the only reference to a GC object would lose it to the collector.
    fn mark_scan(&mut self, arena: &Option<String>) {
        if let Some(a) = arena {
            self.arena_scan.insert(a.clone());
        }
    }

    fn destroy_arenas_from(&mut self, depth: usize) {
        let names: Vec<String> = self.arenas[depth..].iter().rev().cloned().collect();
        for a in names {
            self.inst(&format!("call void @ubel_arena_destroy(ptr {a})"));
        }
    }

    fn need(&self, want: Ty, got: &Val, span: Span) -> R<()> {
        if got.ty == want {
            Ok(())
        } else if matches!(got.ty, Ty::Opt(i) if self.opt_inner(i) == want) {
            unsupported(
                span,
                "an optional value where a plain value is expected (unwrap it first with `or return`, `or break` or `or continue`)",
            )
        } else if got.ty == Ty::Null {
            unsupported(span, "`null` where the type is not an optional")
        } else {
            Err(CompileError::at(
                span,
                format!("emitter expected {} here but the expression is {}", want.ir(), got.ty.ir()),
            ))
        }
    }

    /// A value that goes INTO a string that is built (an interpolated string used as a value): numbers, bool and
    /// strings. Lists and optionals can be printed directly (`println(xs)`) but are not converted to strings yet.
    fn need_printable(&self, got: &Val, span: Span) -> R<()> {
        self.need_value(got, span)?;
        match got.ty {
            Ty::Struct(_) => return unsupported(span, "printing a struct value (print its fields)"),
            Ty::Enum(_) => return unsupported(span, "printing an enum value (match on it and print the payload)"),
            Ty::Fn(_) => return unsupported(span, "printing a closure"),
            Ty::List(_) => {
                return unsupported(span, "a list inside a string that is built (print it directly: `println(xs)`)")
            }
            Ty::Opt(_) => {
                return unsupported(span, "an optional inside a string that is built (print it directly: `println(x)`)")
            }
            _ => {}
        }
        Ok(())
    }

    /// A value that is printed directly by `print` / `println` (also from inside an interpolated string they print):
    /// numbers, bool, strings, `null`, and lists and optionals of those.
    fn need_print_item(&self, got: &Val, span: Span) -> R<()> {
        if got.ty == Ty::Null {
            return Ok(());
        }
        self.need_value(got, span)?;
        match self.unprintable(got.ty) {
            Some(what) => unsupported(span, what),
            None => Ok(()),
        }
    }

    /// Why a value of this type cannot be printed yet; `None` when it can.
    fn unprintable(&self, ty: Ty) -> Option<&'static str> {
        match ty {
            Ty::Struct(_) => Some("printing a struct value (print its fields)"),
            Ty::Enum(_) => Some("printing an enum value (match on it and print the payload)"),
            Ty::Fn(_) => Some("printing a closure"),
            Ty::List(li) => self
                .unprintable(self.list_elem(li))
                .map(|_| "printing a list of structs, enums or closures (print its elements)"),
            Ty::Opt(oi) => self.unprintable(self.opt_inner(oi)).map(|_| "printing an optional struct, enum or closure"),
            _ => None,
        }
    }

    fn need_value(&self, got: &Val, span: Span) -> R<()> {
        if got.ty == Ty::Void {
            Err(CompileError::at(span, "this expression has no value"))
        } else if got.ty == Ty::Null {
            unsupported(span, "`null` where its optional type is not known (write the type where it is stored: `let x: int? = null`)")
        } else {
            Ok(())
        }
    }

    // ── types ────────────────────────────────────────────────────

    fn ty_of_type(&self, t: &ubel_stratum::ast::Type) -> R<Ty> {
        match t.kind {
            TypeKind::Int | TypeKind::Long | TypeKind::I64 | TypeKind::Isize => Ok(Ty::I64),
            TypeKind::I8 | TypeKind::Byte => Ok(Ty::Int(IntTy::I8)),
            TypeKind::I16 => Ok(Ty::Int(IntTy::I16)),
            TypeKind::I32 => Ok(Ty::Int(IntTy::I32)),
            TypeKind::U8 | TypeKind::Ubyte => Ok(Ty::Int(IntTy::U8)),
            TypeKind::U16 => Ok(Ty::Int(IntTy::U16)),
            TypeKind::U32 | TypeKind::Uint => Ok(Ty::Int(IntTy::U32)),
            TypeKind::U64 | TypeKind::Ulong | TypeKind::Usize => Ok(Ty::Int(IntTy::U64)),
            TypeKind::Short | TypeKind::Ushort => {
                unsupported(t.span, "`short` and `ushort` (sema widens them; write `i16` or `u16`)")
            }
            TypeKind::Float | TypeKind::F32 => Ok(Ty::F32),
            TypeKind::Double | TypeKind::F64 => Ok(Ty::F64),
            TypeKind::Bool => Ok(Ty::I1),
            TypeKind::Void => Ok(Ty::Void),
            TypeKind::Str => Ok(Ty::Str),
            TypeKind::List(Some(inner)) => {
                let elem = self.ty_of_type(inner)?;
                if !Self::list_elem_ok(elem) {
                    return unsupported(inner.span, "a list of this element type (numbers, bool, strings, structs, enums and lists so far)");
                }
                Ok(self.list_ty(elem))
            }
            TypeKind::List(None) => unsupported(t.span, "a list type without an element type"),
            TypeKind::Optional(inner) => {
                let it = self.ty_of_type(inner)?;
                if !Self::opt_inner_ok(it) {
                    return unsupported(t.span, "an optional of this type (numbers, bool, strings, structs, enums and lists so far)");
                }
                Ok(self.opt_ty(it))
            }
            TypeKind::Function(ft) => {
                if ft.is_fallible {
                    return unsupported(t.span, "fallible function types");
                }
                let mut params = Vec::new();
                for p in ft.params {
                    let ty = self.ty_of_type(p)?;
                    if ty == Ty::Void {
                        return unsupported(p.span, "`void` parameters");
                    }
                    params.push(ty);
                }
                let ret = match ft.return_type {
                    None => Ty::Void,
                    Some(r) => self.ty_of_type(r)?,
                };
                Ok(self.fn_ty(params, ret))
            }
            TypeKind::Reference { mutable, inner, .. } => {
                if mutable {
                    return unsupported(t.span, "`&mut` references");
                }
                match self.ty_of_type(inner)? {
                    Ty::Struct(sid) => Ok(Ty::Struct(sid)),
                    _ => unsupported(t.span, "references to anything but a struct (`&L int` and the like)"),
                }
            }
            TypeKind::Named { path, args } => {
                if !args.is_empty() {
                    return unsupported(t.span, "generic types");
                }
                match path {
                    [name] => match (self.struct_ids.get(*name), self.enum_ids.get(*name)) {
                        (Some(&sid), _) => Ok(Ty::Struct(sid)),
                        (None, Some(&eid)) => Ok(Ty::Enum(eid)),
                        // `Str` is another spelling of `string` (sema and the interpreter accept it; the
                        // self-hosted tokenizer writes its string fields that way).
                        (None, None) if *name == "Str" => Ok(Ty::Str),
                        (None, None) => unsupported(t.span, format!("the type `{name}` (only integers, floats, bool, string, void, plain structs and enums, and lists of those, so far)")),
                    },
                    _ => unsupported(t.span, "qualified type names"),
                }
            }
            _ => unsupported(t.span, "this type (only integers, `bool`, `string`, `void`, plain structs and enums so far)"),
        }
    }

    /// Register every struct before any signature is resolved, so structs can refer to
    /// each other in any order. Pass one claims the names, pass two resolves field types.
    fn collect_structs(&mut self, program: &Program) -> R<()> {
        let mut decls: Vec<&StructDecl> = Vec::new();
        for item in program.items {
            if item.span().is_prelude() {
                continue;
            }
            if let Item::Struct(s) = item {
                if !s.generic_params.is_empty() {
                    return unsupported(s.span, "generic structs");
                }
                // `edge struct Name [lifetime L]`: the lifetime is sema's business, a `&L T`
                // field is an ordinary pointer here, so edge structs need nothing special.
                if !s.attributes.is_empty() {
                    return unsupported(s.span, "attributes on structs");
                }
                if self.struct_ids.contains_key(s.name) {
                    return Err(CompileError::at(s.span, format!("the struct `{}` is declared twice", s.name)));
                }
                self.struct_ids.insert(s.name.to_string(), self.structs.len());
                self.structs.push(StructInfo { name: s.name.to_string(), fields: Vec::new() });
                decls.push(s);
            }
        }
        // Enums: names first (they can refer to structs, structs to them, and an enum to itself).
        let mut edecls: Vec<&EnumDecl> = Vec::new();
        for item in program.items {
            if item.span().is_prelude() {
                continue;
            }
            if let Item::Enum(en) = item {
                if !en.generic_params.is_empty() {
                    return unsupported(en.span, "generic enums");
                }
                if !en.attributes.is_empty() {
                    return unsupported(en.span, "attributes on enums");
                }
                if self.enum_ids.contains_key(en.name) || self.struct_ids.contains_key(en.name) {
                    return Err(CompileError::at(en.span, format!("the name `{}` is declared twice", en.name)));
                }
                if en.variants.is_empty() {
                    return unsupported(en.span, "enums with no variants");
                }
                self.enum_ids.insert(en.name.to_string(), self.enums.len());
                self.enums.push(EnumInfo { name: en.name.to_string(), variants: Vec::new(), slots: 0 });
                edecls.push(en);
            }
        }
        for (sid, s) in decls.iter().enumerate() {
            let mut fields = Vec::new();
            for m in s.members {
                match m {
                    StructMember::Field(f) => {
                        let ty = self.ty_of_type(f.ty)?;
                        if ty == Ty::Void {
                            return unsupported(f.span, "`void` fields");
                        }
                        if matches!(ty, Ty::Opt(_)) {
                            return unsupported(f.span, "an optional struct field (`T?`; optionals live in locals, parameters and results so far)");
                        }
                        fields.push(FieldInfo { name: f.name.to_string(), ty });
                    }
                    StructMember::Method(m) => return unsupported(m.span, "struct methods"),
                    StructMember::Property(p) => return unsupported(p.span, "struct properties"),
                }
            }
            if fields.is_empty() {
                return unsupported(s.span, "structs with no fields");
            }
            if fields.len() > MAX_STRUCT_FIELDS {
                return unsupported(s.span, format!("structs with more than {MAX_STRUCT_FIELDS} fields (runtime object size limit)"));
            }
            self.structs[sid].fields = fields;
        }
        for (eid, en) in edecls.iter().enumerate() {
            let mut variants = Vec::new();
            for v in en.variants {
                let fields = match v.payload {
                    // A written discriminant is validated by sema and never read back (no
                    // `as int` exists yet), so such a variant behaves like a payload-free one.
                    EnumVariantPayload::None | EnumVariantPayload::Discriminant(_) => Vec::new(),
                    EnumVariantPayload::Tuple(types) => {
                        let mut fs = Vec::new();
                        for t in types {
                            let ty = self.ty_of_type(t)?;
                            if ty == Ty::Void {
                                return unsupported(t.span, "`void` payloads");
                            }
                            if matches!(ty, Ty::Opt(_)) {
                                return unsupported(t.span, "an optional enum payload (`T?`; optionals live in locals, parameters and results so far)");
                            }
                            fs.push(ty);
                        }
                        fs
                    }
                    EnumVariantPayload::Struct(_) => return unsupported(v.span, "struct-style enum variants (`V { a, b }`)"),
                };
                if fields.len() > MAX_VARIANT_FIELDS {
                    return unsupported(v.span, format!("variants with more than {MAX_VARIANT_FIELDS} payload fields (runtime object size limit)"));
                }
                variants.push(VariantInfo { name: v.name.to_string(), fields });
            }
            let slots = variants.iter().map(|v| v.fields.len()).max().unwrap_or(0);
            self.enums[eid].variants = variants;
            self.enums[eid].slots = slots;
        }
        Ok(())
    }

    fn field_index(&self, sid: usize, name: &str, span: Span) -> R<(usize, Ty)> {
        let s = &self.structs[sid];
        s.fields
            .iter()
            .enumerate()
            .find(|(_, f)| f.name == name)
            .map(|(i, f)| (i, f.ty))
            .ok_or_else(|| CompileError::at(span, format!("the struct `{}` has no field `{name}`", s.name)))
    }

    /// Refuse expressions sema typed as something the emitter does not lower, such as
    /// a sized integer, a float or a collection, before they can be miscompiled as `int`.
    fn check_sema_type(&self, e: &Expr) -> R<()> {
        // An assignment has no value the emitter uses (it is lowered as void, and both the
        // place and the right-hand side are checked on their own), and sema leaves its type
        // `Unknown` inside `with arena`, so its own type says nothing useful.
        if let ExprKind::Assign { .. } = e.kind {
            return Ok(());
        }
        if let Some(&start) = self.ctx.expr_types.get(&e.span) {
            // Sema wraps a struct value in the tier of the place it was built in (a GC or an
            // arena reference); the emitter only needs the struct underneath.
            //
            // Inside `with arena`, sema leaves some expressions typed `Unknown` (field access on
            // an arena struct, and every operator built on it). For the expression kinds the
            // emitter lowers, its own type comes from declarations it has already restricted to
            // int, bool, void and plain structs (parameters, fields, signatures) and from
            // operands it checks itself, so `Unknown` is let through for exactly those kinds.
            // Every other kind is refused by the emitter whatever sema says.
            let own_type_is_authoritative = matches!(
                e.kind,
                ExprKind::Ident(_)
                    | ExprKind::Field { .. }
                    | ExprKind::Call { .. }
                    | ExprKind::StructLit { .. }
                    | ExprKind::BinOp { .. }
                    | ExprKind::UnaryOp { .. }
                    | ExprKind::As { .. }
                    | ExprKind::If(_)
                    | ExprKind::ShortDecl { .. }
                    | ExprKind::Borrow { .. }
                    | ExprKind::Match(_)
                    | ExprKind::Lambda(_)
            );
            let mut id = start;
            loop {
                match self.ctx.types.get(id) {
                    SemaType::Int
                    | SemaType::Long
                    | SemaType::I8
                    | SemaType::I16
                    | SemaType::I32
                    | SemaType::I64
                    | SemaType::Isize
                    | SemaType::Uint
                    | SemaType::Ulong
                    | SemaType::U8
                    | SemaType::U16
                    | SemaType::U32
                    | SemaType::U64
                    | SemaType::Usize
                    | SemaType::Float
                    | SemaType::F32
                    | SemaType::Double
                    | SemaType::F64
                    | SemaType::Bool
                    | SemaType::Void
                    | SemaType::Str
                    | SemaType::Null
                    | SemaType::Var(_) => return Ok(()),
                    SemaType::Optional(inner) => id = *inner,
                    SemaType::Unknown if own_type_is_authoritative => return Ok(()),
                    SemaType::Function { is_fallible: false, .. } => return Ok(()),
                    SemaType::GcRef(inner) => id = *inner,
                    SemaType::ArenaRef { inner, .. } => id = *inner,
                    SemaType::OwnedRef { mutable: false, inner } => id = *inner,
                    SemaType::Reference { mutable: false, inner, .. } => id = *inner,
                    SemaType::List(inner) => id = *inner,
                    SemaType::Named { def, args }
                        if args.is_empty()
                            && (self.struct_ids.contains_key(&self.ctx.symbols.lookup(*def).name)
                                || self.enum_ids.contains_key(&self.ctx.symbols.lookup(*def).name)) =>
                    {
                        return Ok(())
                    }
                    _ => {
                        let shown = self.ctx.types.get(start).display(&self.ctx.types, &self.ctx.symbols);
                        return unsupported(e.span, format!("values of type `{shown}` (only integers, floats, bool, string, void, plain structs and enums, and lists of those, so far)"));
                    }
                }
            }
        }
        Ok(())
    }

    // ── functions ────────────────────────────────────────────────

    fn signature(&self, f: &FunctionDecl) -> R<Sig> {
        if f.is_async {
            return unsupported(f.span, "async functions");
        }
        if !f.generic_params.is_empty() {
            return unsupported(f.span, "generic functions");
        }
        let mut params = Vec::new();
        for p in f.params {
            match p.kind {
                ParamKind::Named { ty: Some(t), default: None, .. } => params.push(self.ty_of_type(t)?),
                ParamKind::Named { .. } => {
                    return unsupported(p.span, "untyped parameters and default values");
                }
                _ => return unsupported(p.span, "`self` and discard parameters"),
            }
        }
        let ret = match f.return_type {
            None => Ty::Void,
            Some(rt) => {
                if rt.is_fallible {
                    return unsupported(f.span, "fallible return types");
                }
                self.ty_of_type(rt.ty)?
            }
        };
        Ok(Sig { params, ret, tier: f.tier })
    }

    fn function(&mut self, f: &FunctionDecl) -> R<()> {
        let sig = self.sigs[f.name].clone();

        self.body.clear();
        self.allocas.clear();
        self.tmp = 0;
        self.lbl = 0;
        self.terminated = false;
        self.scopes = vec![HashMap::new()];
        self.loops.clear();
        self.arenas.clear();
        self.ret = sig.ret;
        self.cur_tier = f.tier;

        let mut header_params = Vec::new();
        for (i, p) in f.params.iter().enumerate() {
            if let ParamKind::Named { name, mutable, .. } = p.kind {
                let ty = sig.params[i];
                // A name no Ubel identifier can produce (it starts with a dot), so a parameter
                // register never collides with the stack slot of a local named `p`, `arg`, ...
                let reg = format!("%.arg{i}");
                header_params.push(format!("{} {}", ty.ir(), reg));
                let slot = self.declare(name, ty);
                if mutable {
                    self.mark_mutable(name);
                }
                self.inst(&format!("store {} {}, ptr {}", ty.ir(), reg, slot));
            }
        }

        self.lower_body(f.body.stmts)?;
        self.end_body();

        let _ = writeln!(self.funcs, "; tier: {}", tier_name(f.tier));
        let _ = writeln!(
            self.funcs,
            "define {} @ubel_{}({}) {{\nentry:\n{}{}}}\n",
            self.ret.ir(),
            f.name,
            header_params.join(", "),
            self.allocas,
            self.body
        );
        Ok(())
    }

    /// The statements of a function or closure body. The last expression of a non-void body is its result.
    fn lower_body(&mut self, stmts: &[Stmt]) -> R<()> {
        let n = stmts.len();
        for (i, s) in stmts.iter().enumerate() {
            if i + 1 == n && self.ret_infer && self.ret_seen.is_none() {
                // A closure whose result type is still open: a non-void last expression is its result.
                if let StmtKind::Expr(e) = s.kind {
                    let v = if let ExprKind::Match(m) = e.kind {
                        self.match_lower(m.scrutinee, m.arms, e.span, true)?
                    } else {
                        self.expr(e)?
                    };
                    if v.ty != Ty::Void {
                        self.do_return(Some(v), s.span)?;
                    }
                    continue;
                }
            }
            if i + 1 == n && self.ret != Ty::Void {
                if let StmtKind::Expr(e) = s.kind {
                    if let ExprKind::Match(m) = e.kind {
                        let v = self.match_lower(m.scrutinee, m.arms, e.span, true)?;
                        if v.ty == self.ret {
                            self.do_return(Some(v), s.span)?;
                        }
                        continue;
                    }
                    let v = self.expr(e)?;
                    if v.ty == self.ret {
                        self.do_return(Some(v), s.span)?;
                        continue;
                    }
                }
            }
            self.stmt(s)?;
        }
        Ok(())
    }

    /// Falling off the end: a void body returns, any other traps (sema should have caught it).
    fn end_body(&mut self) {
        if !self.terminated {
            if self.ret == Ty::Void {
                self.term("ret void");
            } else {
                self.inst("call void @ubel_trap_no_return()");
                self.term("unreachable");
            }
        }
    }

    // ── statements ───────────────────────────────────────────────

    fn block(&mut self, b: &Block) -> R<()> {
        self.scopes.push(HashMap::new());
        for s in b.stmts {
            self.stmt(s)?;
        }
        self.scopes.pop();
        Ok(())
    }

    fn do_return(&mut self, v: Option<Val>, span: Span) -> R<()> {
        if self.ret_infer {
            let ty = v.as_ref().map(|x| x.ty).unwrap_or(Ty::Void);
            match self.ret_seen {
                None => {
                    self.ret_seen = Some(ty);
                    self.ret = ty;
                }
                Some(t) if t == ty => {}
                Some(_) => return Err(CompileError::at(span, "the values this closure returns have different types")),
            }
        }
        match (v, self.ret) {
            (None, Ty::Void) => {
                self.destroy_arenas_from(0);
                self.term("ret void");
                Ok(())
            }
            (Some(val), want) if want != Ty::Void && val.ty == want => {
                self.destroy_arenas_from(0);
                self.term(&format!("ret {} {}", want.ir(), val.v));
                Ok(())
            }
            _ => Err(CompileError::at(span, "the returned value does not match the function's return type")),
        }
    }

    fn stmt(&mut self, s: &Stmt) -> R<()> {
        match s.kind {
            StmtKind::Let { mutable, binding, ty, value } => {
                let name = match binding {
                    BindingTarget::Ident(n) => n,
                    BindingTarget::Destructure(_) => return unsupported(s.span, "destructuring `let`"),
                };
                // An annotation is otherwise ignored (sema has checked it; the value's type is used), except that a
                // `List.new()` takes its element type from it.
                let want = match ty.map(|t| self.ty_of_type(t)) {
                    Some(Ok(w)) => Some(w),
                    // an annotation the emitter cannot lower matters for `List.new()` (its element type): say why
                    Some(Err(err)) if self.is_list_new_call(value) => return Err(err),
                    _ => None,
                };
                let v = match want {
                    Some(w) => self.expr_as(value, w)?,
                    None => self.expr(value)?,
                };
                self.need_value(&v, value.span)?;
                // An annotation is otherwise not checked here, but an optional must not slip into a plain place.
                if let Some(w) = want {
                    if v.ty != w && matches!(v.ty, Ty::Opt(_)) {
                        self.need(w, &v, value.span)?;
                    }
                }
                let slot = self.declare(name, v.ty);
                if mutable {
                    self.mark_mutable(name);
                } else if matches!(value.kind, ExprKind::Lambda(_)) {
                    self.mark_lambda(name);
                }
                self.inst(&format!("store {} {}, ptr {}", v.ty.ir(), v.v, slot));
                Ok(())
            }
            StmtKind::Expr(e) => {
                if let ExprKind::Match(m) = e.kind {
                    self.match_lower(m.scrutinee, m.arms, e.span, false)?;
                    return Ok(());
                }
                self.expr(e)?;
                Ok(())
            }
            StmtKind::Match { scrutinee, arms } => {
                self.match_lower(scrutinee, arms, s.span, false)?;
                Ok(())
            }
            StmtKind::Return(opt) => {
                let v = match opt {
                    Some(e) => Some(if self.ret_infer { self.expr(e)? } else { self.expr_as(e, self.ret)? }),
                    None => None,
                };
                self.do_return(v, s.span)
            }
            StmtKind::If(ife) => self.if_stmt(ife),
            StmtKind::While { condition, body } => {
                let cond_l = self.label("while.cond");
                let body_l = self.label("while.body");
                let end_l = self.label("while.end");
                self.term(&format!("br label %{cond_l}"));
                self.begin(&cond_l);
                let c = self.expr(condition)?;
                self.need(Ty::I1, &c, condition.span)?;
                self.term(&format!("br i1 {}, label %{body_l}, label %{end_l}", c.v));
                self.begin(&body_l);
                self.loops.push(LoopCtx { cont: cond_l.clone(), brk: end_l.clone(), arena_depth: self.arenas.len() });
                self.block(body)?;
                self.loops.pop();
                if !self.terminated {
                    self.term(&format!("br label %{cond_l}"));
                }
                self.begin(&end_l);
                Ok(())
            }
            StmtKind::Loop(body) => {
                let body_l = self.label("loop.body");
                let end_l = self.label("loop.end");
                self.term(&format!("br label %{body_l}"));
                self.begin(&body_l);
                self.loops.push(LoopCtx { cont: body_l.clone(), brk: end_l.clone(), arena_depth: self.arenas.len() });
                self.block(body)?;
                self.loops.pop();
                if !self.terminated {
                    self.term(&format!("br label %{body_l}"));
                }
                self.begin(&end_l);
                Ok(())
            }
            StmtKind::For { binding, iter, body } => self.for_stmt(s.span, binding, iter, body),
            StmtKind::Break(None) => {
                let ctx = self
                    .loops
                    .last()
                    .cloned()
                    .ok_or_else(|| CompileError::at(s.span, "`break` outside a loop"))?;
                self.destroy_arenas_from(ctx.arena_depth);
                self.term(&format!("br label %{}", ctx.brk));
                Ok(())
            }
            StmtKind::Break(Some(_)) => unsupported(s.span, "`break` with a value"),
            StmtKind::Continue => {
                let ctx = self
                    .loops
                    .last()
                    .cloned()
                    .ok_or_else(|| CompileError::at(s.span, "`continue` outside a loop"))?;
                self.destroy_arenas_from(ctx.arena_depth);
                self.term(&format!("br label %{}", ctx.cont));
                Ok(())
            }
            StmtKind::With { allocator, body } => match allocator {
                AllocatorKind::Arena(size) => {
                    let bytes = match size {
                        SizeExpr::WithUnit { value, unit } => format!("{}", value * unit.multiplier()),
                        SizeExpr::Expr(e) => {
                            let v = self.expr(e)?;
                            self.need(Ty::I64, &v, e.span)?;
                            v.v
                        }
                    };
                    let a = self.fresh().replace("%t", "%arena.");
                    let create = format!("{a} = call ptr @ubel_arena_create(i64 {bytes})");
                    self.inst(&create);
                    self.arenas.push(a.clone());
                    self.block(body)?;
                    // Whether the arena has to be scannable is only known once the block has been emitted
                    // (it depends on what was allocated in it), so the creation is rewritten here.
                    if self.arena_scan.remove(&a) {
                        self.uses_scan = true;
                        let scan = format!("{a} = call ptr @ubel_arena_create_scan(i64 {bytes}) ; scannable: it holds pointers to GC objects");
                        self.body = self.body.replacen(&create, &scan, 1);
                    }
                    // Reaching here normally means the body fell off its end; every early
                    // exit (`return`, `break`, `continue`) destroyed the arena where it left.
                    if !self.terminated {
                        self.inst(&format!("call void @ubel_arena_destroy(ptr {a})"));
                    }
                    self.arena_ids.retain(|(_, p)| p != &a);
                    self.arenas.pop();
                    Ok(())
                }
                _ => unsupported(s.span, "`with gc`, `with heap` and `with pool` blocks"),
            },
            other => unsupported(s.span, stmt_kind_name(&other)),
        }
    }

    fn for_stmt(&mut self, span: Span, binding: BindingTarget, iter: &Expr, body: &Block) -> R<()> {
        let name = match binding {
            BindingTarget::Ident(n) => n,
            BindingTarget::Destructure(_) => return unsupported(span, "destructuring in `for`"),
        };
        let (inclusive, lo_e, hi_e) = match iter.kind {
            ExprKind::BinOp { op: BinOp::Range, lhs, rhs } => (false, lhs, rhs),
            ExprKind::BinOp { op: BinOp::RangeIncl, lhs, rhs } => (true, lhs, rhs),
            _ => return self.for_list(span, name, iter, body),
        };
        let lo = self.expr(lo_e)?;
        let hi = self.expr(hi_e)?;
        if matches!(lo.ty, Ty::Int(_)) || matches!(hi.ty, Ty::Int(_)) {
            return unsupported(
                iter.span,
                "a `for` loop over a range of a sized integer type (use `int` bounds and cast inside the loop)",
            );
        }
        self.need(Ty::I64, &lo, lo_e.span)?;
        self.need(Ty::I64, &hi, hi_e.span)?;

        let counter = self.alloca(Ty::I64, "for.i");
        let limit = self.alloca(Ty::I64, "for.hi");
        self.inst(&format!("store i64 {}, ptr {counter}", lo.v));
        self.inst(&format!("store i64 {}, ptr {limit}", hi.v));

        let cond_l = self.label("for.cond");
        let body_l = self.label("for.body");
        let inc_l = self.label("for.inc");
        let end_l = self.label("for.end");

        self.term(&format!("br label %{cond_l}"));
        self.begin(&cond_l);
        let i = self.fresh();
        self.inst(&format!("{i} = load i64, ptr {counter}"));
        let h = self.fresh();
        self.inst(&format!("{h} = load i64, ptr {limit}"));
        let c = self.fresh();
        let cmp = if inclusive { "sle" } else { "slt" };
        self.inst(&format!("{c} = icmp {cmp} i64 {i}, {h}"));
        self.term(&format!("br i1 {c}, label %{body_l}, label %{end_l}"));

        self.begin(&body_l);
        self.loops.push(LoopCtx { cont: inc_l.clone(), brk: end_l.clone(), arena_depth: self.arenas.len() });
        self.scopes.push(HashMap::new());
        let slot = self.declare(name, Ty::I64);
        self.mark_mutable(name);
        let cur = self.fresh();
        self.inst(&format!("{cur} = load i64, ptr {counter}"));
        self.inst(&format!("store i64 {cur}, ptr {slot}"));
        for s in body.stmts {
            self.stmt(s)?;
        }
        self.scopes.pop();
        self.loops.pop();
        if !self.terminated {
            self.term(&format!("br label %{inc_l}"));
        }

        self.begin(&inc_l);
        let cur2 = self.fresh();
        self.inst(&format!("{cur2} = load i64, ptr {counter}"));
        let next = self.fresh();
        self.inst(&format!("{next} = add i64 {cur2}, 1"));
        self.inst(&format!("store i64 {next}, ptr {counter}"));
        self.term(&format!("br label %{cond_l}"));
        self.begin(&end_l);
        Ok(())
    }

    fn if_stmt(&mut self, ife: &IfExpr) -> R<()> {
        let end = self.label("if.end");
        let mut arms: Vec<(&Expr, IfBranchBody)> = vec![(ife.condition, ife.then_body)];
        for el in ife.elif_branches {
            arms.push((el.condition, el.body));
        }
        for (cond, body) in arms {
            let c = self.expr(cond)?;
            self.need(Ty::I1, &c, cond.span)?;
            let then_l = self.label("then");
            let else_l = self.label("else");
            self.term(&format!("br i1 {}, label %{then_l}, label %{else_l}", c.v));
            self.begin(&then_l);
            match body {
                IfBranchBody::Block(b) => self.block(&b)?,
                IfBranchBody::Expr(e) => {
                    self.expr(e)?;
                }
            }
            if !self.terminated {
                self.term(&format!("br label %{end}"));
            }
            self.begin(&else_l);
        }
        if let Some(body) = ife.else_body {
            match body {
                IfBranchBody::Block(b) => self.block(&b)?,
                IfBranchBody::Expr(e) => {
                    self.expr(e)?;
                }
            }
        }
        self.begin(&end);
        Ok(())
    }

    /// `if` used for its value. Each branch stores into one slot, which is loaded after.
    /// The slot's type is taken from the first branch value (sema leaves the type of
    /// an `if` expression as an unresolved variable in some contexts), and every other
    /// branch must agree. The slot is an `alloca`, which always goes in the entry block.
    fn if_value(&mut self, ife: &IfExpr, span: Span) -> R<Val> {
        if ife.else_body.is_none() {
            return unsupported(span, "an `if` expression without `else`");
        }
        let mut slot: Option<(String, Ty)> = None;
        let end = self.label("ifv.end");
        let mut arms: Vec<(&Expr, IfBranchBody)> = vec![(ife.condition, ife.then_body)];
        for el in ife.elif_branches {
            arms.push((el.condition, el.body));
        }
        for (cond, body) in arms {
            let c = self.expr(cond)?;
            self.need(Ty::I1, &c, cond.span)?;
            let then_l = self.label("ifv.then");
            let else_l = self.label("ifv.else");
            self.term(&format!("br i1 {}, label %{then_l}, label %{else_l}", c.v));
            self.begin(&then_l);
            self.branch_value(body, &mut slot, span)?;
            if !self.terminated {
                self.term(&format!("br label %{end}"));
            }
            self.begin(&else_l);
        }
        if let Some(body) = ife.else_body {
            self.branch_value(body, &mut slot, span)?;
        }
        self.begin(&end);
        match slot {
            Some((slot, ty)) => {
                let out = self.fresh();
                self.inst(&format!("{out} = load {}, ptr {slot}", ty.ir()));
                Ok(Val { v: out, ty })
            }
            None => unsupported(span, "an `if` expression with no value in any branch"),
        }
    }

    fn store_branch_value(&mut self, v: Val, slot: &mut Option<(String, Ty)>, span: Span) -> R<()> {
        self.need_value(&v, span)?;
        let name = match slot {
            Some((name, ty)) => {
                if *ty != v.ty {
                    return Err(CompileError::at(span, "the branches of this `if` expression have different types"));
                }
                name.clone()
            }
            None => {
                let name = self.alloca(v.ty, "ifv");
                *slot = Some((name.clone(), v.ty));
                name
            }
        };
        self.inst(&format!("store {} {}, ptr {name}", v.ty.ir(), v.v));
        Ok(())
    }

    fn branch_value(&mut self, body: IfBranchBody, slot: &mut Option<(String, Ty)>, span: Span) -> R<()> {
        match body {
            IfBranchBody::Expr(e) => {
                let v = self.expr(e)?;
                self.store_branch_value(v, slot, e.span)
            }
            IfBranchBody::Block(b) => {
                self.scopes.push(HashMap::new());
                let n = b.stmts.len();
                if n == 0 {
                    return unsupported(span, "an empty branch in an `if` expression");
                }
                for (i, s) in b.stmts.iter().enumerate() {
                    if i + 1 == n {
                        if let StmtKind::Expr(e) = s.kind {
                            let v = self.expr(e)?;
                            self.store_branch_value(v, slot, e.span)?;
                            continue;
                        }
                        return unsupported(s.span, "a branch of an `if` expression that does not end in a value");
                    }
                    self.stmt(s)?;
                }
                self.scopes.pop();
                Ok(())
            }
        }
    }

    // ── expressions ──────────────────────────────────────────────

    fn expr(&mut self, e: &Expr) -> R<Val> {
        self.check_sema_type(e)?;
        match e.kind {
            ExprKind::Lit(Literal::Int(n)) => {
                // Sema records every integer literal whose context gave it a width other than
                // the default 64-bit `int`. Such a literal must not be lowered as an i64.
                if let Some(&suffix) = self.ctx.int_literal_types.get(&e.span) {
                    let ty = int_suffix_ty(suffix);
                    return Ok(Val { v: int_const(ty, n as u64), ty });
                }
                Ok(Val { v: n.to_string(), ty: Ty::I64 })
            }
            ExprKind::Lit(Literal::TypedInt { raw, suffix }) => {
                let ty = int_suffix_ty(suffix);
                Ok(Val { v: int_const(ty, raw), ty })
            }
            ExprKind::Lit(Literal::Bool(b)) => Ok(Val { v: b.to_string(), ty: Ty::I1 }),
            ExprKind::Lit(Literal::Str(s)) | ExprKind::Lit(Literal::VerbatimStr(s)) => Ok(self.static_str(s)),
            ExprKind::Lit(Literal::InterpolatedStr(parts))
            | ExprKind::Lit(Literal::InterpolatedVerbatimStr(parts)) => self.interp_string(e, parts),
            ExprKind::Lit(Literal::Double(d)) => Ok(Val { v: float_const(d), ty: Ty::F64 }),
            // The lexer already rounded a `float` literal to f32; the interpreter holds that same f32.
            ExprKind::Lit(Literal::Float(f)) => Ok(Val { v: float_const(f as f64), ty: Ty::F32 }),
            ExprKind::Lit(Literal::Null) => Ok(Val { v: "null".to_string(), ty: Ty::Null }),
            ExprKind::Lit(_) => unsupported(e.span, "this literal as a value (only int, float, bool, string and null literals so far)"),
            ExprKind::Ident(name) => {
                let local = match self.lookup(name) {
                    Some(l) => l,
                    None => match self.capture_local(name, e.span)? {
                        Some(l) => l,
                        None => return Err(self.unknown_name(name, e.span, format!("not supported yet: the name `{name}` (only local variables so far)"))),
                    },
                };
                let t = self.fresh();
                self.inst(&format!("{t} = load {}, ptr {}", local.ty.ir(), local.slot));
                Ok(Val { v: t, ty: local.ty })
            }
            ExprKind::As { expr: inner, ty } => {
                let target = self.ty_of_type(ty)?;
                let v = self.expr(inner)?;
                self.cast(&v, target, e.span)
            }
            ExprKind::UnaryOp { op, operand } => {
                let v = self.expr(operand)?;
                let t = self.fresh();
                match op {
                    UnaryOp::Neg if v.ty.is_float() => {
                        self.inst(&format!("{t} = fneg {} {}", v.ty.ir(), v.v));
                        Ok(Val { v: t, ty: v.ty })
                    }
                    UnaryOp::Neg => match v.ty.int_info() {
                        // Wrapping negation, like the interpreter (`-MIN` is `MIN`).
                        Some((_, true)) => {
                            self.inst(&format!("{t} = sub {} 0, {}", v.ty.ir(), v.v));
                            Ok(Val { v: t, ty: v.ty })
                        }
                        Some((_, false)) => unsupported(e.span, "negating an unsigned integer"),
                        None => Err(CompileError::at(
                            operand.span,
                            format!("emitter expected an integer here but the expression is {}", v.ty.ir()),
                        )),
                    },
                    UnaryOp::BitNot => {
                        if v.ty.int_info().is_none() {
                            return Err(CompileError::at(
                                operand.span,
                                format!("emitter expected an integer here but the expression is {}", v.ty.ir()),
                            ));
                        }
                        self.inst(&format!("{t} = xor {} {}, -1", v.ty.ir(), v.v));
                        Ok(Val { v: t, ty: v.ty })
                    }
                    UnaryOp::Not => {
                        self.need(Ty::I1, &v, operand.span)?;
                        self.inst(&format!("{t} = xor i1 {}, true", v.v));
                        Ok(Val { v: t, ty: Ty::I1 })
                    }
                    UnaryOp::Await => unsupported(e.span, "`await`"),
                }
            }
            ExprKind::BinOp { op, lhs, rhs } => match op {
                BinOp::And | BinOp::Or => self.short_circuit(op, lhs, rhs),
                BinOp::Range | BinOp::RangeIncl => unsupported(e.span, "ranges outside `for`"),
                _ => {
                    let l = self.expr(lhs)?;
                    let r = self.expr(rhs)?;
                    self.binop(op, &l, &r, e.span)
                }
            },
            ExprKind::Assign { op, target, value } => {
                // The place being assigned: a local's slot, or the address of a struct field.
                // For a field the object expression is evaluated before the right-hand side.
                let (addr, pty) = match target.kind {
                    ExprKind::Ident(name) => {
                        let local = self
                            .lookup(name)
                            .ok_or_else(|| self.unknown_name(name, target.span, format!("not supported yet: assignment to `{name}` (only local variables and struct fields so far)")))?;
                        (local.slot, local.ty)
                    }
                    ExprKind::Field { target: obj, field } => self.field_addr(obj, field, target.span)?,
                    ExprKind::Index { target: obj, index } => self.list_elem_addr(obj, index, target.span)?,
                    _ => return unsupported(target.span, "assignment to anything but a local variable, a struct field or a list element"),
                };
                let rhs = self.expr_as(value, pty)?;
                if matches!(target.kind, ExprKind::Index { .. }) && self.sema_tags_arena_string(value) {
                    return unsupported(value.span, "storing a value that lives in an arena into a list (the list could outlive the arena)");
                }
                let stored = match assign_binop(op) {
                    None => {
                        self.need(pty, &rhs, value.span)?;
                        rhs
                    }
                    Some(bop) => {
                        let cur = self.fresh();
                        self.inst(&format!("{cur} = load {}, ptr {addr}", pty.ir()));
                        let curv = Val { v: cur, ty: pty };
                        self.binop(bop, &curv, &rhs, e.span)?
                    }
                };
                self.inst(&format!("store {} {}, ptr {addr}", stored.ty.ir(), stored.v));
                Ok(Val::void())
            }
            ExprKind::ShortDecl { name, value } => {
                let v = self.expr(value)?;
                self.need_value(&v, value.span)?;
                let slot = self.declare(name, v.ty);
                self.mark_mutable(name);
                self.inst(&format!("store {} {}, ptr {}", v.ty.ir(), v.v, slot));
                Ok(Val::void())
            }
            ExprKind::Call { callee, args } => self.call(e, callee, args),
            ExprKind::Lambda(lam) => self.lambda(e, lam),
            ExprKind::If(ife) => self.if_value(ife, e.span),
            ExprKind::StructLit { path, fields } => self.struct_lit(e, path, fields),
            // `&x` of a struct is the struct's own pointer: the interpreter's reference to a
            // struct aliases it, and sema has already checked the lifetime.
            ExprKind::Borrow { mutable, place } => {
                if mutable {
                    return unsupported(e.span, "`&mut` borrows");
                }
                let v = self.expr(place)?;
                match v.ty {
                    Ty::Struct(_) => Ok(v),
                    _ => unsupported(e.span, "borrowing anything but a struct value (`&n` of an int)"),
                }
            }
            ExprKind::Deref(_) => {
                unsupported(e.span, "explicit `*` dereference (field access already goes through a reference)")
            }
            ExprKind::Match(m) => self.match_lower(m.scrutinee, m.arms, e.span, true),
            ExprKind::Field { target, field } => {
                // `Enum.Variant` with no payload.
                if let ExprKind::Ident(en) = target.kind {
                    if self.lookup(en).is_none() {
                        if let Some(&eid) = self.enum_ids.get(en) {
                            let vi = self.variant_index(eid, field, e.span)?;
                            return self.enum_ctor(e, eid, vi, &[]);
                        }
                    }
                }
                let (addr, ty) = self.field_addr(target, field, e.span)?;
                let t = self.fresh();
                self.inst(&format!("{t} = load {}, ptr {addr}", ty.ir()));
                Ok(Val { v: t, ty })
            }
            ExprKind::Index { target, index } => {
                let (addr, ty) = self.list_elem_addr(target, index, e.span)?;
                let t = self.fresh();
                self.inst(&format!("{t} = load {}, ptr {addr}", ty.ir()));
                Ok(Val { v: t, ty })
            }
            ExprKind::OrElse { expr: inner, fallback } => self.or_else(e, inner, fallback),
            ref other => unsupported(e.span, expr_kind_name(other)),
        }
    }

    /// Address of `obj.field`: evaluates `obj` (a pointer) and indexes into the struct.
    fn field_addr(&mut self, obj: &Expr, field: &str, span: Span) -> R<(String, Ty)> {
        let o = self.expr(obj)?;
        // A field through an optional struct stops the program when the value is null, like the interpreter
        // ("cannot access field on null", exit status 1).
        let o = match o.ty {
            Ty::Opt(oi) if matches!(self.opt_inner(oi), Ty::Struct(_)) => {
                let inner = self.opt_inner(oi);
                let isnull = self.opt_is_null(&o.v);
                let bad = self.label("nullfield");
                let ok = self.label("fieldok");
                self.term(&format!("br i1 {isnull}, label %{bad}, label %{ok}"));
                self.begin(&bad);
                let (msg, len) = self.string_const(&format!("cannot access field '{field}' on null"));
                self.inst(&format!("call void @ubel_panic(ptr {msg}, i64 {len})"));
                self.term("unreachable");
                self.begin(&ok);
                Val { v: o.v, ty: inner }
            }
            _ => o,
        };
        let sid = match o.ty {
            Ty::Struct(i) => i,
            _ => return unsupported(span, "field access on anything but a struct"),
        };
        let (idx, fty) = self.field_index(sid, field, span)?;
        let name = self.structs[sid].name.clone();
        let g = self.fresh();
        self.inst(&format!("{g} = getelementptr inbounds %struct.{name}, ptr {}, i32 0, i32 {idx}", o.v));
        Ok((g, fty))
    }

    /// `Name { a = .., b = .. }`. The field expressions are evaluated first, in source order
    /// (side effects happen in the order written, as in the interpreter), then the object is
    /// allocated and filled. Where it is allocated follows the tier the literal is written in:
    ///
    /// * inside `with arena`: bump allocation from the innermost open arena (MID);
    /// * in a HIGH function: the GC heap, with a pointer mask from the target's layout;
    /// * in a MID function outside any arena, and in LOW: refused, because the language does
    ///   not say yet where those allocate (MEMORY_MODEL.md, open decision 3).
    fn struct_lit(&mut self, e: &Expr, path: &[&str], inits: &[FieldInit]) -> R<Val> {
        let name = match path {
            [n] => *n,
            _ => return unsupported(e.span, "qualified struct names"),
        };
        let sid = match self.struct_ids.get(name) {
            Some(&i) => i,
            None => return unsupported(e.span, format!("struct literals of `{name}` (only plain structs declared in this file)")),
        };
        let arena = self.alloc_site(e.span, "a struct literal")?;
        if self.structs[sid].fields.iter().any(|f| f.ty.is_ptr()) {
            self.mark_scan(&arena);
        }

        let n = self.structs[sid].fields.len();
        let mut vals: Vec<Option<Val>> = vec![None; n];
        for init in inits {
            let (idx, fty) = self.field_index(sid, init.name, init.span)?;
            if vals[idx].is_some() {
                return Err(CompileError::at(init.span, format!("the field `{}` is given twice", init.name)));
            }
            let v = self.expr_as(init.value, fty)?;
            self.need(fty, &v, init.value.span)?;
            vals[idx] = Some(v);
        }
        for (i, v) in vals.iter().enumerate() {
            if v.is_none() {
                let missing = self.structs[sid].fields[i].name.clone();
                return Err(CompileError::at(e.span, format!("the literal is missing the field `{missing}`")));
            }
        }

        let sname = self.structs[sid].name.clone();
        self.sized.insert(sid);
        let size = self.fresh();
        self.inst(&format!("{size} = call i64 @ubel_sz.{sname}()"));
        let mask = if arena.is_none() && self.structs[sid].fields.iter().any(|f| f.ty.is_ptr()) {
            self.masked.insert(sid);
            let m = self.fresh();
            self.inst(&format!("{m} = call i64 @ubel_mask.{sname}()"));
            m
        } else {
            "0".to_string()
        };
        let p = self.alloc_obj(arena, &size, &mask, &sname);
        for (i, v) in vals.into_iter().enumerate() {
            let v = v.expect("checked above");
            let g = self.fresh();
            self.inst(&format!("{g} = getelementptr inbounds %struct.{sname}, ptr {p}, i32 0, i32 {i}"));
            self.inst(&format!("store {} {}, ptr {g}", v.ty.ir(), v.v));
        }
        Ok(Val { v: p, ty: Ty::Struct(sid) })
    }

    /// Where an object written here is allocated: `Some(arena)` inside `with arena`, `None`
    /// (the GC heap) in a HIGH function, and refused elsewhere, because the language does not
    /// say yet where a MID function outside `with arena`, or LOW code, allocates.
    fn alloc_site(&self, span: Span, what: &str) -> R<Option<String>> {
        if let Some(a) = self.arenas.last() {
            return Ok(Some(a.clone()));
        }
        if self.in_arena_closure.last().copied().unwrap_or(false) && self.cur_tier != TierAnnotation::High {
            return unsupported(
                span,
                format!("{what} inside a closure that was written inside `with arena` (the closure's body does not hold that arena yet, so there is nowhere to allocate it)"),
            );
        }
        match self.cur_tier {
            TierAnnotation::High => Ok(None),
            TierAnnotation::Mid => unsupported(
                span,
                format!("{what} in a `@tier(mid)` function outside `with arena` (where it allocates is not decided yet)"),
            ),
            TierAnnotation::Low => unsupported(span, format!("{what} in `@tier(low)` (how LOW allocates is not decided yet)")),
        }
    }

    /// Allocate `size` bytes: bump allocation from `arena`, or the GC heap with `mask` (bit i set
    /// when the pointer-sized word at byte offset i * sizeof(ptr) holds a pointer).
    fn alloc_obj(&mut self, arena: Option<String>, size: &str, mask: &str, label: &str) -> String {
        let p = self.fresh();
        match arena {
            Some(a) => {
                self.uses_bump = true;
                let up = self.fresh();
                self.inst(&format!("{up} = add i64 {size}, 15"));
                let rounded = self.fresh();
                self.inst(&format!("{rounded} = and i64 {up}, -16"));
                self.inst(&format!("{p} = call ptr @ubel_arena_bump(ptr {a}, i64 {rounded}) ; MID: bump allocation of {label}"));
            }
            None => {
                self.uses_gc = true;
                self.inst(&format!("{p} = call ptr @ubel_gc_alloc(i64 {size}, i64 {mask}) ; HIGH: GC allocation of {label}"));
            }
        }
        p
    }

    fn variant_index(&self, eid: usize, name: &str, span: Span) -> R<usize> {
        let en = &self.enums[eid];
        en.variants
            .iter()
            .position(|v| v.name == name)
            .ok_or_else(|| CompileError::at(span, format!("the enum `{}` has no variant `{name}`", en.name)))
    }

    /// Address of payload slot `slot` of the enum object `obj`.
    fn enum_slot(&mut self, eid: usize, obj: &str, slot: usize) -> String {
        let g = self.fresh();
        let name = self.enums[eid].name.clone();
        self.inst(&format!("{g} = getelementptr inbounds %enum.{name}, ptr {obj}, i32 0, i32 1, i64 {slot}"));
        g
    }

    /// `Enum.Variant(a, b)` and the payload-free `Enum.Variant`. Arguments are evaluated first,
    /// in order; then the object is allocated (same site rules as a struct literal), the tag
    /// stored, and the payload stored slot by slot. A variant without a payload is not allocated at all
    /// (E1, docs/LLVM_POC.md "Enums"): it is a static constant object with the tag and zeroed slots, outside
    /// the heap like a string literal, so it is legal in every tier and the collector ignores pointers to it.
    /// Nothing writes into an enum object after it is built, so sharing one object per variant is safe.
    fn enum_ctor(&mut self, e: &Expr, eid: usize, vi: usize, args: &[Arg]) -> R<Val> {
        let fields = self.enums[eid].variants[vi].fields.clone();
        if args.len() != fields.len() {
            let v = &self.enums[eid].variants[vi].name;
            return Err(CompileError::at(
                e.span,
                format!("the variant `{v}` takes {} value(s), found {}", fields.len(), args.len()),
            ));
        }
        if fields.is_empty() {
            let ename = self.enums[eid].name.clone();
            let vname = self.enums[eid].variants[vi].name.clone();
            let slots = self.enums[eid].slots;
            let name = format!("@.enumv.{ename}.{vname}");
            if self.estatics.insert((eid, vi)) {
                let init = if slots == 0 {
                    format!("{{ i64 {vi} }}")
                } else {
                    format!("{{ i64 {vi}, [{slots} x i64] zeroinitializer }}")
                };
                let _ = writeln!(self.estatic_ir, "{name} = private constant %enum.{ename} {init}, align 8");
            }
            return Ok(Val { v: name, ty: Ty::Enum(eid) });
        }
        let arena = self.alloc_site(e.span, "an enum value")?;
        if fields.iter().any(|f| f.is_ptr()) {
            self.mark_scan(&arena);
        }
        let mut vals = Vec::new();
        for (i, a) in args.iter().enumerate() {
            let ae = match a.kind {
                ArgKind::Positional(x) => x,
                ArgKind::Named { .. } => return unsupported(a.span, "named arguments"),
            };
            let v = self.expr_as(ae, fields[i])?;
            self.need(fields[i], &v, ae.span)?;
            vals.push(v);
        }
        let ename = self.enums[eid].name.clone();
        let vname = self.enums[eid].variants[vi].name.clone();
        self.esized.insert(eid);
        let size = self.fresh();
        self.inst(&format!("{size} = call i64 @ubel_esz.{ename}()"));
        let mask = if arena.is_none() && fields.iter().any(|f| f.is_ptr()) {
            self.emasked.insert((eid, vi));
            let m = self.fresh();
            self.inst(&format!("{m} = call i64 @ubel_emask.{ename}.{vname}()"));
            m
        } else {
            "0".to_string()
        };
        let label = format!("{ename}.{vname}");
        let p = self.alloc_obj(arena, &size, &mask, &label);
        let tagp = self.fresh();
        self.inst(&format!("{tagp} = getelementptr inbounds %enum.{ename}, ptr {p}, i32 0, i32 0"));
        self.inst(&format!("store i64 {vi}, ptr {tagp}"));
        for (slot, v) in vals.into_iter().enumerate() {
            let g = self.enum_slot(eid, &p, slot);
            self.inst(&format!("store {} {}, ptr {g}", v.ty.ir(), v.v));
        }
        Ok(Val { v: p, ty: Ty::Enum(eid) })
    }

    /// `match scrutinee { arms }` on an enum value, as a statement or (`want_value`) as an
    /// expression. Lowered to a `switch` on the tag. Patterns supported: `Enum.Variant(a, _)`,
    /// a bare payload-free variant name, `_`, and a plain name that binds the whole value;
    /// everything else is refused, including guards and nested patterns. Exhaustiveness is
    /// sema's job; a tag no arm covers traps instead of being undefined behaviour.
    fn match_lower(&mut self, scrutinee: &Expr, arms: &[MatchArm], span: Span, want_value: bool) -> R<Val> {
        let sv = self.expr(scrutinee)?;
        let eid = match sv.ty {
            Ty::Enum(i) => i,
            Ty::Str => return self.match_str(sv, arms, span, want_value),
            _ => return unsupported(scrutinee.span, "`match` on anything but an enum or a string value"),
        };
        let en = self.enums[eid].clone();

        // Resolve every pattern before emitting any IR, so refusals come first. Each entry:
        // (variant or None for the catch-all, payload bindings, whole-value binding, arm index).
        type Lowered = (Option<usize>, Vec<Option<String>>, Option<String>, usize);
        let mut lowered: Vec<Lowered> = Vec::new();
        let mut seen = vec![false; en.variants.len()];
        let mut has_default = false;
        for (ai, arm) in arms.iter().enumerate() {
            if has_default {
                break; // nothing after a catch-all can run
            }
            if arm.guard.is_some() {
                return unsupported(arm.span, "`where` guards in `match` arms");
            }
            match arm.pattern.kind {
                PatternKind::Wildcard => {
                    lowered.push((None, Vec::new(), None, ai));
                    has_default = true;
                }
                PatternKind::Ident { name, mutable } => {
                    if mutable {
                        return unsupported(arm.pattern.span, "`mut` bindings in patterns");
                    }
                    match en.variants.iter().position(|v| v.name == name && v.fields.is_empty()) {
                        Some(vi) => {
                            if !seen[vi] {
                                seen[vi] = true;
                                lowered.push((Some(vi), Vec::new(), None, ai));
                            }
                        }
                        None => {
                            lowered.push((None, Vec::new(), Some(name.to_string()), ai));
                            has_default = true;
                        }
                    }
                }
                PatternKind::Enum { path, payload } => {
                    let vname = match path {
                        [v] => *v,
                        [en_name, v] if *en_name == en.name => *v,
                        _ => return unsupported(arm.pattern.span, "a pattern naming a different enum than the one matched"),
                    };
                    let vi = self.variant_index(eid, vname, arm.pattern.span)?;
                    let fields = &en.variants[vi].fields;
                    let binds: Vec<Option<String>> = match payload {
                        EnumPatternPayload::None => {
                            if !fields.is_empty() {
                                return Err(CompileError::at(arm.pattern.span, format!("the variant `{vname}` has a payload, the pattern has none")));
                            }
                            Vec::new()
                        }
                        EnumPatternPayload::Tuple(pats) => {
                            if pats.len() != fields.len() {
                                return Err(CompileError::at(arm.pattern.span, format!("the variant `{vname}` has {} payload value(s), the pattern has {}", fields.len(), pats.len())));
                            }
                            let mut out = Vec::new();
                            for (k, sp) in pats.iter().enumerate() {
                                match sp.kind {
                                    PatternKind::Wildcard => out.push(None),
                                    PatternKind::Ident { name, mutable: false } => {
                                        // A name that is a payload-free variant of the payload's own
                                        // enum would be a nested pattern, not a binding.
                                        if let Ty::Enum(inner) = fields[k] {
                                            if self.enums[inner].variants.iter().any(|v| v.name == name && v.fields.is_empty()) {
                                                return unsupported(sp.span, "nested patterns (a variant name inside a payload)");
                                            }
                                        }
                                        out.push(Some(name.to_string()));
                                    }
                                    _ => return unsupported(sp.span, "nested patterns (only names and `_` inside a payload so far)"),
                                }
                            }
                            out
                        }
                        EnumPatternPayload::Struct(_) => return unsupported(arm.pattern.span, "struct-style enum patterns"),
                    };
                    if !seen[vi] {
                        seen[vi] = true;
                        lowered.push((Some(vi), binds, None, ai));
                    }
                }
                _ => {
                    return unsupported(
                        arm.pattern.span,
                        "this kind of pattern in a `match` (only enum variants, `_` and names so far)",
                    )
                }
            }
        }

        let ename = en.name.clone();
        let tagp = self.fresh();
        self.inst(&format!("{tagp} = getelementptr inbounds %enum.{ename}, ptr {}, i32 0, i32 0", sv.v));
        let tag = self.fresh();
        self.inst(&format!("{tag} = load i64, ptr {tagp}"));
        let end = self.label("match.end");
        let default_l = self.label("match.default");
        let mut arm_labels: Vec<String> = Vec::new();
        let mut cases = Vec::new();
        for (variant, _, _, _) in &lowered {
            let l = match variant {
                Some(_) => self.label("match.arm"),
                None => default_l.clone(),
            };
            if let Some(vi) = variant {
                cases.push(format!("i64 {vi}, label %{l}"));
            }
            arm_labels.push(l);
        }
        self.term(&format!("switch i64 {tag}, label %{default_l} [ {} ]", cases.join(" ")));

        let mut slot: Option<(String, Ty)> = None;
        for (idx, (variant, binds, whole, ai)) in lowered.iter().enumerate() {
            self.begin(&arm_labels[idx]);
            self.scopes.push(HashMap::new());
            if let Some(vi) = variant {
                for (k, b) in binds.iter().enumerate() {
                    if let Some(name) = b {
                        let fty = en.variants[*vi].fields[k];
                        let g = self.enum_slot(eid, &sv.v, k);
                        let t = self.fresh();
                        self.inst(&format!("{t} = load {}, ptr {g}", fty.ir()));
                        let local = self.declare(name, fty);
                        self.inst(&format!("store {} {t}, ptr {local}", fty.ir()));
                    }
                }
            }
            if let Some(name) = whole {
                let local = self.declare(name, Ty::Enum(eid));
                self.inst(&format!("store ptr {}, ptr {local}", sv.v));
            }
            let produced = self.match_arm_body(arms[*ai].body, want_value, &mut slot, arms[*ai].span)?;
            self.scopes.pop();
            if !self.terminated {
                if want_value && !produced {
                    return Err(CompileError::at(arms[*ai].span, "this `match` arm has no value, but the `match` is used as one"));
                }
                self.term(&format!("br label %{end}"));
            }
        }
        if !has_default {
            self.begin(&default_l);
            self.inst("call void @ubel_trap_no_return()");
            self.term("unreachable");
        }
        self.begin(&end);
        match slot {
            Some((slot, ty)) if want_value => {
                let out = self.fresh();
                self.inst(&format!("{out} = load {}, ptr {slot}", ty.ir()));
                Ok(Val { v: out, ty })
            }
            // Every arm left the function or the loop (or this is a statement): no value.
            _ => {
                let _ = span;
                Ok(Val::void())
            }
        }
    }

    /// `match` on a string (A9b, docs/LLVM_POC.md "String match"). Patterns: a string literal, `a | b` of
    /// string literals, `_`, and a plain name that binds the string (a catch-all). Lowered to a chain of
    /// `ubel_str_eq` tests in arm order, so the first matching arm wins, like the interpreter; the scrutinee is
    /// evaluated once. Refused: guards, `mut` bindings, and any other pattern. The interpreter gives no value
    /// when no arm matches (it does not fail), which native code cannot copy for a value of a known type, so a
    /// `match` used as a value must end in a catch-all; used as a statement without one, nothing happens when
    /// no arm matches, as in the interpreter. Sema does not ask for a catch-all on a string `match`.
    fn match_str(&mut self, sv: Val, arms: &[MatchArm], span: Span, want_value: bool) -> R<Val> {
        // Resolve every pattern before emitting IR, so refusals come first. Each entry: (the literals to
        // test, empty for the catch-all; the name that binds the whole string; arm index).
        let mut lowered: Vec<(Vec<String>, Option<String>, usize)> = Vec::new();
        let mut has_default = false;
        for (ai, arm) in arms.iter().enumerate() {
            if has_default {
                break; // nothing after a catch-all can run
            }
            if arm.guard.is_some() {
                return unsupported(arm.span, "`where` guards in `match` arms");
            }
            match arm.pattern.kind {
                PatternKind::Wildcard => {
                    lowered.push((Vec::new(), None, ai));
                    has_default = true;
                }
                PatternKind::Ident { name, mutable } => {
                    if mutable {
                        return unsupported(arm.pattern.span, "`mut` bindings in patterns");
                    }
                    lowered.push((Vec::new(), Some(name.to_string()), ai));
                    has_default = true;
                }
                PatternKind::Literal(lit) => {
                    lowered.push((vec![Self::str_pattern(lit, arm.pattern.span)?], None, ai));
                }
                PatternKind::Or(alts) => {
                    let mut lits = Vec::new();
                    for alt in alts {
                        match alt.kind {
                            PatternKind::Literal(lit) => lits.push(Self::str_pattern(lit, alt.span)?),
                            _ => return unsupported(alt.span, "anything but string literals inside `|` in a `match` on a string"),
                        }
                    }
                    lowered.push((lits, None, ai));
                }
                _ => {
                    return unsupported(
                        arm.pattern.span,
                        "this kind of pattern in a `match` on a string (only string literals, `|`, `_` and names so far)",
                    )
                }
            }
        }
        if want_value && !has_default {
            return unsupported(
                span,
                "a `match` on a string used as a value without a `_` or name arm (the interpreter gives no value when no arm matches)",
            );
        }

        self.uses_str = true;
        let end = self.label("match.end");
        let mut arm_labels: Vec<String> = Vec::new();
        for _ in &lowered {
            let l = self.label("match.arm");
            arm_labels.push(l);
        }
        for (idx, (lits, _, _)) in lowered.iter().enumerate() {
            for lit in lits {
                let p = self.static_str(lit);
                let c = self.fresh();
                self.inst(&format!("{c} = call i32 @ubel_str_eq(ptr {}, ptr {})", sv.v, p.v));
                let t = self.fresh();
                self.inst(&format!("{t} = icmp ne i32 {c}, 0"));
                let next = self.label("match.next");
                self.term(&format!("br i1 {t}, label %{}, label %{next}", arm_labels[idx]));
                self.begin(&next);
            }
        }
        // No literal matched: the catch-all (always the last arm lowered), or the end of a statement.
        match lowered.iter().position(|(lits, _, _)| lits.is_empty()) {
            Some(di) => self.term(&format!("br label %{}", arm_labels[di])),
            None => self.term(&format!("br label %{end}")),
        }

        let mut slot: Option<(String, Ty)> = None;
        for (idx, (_, whole, ai)) in lowered.iter().enumerate() {
            self.begin(&arm_labels[idx]);
            self.scopes.push(HashMap::new());
            if let Some(name) = whole {
                let local = self.declare(name, Ty::Str);
                self.inst(&format!("store ptr {}, ptr {local}", sv.v));
            }
            let produced = self.match_arm_body(arms[*ai].body, want_value, &mut slot, arms[*ai].span)?;
            self.scopes.pop();
            if !self.terminated {
                if want_value && !produced {
                    return Err(CompileError::at(arms[*ai].span, "this `match` arm has no value, but the `match` is used as one"));
                }
                self.term(&format!("br label %{end}"));
            }
        }
        self.begin(&end);
        match slot {
            Some((slot, ty)) if want_value => {
                let out = self.fresh();
                self.inst(&format!("{out} = load {}, ptr {slot}", ty.ir()));
                Ok(Val { v: out, ty })
            }
            _ => Ok(Val::void()),
        }
    }

    /// The text of a pattern literal in a `match` on a string; any other literal is refused.
    fn str_pattern(lit: Literal, span: Span) -> R<String> {
        match lit {
            Literal::Str(s) | Literal::VerbatimStr(s) => Ok(s.to_string()),
            _ => unsupported(span, "a pattern that is not a plain string literal in a `match` on a string"),
        }
    }

    /// Lower one arm body. Returns whether it stored a value for the `match` expression.
    fn match_arm_body(&mut self, body: MatchArmBody, want_value: bool, slot: &mut Option<(String, Ty)>, span: Span) -> R<bool> {
        match body {
            MatchArmBody::Expr(e) => {
                let v = self.expr(e)?;
                if want_value && !self.terminated {
                    self.store_branch_value(v, slot, e.span)?;
                    return Ok(true);
                }
                Ok(false)
            }
            MatchArmBody::Block(b) => {
                self.scopes.push(HashMap::new());
                let n = b.stmts.len();
                let mut produced = false;
                for (i, st) in b.stmts.iter().enumerate() {
                    if want_value && i + 1 == n {
                        if let StmtKind::Expr(e) = st.kind {
                            let v = self.expr(e)?;
                            if !self.terminated {
                                self.store_branch_value(v, slot, e.span)?;
                                produced = true;
                            }
                            continue;
                        }
                    }
                    self.stmt(st)?;
                }
                self.scopes.pop();
                let _ = span;
                Ok(produced)
            }
        }
    }

    fn short_circuit(&mut self, op: BinOp, lhs: &Expr, rhs: &Expr) -> R<Val> {
        let slot = self.alloca(Ty::I1, "sc");
        let l = self.expr(lhs)?;
        self.need(Ty::I1, &l, lhs.span)?;
        self.inst(&format!("store i1 {}, ptr {slot}", l.v));
        let rhs_l = self.label("sc.rhs");
        let end_l = self.label("sc.end");
        if op == BinOp::And {
            self.term(&format!("br i1 {}, label %{rhs_l}, label %{end_l}", l.v));
        } else {
            self.term(&format!("br i1 {}, label %{end_l}, label %{rhs_l}", l.v));
        }
        self.begin(&rhs_l);
        let r = self.expr(rhs)?;
        self.need(Ty::I1, &r, rhs.span)?;
        self.inst(&format!("store i1 {}, ptr {slot}", r.v));
        self.term(&format!("br label %{end_l}"));
        self.begin(&end_l);
        let out = self.fresh();
        self.inst(&format!("{out} = load i1, ptr {slot}"));
        Ok(Val { v: out, ty: Ty::I1 })
    }

    /// `+ - * / %` and the comparisons on `double` and `float` (M2b, docs/LLVM_POC.md "Floating point"). Both
    /// operands have the same type (sema allows no mixing). The interpreter computes in f64 and rounds a `float`
    /// result to f32, which is the correctly rounded f32 result for these operations (f64 has more than twice the
    /// bits), so plain f32 instructions agree. A zero divisor, `-0.0` too, stops the program like the interpreter
    /// (`1.0 / 0.0` is a panic there, not infinity); a NaN divisor does not. `==` is false for NaN and `!=` true
    /// (IEEE), `<` and the like are false when either side is NaN.
    fn float_binop(&mut self, op: BinOp, l: &Val, r: &Val, span: Span) -> R<Val> {
        let ty = l.ty;
        let ir = ty.ir();
        let t = self.fresh();
        match op {
            BinOp::Add | BinOp::Sub | BinOp::Mul => {
                let name = match op {
                    BinOp::Add => "fadd",
                    BinOp::Sub => "fsub",
                    _ => "fmul",
                };
                self.inst(&format!("{t} = {name} {ir} {}, {}", l.v, r.v));
                Ok(Val { v: t, ty })
            }
            BinOp::Div | BinOp::Rem => {
                let zero = self.fresh();
                self.inst(&format!("{zero} = fcmp oeq {ir} {}, 0.0", r.v));
                let bad = self.label("divz");
                let ok = self.label("divok");
                self.term(&format!("br i1 {zero}, label %{bad}, label %{ok}"));
                self.begin(&bad);
                self.inst("call void @ubel_trap_div_zero()");
                self.term("unreachable");
                self.begin(&ok);
                let name = if op == BinOp::Div { "fdiv" } else { "frem" };
                self.inst(&format!("{t} = {name} {ir} {}, {}", l.v, r.v));
                Ok(Val { v: t, ty })
            }
            BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
                let pred = match op {
                    BinOp::Eq => "oeq",
                    BinOp::Ne => "une",
                    BinOp::Lt => "olt",
                    BinOp::Le => "ole",
                    BinOp::Gt => "ogt",
                    _ => "oge",
                };
                self.inst(&format!("{t} = fcmp {pred} {ir} {}, {}", l.v, r.v));
                Ok(Val { v: t, ty: Ty::I1 })
            }
            _ => unsupported(span, "this operator on floating-point values (only `+ - * / %` and the comparisons)"),
        }
    }

    /// Conversions with a float on either side, with Rust's `as` rules (which the interpreter uses): an integer
    /// becomes the nearest float (`sitofp` for a signed source, `uitofp` for an unsigned one), `double` and
    /// `float` convert by rounding or widening, and a float becomes an integer by SATURATING (too large gives the
    /// type's maximum, too small its minimum, NaN gives 0), which is `llvm.fptosi.sat` and `llvm.fptoui.sat`.
    /// Bool and string conversions are not supported (the interpreter panics for bool to float).
    fn float_cast(&mut self, v: &Val, to: Ty, span: Span) -> R<Val> {
        let t = self.fresh();
        match (v.ty.is_float(), to.is_float()) {
            (true, true) => {
                let op = if to == Ty::F64 { "fpext" } else { "fptrunc" };
                self.inst(&format!("{t} = {op} {} {} to {}", v.ty.ir(), v.v, to.ir()));
            }
            (false, true) => {
                let Some((_, signed)) = v.ty.int_info() else {
                    return unsupported(span, "this conversion to a float (only integers convert)");
                };
                let op = if signed { "sitofp" } else { "uitofp" };
                self.inst(&format!("{t} = {op} {} {} to {}", v.ty.ir(), v.v, to.ir()));
            }
            (true, false) => {
                let Some((_, signed)) = to.int_info() else {
                    return unsupported(span, "this conversion from a float (only to integers)");
                };
                let src = if v.ty == Ty::F64 { "f64" } else { "f32" };
                let name = format!("llvm.fpto{}i.sat.{}.{src}", if signed { "s" } else { "u" }, to.ir());
                self.declare_intrinsic(format!("declare {} @{name}({})", to.ir(), v.ty.ir()));
                self.inst(&format!("{t} = call {} @{name}({} {})", to.ir(), v.ty.ir(), v.v));
            }
            (false, false) => unreachable!("float_cast is only called with a float on one side"),
        }
        Ok(Val { v: t, ty: to })
    }

    // ?? lists (M3a) ??????????????????????????????????????????????

    /// Evaluates `e` for a place that wants a value of type `want`. Only `List.new()` looks at it: its element type
    /// comes from the annotation, parameter, field, element or result type it is stored in.
    fn expr_as(&mut self, e: &Expr, want: Ty) -> R<Val> {
        // A place of optional type takes `null`, a value of the inner type (wrapped) or an optional as it is.
        if let Ty::Opt(oi) = want {
            let inner = self.opt_inner(oi);
            let v = self.expr_as(e, inner)?;
            if v.ty == want {
                return Ok(v);
            }
            if v.ty == Ty::Null {
                return Ok(Val { v: v.v, ty: want });
            }
            if v.ty == inner {
                return self.opt_wrap(v, want, e.span);
            }
            return Ok(v); // the caller's `need` reports the mismatch
        }
        if let (Ty::List(_), ExprKind::Call { args, .. }) = (want, e.kind) {
            if self.is_list_new_call(e) {
                return self.list_new(e, args, Some(want));
            }
        }
        self.expr(e)
    }

    /// Is `e` the call `List.new(..)` (and not a call of something the program itself named `List`)?
    fn is_list_new_call(&self, e: &Expr) -> bool {
        let ExprKind::Call { callee, .. } = e.kind else { return false };
        let ExprKind::Field { target, field: "new" } = callee.kind else { return false };
        let ExprKind::Ident("List") = target.kind else { return false };
        self.lookup("List").is_none() && !self.struct_ids.contains_key("List") && !self.enum_ids.contains_key("List")
    }

    // ── optionals and the rest of the list methods (M3b) ──────────

    /// Intern an optional type: equal inner types give equal `Ty::Opt` indexes.
    fn opt_ty(&self, inner: Ty) -> Ty {
        let mut tys = self.opt_tys.borrow_mut();
        if let Some(i) = tys.iter().position(|t| *t == inner) {
            return Ty::Opt(i);
        }
        tys.push(inner);
        Ty::Opt(tys.len() - 1)
    }

    fn opt_inner(&self, idx: usize) -> Ty {
        self.opt_tys.borrow()[idx]
    }

    /// What a `T?` may wrap so far: numbers, bool, strings, structs, enums and lists (not closures, not `void`, not
    /// another optional).
    fn opt_inner_ok(inner: Ty) -> bool {
        matches!(inner, Ty::I1 | Ty::I64 | Ty::Int(_) | Ty::F64 | Ty::F32 | Ty::Str | Ty::Struct(_) | Ty::Enum(_) | Ty::List(_))
    }

    /// Boxing a number or bool for an optional allocates in the GC heap: LOW must not, and MID code outside
    /// `with arena` has no decided place to allocate (the same rule as for lists).
    fn box_alloc_allowed(&self, span: Span, what: &str) -> R<()> {
        let in_arena = !self.arenas.is_empty() || self.in_arena_closure.last().copied().unwrap_or(false);
        match self.cur_tier {
            TierAnnotation::Low => unsupported(
                span,
                format!("{what} in `@tier(low)` (its value is boxed in the GC heap, which LOW must not use)"),
            ),
            TierAnnotation::Mid if !in_arena => unsupported(
                span,
                format!("{what} in a `@tier(mid)` function outside `with arena` (where it allocates is not decided yet)"),
            ),
            _ => Ok(()),
        }
    }

    /// `p == null` as an i1.
    fn opt_is_null(&mut self, p: &str) -> String {
        let t = self.fresh();
        self.inst(&format!("{t} = icmp eq ptr {p}, null"));
        t
    }

    /// The value inside an optional that is known not to be null.
    fn opt_load(&mut self, v: &Val, inner: Ty) -> Val {
        if inner.is_ptr() {
            return Val { v: v.v.clone(), ty: inner };
        }
        let t = self.fresh();
        self.inst(&format!("{t} = load {}, ptr {}", inner.ir(), v.v));
        Val { v: t, ty: inner }
    }

    /// A value of the inner type as an optional: the pointer itself when the value is a pointer, otherwise a GC box.
    fn opt_wrap(&mut self, v: Val, opt: Ty, span: Span) -> R<Val> {
        if v.ty.is_ptr() {
            return Ok(Val { v: v.v, ty: opt });
        }
        self.box_alloc_allowed(span, "an optional number or bool")?;
        let size = format!("ptrtoint (ptr getelementptr ({}, ptr null, i32 1) to i64)", v.ty.ir());
        let p = self.alloc_obj(None, &size, "0", "an optional's value");
        self.inst(&format!("store {} {}, ptr {p}", v.ty.ir(), v.v));
        Ok(Val { v: p, ty: opt })
    }

    /// `==` and `!=` where an operand is an optional or `null`: against `null`, against another optional of the same
    /// type (equal when both are null, or both hold equal values) and against a plain value of the inner type (equal
    /// when it is not null and holds that value), like the interpreter's `Value` equality.
    fn opt_binop(&mut self, op: BinOp, l: &Val, r: &Val, span: Span) -> R<Val> {
        if !matches!(op, BinOp::Eq | BinOp::Ne) {
            return unsupported(
                span,
                "this operator on an optional value (only `==` and `!=`; unwrap it first with `or return`, `or break` or `or continue`)",
            );
        }
        let eq = match (l.ty, r.ty) {
            (Ty::Null, Ty::Null) => "true".to_string(),
            (Ty::Opt(_), Ty::Null) => self.opt_is_null(&l.v),
            (Ty::Null, Ty::Opt(_)) => self.opt_is_null(&r.v),
            (Ty::Opt(a), Ty::Opt(b)) => {
                if a != b {
                    return Err(CompileError::at(span, "the operands of this operator have different types"));
                }
                let inner = self.opt_inner(a);
                let res = self.alloca(Ty::I1, "oeq");
                let ln = self.opt_is_null(&l.v);
                let rn = self.opt_is_null(&r.v);
                let either = self.fresh();
                self.inst(&format!("{either} = or i1 {ln}, {rn}"));
                let both = self.fresh();
                self.inst(&format!("{both} = and i1 {ln}, {rn}"));
                let edge = self.label("oeq.edge");
                let val = self.label("oeq.val");
                let end = self.label("oeq.end");
                self.term(&format!("br i1 {either}, label %{edge}, label %{val}"));
                self.begin(&edge);
                self.inst(&format!("store i1 {both}, ptr {res}"));
                self.term(&format!("br label %{end}"));
                self.begin(&val);
                let lu = self.opt_load(l, inner);
                let ru = self.opt_load(r, inner);
                let c = self.binop(BinOp::Eq, &lu, &ru, span)?;
                self.inst(&format!("store i1 {}, ptr {res}", c.v));
                self.term(&format!("br label %{end}"));
                self.begin(&end);
                let t = self.fresh();
                self.inst(&format!("{t} = load i1, ptr {res}"));
                t
            }
            (Ty::Opt(a), _) => self.opt_eq_value(l, a, r, span)?,
            (_, Ty::Opt(a)) => self.opt_eq_value(r, a, l, span)?,
            _ => return unsupported(span, "`null` compared with a value that is not an optional"),
        };
        if op == BinOp::Eq {
            return Ok(Val { v: eq, ty: Ty::I1 });
        }
        let t = self.fresh();
        self.inst(&format!("{t} = xor i1 {eq}, true"));
        Ok(Val { v: t, ty: Ty::I1 })
    }

    /// An optional against a plain value: false when the optional is null, otherwise the values compared.
    fn opt_eq_value(&mut self, opt: &Val, oi: usize, other: &Val, span: Span) -> R<String> {
        let inner = self.opt_inner(oi);
        if other.ty != inner {
            return Err(CompileError::at(span, "the operands of this operator have different types"));
        }
        let res = self.alloca(Ty::I1, "oev");
        let isnull = self.opt_is_null(&opt.v);
        let nl = self.label("oev.null");
        let vl = self.label("oev.val");
        let end = self.label("oev.end");
        self.term(&format!("br i1 {isnull}, label %{nl}, label %{vl}"));
        self.begin(&nl);
        self.inst(&format!("store i1 false, ptr {res}"));
        self.term(&format!("br label %{end}"));
        self.begin(&vl);
        let u = self.opt_load(opt, inner);
        let c = self.binop(BinOp::Eq, &u, other, span)?;
        self.inst(&format!("store i1 {}, ptr {res}", c.v));
        self.term(&format!("br label %{end}"));
        self.begin(&end);
        let t = self.fresh();
        self.inst(&format!("{t} = load i1, ptr {res}"));
        Ok(t)
    }

    /// `x or return`, `x or return v`, `x or break`, `x or continue` on an optional: when it is null the fallback
    /// leaves; otherwise the value is the optional's contents.
    fn or_else(&mut self, e: &Expr, inner: &Expr, fallback: OrElseFallback) -> R<Val> {
        let v = self.expr(inner)?;
        let Ty::Opt(oi) = v.ty else {
            return unsupported(e.span, "`or` with a fallback on anything but an optional value");
        };
        let payload = self.opt_inner(oi);
        let isnull = self.opt_is_null(&v.v);
        let fb = self.label("or.fallback");
        let ok = self.label("or.ok");
        self.term(&format!("br i1 {isnull}, label %{fb}, label %{ok}"));
        self.begin(&fb);
        match fallback {
            OrElseFallback::Return(val) => {
                let rv = match val {
                    Some(x) => Some(if self.ret_infer { self.expr(x)? } else { self.expr_as(x, self.ret)? }),
                    None => None,
                };
                self.do_return(rv, e.span)?;
            }
            OrElseFallback::Break => {
                let ctx = self.loops.last().cloned().ok_or_else(|| CompileError::at(e.span, "`break` outside a loop"))?;
                self.destroy_arenas_from(ctx.arena_depth);
                self.term(&format!("br label %{}", ctx.brk));
            }
            OrElseFallback::Continue => {
                let ctx = self.loops.last().cloned().ok_or_else(|| CompileError::at(e.span, "`continue` outside a loop"))?;
                self.destroy_arenas_from(ctx.arena_depth);
                self.term(&format!("br label %{}", ctx.cont));
            }
            OrElseFallback::Expr(_) => return unsupported(e.span, "`or` with a default expression"),
        }
        self.begin(&ok);
        Ok(self.opt_load(&v, payload))
    }

    /// `xs.pop()`, `xs.first()` and `xs.last()`: the element as an optional, null for an empty list (sema types
    /// them `T?`). `pop` also removes the element, and clears the vacated slot when it holds a pointer so the
    /// collector does not keep the object alive.
    fn list_take(&mut self, e: &Expr, callee: &Expr, list: Val, elem: Ty, field: &str, args: &[Arg]) -> R<Val> {
        if !args.is_empty() {
            return Err(CompileError::at(e.span, "wrong number of arguments"));
        }
        if !Self::opt_inner_ok(elem) {
            return unsupported(callee.span, format!("`{field}` on a list of this element type"));
        }
        if !elem.is_ptr() {
            self.box_alloc_allowed(callee.span, &format!("`{field}` on a list of numbers or bools (the optional result is a GC box)"))?;
        }
        let opt = self.opt_ty(elem);
        self.uses_list = true;
        let lenp = self.fresh();
        self.inst(&format!("{lenp} = getelementptr inbounds %ubel.list, ptr {}, i32 0, i32 0", list.v));
        let len = self.fresh();
        self.inst(&format!("{len} = load i64, ptr {lenp}"));
        let res = self.alloca(opt, "take");
        let empty = self.fresh();
        self.inst(&format!("{empty} = icmp eq i64 {len}, 0"));
        let none_l = self.label("take.none");
        let some_l = self.label("take.some");
        let end_l = self.label("take.end");
        self.term(&format!("br i1 {empty}, label %{none_l}, label %{some_l}"));
        self.begin(&none_l);
        self.inst(&format!("store ptr null, ptr {res}"));
        self.term(&format!("br label %{end_l}"));
        self.begin(&some_l);
        let idx = if field == "first" {
            "0".to_string()
        } else {
            let last = self.fresh();
            self.inst(&format!("{last} = sub i64 {len}, 1"));
            last
        };
        let datap = self.fresh();
        self.inst(&format!("{datap} = getelementptr inbounds %ubel.list, ptr {}, i32 0, i32 2", list.v));
        let data = self.fresh();
        self.inst(&format!("{data} = load ptr, ptr {datap}"));
        let addr = self.fresh();
        self.inst(&format!("{addr} = getelementptr {}, ptr {data}, i64 {idx}", elem.ir()));
        let x = self.fresh();
        self.inst(&format!("{x} = load {}, ptr {addr}", elem.ir()));
        if field == "pop" {
            self.inst(&format!("store i64 {idx}, ptr {lenp}"));
            if elem.is_ptr() {
                self.inst(&format!("store ptr null, ptr {addr}"));
            }
        }
        let ov = self.opt_wrap(Val { v: x, ty: elem }, opt, callee.span)?;
        self.inst(&format!("store ptr {}, ptr {res}", ov.v));
        self.term(&format!("br label %{end_l}"));
        self.begin(&end_l);
        let t = self.fresh();
        self.inst(&format!("{t} = load ptr, ptr {res}"));
        Ok(Val { v: t, ty: opt })
    }

    /// `xs.contains(v)` for numbers, bool and strings: a scan with the same `==` the language uses on them. Structs,
    /// enums and lists are refused (the interpreter's equality for those depends on `@derive` and user operators).
    fn list_contains(&mut self, e: &Expr, list: Val, elem: Ty, args: &[Arg]) -> R<Val> {
        if args.len() != 1 {
            return Err(CompileError::at(e.span, "wrong number of arguments"));
        }
        let ae = match args[0].kind {
            ArgKind::Positional(x) => x,
            ArgKind::Named { .. } => return unsupported(args[0].span, "named arguments"),
        };
        if elem.is_ptr() && elem != Ty::Str {
            return unsupported(e.span, "`contains` on a list of structs, enums or lists (their equality is not lowered yet)");
        }
        let v = self.expr_as(ae, elem)?;
        self.need_value(&v, ae.span)?;
        self.need(elem, &v, ae.span)?;
        self.uses_list = true;
        let lenp = self.fresh();
        self.inst(&format!("{lenp} = getelementptr inbounds %ubel.list, ptr {}, i32 0, i32 0", list.v));
        let len = self.fresh();
        self.inst(&format!("{len} = load i64, ptr {lenp}"));
        let datap = self.fresh();
        self.inst(&format!("{datap} = getelementptr inbounds %ubel.list, ptr {}, i32 0, i32 2", list.v));
        let data = self.fresh();
        self.inst(&format!("{data} = load ptr, ptr {datap}"));
        let res = self.alloca(Ty::I1, "has");
        self.inst(&format!("store i1 false, ptr {res}"));
        let idx = self.alloca(Ty::I64, "has.i");
        self.inst(&format!("store i64 0, ptr {idx}"));
        let cond = self.label("has.cond");
        let body = self.label("has.body");
        let found = self.label("has.found");
        let next = self.label("has.next");
        let done = self.label("has.done");
        self.term(&format!("br label %{cond}"));
        self.begin(&cond);
        let i = self.fresh();
        self.inst(&format!("{i} = load i64, ptr {idx}"));
        let c = self.fresh();
        self.inst(&format!("{c} = icmp slt i64 {i}, {len}"));
        self.term(&format!("br i1 {c}, label %{body}, label %{done}"));
        self.begin(&body);
        let addr = self.fresh();
        self.inst(&format!("{addr} = getelementptr {}, ptr {data}, i64 {i}", elem.ir()));
        let x = self.fresh();
        self.inst(&format!("{x} = load {}, ptr {addr}", elem.ir()));
        let same = self.binop(BinOp::Eq, &Val { v: x, ty: elem }, &v, e.span)?;
        self.term(&format!("br i1 {}, label %{found}, label %{next}", same.v));
        self.begin(&found);
        self.inst(&format!("store i1 true, ptr {res}"));
        self.term(&format!("br label %{done}"));
        self.begin(&next);
        let i2 = self.fresh();
        self.inst(&format!("{i2} = add i64 {i}, 1"));
        self.inst(&format!("store i64 {i2}, ptr {idx}"));
        self.term(&format!("br label %{cond}"));
        self.begin(&done);
        let t = self.fresh();
        self.inst(&format!("{t} = load i1, ptr {res}"));
        Ok(Val { v: t, ty: Ty::I1 })
    }

    /// `for x in xs`: iterates a COPY of the elements taken when the loop starts, like the interpreter (a push or
    /// an element write in the body is not seen by the loop). The copy is a GC array held in a stack slot, which
    /// is cleared when the loop ends. Allocating it follows the list rule: not in LOW, not in MID outside an arena.
    fn for_list(&mut self, span: Span, name: &str, iter: &Expr, body: &Block) -> R<()> {
        let list = self.expr(iter)?;
        let Ty::List(li) = list.ty else {
            return unsupported(iter.span, "`for` over anything but an integer range or a list");
        };
        let elem = self.list_elem(li);
        self.list_alloc_allowed(span, "`for` over a list (it iterates a GC copy of the elements)")?;
        self.uses_list = true;
        let size = format!("ptrtoint (ptr getelementptr ({}, ptr null, i32 1) to i64)", elem.ir());
        let mask = if elem.is_ptr() { 1 } else { 0 };
        let lenp = self.fresh();
        self.inst(&format!("{lenp} = getelementptr inbounds %ubel.list, ptr {}, i32 0, i32 0", list.v));
        let len = self.fresh();
        self.inst(&format!("{len} = load i64, ptr {lenp}"));
        let snap = self.fresh();
        self.inst(&format!("{snap} = call ptr @ubel_list_snapshot(ptr {}, i64 {size}, i64 {mask})", list.v));
        let snap_slot = self.alloca(Ty::List(li), "for.snap");
        self.inst(&format!("store ptr {snap}, ptr {snap_slot}"));
        let counter = self.alloca(Ty::I64, "for.i");
        let limit = self.alloca(Ty::I64, "for.n");
        self.inst(&format!("store i64 0, ptr {counter}"));
        self.inst(&format!("store i64 {len}, ptr {limit}"));

        let cond_l = self.label("for.cond");
        let body_l = self.label("for.body");
        let inc_l = self.label("for.inc");
        let end_l = self.label("for.end");
        self.term(&format!("br label %{cond_l}"));
        self.begin(&cond_l);
        let i = self.fresh();
        self.inst(&format!("{i} = load i64, ptr {counter}"));
        let n = self.fresh();
        self.inst(&format!("{n} = load i64, ptr {limit}"));
        let c = self.fresh();
        self.inst(&format!("{c} = icmp slt i64 {i}, {n}"));
        self.term(&format!("br i1 {c}, label %{body_l}, label %{end_l}"));

        self.begin(&body_l);
        self.loops.push(LoopCtx { cont: inc_l.clone(), brk: end_l.clone(), arena_depth: self.arenas.len() });
        self.scopes.push(HashMap::new());
        let slot = self.declare(name, elem);
        self.mark_mutable(name);
        let sp = self.fresh();
        self.inst(&format!("{sp} = load ptr, ptr {snap_slot}"));
        let cur = self.fresh();
        self.inst(&format!("{cur} = load i64, ptr {counter}"));
        let addr = self.fresh();
        self.inst(&format!("{addr} = getelementptr {}, ptr {sp}, i64 {cur}", elem.ir()));
        let x = self.fresh();
        self.inst(&format!("{x} = load {}, ptr {addr}", elem.ir()));
        self.inst(&format!("store {} {x}, ptr {slot}", elem.ir()));
        for st in body.stmts {
            self.stmt(st)?;
        }
        self.scopes.pop();
        self.loops.pop();
        if !self.terminated {
            self.term(&format!("br label %{inc_l}"));
        }

        self.begin(&inc_l);
        let cur2 = self.fresh();
        self.inst(&format!("{cur2} = load i64, ptr {counter}"));
        let next = self.fresh();
        self.inst(&format!("{next} = add i64 {cur2}, 1"));
        self.inst(&format!("store i64 {next}, ptr {counter}"));
        self.term(&format!("br label %{cond_l}"));
        self.begin(&end_l);
        self.inst(&format!("store ptr null, ptr {snap_slot}"));
        Ok(())
    }


    /// Intern a list type: equal element types give equal `Ty::List` indexes (so `need` can compare them).
    fn list_ty(&self, elem: Ty) -> Ty {
        let mut tys = self.list_tys.borrow_mut();
        if let Some(i) = tys.iter().position(|t| *t == elem) {
            return Ty::List(i);
        }
        tys.push(elem);
        Ty::List(tys.len() - 1)
    }

    fn list_elem(&self, idx: usize) -> Ty {
        self.list_tys.borrow()[idx]
    }

    /// The element types a list may have so far: numbers, bool, strings, structs, enums and other lists. Closures
    /// and void are refused.
    fn list_elem_ok(elem: Ty) -> bool {
        matches!(elem, Ty::I1 | Ty::I64 | Ty::Int(_) | Ty::F64 | Ty::F32 | Ty::Str | Ty::Struct(_) | Ty::Enum(_) | Ty::List(_))
    }

    /// Creating or growing a list allocates in the GC heap, like building a string: LOW must not, and MID code
    /// outside `with arena` has no decided place to allocate.
    fn list_alloc_allowed(&self, span: Span, what: &str) -> R<()> {
        let in_arena = !self.arenas.is_empty() || self.in_arena_closure.last().copied().unwrap_or(false);
        match self.cur_tier {
            TierAnnotation::Low => {
                unsupported(span, format!("{what} in `@tier(low)` (a list lives in the GC heap, which LOW must not use)"))
            }
            TierAnnotation::Mid if !in_arena => unsupported(
                span,
                format!("{what} in a `@tier(mid)` function outside `with arena` (where it allocates is not decided yet)"),
            ),
            _ => Ok(()),
        }
    }

    /// `List.new()`: an empty list on the GC heap. Its element type is the one sema fixed for the expression (from
    /// the annotation, or from how the list is used later). A list that sema places in an arena is refused:
    /// arena lists are a later slice.
    fn list_new(&mut self, e: &Expr, args: &[Arg], want: Option<Ty>) -> R<Val> {
        if !args.is_empty() {
            return Err(CompileError::at(e.span, "wrong number of arguments"));
        }
        if self.sema_tags_arena_string(e) {
            return unsupported(e.span, "a list allocated in an arena (lists live in the GC heap so far)");
        }
        // Sema leaves the type of this call as an unsolved variable (`List<?T8>`), so the element type comes from
        // the place the list is stored in (`expr_as`); a type sema did fix is the fallback.
        let ty = match want.or_else(|| self.ctx.expr_types.get(&e.span).and_then(|&id| self.ty_of_sema(id))) {
            Some(t @ Ty::List(_)) => t,
            _ => {
                return unsupported(
                    e.span,
                    "`List.new()` where the element type is not known here (write the type where the list is stored, as in `let xs: List<int> = List.new()`, or pass it where a `List<T>` parameter is expected)",
                )
            }
        };
        self.list_alloc_allowed(e.span, "`List.new()`")?;
        self.uses_list = true;
        let t = self.fresh();
        self.inst(&format!("{t} = call ptr @ubel_list_new()"));
        Ok(Val { v: t, ty })
    }

    /// `xs.len()`, `xs.is_empty()` and `xs.push(v)` on a list (the receiver is already evaluated).
    fn list_method(&mut self, e: &Expr, callee: &Expr, recv: Val, li: usize, field: &str, args: &[Arg]) -> R<Val> {
        let elem = self.list_elem(li);
        self.uses_list = true;
        match field {
            "len" | "is_empty" => {
                if !args.is_empty() {
                    return Err(CompileError::at(e.span, "wrong number of arguments"));
                }
                let lenp = self.fresh();
                self.inst(&format!("{lenp} = getelementptr inbounds %ubel.list, ptr {}, i32 0, i32 0", recv.v));
                let len = self.fresh();
                self.inst(&format!("{len} = load i64, ptr {lenp}"));
                if field == "len" {
                    return Ok(Val { v: len, ty: Ty::I64 });
                }
                let z = self.fresh();
                self.inst(&format!("{z} = icmp eq i64 {len}, 0"));
                Ok(Val { v: z, ty: Ty::I1 })
            }
            "push" => self.list_push(e, callee, recv, elem, args),
            "pop" | "first" | "last" => self.list_take(e, callee, recv, elem, field, args),
            "contains" => self.list_contains(e, recv, elem, args),
            _ => unsupported(
                callee.span,
                format!("the method `{field}` on a list (only `len`, `is_empty`, `push`, `pop`, `first`, `last` and `contains` so far)"),
            ),
        }
    }

    /// `xs.push(v)`: grow the array when it is full, then store at `len` and add one. The value is checked
    /// against the element type, and a value that sema places in an arena is refused: it would dangle once the
    /// arena ends, and sema does not stop a list from being filled with one (the interpreter, which keeps
    /// everything alive, would not show it).
    fn list_push(&mut self, e: &Expr, callee: &Expr, list: Val, elem: Ty, args: &[Arg]) -> R<Val> {
        if args.len() != 1 {
            return Err(CompileError::at(e.span, "wrong number of arguments"));
        }
        let ae = match args[0].kind {
            ArgKind::Positional(x) => x,
            ArgKind::Named { .. } => return unsupported(args[0].span, "named arguments"),
        };
        let v = self.expr_as(ae, elem)?;
        self.need_value(&v, ae.span)?;
        self.need(elem, &v, ae.span)?;
        if self.sema_tags_arena_string(ae) {
            return unsupported(ae.span, "pushing a value that lives in an arena into a list (the list could outlive the arena)");
        }
        self.list_alloc_allowed(callee.span, "`push`")?;
        let size = format!("ptrtoint (ptr getelementptr ({}, ptr null, i32 1) to i64)", elem.ir());
        let mask = if elem.is_ptr() { 1 } else { 0 };
        let lenp = self.fresh();
        self.inst(&format!("{lenp} = getelementptr inbounds %ubel.list, ptr {}, i32 0, i32 0", list.v));
        let len = self.fresh();
        self.inst(&format!("{len} = load i64, ptr {lenp}"));
        let capp = self.fresh();
        self.inst(&format!("{capp} = getelementptr inbounds %ubel.list, ptr {}, i32 0, i32 1", list.v));
        let cap = self.fresh();
        self.inst(&format!("{cap} = load i64, ptr {capp}"));
        let full = self.fresh();
        self.inst(&format!("{full} = icmp eq i64 {len}, {cap}"));
        let grow = self.label("pushgrow");
        let ok = self.label("pushok");
        self.term(&format!("br i1 {full}, label %{grow}, label %{ok}"));
        self.begin(&grow);
        self.inst(&format!("call void @ubel_list_grow(ptr {}, i64 {size}, i64 {mask})", list.v));
        self.term(&format!("br label %{ok}"));
        self.begin(&ok);
        let datap = self.fresh();
        self.inst(&format!("{datap} = getelementptr inbounds %ubel.list, ptr {}, i32 0, i32 2", list.v));
        let data = self.fresh();
        self.inst(&format!("{data} = load ptr, ptr {datap}"));
        let slot = self.fresh();
        self.inst(&format!("{slot} = getelementptr {}, ptr {data}, i64 {len}", elem.ir()));
        self.inst(&format!("store {} {}, ptr {slot}", elem.ir(), v.v));
        let next = self.fresh();
        self.inst(&format!("{next} = add i64 {len}, 1"));
        self.inst(&format!("store i64 {next}, ptr {lenp}"));
        Ok(Val::void())
    }

    /// The address of `xs[i]` and the element type, after a bounds check that follows the interpreter: the index
    /// must be an `int`, a negative one counts from the end (`-1` is the last element), and anything outside
    /// the list stops the program with exit status 1.
    fn list_elem_addr(&mut self, target: &Expr, index: &Expr, span: Span) -> R<(String, Ty)> {
        let list = self.expr(target)?;
        let Ty::List(li) = list.ty else {
            return unsupported(span, "indexing anything but a list");
        };
        let elem = self.list_elem(li);
        let i = self.expr(index)?;
        self.need(Ty::I64, &i, index.span)?;
        self.uses_list = true;
        let lenp = self.fresh();
        self.inst(&format!("{lenp} = getelementptr inbounds %ubel.list, ptr {}, i32 0, i32 0", list.v));
        let len = self.fresh();
        self.inst(&format!("{len} = load i64, ptr {lenp}"));
        let neg = self.fresh();
        self.inst(&format!("{neg} = icmp slt i64 {}, 0", i.v));
        let adj = self.fresh();
        self.inst(&format!("{adj} = add i64 {len}, {}", i.v));
        let at = self.fresh();
        self.inst(&format!("{at} = select i1 {neg}, i64 {adj}, i64 {}", i.v));
        let oob = self.fresh();
        self.inst(&format!("{oob} = icmp uge i64 {at}, {len}"));
        let bad = self.label("idxbad");
        let ok = self.label("idxok");
        self.term(&format!("br i1 {oob}, label %{bad}, label %{ok}"));
        self.begin(&bad);
        self.inst(&format!("call void @ubel_trap_index(i64 {}, i64 {len})", i.v));
        self.term("unreachable");
        self.begin(&ok);
        let datap = self.fresh();
        self.inst(&format!("{datap} = getelementptr inbounds %ubel.list, ptr {}, i32 0, i32 2", list.v));
        let data = self.fresh();
        self.inst(&format!("{data} = load ptr, ptr {datap}"));
        let addr = self.fresh();
        self.inst(&format!("{addr} = getelementptr {}, ptr {data}, i64 {at}", elem.ir()));
        Ok((addr, elem))
    }

    /// Records an LLVM declaration (an intrinsic) to be written once with the other declarations.
    fn declare_intrinsic(&mut self, line: String) {
        self.decls.insert(line);
    }

    /// `sqrt`, `floor`, `ceil`, `abs`, `min` and `max` (M2b), for `double`, `float` and `int` arguments of one
    /// type, with the interpreter's results: `sqrt`, `floor` and `ceil` of an `int` are a `double`, `abs` and
    /// `min`/`max` of `int`s are `int`s. `min(a, b)` is `a` when `a <= b`, else `b`, and `max` is `a` when
    /// `a >= b`, else `b`, so a NaN argument and the two zeros behave exactly as in the interpreter (which is why
    /// this is a compare and a select and not `llvm.minnum`, whose NaN and zero rules differ). The sized
    /// integers and mixed argument types are refused (the interpreter panics or promotes for those).
    fn math_call(&mut self, e: &Expr, name: &str, args: &[Arg]) -> R<Val> {
        let vals = self.panic_args(args)?;
        let want = if matches!(name, "min" | "max") { 2 } else { 1 };
        if vals.len() != want {
            return Err(CompileError::at(e.span, format!("`{name}` takes {want} argument(s), found {}", vals.len())));
        }
        for (v, sp) in &vals {
            self.need_value(v, *sp)?;
        }
        let (a, a_span) = (vals[0].0.clone(), vals[0].1);
        if !matches!(a.ty, Ty::F64 | Ty::F32 | Ty::I64) {
            return unsupported(a_span, format!("`{name}` of this type (only `int`, `float` and `double` so far)"));
        }
        if want == 2 {
            let (b, b_span) = (&vals[1].0, vals[1].1);
            if b.ty != a.ty {
                return unsupported(b_span, format!("`{name}` of two different types (both must be `int`, `float` or `double`)"));
            }
            let ir = a.ty.ir();
            let c = self.fresh();
            let pred = match (a.ty.is_float(), name) {
                (true, "min") => "fcmp ole",
                (true, _) => "fcmp oge",
                (false, "min") => "icmp sle",
                (false, _) => "icmp sge",
            };
            self.inst(&format!("{c} = {pred} {ir} {}, {}", a.v, b.v));
            let t = self.fresh();
            self.inst(&format!("{t} = select i1 {c}, {ir} {}, {ir} {}", a.v, b.v));
            return Ok(Val { v: t, ty: a.ty });
        }
        // one argument
        if name == "abs" {
            let t = self.fresh();
            if a.ty == Ty::I64 {
                // `abs(MIN)` is `MIN` (wrapping), like the interpreter's release build.
                self.declare_intrinsic("declare i64 @llvm.abs.i64(i64, i1 immarg)".to_string());
                self.inst(&format!("{t} = call i64 @llvm.abs.i64(i64 {}, i1 false)", a.v));
            } else {
                let (ir, sfx) = if a.ty == Ty::F64 { ("double", "f64") } else { ("float", "f32") };
                self.declare_intrinsic(format!("declare {ir} @llvm.fabs.{sfx}({ir})"));
                self.inst(&format!("{t} = call {ir} @llvm.fabs.{sfx}({ir} {})", a.v));
            }
            return Ok(Val { v: t, ty: a.ty });
        }
        // sqrt, floor, ceil
        let x = if a.ty == Ty::I64 {
            let w = self.fresh();
            self.inst(&format!("{w} = sitofp i64 {} to double", a.v));
            Val { v: w, ty: Ty::F64 }
        } else {
            a
        };
        let (ir, sfx) = if x.ty == Ty::F64 { ("double", "f64") } else { ("float", "f32") };
        self.declare_intrinsic(format!("declare {ir} @llvm.{name}.{sfx}({ir})"));
        let t = self.fresh();
        self.inst(&format!("{t} = call {ir} @llvm.{name}.{sfx}({ir} {})", x.v));
        Ok(Val { v: t, ty: x.ty })
    }

    /// `value as type` between integers, and from bool to an integer, with Rust's `as` rules: a narrower
    /// target truncates, a wider one extends by the SOURCE's signedness, the same width reinterprets.
    fn cast(&mut self, v: &Val, to: Ty, span: Span) -> R<Val> {
        if v.ty == to {
            return Ok(v.clone());
        }
        if v.ty.is_float() || to.is_float() {
            return self.float_cast(v, to, span);
        }
        let Some((to_bits, _)) = to.int_info() else {
            return unsupported(span, "this conversion (only to an integer type so far)");
        };
        if v.ty == Ty::I1 {
            let t = self.fresh();
            self.inst(&format!("{t} = zext i1 {} to {}", v.v, to.ir()));
            return Ok(Val { v: t, ty: to });
        }
        let Some((from_bits, from_signed)) = v.ty.int_info() else {
            return unsupported(span, "this conversion (only integers and bools can be converted so far)");
        };
        if from_bits == to_bits {
            return Ok(Val { v: v.v.clone(), ty: to });
        }
        let op = if to_bits < from_bits {
            "trunc"
        } else if from_signed {
            "sext"
        } else {
            "zext"
        };
        let t = self.fresh();
        self.inst(&format!("{t} = {op} {} {} to {}", v.ty.ir(), v.v, to.ir()));
        Ok(Val { v: t, ty: to })
    }

    /// An integer as an `i64` for the print and format helpers: sign-extended when it is signed,
    /// zero-extended when it is not. A 64-bit value is returned as it is (`u64` has its own helpers).
    fn widen_i64(&mut self, v: &Val) -> String {
        match v.ty.int_info() {
            Some((64, _)) | None => v.v.clone(),
            Some((_, signed)) => {
                let t = self.fresh();
                let op = if signed { "sext" } else { "zext" };
                self.inst(&format!("{t} = {op} {} {} to i64", v.ty.ir(), v.v));
                t
            }
        }
    }

    fn binop(&mut self, op: BinOp, l: &Val, r: &Val, span: Span) -> R<Val> {
        if matches!(l.ty, Ty::Opt(_) | Ty::Null) || matches!(r.ty, Ty::Opt(_) | Ty::Null) {
            return self.opt_binop(op, l, r, span);
        }
        if l.ty != r.ty || l.ty == Ty::Void {
            return Err(CompileError::at(span, "the operands of this operator have different types"));
        }
        if l.ty == Ty::Str {
            return match op {
                BinOp::Eq | BinOp::Ne => {
                    self.uses_str = true;
                    let c = self.fresh();
                    self.inst(&format!("{c} = call i32 @ubel_str_eq(ptr {}, ptr {})", l.v, r.v));
                    let t = self.fresh();
                    let pred = if op == BinOp::Eq { "ne" } else { "eq" };
                    self.inst(&format!("{t} = icmp {pred} i32 {c}, 0"));
                    Ok(Val { v: t, ty: Ty::I1 })
                }
                BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
                    // Byte-wise lexicographic order (UTF-8 bytes), like the interpreter's string comparison.
                    self.uses_str = true;
                    let c = self.fresh();
                    self.inst(&format!("{c} = call i32 @ubel_str_cmp(ptr {}, ptr {})", l.v, r.v));
                    let t = self.fresh();
                    let pred = match op {
                        BinOp::Lt => "slt",
                        BinOp::Le => "sle",
                        BinOp::Gt => "sgt",
                        _ => "sge",
                    };
                    self.inst(&format!("{t} = icmp {pred} i32 {c}, 0"));
                    Ok(Val { v: t, ty: Ty::I1 })
                }
                BinOp::Add => self.str_concat(l, r, span),
                _ => unsupported(span, "this operator on strings (only `+`, `==`, `!=`, `<`, `<=`, `>` and `>=` so far)"),
            };
        }
        if matches!(l.ty, Ty::Fn(_)) {
            return unsupported(span, "operators on closure values");
        }
        if l.ty.is_ptr() {
            return unsupported(span, "operators on struct, enum or list values (compare fields or elements, or `match`, instead)");
        }
        if l.ty.is_float() {
            return self.float_binop(op, l, r, span);
        }
        let ty = l.ty;
        let t = self.fresh();
        let arith = |name: &str| format!("{t} = {name} {} {}, {}", ty.ir(), l.v, r.v);
        // Width and signedness of an integer operand; `None` for bool.
        let info = ty.int_info();

        match op {
            BinOp::Add | BinOp::Sub | BinOp::Mul => {
                if info.is_none() {
                    return unsupported(span, "this operator on bools");
                }
                let name = match op {
                    BinOp::Add => "add",
                    BinOp::Sub => "sub",
                    _ => "mul",
                };
                self.inst(&arith(name));
                Ok(Val { v: t, ty })
            }
            BinOp::Shl | BinOp::Shr => {
                let Some((bits, signed)) = info else {
                    return unsupported(span, "this operator on bools");
                };
                // The interpreter masks the shift amount to the width (`b & (bits - 1)`), so a shift by 64 or
                // by a negative amount is defined. LLVM calls it poison, hence the explicit mask.
                let masked = self.fresh();
                self.inst(&format!("{masked} = and {} {}, {}", ty.ir(), r.v, bits - 1));
                let name = match (op, signed) {
                    (BinOp::Shl, _) => "shl",
                    (_, true) => "ashr",
                    (_, false) => "lshr",
                };
                self.inst(&format!("{t} = {name} {} {}, {masked}", ty.ir(), l.v));
                Ok(Val { v: t, ty })
            }
            BinOp::BitAnd | BinOp::BitOr | BinOp::BitXor => {
                let name = match op {
                    BinOp::BitAnd => "and",
                    BinOp::BitOr => "or",
                    _ => "xor",
                };
                self.inst(&arith(name));
                Ok(Val { v: t, ty })
            }
            BinOp::Div | BinOp::Rem => {
                let Some((_, signed)) = info else {
                    return unsupported(span, "this operator on bools");
                };
                let ir = ty.ir();
                let zero = self.fresh();
                self.inst(&format!("{zero} = icmp eq {ir} {}, 0", r.v));
                let bad = self.label("divz");
                let ok = self.label("divok");
                self.term(&format!("br i1 {zero}, label %{bad}, label %{ok}"));
                self.begin(&bad);
                self.inst("call void @ubel_trap_div_zero()");
                self.term("unreachable");
                self.begin(&ok);
                if !signed {
                    let name = if op == BinOp::Div { "udiv" } else { "urem" };
                    self.inst(&format!("{t} = {name} {ir} {}, {}", l.v, r.v));
                } else {
                    // `MIN / -1` does not fit; Ubel's integers wrap, so it is `MIN` (and `MIN % -1` is 0).
                    // LLVM's `sdiv` and `srem` are undefined there (a trap on x86), so a divisor of -1 is
                    // swapped for 1 and the quotient is corrected: `x / -1` is `0 - x`, `x % -1` is 0 either way.
                    let m1 = self.fresh();
                    self.inst(&format!("{m1} = icmp eq {ir} {}, -1", r.v));
                    let d = self.fresh();
                    self.inst(&format!("{d} = select i1 {m1}, {ir} 1, {ir} {}", r.v));
                    if op == BinOp::Div {
                        let q = self.fresh();
                        self.inst(&format!("{q} = sdiv {ir} {}, {d}", l.v));
                        let neg = self.fresh();
                        self.inst(&format!("{neg} = sub {ir} 0, {}", l.v));
                        self.inst(&format!("{t} = select i1 {m1}, {ir} {neg}, {ir} {q}"));
                    } else {
                        self.inst(&format!("{t} = srem {ir} {}, {d}", l.v));
                    }
                }
                Ok(Val { v: t, ty })
            }
            BinOp::Eq | BinOp::Ne => {
                let name = if op == BinOp::Eq { "eq" } else { "ne" };
                self.inst(&format!("{t} = icmp {name} {} {}, {}", ty.ir(), l.v, r.v));
                Ok(Val { v: t, ty: Ty::I1 })
            }
            BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
                let Some((_, signed)) = info else {
                    return unsupported(span, "ordering comparisons on bools");
                };
                let name = match (op, signed) {
                    (BinOp::Lt, true) => "slt",
                    (BinOp::Le, true) => "sle",
                    (BinOp::Gt, true) => "sgt",
                    (BinOp::Ge, true) => "sge",
                    (BinOp::Lt, false) => "ult",
                    (BinOp::Le, false) => "ule",
                    (BinOp::Gt, false) => "ugt",
                    _ => "uge",
                };
                self.inst(&format!("{t} = icmp {name} {} {}, {}", ty.ir(), l.v, r.v));
                Ok(Val { v: t, ty: Ty::I1 })
            }
            BinOp::And | BinOp::Or | BinOp::Range | BinOp::RangeIncl => {
                unsupported(span, "this operator here")
            }
        }
    }

    // ── calls and printing ───────────────────────────────────────

    fn call(&mut self, e: &Expr, callee: &Expr, args: &[Arg]) -> R<Val> {
        // `Enum.Variant(..)` builds an enum value.
        if let ExprKind::Field { target, field } = callee.kind {
            if let ExprKind::Ident(en) = target.kind {
                if self.lookup(en).is_none() {
                    if let Some(&eid) = self.enum_ids.get(en) {
                        let vi = self.variant_index(eid, field, callee.span)?;
                        return self.enum_ctor(e, eid, vi, args);
                    }
                }
            }
        }
        // `List.new()`: an empty list on the GC heap (M3a).
        if let ExprKind::Field { target, field: "new" } = callee.kind {
            if let ExprKind::Ident("List") = target.kind {
                if self.lookup("List").is_none() && !self.struct_ids.contains_key("List") && !self.enum_ids.contains_key("List") {
                    return self.list_new(e, args, None);
                }
            }
        }
        // `s.len()`, `s.contains(t)`, ...: a method call on a string. A callee that names a declared type
        // (`Color.make()`) keeps the old refusal below.
        if let ExprKind::Field { target, field } = callee.kind {
            let names_a_type = match target.kind {
                ExprKind::Ident(n) => {
                    self.lookup(n).is_none() && (self.struct_ids.contains_key(n) || self.enum_ids.contains_key(n))
                }
                _ => false,
            };
            if !names_a_type {
                return self.str_method(e, callee, target, field, args);
            }
        }
        let name = match callee.kind {
            ExprKind::Ident(n) => n,
            _ => return unsupported(callee.span, "calling anything but a named function"),
        };
        match name {
            "println" => return self.print_call(args, true),
            "print" => return self.print_call(args, false),
            "assert" if !self.sigs.contains_key(name) => return self.assert_call(e, args),
            "sqrt" | "floor" | "ceil" | "abs" | "min" | "max" if !self.sigs.contains_key(name) && self.lookup(name).is_none() => {
                return self.math_call(e, name, args)
            }
            "panic" if !self.sigs.contains_key(name) => return self.panic_call(e, args),
            _ => {}
        }
        let found = match self.lookup(name) {
            Some(l) => Some(l),
            None if !self.sigs.contains_key(name) => self.capture_local(name, e.span)?,
            None => None,
        };
        if let Some(local) = found {
            if let Ty::Fn(idx) = local.ty {
                return self.call_closure(e, &local, idx, args);
            }
        }
        let sig = match self.sigs.get(name) {
            Some(s) => s.clone(),
            None => return unsupported(e.span, format!("the call to `{name}` (only your own functions, `println` and `print` so far)")),
        };
        if args.len() != sig.params.len() {
            return Err(CompileError::at(e.span, "wrong number of arguments"));
        }
        let mut ops = Vec::new();
        for (i, a) in args.iter().enumerate() {
            let ae = match a.kind {
                ArgKind::Positional(x) => x,
                ArgKind::Named { .. } => return unsupported(a.span, "named arguments"),
            };
            let v = self.expr_as(ae, sig.params[i])?;
            self.need(sig.params[i], &v, ae.span)?;
            ops.push(format!("{} {}", sig.params[i].ir(), v.v));
        }
        let boundary = if sig.tier != self.cur_tier {
            format!(" ; {} -> {} call", tier_name(self.cur_tier), tier_name(sig.tier))
        } else {
            String::new()
        };
        if sig.ret == Ty::Void {
            self.inst(&format!("call void @ubel_{name}({}){boundary}", ops.join(", ")));
            Ok(Val::void())
        } else {
            let t = self.fresh();
            self.inst(&format!("{t} = call {} @ubel_{name}({}){boundary}", sig.ret.ir(), ops.join(", ")));
            Ok(Val { v: t, ty: sig.ret })
        }
    }

    /// A method call on a string (docs/LLVM_POC.md, "Strings"). Two groups. The ones that read the string and
    /// allocate nothing, so they carry no borrow and are legal in every tier: `len` is the byte length the
    /// object stores (the interpreter's `len` is bytes too), `is_empty` is `len == 0`, `contains`,
    /// `starts_with` and `ends_with` take one string and call a runtime helper. And the ones that build a new
    /// string: `to_upper`, `to_lower`, `trim`, `trim_start`, `trim_end` and `replace` (two strings) call a
    /// runtime helper that returns a new GC string, a copy even when nothing changes. Where the result lives is
    /// the same rule as string `+`: always the GC heap, refused in `@tier(low)` and in `@tier(mid)` outside
    /// `with arena`. Sema types such a result with the receiver's arena tag, so on an arena receiver the
    /// result is stricter than it needs to be, never looser. The receiver is evaluated first, then the
    /// arguments. A receiver that is not a string keeps the old refusal (`h.run(4)` on a struct field).
    fn str_method(&mut self, e: &Expr, callee: &Expr, target: &Expr, field: &str, args: &[Arg]) -> R<Val> {
        let recv = self.expr(target)?;
        if let Ty::List(li) = recv.ty {
            return self.list_method(e, callee, recv, li, field, args);
        }
        if recv.ty != Ty::Str {
            return unsupported(callee.span, "calling anything but a named function");
        }
        // (runtime helper, number of string arguments, builds a new string)
        let (helper, want, builds) = match field {
            "len" | "is_empty" => (None, 0, false),
            "contains" => (Some("ubel_str_contains"), 1, false),
            "starts_with" => (Some("ubel_str_starts_with"), 1, false),
            "ends_with" => (Some("ubel_str_ends_with"), 1, false),
            "to_upper" => (Some("ubel_str_to_upper"), 0, true),
            "to_lower" => (Some("ubel_str_to_lower"), 0, true),
            "trim" => (Some("ubel_str_trim"), 0, true),
            "trim_start" => (Some("ubel_str_trim_start"), 0, true),
            "trim_end" => (Some("ubel_str_trim_end"), 0, true),
            "replace" => (Some("ubel_str_replace"), 2, true),
            _ => {
                return unsupported(
                    callee.span,
                    format!("the method `{field}` on a string (only `len`, `is_empty`, `contains`, `starts_with`, `ends_with`, `to_upper`, `to_lower`, `trim`, `trim_start`, `trim_end` and `replace` so far)"),
                );
            }
        };
        if args.len() != want {
            return Err(CompileError::at(e.span, "wrong number of arguments"));
        }
        if builds {
            let in_arena = !self.arenas.is_empty() || self.in_arena_closure.last().copied().unwrap_or(false);
            match self.cur_tier {
                TierAnnotation::Low => {
                    return unsupported(callee.span, format!("the string method `{field}` in `@tier(low)` (it allocates in the GC heap, which LOW must not)"));
                }
                TierAnnotation::Mid if !in_arena => {
                    return unsupported(callee.span, format!("the string method `{field}` in a `@tier(mid)` function outside `with arena` (where it allocates is not decided yet)"));
                }
                _ => {}
            }
        }
        self.uses_str = true;
        // A string that builds from an arena receiver is allocated in the receiver's own arena, the one sema's
        // tag names (so it may live exactly as long as the tag says); anything else is a GC copy. The arena
        // must be one of this function's open arenas: a closure body has none of the enclosing function's.
        let arena_ptr: Option<String> = if builds {
            self.sema_arena_id(target)
                .and_then(|id| self.arena_ids.iter().rev().find(|(i, _)| *i == id).map(|(_, p)| p.clone()))
                .filter(|p| self.arenas.contains(p))
        } else {
            None
        };
        let Some(helper) = helper else {
            let n = self.fresh();
            self.inst(&format!("{n} = load i64, ptr {}", recv.v));
            if field == "len" {
                return Ok(Val { v: n, ty: Ty::I64 });
            }
            let t = self.fresh();
            self.inst(&format!("{t} = icmp eq i64 {n}, 0"));
            return Ok(Val { v: t, ty: Ty::I1 });
        };
        let mut ops: Vec<String> = vec![format!("ptr {}", recv.v)];
        for a in args {
            let ae = match a.kind {
                ArgKind::Positional(x) => x,
                ArgKind::Named { .. } => return unsupported(a.span, "named arguments"),
            };
            let arg = self.expr(ae)?;
            if arg.ty != Ty::Str {
                let what = if builds { "the interpreter panics there" } else { "the interpreter answers false there" };
                return unsupported(ae.span, format!("`{field}` with an argument that is not a string ({what})"));
            }
            ops.push(format!("ptr {}", arg.v));
        }
        if builds {
            ops.push(match &arena_ptr {
                Some(a) => format!("ptr {a}"),
                None => "ptr null".to_string(),
            });
        }
        let ops = ops.join(", ");
        if builds {
            let r = self.fresh();
            self.inst(&format!("{r} = call ptr @{helper}({ops})"));
            return Ok(Val { v: r, ty: Ty::Str });
        }
        let c = self.fresh();
        self.inst(&format!("{c} = call i32 @{helper}({ops})"));
        let t = self.fresh();
        self.inst(&format!("{t} = icmp ne i32 {c}, 0"));
        Ok(Val { v: t, ty: Ty::I1 })
    }

    // ── closures (A7) ────────────────────────────────────────────

    /// Intern a function type: equal shapes get equal `Ty::Fn` indexes.
    fn fn_ty(&self, params: Vec<Ty>, ret: Ty) -> Ty {
        let want = FnTy { params, ret };
        let mut tys = self.fn_tys.borrow_mut();
        if let Some(i) = tys.iter().position(|t| *t == want) {
            return Ty::Fn(i);
        }
        tys.push(want);
        Ty::Fn(tys.len() - 1)
    }

    fn fn_sig(&self, idx: usize) -> FnTy {
        self.fn_tys.borrow()[idx].clone()
    }

    /// The emitter's type for a sema type, when it is one the emitter lowers.
    fn ty_of_sema(&self, start: TypeId) -> Option<Ty> {
        let mut id = start;
        loop {
            match self.ctx.types.get(id) {
                SemaType::Int | SemaType::Long | SemaType::I64 | SemaType::Isize => return Some(Ty::I64),
                SemaType::I8 => return Some(Ty::Int(IntTy::I8)),
                SemaType::I16 => return Some(Ty::Int(IntTy::I16)),
                SemaType::I32 => return Some(Ty::Int(IntTy::I32)),
                SemaType::U8 => return Some(Ty::Int(IntTy::U8)),
                SemaType::U16 => return Some(Ty::Int(IntTy::U16)),
                SemaType::U32 | SemaType::Uint => return Some(Ty::Int(IntTy::U32)),
                SemaType::U64 | SemaType::Ulong | SemaType::Usize => return Some(Ty::Int(IntTy::U64)),
                SemaType::Float | SemaType::F32 => return Some(Ty::F32),
                SemaType::Double | SemaType::F64 => return Some(Ty::F64),
                SemaType::Bool => return Some(Ty::I1),
                SemaType::Void => return Some(Ty::Void),
                SemaType::Str => return Some(Ty::Str),
                SemaType::GcRef(inner) => id = *inner,
                SemaType::ArenaRef { inner, .. } => id = *inner,
                SemaType::OwnedRef { mutable: false, inner } => id = *inner,
                SemaType::Reference { mutable: false, inner, .. } => id = *inner,
                SemaType::Named { def, args } if args.is_empty() => {
                    let name = &self.ctx.symbols.lookup(*def).name;
                    if let Some(&sid) = self.struct_ids.get(name) {
                        return Some(Ty::Struct(sid));
                    }
                    if let Some(&eid) = self.enum_ids.get(name) {
                        return Some(Ty::Enum(eid));
                    }
                    return None;
                }
                SemaType::List(elem) => {
                    let e = self.ty_of_sema(*elem)?;
                    if !Self::list_elem_ok(e) {
                        return None;
                    }
                    return Some(self.list_ty(e));
                }
                SemaType::Optional(inner) => {
                    let it = self.ty_of_sema(*inner)?;
                    if !Self::opt_inner_ok(it) {
                        return None;
                    }
                    return Some(self.opt_ty(it));
                }
                SemaType::Function { params, return_type, is_fallible: false, .. } => {
                    let ps: Option<Vec<Ty>> = params.iter().map(|p| self.ty_of_sema(*p)).collect();
                    let ret = self.ty_of_sema(*return_type)?;
                    return Some(self.fn_ty(ps?, ret));
                }
                _ => return None,
            }
        }
    }

    /// The result type of a function type, looking through the arena and GC wrappers sema puts around a
    /// closure value.
    fn sema_fn_ret(&self, start: TypeId) -> Option<TypeId> {
        let mut id = start;
        loop {
            match self.ctx.types.get(id) {
                SemaType::Function { return_type, is_fallible: false, .. } => return Some(*return_type),
                SemaType::GcRef(inner) => id = *inner,
                SemaType::ArenaRef { inner, .. } => id = *inner,
                _ => return None,
            }
        }
    }

    /// A name that is not a local: a capture when an enclosing function has it, otherwise `other`.
    fn unknown_name(&self, name: &str, span: Span, other: String) -> CompileError {
        let in_frame = self.enclosing.last().map_or(false, |f| f.iter().any(|sc| sc.contains_key(name)));
        if in_frame {
            // Reads are captured by `capture_local`; what arrives here is an assignment.
            CompileError::at(
                span,
                format!("not supported yet: a closure that assigns to `{name}`, a variable of the code it is written in (a capture is a copy, so the assignment would not change the original)"),
            )
        } else if self.outer_names.iter().any(|s| s.contains(name)) {
            CompileError::at(
                span,
                format!("not supported yet: a closure that captures `{name}` from more than one level out (only a variable of the function or closure it is written in so far)"),
            )
        } else {
            CompileError::at(span, other)
        }
    }

    /// A name used inside a closure body that is not one of its own locals: capture it when it is a
    /// variable of the frame the closure is written in. The closure copies the value into its object when
    /// it is created (decision D5) and the body copies it out at entry, so a capture is only right for a
    /// variable that never changes afterwards: `mut` variables, `for` variables and `:=` bindings are
    /// refused. Any value is captured by copy: for a string, struct, enum or closure that is the pointer,
    /// so a captured struct is shared with the variable it came from (as structs always are). A pointer
    /// in a closure object is traced by the collector: through the mask for a GC object, through a
    /// scannable arena for an arena object.
    fn capture_local(&mut self, name: &str, span: Span) -> R<Option<Local>> {
        let found = match self.enclosing.last().and_then(|f| f.iter().rev().find_map(|sc| sc.get(name).cloned())) {
            Some(l) => l,
            None => return Ok(None),
        };
        if found.mutable {
            return unsupported(
                span,
                format!("a closure that captures `{name}`, which can change (declared `mut`, a `for` variable or a `:=` binding): a capture is a copy of the value taken when the closure is created"),
            );
        }
        if matches!(found.ty, Ty::Opt(_)) {
            return unsupported(span, format!("a closure that captures `{name}`, an optional (`T?`; optionals live in locals, parameters and results so far)"));
        }
        let idx = self.captures.last().map_or(0, |c| c.len());
        let slot = self.alloca(found.ty, name);
        let (gep, v) = (self.fresh(), self.fresh());
        let ir = found.ty.ir();
        let _ = writeln!(self.prologue, "  {gep} = getelementptr inbounds i8, ptr %.clo, i64 {}", 8 * (1 + idx));
        let _ = writeln!(self.prologue, "  {v} = load {ir}, ptr {gep}");
        let _ = writeln!(self.prologue, "  store {ir} {v}, ptr {slot}");
        let local = Local { slot, ty: found.ty, mutable: false, lambda: found.lambda };
        if let Some(sc) = self.scopes.first_mut() {
            sc.insert(name.to_string(), local.clone());
        }
        if let Some(c) = self.captures.last_mut() {
            c.push(Capture { ty: found.ty, outer_slot: found.slot });
        }
        Ok(Some(local))
    }

    /// `fn(v: int) body`, a closure that captures nothing. The body becomes its own function whose
    /// first, hidden parameter is the closure object, so a call is one indirect call through the code
    /// pointer in the object's first word (the Stage 1A closure ABI). The object is a static constant
    /// outside the heap: nothing is allocated, and the collector ignores pointers to it. The body is
    /// emitted in the tier of the function it is written in, and allocation sites inside it follow the
    /// usual rules from there (a closure written in HIGH code allocates in the GC heap wherever it is
    /// called from). Parameter types must be written; the result type comes from sema.
    fn lambda(&mut self, e: &Expr, lam: &Lambda) -> R<Val> {
        if lam.is_async_block {
            return unsupported(e.span, "async blocks");
        }
        let mut ptys: Vec<Ty> = Vec::new();
        for p in lam.params {
            match p.ty {
                Some(t) => {
                    let ty = self.ty_of_type(t)?;
                    if ty == Ty::Void {
                        return unsupported(p.span, "`void` parameters");
                    }
                    ptys.push(ty);
                }
                None => return unsupported(p.span, "untyped closure parameters (write the type: `fn(v: int) ...`)"),
            }
        }
        // The result type: sema's when it resolved to one the emitter lowers, otherwise worked out from
        // the body while it is emitted (sema records a lambda's type when it meets it, so an integer
        // literal in the body may not be defaulted to `int` yet).
        let sema_ret: Option<Ty> = self
            .ctx
            .expr_types
            .get(&e.span)
            .and_then(|&id| self.sema_fn_ret(id))
            .and_then(|r| self.ty_of_sema(r));

        // Everything visible here, so a use of it inside the body is reported as a capture.
        let mut visible: BTreeSet<String> = self.outer_names.last().cloned().unwrap_or_default();
        for sc in &self.scopes {
            for k in sc.keys() {
                visible.insert(k.clone());
            }
        }

        // Emit the body as its own function: save the state of the one being emitted.
        let saved_body = std::mem::take(&mut self.body);
        let saved_allocas = std::mem::take(&mut self.allocas);
        let saved_scopes = std::mem::replace(&mut self.scopes, vec![HashMap::new()]);
        let saved_loops = std::mem::take(&mut self.loops);
        let saved_arenas = std::mem::take(&mut self.arenas);
        let (saved_tmp, saved_lbl, saved_term, saved_ret) = (self.tmp, self.lbl, self.terminated, self.ret);
        let (saved_infer, saved_seen) = (self.ret_infer, self.ret_seen);
        let saved_prologue = std::mem::take(&mut self.prologue);
        self.enclosing.push(saved_scopes.clone());
        self.captures.push(Vec::new());
        let inside = !saved_arenas.is_empty() || self.in_arena_closure.last().copied().unwrap_or(false);
        self.in_arena_closure.push(inside);
        self.tmp = 0;
        self.lbl = 0;
        self.terminated = false;
        self.ret = sema_ret.unwrap_or(Ty::Void);
        self.ret_infer = sema_ret.is_none();
        self.ret_seen = None;
        self.outer_names.push(visible);

        let lowered = self.lambda_body(lam, &ptys);
        let ret = self.ret;

        self.outer_names.pop();
        self.enclosing.pop();
        self.in_arena_closure.pop();
        let caps = self.captures.pop().unwrap_or_default();
        let prologue = std::mem::replace(&mut self.prologue, saved_prologue);
        let body = std::mem::replace(&mut self.body, saved_body);
        let allocas = std::mem::replace(&mut self.allocas, saved_allocas);
        self.scopes = saved_scopes;
        self.loops = saved_loops;
        self.arenas = saved_arenas;
        self.tmp = saved_tmp;
        self.lbl = saved_lbl;
        self.terminated = saved_term;
        self.ret = saved_ret;
        self.ret_infer = saved_infer;
        self.ret_seen = saved_seen;
        let header = lowered?;
        let fnty = self.fn_ty(ptys.clone(), ret);

        let n = self.lam_n;
        self.lam_n += 1;
        let code = format!("@ubel_lambda.{n}");
        let obj = format!("@ubel_closure.{n}");
        let _ = writeln!(self.funcs, "; tier: {} (closure)", tier_name(self.cur_tier));
        let _ = writeln!(self.funcs, "define internal {} {code}({header}) {{\nentry:\n{allocas}{prologue}{body}}}\n", ret.ir());
        if caps.is_empty() {
            let _ = writeln!(self.strings, "{obj} = private unnamed_addr constant {{ ptr }} {{ ptr {code} }}, align 8");
            return Ok(Val { v: obj, ty: fnty });
        }
        // A closure that captures: its object is allocated where it is written (an arena inside `with
        // arena`, so it dies with the block; the GC heap in a HIGH function; refused elsewhere, as for a
        // struct literal) and holds the code pointer and a copy of each captured value, 8 bytes apiece.
        // A captured string, struct, enum or closure is a pointer the collector must trace: in a GC object
        // its word is set in the mask (word 0 is the code pointer, a static address outside the heap), in
        // an arena object the arena is made scannable.
        // A slot is 8 bytes, so on a 32-bit target it is two words, and the collector's mask has 64 bits.
        if 1 + caps.len() > 31 {
            return unsupported(e.span, "a closure that captures more than 30 variables");
        }
        let arena = self.alloc_site(e.span, "a closure that captures a variable")?;
        if caps.iter().any(|c| c.ty.is_ptr()) {
            self.mark_scan(&arena);
        }
        // Mask bit n means word n of the object, and a word is a pointer wide, so the bit of the slot at byte
        // offset `off` is off / sizeof(ptr): worked out in the IR from the target's pointer size, like the
        // struct and enum masks, so one .ll is right for 32- and 64-bit.
        let mask = if arena.is_none() && caps.iter().any(|c| c.ty.is_ptr()) {
            let psz = self.fresh();
            self.inst(&format!("{psz} = ptrtoint ptr getelementptr (ptr, ptr null, i32 1) to i64"));
            let mut acc = "0".to_string();
            for (i, c) in caps.iter().enumerate() {
                if !c.ty.is_ptr() {
                    continue;
                }
                let idx = self.fresh();
                self.inst(&format!("{idx} = udiv i64 {}, {psz}", 8 * (1 + i)));
                let bit = self.fresh();
                self.inst(&format!("{bit} = shl i64 1, {idx}"));
                let m = self.fresh();
                self.inst(&format!("{m} = or i64 {acc}, {bit}"));
                acc = m;
            }
            acc
        } else {
            "0".to_string()
        };
        let size = (8 * (1 + caps.len())).to_string();
        let objp = self.alloc_obj(arena, &size, &mask, "closure");
        self.inst(&format!("store ptr {code}, ptr {objp}"));
        for (i, c) in caps.iter().enumerate() {
            let v = self.fresh();
            self.inst(&format!("{v} = load {}, ptr {}", c.ty.ir(), c.outer_slot));
            let g = self.fresh();
            self.inst(&format!("{g} = getelementptr inbounds i8, ptr {objp}, i64 {}", 8 * (1 + i)));
            self.inst(&format!("store {} {v}, ptr {g}", c.ty.ir()));
        }
        Ok(Val { v: objp, ty: fnty })
    }

    /// The parameters and statements of a closure body; returns the function header's parameter list.
    fn lambda_body(&mut self, lam: &Lambda, ptys: &[Ty]) -> R<String> {
        let mut header = vec!["ptr %.clo".to_string()];
        for (i, p) in lam.params.iter().enumerate() {
            let reg = format!("%.arg{i}");
            header.push(format!("{} {}", ptys[i].ir(), reg));
            let slot = self.declare(p.name, ptys[i]);
            self.inst(&format!("store {} {}, ptr {}", ptys[i].ir(), reg, slot));
        }
        match lam.body {
            LambdaBody::Expr(be) => {
                let v = self.expr(be)?;
                if v.ty == Ty::Void {
                    // Nothing to return: the closure's result is void (and so is a declared one).
                    if self.ret_infer {
                        self.ret = Ty::Void;
                    }
                    self.destroy_arenas_from(0);
                    self.term("ret void");
                } else {
                    self.do_return(Some(v), be.span)?;
                }
            }
            LambdaBody::Block(b) => {
                self.lower_body(b.stmts)?;
                self.end_body();
            }
        }
        Ok(header.join(", "))
    }

    /// `f(args)` where `f` is a local of function type: load the code pointer from the closure object
    /// and call it with the object as the hidden first argument.
    fn call_closure(&mut self, e: &Expr, local: &Local, idx: usize, args: &[Arg]) -> R<Val> {
        // Decision D2: a closure has the tier of the function it is created in. LOW may call only one it
        // created itself: a local bound by an immutable `let` straight to a lambda. Anything else could
        // be a closure from a higher tier. Sema refuses it too (TIER-017); this keeps the emitter safe
        // on its own.
        if self.cur_tier == TierAnnotation::Low && !(local.lambda && !local.mutable) {
            return unsupported(
                e.span,
                "calling a closure in `@tier(low)` that was not bound by a `let` straight to a lambda in this function (its tier is not known, decision D2; sema refuses it too, TIER-017)",
            );
        }
        let sig = self.fn_sig(idx);
        if args.len() != sig.params.len() {
            return Err(CompileError::at(e.span, "wrong number of arguments"));
        }
        let mut ops = Vec::new();
        for (i, a) in args.iter().enumerate() {
            let ae = match a.kind {
                ArgKind::Positional(x) => x,
                ArgKind::Named { .. } => return unsupported(a.span, "named arguments"),
            };
            let v = self.expr_as(ae, sig.params[i])?;
            self.need(sig.params[i], &v, ae.span)?;
            ops.push(format!("{} {}", sig.params[i].ir(), v.v));
        }
        let clo = self.fresh();
        self.inst(&format!("{clo} = load ptr, ptr {}", local.slot));
        let code = self.fresh();
        self.inst(&format!("{code} = load ptr, ptr {clo}"));
        let mut all = vec![format!("ptr {clo}")];
        all.extend(ops);
        if sig.ret == Ty::Void {
            self.inst(&format!("call void {code}({}) ; closure call", all.join(", ")));
            Ok(Val::void())
        } else {
            let t = self.fresh();
            self.inst(&format!("{t} = call {} {code}({}) ; closure call", sig.ret.ir(), all.join(", ")));
            Ok(Val { v: t, ty: sig.ret })
        }
    }

    /// A string literal used as a value: a static `{ i64 length, bytes }` constant. It sits outside
    /// the GC heap, so the collector ignores pointers to it and nothing ever frees it.
    fn static_str(&mut self, s: &str) -> Val {
        let name = format!("@.strv.{}", self.str_n);
        self.str_n += 1;
        let bytes = s.as_bytes();
        let n = bytes.len();
        let _ = writeln!(
            self.strings,
            "{name} = private unnamed_addr constant {{ i64, [{n} x i8] }} {{ i64 {n}, [{n} x i8] c\"{}\" }}, align 8",
            ir_escape(bytes)
        );
        Val { v: name, ty: Ty::Str }
    }

    /// Inside `with arena` an interpolated string is allocated in the arena, which is only safe when
    /// sema tracks it as an arena value (D3b, docs/ARENA_STRINGS.md); sema types such a literal
    /// `&arena string`. Without that, a string could be returned out of the block with nothing to stop it.
    fn sema_arena_id(&self, e: &Expr) -> Option<usize> {
        match self.ctx.expr_types.get(&e.span) {
            Some(&id) => match self.ctx.types.get(id) {
                SemaType::ArenaRef { arena, .. } => Some(arena.0),
                _ => None,
            },
            None => None,
        }
    }

    fn sema_tags_arena_string(&self, e: &Expr) -> bool {
        match self.ctx.expr_types.get(&e.span) {
            Some(&id) => matches!(self.ctx.types.get(id), SemaType::ArenaRef { .. }),
            None => false,
        }
    }

    /// `a + b` on strings (rule S4 in docs/ARENA_STRINGS.md). The result is a copy of both operands, always
    /// allocated in the GC heap, wherever the `+` is written: it holds no borrow of an operand, so it may
    /// leave a `with arena` block even when an operand cannot. That also means it allocates in the GC heap
    /// from MID code, which is only allowed inside `with arena` (a MID function outside one has no
    /// decided place to allocate), and never in LOW.
    fn str_concat(&mut self, l: &Val, r: &Val, span: Span) -> R<Val> {
        let in_arena = !self.arenas.is_empty() || self.in_arena_closure.last().copied().unwrap_or(false);
        match self.cur_tier {
            TierAnnotation::Low => {
                return unsupported(span, "string `+` in `@tier(low)` (it allocates in the GC heap, which LOW must not)");
            }
            TierAnnotation::Mid if !in_arena => {
                return unsupported(span, "string `+` in a `@tier(mid)` function outside `with arena` (where it allocates is not decided yet)");
            }
            _ => {}
        }
        self.uses_str = true;
        let (la, lb) = (self.fresh(), self.fresh());
        self.inst(&format!("{la} = load i64, ptr {}", l.v));
        self.inst(&format!("{lb} = load i64, ptr {}", r.v));
        let total = self.fresh();
        self.inst(&format!("{total} = add i64 {la}, {lb}"));
        let size = self.fresh();
        self.inst(&format!("{size} = add i64 {total}, 8"));
        let p = self.alloc_obj(None, &size, "0", "string");
        self.inst(&format!("store i64 {total}, ptr {p}"));
        let dst = self.fresh();
        self.inst(&format!("{dst} = getelementptr inbounds i8, ptr {p}, i64 8"));
        let src_a = self.fresh();
        self.inst(&format!("{src_a} = getelementptr inbounds i8, ptr {}, i64 8", l.v));
        self.inst(&format!("call void @ubel_put(ptr {dst}, ptr {src_a}, i64 {la})"));
        let dst_b = self.fresh();
        self.inst(&format!("{dst_b} = getelementptr inbounds i8, ptr {dst}, i64 {la}"));
        let src_b = self.fresh();
        self.inst(&format!("{src_b} = getelementptr inbounds i8, ptr {}, i64 8", r.v));
        self.inst(&format!("call void @ubel_put(ptr {dst_b}, ptr {src_b}, i64 {lb})"));
        Ok(Val { v: p, ty: Ty::Str })
    }

    /// `$"text {a} text {b}"` as a value (D3b). Every hole is evaluated first, in order. Then the total
    /// length is computed, one object of `8 + length` bytes is allocated where the literal is written
    /// (arena inside `with arena`, GC heap in a HIGH function, refused elsewhere, exactly like a struct
    /// literal) and the pieces are copied in. A string holds no pointers, so the GC object has mask 0.
    /// ints print in decimal and bools as `true` / `false`, like `println`.
    fn interp_string(&mut self, e: &Expr, parts: &[InterpolationPart]) -> R<Val> {
        enum Piece {
            Text(String),
            Val(Val),
        }
        let arena = self.alloc_site(e.span, "an interpolated string")?;
        if arena.is_some() && !self.sema_tags_arena_string(e) {
            return unsupported(
                e.span,
                "building a string inside `with arena` while sema does not track arena strings (apply the D3b sema patch, docs/ARENA_STRINGS.md)",
            );
        }
        if let (Some(a), Some(id)) = (&arena, self.sema_arena_id(e)) {
            if !self.arena_ids.iter().any(|(i, _)| *i == id) {
                self.arena_ids.push((id, a.clone()));
            }
        }
        let mut pieces: Vec<Piece> = Vec::new();
        for part in parts {
            match *part {
                InterpolationPart::Text(t) => {
                    if !t.is_empty() {
                        pieces.push(Piece::Text(t.to_string()));
                    }
                }
                InterpolationPart::Expr { expr, spec: None } => {
                    let v = self.expr(expr)?;
                    self.need_printable(&v, expr.span)?;
                    pieces.push(Piece::Val(v));
                }
                InterpolationPart::Expr { expr, spec: Some(_) } => {
                    return unsupported(expr.span, "format specifiers in interpolated strings");
                }
            }
        }
        self.uses_str = true;

        // Length of every piece, then the total.
        let mut lens: Vec<String> = Vec::new();
        let mut const_len = 0usize;
        let mut dyn_lens: Vec<String> = Vec::new();
        for piece in &pieces {
            match piece {
                Piece::Text(t) => {
                    const_len += t.len();
                    lens.push(t.len().to_string());
                }
                Piece::Val(v) => {
                    let l = self.fresh();
                    match v.ty {
                        Ty::I64 => self.inst(&format!("{l} = call i64 @ubel_i64_len(i64 {})", v.v)),
                        Ty::F64 => self.inst(&format!("{l} = call i64 @ubel_f64_len(double {})", v.v)),
                        Ty::F32 => self.inst(&format!("{l} = call i64 @ubel_f32_len(float {})", v.v)),
                        Ty::Int(IntTy::U64) => self.inst(&format!("{l} = call i64 @ubel_u64_len(i64 {})", v.v)),
                        Ty::Int(_) => {
                            let w = self.widen_i64(v);
                            self.inst(&format!("{l} = call i64 @ubel_i64_len(i64 {w})"));
                        }
                        Ty::I1 => self.inst(&format!("{l} = select i1 {}, i64 4, i64 5", v.v)),
                        Ty::Str => self.inst(&format!("{l} = load i64, ptr {}", v.v)),
                        _ => unreachable!("void, struct and enum values are rejected by need_printable"),
                    }
                    dyn_lens.push(l.clone());
                    lens.push(l);
                }
            }
        }
        let mut total = const_len.to_string();
        for d in &dyn_lens {
            let sum = self.fresh();
            self.inst(&format!("{sum} = add i64 {total}, {d}"));
            total = sum;
        }
        let size = self.fresh();
        self.inst(&format!("{size} = add i64 {total}, 8"));
        let p = self.alloc_obj(arena, &size, "0", "string");
        self.inst(&format!("store i64 {total}, ptr {p}"));

        let mut dst = self.fresh();
        self.inst(&format!("{dst} = getelementptr inbounds i8, ptr {p}, i64 8"));
        for (i, piece) in pieces.iter().enumerate() {
            let len = lens[i].clone();
            match piece {
                Piece::Text(t) => {
                    let (name, n) = self.string_const(t);
                    self.inst(&format!("call void @ubel_put(ptr {dst}, ptr {name}, i64 {n})"));
                }
                Piece::Val(v) => match v.ty {
                    Ty::I64 => self.inst(&format!("call void @ubel_i64_write(ptr {dst}, i64 {})", v.v)),
                    Ty::F64 => self.inst(&format!("call void @ubel_f64_write(ptr {dst}, double {})", v.v)),
                    Ty::F32 => self.inst(&format!("call void @ubel_f32_write(ptr {dst}, float {})", v.v)),
                    Ty::Int(IntTy::U64) => self.inst(&format!("call void @ubel_u64_write(ptr {dst}, i64 {})", v.v)),
                    Ty::Int(_) => {
                        let w = self.widen_i64(v);
                        self.inst(&format!("call void @ubel_i64_write(ptr {dst}, i64 {w})"));
                    }
                    Ty::I1 => {
                        let (tn, _) = self.string_const("true");
                        let (fnm, _) = self.string_const("false");
                        let sel = self.fresh();
                        self.inst(&format!("{sel} = select i1 {}, ptr {tn}, ptr {fnm}", v.v));
                        self.inst(&format!("call void @ubel_put(ptr {dst}, ptr {sel}, i64 {len})"));
                    }
                    _ => {
                        let bytes = self.fresh();
                        self.inst(&format!("{bytes} = getelementptr inbounds i8, ptr {}, i64 8", v.v));
                        self.inst(&format!("call void @ubel_put(ptr {dst}, ptr {bytes}, i64 {len})"));
                    }
                },
            }
            if i + 1 < pieces.len() {
                let next = self.fresh();
                self.inst(&format!("{next} = getelementptr inbounds i8, ptr {dst}, i64 {len}"));
                dst = next;
            }
        }
        Ok(Val { v: p, ty: Ty::Str })
    }

    fn string_const(&mut self, s: &str) -> (String, usize) {
        let name = format!("@.str.{}", self.str_n);
        self.str_n += 1;
        let bytes = s.as_bytes();
        let esc = ir_escape(bytes);
        let _ = writeln!(
            self.strings,
            "{name} = private unnamed_addr constant [{} x i8] c\"{esc}\"",
            bytes.len()
        );
        (name, bytes.len())
    }

    fn print_str(&mut self, s: &str) {
        if s.is_empty() {
            return;
        }
        let (name, len) = self.string_const(s);
        self.inst(&format!("call void @ubel_print_str(ptr {name}, i64 {len})"));
    }

    /// `println(a, b, c)` evaluates every argument first (including everything inside an
    /// interpolated string) and only then prints, exactly like the interpreter, so side
    /// effects in the arguments happen before any of the output.
    /// Evaluate every argument of `assert` or `panic`, in order and before anything fails, as the
    /// interpreter does. Returns the values with their spans.
    fn panic_args(&mut self, args: &[Arg]) -> R<Vec<(Val, Span)>> {
        let mut vals = Vec::new();
        for a in args {
            let ae = match a.kind {
                ArgKind::Positional(x) => x,
                ArgKind::Named { .. } => return unsupported(a.span, "named arguments"),
            };
            let v = self.expr(ae)?;
            self.need_value(&v, ae.span)?;
            vals.push((v, ae.span));
        }
        Ok(vals)
    }

    /// The message of a failed `assert` or a `panic`: the default text, or one string.
    fn panic_text(&mut self, msg: &[(Val, Span)], default: &str, span: Span) -> R<Val> {
        match msg {
            [] => Ok(self.static_str(default)),
            [(v, _)] if v.ty == Ty::Str => Ok(v.clone()),
            _ => unsupported(span, "a message that is not a single string (the interpreter prints any value)"),
        }
    }

    /// Print the message to stderr and exit with status 1 (the block ends in `unreachable`).
    fn call_panic(&mut self, msg: &Val) {
        let len = self.fresh();
        self.inst(&format!("{len} = load i64, ptr {}", msg.v));
        let bytes = self.fresh();
        self.inst(&format!("{bytes} = getelementptr inbounds i8, ptr {}, i64 8", msg.v));
        self.inst(&format!("call void @ubel_panic(ptr {bytes}, i64 {len})"));
        self.term("unreachable");
    }

    /// `assert(cond)` or `assert(cond, "message")`.
    fn assert_call(&mut self, e: &Expr, args: &[Arg]) -> R<Val> {
        let vals = self.panic_args(args)?;
        let Some((cond, cond_span)) = vals.first() else {
            return Err(CompileError::at(e.span, "`assert` needs a condition"));
        };
        self.need(Ty::I1, cond, *cond_span)?;
        let cond = cond.v.clone();
        let msg = self.panic_text(&vals[1..], "assertion failed", e.span)?;
        let fail = self.label("assert.fail");
        let ok = self.label("assert.ok");
        self.term(&format!("br i1 {cond}, label %{ok}, label %{fail}"));
        self.begin(&fail);
        self.call_panic(&msg);
        self.begin(&ok);
        Ok(Val::void())
    }

    /// `panic()` or `panic("message")`: the program stops here.
    fn panic_call(&mut self, e: &Expr, args: &[Arg]) -> R<Val> {
        let vals = self.panic_args(args)?;
        let msg = self.panic_text(&vals, "explicit panic", e.span)?;
        self.call_panic(&msg);
        Ok(Val::void())
    }

    fn print_call(&mut self, args: &[Arg], newline: bool) -> R<Val> {
        let mut items: Vec<PItem> = Vec::new();
        for (i, a) in args.iter().enumerate() {
            let ae = match a.kind {
                ArgKind::Positional(x) => x,
                ArgKind::Named { .. } => return unsupported(a.span, "named arguments"),
            };
            if i > 0 {
                items.push(PItem::Text(" ".to_string()));
            }
            self.print_items(ae, &mut items)?;
        }
        if newline {
            items.push(PItem::Text("\n".to_string()));
        }
        for item in items {
            match item {
                PItem::Text(t) => self.print_str(&t),
                PItem::Val(v) => self.print_val(&v)?,
            }
        }
        Ok(Val::void())
    }

    /// Print one evaluated value. Numbers, bool and strings as before; `null` as `null`; an optional as `null` or its
    /// value; a list as `[a, b, c]` with its elements printed the same way (strings are not quoted), like the
    /// interpreter's `Display`. Nothing here allocates, so it is allowed in every tier.
    fn print_val(&mut self, v: &Val) -> R<()> {
        match v.ty {
            Ty::I64 => self.inst(&format!("call void @ubel_print_i64(i64 {})", v.v)),
            Ty::F64 => self.inst(&format!("call void @ubel_print_f64(double {})", v.v)),
            Ty::F32 => self.inst(&format!("call void @ubel_print_f32(float {})", v.v)),
            Ty::Int(IntTy::U64) => self.inst(&format!("call void @ubel_print_u64(i64 {})", v.v)),
            Ty::Int(_) => {
                let w = self.widen_i64(v);
                self.inst(&format!("call void @ubel_print_i64(i64 {w})"));
            }
            Ty::I1 => {
                let z = self.fresh();
                self.inst(&format!("{z} = zext i1 {} to i32", v.v));
                self.inst(&format!("call void @ubel_print_bool(i32 {z})"));
            }
            Ty::Str => {
                let len = self.fresh();
                self.inst(&format!("{len} = load i64, ptr {}", v.v));
                let bytes = self.fresh();
                self.inst(&format!("{bytes} = getelementptr inbounds i8, ptr {}, i64 8", v.v));
                self.inst(&format!("call void @ubel_print_str(ptr {bytes}, i64 {len})"));
            }
            Ty::Null => self.print_str("null"),
            Ty::Opt(oi) => {
                let inner = self.opt_inner(oi);
                let isnull = self.opt_is_null(&v.v);
                let nl = self.label("pnull");
                let vl = self.label("pval");
                let end = self.label("pend");
                self.term(&format!("br i1 {isnull}, label %{nl}, label %{vl}"));
                self.begin(&nl);
                self.print_str("null");
                self.term(&format!("br label %{end}"));
                self.begin(&vl);
                let u = self.opt_load(v, inner);
                self.print_val(&u)?;
                self.term(&format!("br label %{end}"));
                self.begin(&end);
            }
            Ty::List(li) => {
                let elem = self.list_elem(li);
                self.uses_list = true;
                let lenp = self.fresh();
                self.inst(&format!("{lenp} = getelementptr inbounds %ubel.list, ptr {}, i32 0, i32 0", v.v));
                let len = self.fresh();
                self.inst(&format!("{len} = load i64, ptr {lenp}"));
                let datap = self.fresh();
                self.inst(&format!("{datap} = getelementptr inbounds %ubel.list, ptr {}, i32 0, i32 2", v.v));
                let data = self.fresh();
                self.inst(&format!("{data} = load ptr, ptr {datap}"));
                let idx = self.alloca(Ty::I64, "pl.i");
                self.inst(&format!("store i64 0, ptr {idx}"));
                self.print_str("[");
                let cond = self.label("pl.cond");
                let body = self.label("pl.body");
                let sep = self.label("pl.sep");
                let el = self.label("pl.el");
                let end = self.label("pl.end");
                self.term(&format!("br label %{cond}"));
                self.begin(&cond);
                let i = self.fresh();
                self.inst(&format!("{i} = load i64, ptr {idx}"));
                let c = self.fresh();
                self.inst(&format!("{c} = icmp slt i64 {i}, {len}"));
                self.term(&format!("br i1 {c}, label %{body}, label %{end}"));
                self.begin(&body);
                let first = self.fresh();
                self.inst(&format!("{first} = icmp eq i64 {i}, 0"));
                self.term(&format!("br i1 {first}, label %{el}, label %{sep}"));
                self.begin(&sep);
                self.print_str(", ");
                self.term(&format!("br label %{el}"));
                self.begin(&el);
                let addr = self.fresh();
                self.inst(&format!("{addr} = getelementptr {}, ptr {data}, i64 {i}", elem.ir()));
                let x = self.fresh();
                self.inst(&format!("{x} = load {}, ptr {addr}", elem.ir()));
                self.print_val(&Val { v: x, ty: elem })?;
                let next = self.fresh();
                self.inst(&format!("{next} = add i64 {i}, 1"));
                self.inst(&format!("store i64 {next}, ptr {idx}"));
                self.term(&format!("br label %{cond}"));
                self.begin(&end);
                self.print_str("]");
            }
            Ty::Void => unreachable!("void values are rejected when collected"),
            Ty::Struct(_) | Ty::Enum(_) | Ty::Fn(_) => unreachable!("struct, enum and closure values are rejected when collected"),
        }
        Ok(())
    }

    fn print_items(&mut self, e: &Expr, out: &mut Vec<PItem>) -> R<()> {
        match e.kind {
            ExprKind::Lit(Literal::Str(s)) | ExprKind::Lit(Literal::VerbatimStr(s)) => {
                out.push(PItem::Text(s.to_string()));
                Ok(())
            }
            ExprKind::Lit(Literal::InterpolatedStr(parts))
            | ExprKind::Lit(Literal::InterpolatedVerbatimStr(parts)) => {
                for part in parts {
                    match *part {
                        InterpolationPart::Text(t) => out.push(PItem::Text(t.to_string())),
                        InterpolationPart::Expr { expr, spec: None } => {
                            let v = self.expr(expr)?;
                            self.need_print_item(&v, expr.span)?;
                            out.push(PItem::Val(v));
                        }
                        InterpolationPart::Expr { expr, spec: Some(_) } => {
                            return unsupported(expr.span, "format specifiers in interpolated strings");
                        }
                    }
                }
                Ok(())
            }
            _ => {
                let v = self.expr(e)?;
                self.need_print_item(&v, e.span)?;
                out.push(PItem::Val(v));
                Ok(())
            }
        }
    }
}

enum PItem {
    Text(String),
    Val(Val),
}

/// The bytes of a string as the body of an LLVM `c"..."` constant.
fn ir_escape(bytes: &[u8]) -> String {
    let mut esc = String::new();
    for &b in bytes {
        if (0x20..0x7f).contains(&b) && b != b'"' && b != b'\\' {
            esc.push(b as char);
        } else {
            let _ = write!(esc, "\\{:02X}", b);
        }
    }
    esc
}

fn assign_binop(op: AssignOp) -> Option<BinOp> {
    match op {
        AssignOp::Assign => None,
        AssignOp::AddAssign => Some(BinOp::Add),
        AssignOp::SubAssign => Some(BinOp::Sub),
        AssignOp::MulAssign => Some(BinOp::Mul),
        AssignOp::DivAssign => Some(BinOp::Div),
        AssignOp::RemAssign => Some(BinOp::Rem),
        AssignOp::BitAndAssign => Some(BinOp::BitAnd),
        AssignOp::BitOrAssign => Some(BinOp::BitOr),
        AssignOp::BitXorAssign => Some(BinOp::BitXor),
        AssignOp::ShlAssign => Some(BinOp::Shl),
        AssignOp::ShrAssign => Some(BinOp::Shr),
    }
}
