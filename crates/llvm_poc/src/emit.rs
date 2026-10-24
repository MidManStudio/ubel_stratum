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
//! * plain structs (fields of `int`, `bool` or another struct): literals, field reads,
//!   field assignment (also compound and nested), and passing/returning them. A struct
//!   value is a pointer, like the interpreter's reference semantics: `let b = a` aliases.
//!   A literal allocates from the GC heap in a HIGH function and from the innermost open
//!   arena inside `with arena`; anywhere else (MID outside an arena, LOW) it is refused.
//!
//! Everything else is refused with a located "not supported yet" error instead of being
//! guessed at: enums, strings as values, collections, closures, generics, struct methods,
//! `match`, sized integers, floats.
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

use std::collections::{BTreeSet, HashMap};
use std::fmt;
use std::fmt::Write as _;

use ubel_stratum::ast::{
    AllocatorKind, Arg, ArgKind, AssignOp, BinOp, BindingTarget, Block, Expr, ExprKind,
    FieldInit, FunctionDecl, IfBranchBody, IfExpr, InterpolationPart, Item, Literal, ParamKind, Program,
    SizeExpr, Span, Stmt, StmtKind, StructDecl, StructMember, TierAnnotation, TypeKind, UnaryOp,
};
use ubel_stratum::sema::{SemaContext, SemaType};

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
}

