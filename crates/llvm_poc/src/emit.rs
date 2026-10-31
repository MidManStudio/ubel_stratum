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
use ubel_stratum::ast::expressions::{Lambda, LambdaBody, MatchArm, MatchArmBody};
use ubel_stratum::ast::patterns::{EnumPatternPayload, PatternKind};
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

// ── small types ──────────────────────────────────────────────────

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
            Ty::Struct(_) | Ty::Enum(_) | Ty::Str | Ty::Fn(_) => "ptr",
        }
    }

    /// Values that are pointers to objects, and so need a collector mask bit.
    fn is_ptr(self) -> bool {
        matches!(self, Ty::Struct(_) | Ty::Enum(_) | Ty::Str | Ty::Fn(_))
    }
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

/// Why a string-typed field cannot live in an arena object yet: the emitter only creates plain arenas,
/// which the collector does not scan, so a GC string stored there could be freed while still in use.
const STR_IN_ARENA: &str = "a string field in a struct or enum allocated in an arena (the collector does not scan plain arenas yet)";

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
    /// Which structs are allocated somewhere, so only their helper functions are emitted.
    sized: BTreeSet<usize>,
    masked: BTreeSet<usize>,
    uses_gc: bool,
    uses_bump: bool,
    /// Any string operation was emitted, so the runtime's string helpers are declared.
    uses_str: bool,
    /// Function types seen so far (interned, so equal shapes have equal `Ty::Fn` indexes).
    fn_tys: RefCell<Vec<FnTy>>,
    /// Closures emitted so far, for unique names.
    lam_n: usize,
    /// While a closure body is emitted and sema left its result type open (an integer literal not yet
    /// defaulted, say): the result type is taken from the first value the body returns.
    ret_infer: bool,
    ret_seen: Option<Ty>,
    /// While a closure body is emitted: every name visible in the enclosing code, so a use of one of
    /// them is reported as a capture instead of as an unknown name.
    outer_names: Vec<BTreeSet<String>>,

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
            sized: BTreeSet::new(),
            masked: BTreeSet::new(),
            uses_gc: false,
            uses_bump: false,
            uses_str: false,
            fn_tys: RefCell::new(Vec::new()),
            lam_n: 0,
            ret_infer: false,
            ret_seen: None,
            outer_names: Vec::new(),
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
        out.push_str("declare void @ubel_arena_destroy(ptr)\n");
        out.push_str("declare void @ubel_print_i64(i64)\n");
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
            out.push_str("declare i64  @ubel_i64_len(i64)\n");
            out.push_str("declare void @ubel_i64_write(ptr, i64)\n");
            out.push_str("declare void @ubel_put(ptr, ptr, i64)\n");
        }
        if self.uses_gc || self.uses_bump || self.uses_str {
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
            scope.insert(name.to_string(), Local { slot: slot.clone(), ty });
        }
        slot
    }

    fn lookup(&self, name: &str) -> Option<Local> {
        self.scopes.iter().rev().find_map(|s| s.get(name).cloned())
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
        } else {
            Err(CompileError::at(
                span,
                format!("emitter expected {} here but the expression is {}", want.ir(), got.ty.ir()),
            ))
        }
    }

    fn need_printable(&self, got: &Val, span: Span) -> R<()> {
        self.need_value(got, span)?;
        match got.ty {
            Ty::Struct(_) => return unsupported(span, "printing a struct value (print its fields)"),
            Ty::Enum(_) => return unsupported(span, "printing an enum value (match on it and print the payload)"),
            Ty::Fn(_) => return unsupported(span, "printing a closure"),
            _ => {}
        }
        Ok(())
    }

    fn need_value(&self, got: &Val, span: Span) -> R<()> {
        if got.ty == Ty::Void {
            Err(CompileError::at(span, "this expression has no value"))
        } else {
            Ok(())
        }
    }

    // ── types ────────────────────────────────────────────────────

    fn ty_of_type(&self, t: &ubel_stratum::ast::Type) -> R<Ty> {
        match t.kind {
            TypeKind::Int => Ok(Ty::I64),
            TypeKind::Bool => Ok(Ty::I1),
            TypeKind::Void => Ok(Ty::Void),
            TypeKind::Str => Ok(Ty::Str),
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
                        (None, None) => unsupported(t.span, format!("the type `{name}` (only int, bool, string, void, plain structs and enums so far)")),
                    },
                    _ => unsupported(t.span, "qualified type names"),
                }
            }
            _ => unsupported(t.span, "this type (only `int`, `bool`, `string`, `void`, plain structs and enums so far)"),
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
                    | ExprKind::If(_)
                    | ExprKind::ShortDecl { .. }
                    | ExprKind::Borrow { .. }
                    | ExprKind::Match(_)
                    | ExprKind::Lambda(_)
            );
            let mut id = start;
            loop {
                match self.ctx.types.get(id) {
                    SemaType::Int | SemaType::Bool | SemaType::Void | SemaType::Str | SemaType::Var(_) => return Ok(()),
                    SemaType::Unknown if own_type_is_authoritative => return Ok(()),
                    SemaType::Function { is_fallible: false, .. } => return Ok(()),
                    SemaType::GcRef(inner) => id = *inner,
                    SemaType::ArenaRef { inner, .. } => id = *inner,
                    SemaType::OwnedRef { mutable: false, inner } => id = *inner,
                    SemaType::Reference { mutable: false, inner, .. } => id = *inner,
                    SemaType::Named { def, args }
                        if args.is_empty()
                            && (self.struct_ids.contains_key(&self.ctx.symbols.lookup(*def).name)
                                || self.enum_ids.contains_key(&self.ctx.symbols.lookup(*def).name)) =>
                    {
                        return Ok(())
                    }
                    _ => {
                        let shown = self.ctx.types.get(start).display(&self.ctx.types, &self.ctx.symbols);
                        return unsupported(e.span, format!("values of type `{shown}` (only int, bool, string, void, plain structs and enums so far)"));
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
            if let ParamKind::Named { name, .. } = p.kind {
                let ty = sig.params[i];
                // A name no Ubel identifier can produce (it starts with a dot), so a parameter
                // register never collides with the stack slot of a local named `p`, `arg`, ...
                let reg = format!("%.arg{i}");
                header_params.push(format!("{} {}", ty.ir(), reg));
                let slot = self.declare(name, ty);
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
            StmtKind::Let { binding, value, .. } => {
                let name = match binding {
                    BindingTarget::Ident(n) => n,
                    BindingTarget::Destructure(_) => return unsupported(s.span, "destructuring `let`"),
                };
                let v = self.expr(value)?;
                self.need_value(&v, value.span)?;
                let slot = self.declare(name, v.ty);
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
                    Some(e) => Some(self.expr(e)?),
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
                    self.inst(&format!("{a} = call ptr @ubel_arena_create(i64 {bytes})"));
                    self.arenas.push(a.clone());
                    self.block(body)?;
                    // Reaching here normally means the body fell off its end; every early
                    // exit (`return`, `break`, `continue`) destroyed the arena where it left.
                    if !self.terminated {
                        self.inst(&format!("call void @ubel_arena_destroy(ptr {a})"));
                    }
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
            _ => return unsupported(iter.span, "`for` over anything but an integer range"),
        };
        let lo = self.expr(lo_e)?;
        self.need(Ty::I64, &lo, lo_e.span)?;
        let hi = self.expr(hi_e)?;
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
                if let Some(width) = self.ctx.int_literal_types.get(&e.span) {
                    return unsupported(e.span, format!("sized integers (this literal is `{}`)", width.as_str()));
                }
                Ok(Val { v: n.to_string(), ty: Ty::I64 })
            }
            ExprKind::Lit(Literal::Bool(b)) => Ok(Val { v: b.to_string(), ty: Ty::I1 }),
            ExprKind::Lit(Literal::Str(s)) | ExprKind::Lit(Literal::VerbatimStr(s)) => Ok(self.static_str(s)),
            ExprKind::Lit(Literal::InterpolatedStr(parts))
            | ExprKind::Lit(Literal::InterpolatedVerbatimStr(parts)) => self.interp_string(e, parts),
            ExprKind::Lit(_) => unsupported(e.span, "this literal as a value (only int, bool and string literals so far)"),
            ExprKind::Ident(name) => {
                let local = self
                    .lookup(name)
                    .ok_or_else(|| self.unknown_name(name, e.span, format!("not supported yet: the name `{name}` (only local variables so far)")))?;
                let t = self.fresh();
                self.inst(&format!("{t} = load {}, ptr {}", local.ty.ir(), local.slot));
                Ok(Val { v: t, ty: local.ty })
            }
            ExprKind::UnaryOp { op, operand } => {
                let v = self.expr(operand)?;
                let t = self.fresh();
                match op {
                    UnaryOp::Neg => {
                        self.need(Ty::I64, &v, operand.span)?;
                        self.inst(&format!("{t} = sub i64 0, {}", v.v));
                        Ok(Val { v: t, ty: Ty::I64 })
                    }
                    UnaryOp::BitNot => {
                        self.need(Ty::I64, &v, operand.span)?;
                        self.inst(&format!("{t} = xor i64 {}, -1", v.v));
                        Ok(Val { v: t, ty: Ty::I64 })
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
                    _ => return unsupported(target.span, "assignment to anything but a local variable or a struct field"),
                };
                let rhs = self.expr(value)?;
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
            ref other => unsupported(e.span, expr_kind_name(other)),
        }
    }

    /// Address of `obj.field`: evaluates `obj` (a pointer) and indexes into the struct.
    fn field_addr(&mut self, obj: &Expr, field: &str, span: Span) -> R<(String, Ty)> {
        let o = self.expr(obj)?;
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
        if arena.is_some() && self.structs[sid].fields.iter().any(|f| f.ty == Ty::Str) {
            return unsupported(e.span, STR_IN_ARENA);
        }

        let n = self.structs[sid].fields.len();
        let mut vals: Vec<Option<Val>> = vec![None; n];
        for init in inits {
            let (idx, fty) = self.field_index(sid, init.name, init.span)?;
            if vals[idx].is_some() {
                return Err(CompileError::at(init.span, format!("the field `{}` is given twice", init.name)));
            }
            let v = self.expr(init.value)?;
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
    /// stored, and the payload stored slot by slot.
    fn enum_ctor(&mut self, e: &Expr, eid: usize, vi: usize, args: &[Arg]) -> R<Val> {
        let fields = self.enums[eid].variants[vi].fields.clone();
        if args.len() != fields.len() {
            let v = &self.enums[eid].variants[vi].name;
            return Err(CompileError::at(
                e.span,
                format!("the variant `{v}` takes {} value(s), found {}", fields.len(), args.len()),
            ));
        }
        let arena = self.alloc_site(e.span, "an enum value")?;
        if arena.is_some() && fields.iter().any(|f| *f == Ty::Str) {
            return unsupported(e.span, STR_IN_ARENA);
        }
        let mut vals = Vec::new();
        for (i, a) in args.iter().enumerate() {
            let ae = match a.kind {
                ArgKind::Positional(x) => x,
                ArgKind::Named { .. } => return unsupported(a.span, "named arguments"),
            };
            let v = self.expr(ae)?;
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
            _ => return unsupported(scrutinee.span, "`match` on anything but an enum value"),
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

    fn binop(&mut self, op: BinOp, l: &Val, r: &Val, span: Span) -> R<Val> {
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
                _ => unsupported(span, "this operator on strings (only `==` and `!=` so far)"),
            };
        }
        if matches!(l.ty, Ty::Fn(_)) {
            return unsupported(span, "operators on closure values");
        }
        if l.ty.is_ptr() {
            return unsupported(span, "operators on struct or enum values (compare fields, or `match`, instead)");
        }
        let ty = l.ty;
        let t = self.fresh();
        let arith = |name: &str| format!("{t} = {name} {} {}, {}", ty.ir(), l.v, r.v);

        match op {
            BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Shl | BinOp::Shr => {
                if ty != Ty::I64 {
                    return unsupported(span, "this operator on bools");
                }
                let name = match op {
                    BinOp::Add => "add",
                    BinOp::Sub => "sub",
                    BinOp::Mul => "mul",
                    BinOp::Shl => "shl",
                    _ => "ashr",
                };
                self.inst(&arith(name));
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
                if ty != Ty::I64 {
                    return unsupported(span, "this operator on bools");
                }
                let zero = self.fresh();
                self.inst(&format!("{zero} = icmp eq i64 {}, 0", r.v));
                let bad = self.label("divz");
                let ok = self.label("divok");
                self.term(&format!("br i1 {zero}, label %{bad}, label %{ok}"));
                self.begin(&bad);
                self.inst("call void @ubel_trap_div_zero()");
                self.term("unreachable");
                self.begin(&ok);
                let name = if op == BinOp::Div { "sdiv" } else { "srem" };
                self.inst(&format!("{t} = {name} i64 {}, {}", l.v, r.v));
                Ok(Val { v: t, ty })
            }
            BinOp::Eq | BinOp::Ne => {
                let name = if op == BinOp::Eq { "eq" } else { "ne" };
                self.inst(&format!("{t} = icmp {name} {} {}, {}", ty.ir(), l.v, r.v));
                Ok(Val { v: t, ty: Ty::I1 })
            }
            BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
                if ty != Ty::I64 {
                    return unsupported(span, "ordering comparisons on bools");
                }
                let name = match op {
                    BinOp::Lt => "slt",
                    BinOp::Le => "sle",
                    BinOp::Gt => "sgt",
                    _ => "sge",
                };
                self.inst(&format!("{t} = icmp {name} i64 {}, {}", l.v, r.v));
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
        let name = match callee.kind {
            ExprKind::Ident(n) => n,
            _ => return unsupported(callee.span, "calling anything but a named function"),
        };
        match name {
            "println" => return self.print_call(args, true),
            "print" => return self.print_call(args, false),
            _ => {}
        }
        if let Some(local) = self.lookup(name) {
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
            let v = self.expr(ae)?;
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
                SemaType::Int => return Some(Ty::I64),
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
        if self.outer_names.iter().any(|s| s.contains(name)) {
            CompileError::at(
                span,
                format!("not supported yet: a closure that captures `{name}` (capturing closures are not emitted yet, decision D5)"),
            )
        } else {
            CompileError::at(span, other)
        }
    }

    /// `fn(v: int) body`, a closure that captures nothing. The body becomes its own function whose
    /// first, hidden parameter is the closure object, so a call is one indirect call through the code
    /// pointer in the object's first word (the Stage 1A closure ABI). The object is a static constant
    /// outside the heap: nothing is allocated, and the collector ignores pointers to it. The body is
    /// emitted in the tier of the function it is written in, and allocation sites inside it follow the
    /// usual rules from there (a closure written in HIGH code allocates in the GC heap wherever it is
    /// called from). Parameter types must be written; the result type comes from sema.
    fn lambda(&mut self, e: &Expr, lam: &Lambda) -> R<Val> {
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
        let _ = writeln!(self.funcs, "define internal {} {code}({header}) {{\nentry:\n{allocas}{body}}}\n", ret.ir());
        let _ = writeln!(self.strings, "{obj} = private unnamed_addr constant {{ ptr }} {{ ptr {code} }}, align 8");
        Ok(Val { v: obj, ty: fnty })
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
        if self.cur_tier == TierAnnotation::Low {
            return unsupported(
                e.span,
                "calling a closure in `@tier(low)` (sema does not check a closure's tier yet, decision D2, and LOW must not run code that can allocate)",
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
            let v = self.expr(ae)?;
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
    fn sema_tags_arena_string(&self, e: &Expr) -> bool {
        match self.ctx.expr_types.get(&e.span) {
            Some(&id) => matches!(self.ctx.types.get(id), SemaType::ArenaRef { .. }),
            None => false,
        }
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
                PItem::Val(v) => match v.ty {
                    Ty::I64 => self.inst(&format!("call void @ubel_print_i64(i64 {})", v.v)),
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
                    Ty::Void => unreachable!("void values are rejected when collected"),
                    Ty::Struct(_) | Ty::Enum(_) | Ty::Fn(_) => unreachable!("struct, enum and closure values are rejected when collected"),
                },
            }
        }
        Ok(Val::void())
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
                            self.need_printable(&v, expr.span)?;
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
                self.need_printable(&v, e.span)?;
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