impl Ty {
    fn ir(self) -> &'static str {
        match self {
            Ty::I64 => "i64",
            Ty::I1 => "i1",
            Ty::Void => "void",
            Ty::Struct(_) => "ptr",
        }
    }
}

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
    /// Which structs are allocated somewhere, so only their helper functions are emitted.
    sized: BTreeSet<usize>,
    masked: BTreeSet<usize>,
    uses_gc: bool,
    uses_bump: bool,

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
            Item::Struct(_) => {} // registered by collect_structs
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
            sized: BTreeSet::new(),
            masked: BTreeSet::new(),
            uses_gc: false,
            uses_bump: false,
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
        if self.uses_gc || self.uses_bump {
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
        if self.uses_bump {
            out.push_str("%ubel_arena = type { ptr, ptr, ptr, i64 }\n");
        }
        if !self.structs.is_empty() {
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
                .filter(|(_, f)| matches!(f.ty, Ty::Struct(_)))
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
        if let Ty::Struct(_) = got.ty {
            return unsupported(span, "printing a struct value (print its fields)");
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
            TypeKind::Named { path, args } => {
                if !args.is_empty() {
                    return unsupported(t.span, "generic types");
                }
                match path {
                    [name] => match self.struct_ids.get(*name) {
                        Some(&sid) => Ok(Ty::Struct(sid)),
                        None => unsupported(t.span, format!("the type `{name}` (only int, bool, void and plain structs so far)")),
                    },
                    _ => unsupported(t.span, "qualified type names"),
                }
            }
            _ => unsupported(t.span, "this type (only `int`, `bool`, `void` and plain structs so far)"),
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
                if s.is_edge {
                    return unsupported(s.span, "`edge struct`");
                }
                if !s.generic_params.is_empty() {
                    return unsupported(s.span, "generic structs");
                }
                if !s.lifetime_params.is_empty() {
                    return unsupported(s.span, "structs with lifetime parameters");
                }
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
            );
            let mut id = start;
            loop {
                match self.ctx.types.get(id) {
                    SemaType::Int | SemaType::Bool | SemaType::Void | SemaType::Str | SemaType::Var(_) => return Ok(()),
                    SemaType::Unknown if own_type_is_authoritative => return Ok(()),
                    SemaType::GcRef(inner) => id = *inner,
                    SemaType::ArenaRef { inner, .. } => id = *inner,
                    SemaType::Named { def, args }
                        if args.is_empty() && self.struct_ids.contains_key(&self.ctx.symbols.lookup(*def).name) =>
                    {
                        return Ok(())
                    }
                    _ => {
                        let shown = self.ctx.types.get(start).display(&self.ctx.types, &self.ctx.symbols);
                        return unsupported(e.span, format!("values of type `{shown}` (only int, bool, void and plain structs so far)"));
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

        let n = f.body.stmts.len();
        for (i, s) in f.body.stmts.iter().enumerate() {
            if i + 1 == n && self.ret != Ty::Void {
                // The last expression of a non-void function is its result.
                if let StmtKind::Expr(e) = s.kind {
                    let v = self.expr(e)?;
                    if v.ty == self.ret {
                        self.do_return(Some(v), s.span)?;
                        continue;
                    }
                }
            }
            self.stmt(s)?;
        }

        if !self.terminated {
            if self.ret == Ty::Void {
                self.term("ret void");
            } else {
                self.inst("call void @ubel_trap_no_return()");
                self.term("unreachable");
            }
        }

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
                self.expr(e)?;
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
            ExprKind::Lit(_) => unsupported(e.span, "this literal as a value (only int and bool, and strings inside `println`)"),
            ExprKind::Ident(name) => {
                let local = self
                    .lookup(name)
                    .ok_or_else(|| CompileError::at(e.span, format!("not supported yet: the name `{name}` (only local variables so far)")))?;
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
                            .ok_or_else(|| CompileError::at(target.span, format!("not supported yet: assignment to `{name}` (only local variables and struct fields so far)")))?;
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
            ExprKind::If(ife) => self.if_value(ife, e.span),
            ExprKind::StructLit { path, fields } => self.struct_lit(e, path, fields),
            ExprKind::Field { target, field } => {
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
        let arena = self.arenas.last().cloned();
        if arena.is_none() {
            match self.cur_tier {
                TierAnnotation::High => {}
                TierAnnotation::Mid => {
                    return unsupported(
                        e.span,
                        "a struct literal in a `@tier(mid)` function outside `with arena` (where it allocates is not decided yet)",
                    )
                }
                TierAnnotation::Low => {
                    return unsupported(e.span, "struct literals in `@tier(low)` (how LOW allocates is not decided yet)")
                }
            }
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
        let p = self.fresh();
        match arena {
            Some(a) => {
                self.uses_bump = true;
                let up = self.fresh();
                self.inst(&format!("{up} = add i64 {size}, 15"));
                let rounded = self.fresh();
                self.inst(&format!("{rounded} = and i64 {up}, -16"));
                self.inst(&format!("{p} = call ptr @ubel_arena_bump(ptr {a}, i64 {rounded}) ; MID: bump allocation of {sname}"));
            }
            None => {
                self.uses_gc = true;
                let has_ptr = self.structs[sid].fields.iter().any(|f| matches!(f.ty, Ty::Struct(_)));
                let mask = if has_ptr {
                    self.masked.insert(sid);
                    let m = self.fresh();
                    self.inst(&format!("{m} = call i64 @ubel_mask.{sname}()"));
                    m
                } else {
                    "0".to_string()
                };
                self.inst(&format!("{p} = call ptr @ubel_gc_alloc(i64 {size}, i64 {mask}) ; HIGH: GC allocation of {sname}"));
            }
        }
        for (i, v) in vals.into_iter().enumerate() {
            let v = v.expect("checked above");
            let g = self.fresh();
            self.inst(&format!("{g} = getelementptr inbounds %struct.{sname}, ptr {p}, i32 0, i32 {i}"));
            self.inst(&format!("store {} {}, ptr {g}", v.ty.ir(), v.v));
        }
        Ok(Val { v: p, ty: Ty::Struct(sid) })
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
        if let Ty::Struct(_) = l.ty {
            return unsupported(span, "operators on struct values (compare the fields instead)");
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
        let name = match callee.kind {
            ExprKind::Ident(n) => n,
            _ => return unsupported(callee.span, "calling anything but a named function"),
        };
        match name {
            "println" => return self.print_call(args, true),
            "print" => return self.print_call(args, false),
            _ => {}
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

    fn string_const(&mut self, s: &str) -> (String, usize) {
        let name = format!("@.str.{}", self.str_n);
        self.str_n += 1;
        let bytes = s.as_bytes();
        let mut esc = String::new();
        for &b in bytes {
            if (0x20..0x7f).contains(&b) && b != b'"' && b != b'\\' {
                esc.push(b as char);
            } else {
                let _ = write!(esc, "\\{:02X}", b);
            }
        }
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
                    Ty::Void => unreachable!("void values are rejected when collected"),
                    Ty::Struct(_) => unreachable!("struct values are rejected when collected"),
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
