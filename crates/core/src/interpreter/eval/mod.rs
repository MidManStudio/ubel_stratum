// ============================================================================
// NOTICE: Full documentation, design decisions, and fix history for this file
// live in docs/ubel_stratum.md, section "interpreter/eval/mod.rs"
// ============================================================================
// src/interpreter/eval/mod.rs
//! Interpreter struct, function table, and call dispatch.

#![allow(dead_code)]

pub mod expr;
pub mod stmt;
pub mod pattern;

#[cfg(test)]
mod tests;

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use crate::ast::arena::AstArena;
use crate::ast::common::TierAnnotation;
use crate::ast::declarations::{FunctionDecl, MethodDecl, ParamKind, StructMember};
use crate::ast::expressions::Expr;
use crate::ast::root::{Item, Program};
use crate::ast::statements::Block;
use crate::ast::types::{Type, TypeKind};
use crate::builtins::BuiltinFn;
use crate::interpreter::env::Environment;
use crate::interpreter::value::{EvalResult, FunctionId, Signal, Value};

// ── FunctionBody ──────────────────────────────────────────────────

pub enum FunctionBody<'ast> {
    /// User-defined function with a block body.
    Ast { block: Block<'ast> },
    /// Lambda with an expression body: `fn(x) x * 2`
    /// Stored as a reference into the arena so no allocation is needed.
    ExprBody { expr: &'ast Expr<'ast> },
    /// Native Rust built-in.
    Builtin(BuiltinFn),
}

// ── FunctionDef ───────────────────────────────────────────────────

pub struct FunctionDef<'ast> {
    pub name:     Option<String>,
    /// Parameter names in declaration order.
    /// `self` / `&self` / `mut self` params are excluded — they are bound
    /// by `call_method` directly.
    pub params:   Vec<String>,
    pub body:     FunctionBody<'ast>,
    /// Environment captured at definition time (closure snapshot).
    pub closure:  Environment,
    pub tier:     TierAnnotation,
    pub is_async: bool,
}

// ── Interpreter ───────────────────────────────────────────────────

/// Tree-walking interpreter.
///
/// The `'ast` lifetime is needed because `FunctionDef` stores arena-allocated
/// `Block<'ast>` and `&'ast Expr<'ast>` references.  `Value` and `Environment`
/// are both lifetime-free, so closures and runtime values can be passed around
/// freely without the compiler tracking AST provenance through them.
/// A variant's payload shape, as far as the interpreter needs to know to
/// construct it — no element/field types, since sema has already
/// validated those; see `Interpreter::enum_table`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum VariantKind {
    /// Fieldless or Discriminant — both construct `EnumPayload::None`;
    /// the interpreter doesn't track a discriminant's chosen ordinal
    /// (see MEMORY_MODEL.md's enum-section "Known gap").
    Fieldless,
    Tuple,
    Struct,
}

pub struct Interpreter<'ast> {
    pub(crate) env:          Environment,
    pub(crate) functions:    Vec<FunctionDef<'ast>>,
    /// struct_type_name → method_name → FunctionId.
    /// Static and instance methods both live here; the call site determines
    /// whether a receiver is passed.
    pub(crate) method_table: HashMap<String, HashMap<String, FunctionId>>,
    /// enum_type_name → variant_name → its payload kind. Consulted by
    /// `EnumName.Variant` construction (bare field access for
    /// fieldless/discriminant, call syntax for tuple, struct-literal
    /// syntax for struct — ENUM_RULES.md) to tell "construct an enum
    /// value" apart from an ordinary field/method lookup. Sema has
    /// already validated arity/types by the time any of these run, so
    /// construction here doesn't re-check either.
    pub(crate) enum_table:   HashMap<String, HashMap<String, VariantKind>>,
    /// struct_type_name → set of that type's `@derive`d trait names.
    /// Only `"PartialEq"` is ever inserted today (sema's already
    /// rejected anything else by the time `run_program` runs — see
    /// `TYPE-116` — so this doesn't re-validate, just re-derives the
    /// same fact sema already established, the same relationship
    /// `method_table`/`enum_table` have with their own sema passes).
    /// Consulted once, at `ExprKind::StructLit` construction, to set
    /// `Value::Struct::derives_partial_eq` — not re-checked per
    /// comparison.
    pub(crate) struct_derives: HashMap<String, HashSet<String>>,
    /// struct_type_name → field names in declaration order. Built in the
    /// same pre-declare pass as `struct_derives`, from the same `s:
    /// StructDecl` walk (`StructMember::Field` members, in encounter
    /// order) — re-derives an already-available fact rather than reading
    /// back through sema's own `struct_fields` (def_id-keyed, not
    /// type-name-keyed, and not something the interpreter otherwise
    /// shares with sema). Consulted once, at `ExprKind::StructLit`
    /// construction, to set `Value::Struct::field_order` — see that
    /// field's own doc comment in `interpreter/value.rs` for why a
    /// declaration-derived order matters for `partial_cmp`/`compute_hash`
    /// and not just `HashMap`'s own (unstable, unordered) iteration.
    pub(crate) struct_field_order: HashMap<String, Rc<Vec<String>>>,
    /// The arena used to parse the program.  Kept here so interpolated-string
    /// expression holes (`$"Hello {expr}"`) can be parsed at runtime via
    /// `crate::parser::parse_expr`.
    pub(crate) arena:        &'ast AstArena,
    /// Ambient capacity for the innermost enclosing `with pool<T>(count)
    /// { }` block, pushed/popped by `StmtKind::With`'s execution
    /// (MEMORY_MODEL.md §11). `Pool.new()` has no generic argument of
    /// its own to construct from — unlike `List.new()` etc. — so it
    /// reads capacity from here rather than from its own call args.
    pub(crate) pool_capacity_stack: Vec<usize>,
}

impl<'ast> Interpreter<'ast> {
    pub fn new(arena: &'ast AstArena) -> Self {
        let mut interp = Interpreter {
            env:          Environment::new(),
            functions:    Vec::new(),
            method_table: HashMap::new(),
            enum_table:   HashMap::new(),
            struct_derives: HashMap::new(),
            struct_field_order: HashMap::new(),
            arena,
            pool_capacity_stack: Vec::new(),
        };
        interp.register_builtins();
        interp
    }

    // ── Registration ─────────────────────────────────────────────

    fn register_builtins(&mut self) {
        for sig in crate::builtins::global::GLOBAL_BUILTINS {
            let id = self.functions.len();
            self.functions.push(FunctionDef {
                name:     Some(sig.name.to_string()),
                params:   vec![],
                body:     FunctionBody::Builtin(sig.run),
                closure:  Environment::new(),
                tier:     TierAnnotation::High,
                is_async: false,
            });
            self.env.define(sig.name, Value::Function(id));
        }
    }

    pub fn alloc_function(&mut self, def: FunctionDef<'ast>) -> FunctionId {
        let id = self.functions.len();
        self.functions.push(def);
        id
    }

    /// Register a top-level function declaration.
    /// Takes `FunctionDecl<'ast>` by value — it is `Copy` so fields
    /// (`body: Block<'ast>`, etc.) are copied off the stack while the
    /// underlying arena data they point to lives as long as `'ast`.
    pub fn register_fn(&mut self, f: FunctionDecl<'ast>) -> FunctionId {
        // enumerate() + a synthesized name for Discard, not filter_map
        // dropping it: param_names gets zipped positionally against real
        // call arguments at call sites (see e.g. line ~314 below) --
        // dropping a slot here would shift every later argument onto the
        // wrong name. `$`-prefixed: identifiers can't start with `$`
        // (Letter (Letter|Digit|_)*), so this can never collide with a
        // real binding a user could write or read.
        let params: Vec<String> = f.params.iter().enumerate()
            .filter_map(|(i, p)| match p.kind {
                ParamKind::Named { name, .. } => Some(name.to_string()),
                ParamKind::Discard { .. } => Some(format!("$discard{i}")),
                _ => None,
            })
            .collect();
        let block    = f.body;  // Block<'ast> is Copy
        let closure  = self.env.snapshot();
        let name_str = f.name.to_string();
        let tier     = f.tier;
        let is_async = f.is_async;
        self.alloc_function(FunctionDef {
            name:    Some(name_str),
            params,
            body:    FunctionBody::Ast { block },
            closure,
            tier,
            is_async,
        })
    }

    /// Register a struct method. The FunctionId is stored in `method_table`.
    pub fn register_method(&mut self, struct_name: &str, m: MethodDecl<'ast>) -> FunctionId {
        // Only Named/Discard params contribute a slot -- `self` variants
        // are handled at call sites. Same arity-preserving reasoning as
        // register_fn just above: Discard gets a synthesized `$`-prefixed
        // name (never a real, writable/readable identifier) instead of
        // being dropped, so later params don't shift onto the wrong
        // argument at call time.
        let params: Vec<String> = m.params.iter().enumerate()
            .filter_map(|(i, p)| match p.kind {
                ParamKind::Named { name, .. } => Some(name.to_string()),
                ParamKind::Discard { .. } => Some(format!("$discard{i}")),
                _ => None,
            })
            .collect();
        let block    = m.body;
        let closure  = self.env.snapshot();
        let name_str = format!("{}::{}", struct_name, m.name);
        let tier     = m.tier;
        let is_async = m.is_async;
        self.alloc_function(FunctionDef {
            name:    Some(name_str),
            params,
            body:    FunctionBody::Ast { block },
            closure,
            tier,
            is_async,
        })
    }

    /// The `extend`/inherent-`impl` twin of the `Item::Struct` arm's own
    /// method registration a few lines down in `run_program` — same
    /// `register_method` + `method_table` insert per method, just reached
    /// from a `target_type` instead of a struct's own member list. `None`
    /// for anything that isn't a plain named path (nothing else is a
    /// sensible `extend`/`impl` target) silently registers nothing, same
    /// lenient handling sema's own `target_type_def_id` gives it.
    fn register_extend_or_impl_methods(
        &mut self,
        target_type: &'ast Type<'ast>,
        methods:     &'ast [MethodDecl<'ast>],
        top_level_fns: &mut Vec<FunctionId>,
    ) {
        let TypeKind::Named { path, .. } = target_type.kind else { return; };
        let Some(struct_name) = path.first().copied() else { return; };
        for m in methods.iter().copied() {
            let id = self.register_method(struct_name, m);
            self.method_table
                .entry(struct_name.to_string())
                .or_default()
                .insert(m.name.to_string(), id);
            top_level_fns.push(id);
        }
    }

    // ── Program entry point ───────────────────────────────────────

    pub fn run_program(&mut self, program: &'ast Program<'ast>) -> Result<(), String> {
        // Pre-declare pass: register everything before running any body so
        // forward references and mutual recursion work.
        let mut top_level_fns: Vec<FunctionId> = Vec::new();
        for item in program.items.iter().copied() {
            match item {
                Item::Function(f) => {
                    // f: FunctionDecl<'ast> — Copy value whose fields point into 'ast arena
                    let id = self.register_fn(f);
                    self.env.define(f.name, Value::Function(id));
                    top_level_fns.push(id);
                }
                Item::Struct(s) => {
                    // Sema (TYPE-116) already rejected anything but
                    // `PartialEq` here -- this just re-derives the same
                    // already-validated fact, same relationship
                    // method_table/enum_table have with their own passes.
                    let derived: HashSet<String> =
                        crate::ast::common::derive_trait_names(s.attributes)
                            .into_iter()
                            .map(|name| name.to_string())
                            .collect();
                    if !derived.is_empty() {
                        self.struct_derives.insert(s.name.to_string(), derived);
                    }
                    let field_names: Vec<String> = s.members.iter()
                        .filter_map(|m| match m {
                            StructMember::Field(f) => Some(f.name.to_string()),
                            _ => None,
                        })
                        .collect();
                    self.struct_field_order.insert(s.name.to_string(), Rc::new(field_names));
                    for member in s.members.iter().copied() {
                        if let StructMember::Method(m) = member {
                            let id = self.register_method(s.name, m);
                            self.method_table
                                .entry(s.name.to_string())
                                .or_default()
                                .insert(m.name.to_string(), id);
                            top_level_fns.push(id);
                        }
                    }
                }
                Item::Enum(e) => {
                    use crate::ast::declarations::EnumVariantPayload as P;
                    let variants: HashMap<String, VariantKind> = e.variants.iter()
                        .map(|v| {
                            let kind = match v.payload {
                                P::None | P::Discriminant(_) => VariantKind::Fieldless,
                                P::Tuple(_)                  => VariantKind::Tuple,
                                P::Struct(_)                 => VariantKind::Struct,
                            };
                            (v.name.to_string(), kind)
                        })
                        .collect();
                    self.enum_table.insert(e.name.to_string(), variants);
                }
                // Interpreter-side twin of sema's own
                // `register_extend_impl_methods` (`type_infer.rs`) — same
                // root cause, same fix shape: nothing here ever looked at
                // `target_type` before, so `method_table` (what
                // `receiver.method()` dispatch actually looks up, just
                // below) never got an entry for an `extend`/`impl`-block
                // method, even once sema was taught to accept the call.
                // Trait `impl`s are skipped for the same reason sema
                // skips them: traits aren't dispatched through yet.
                Item::Extend(x)                                => {
                    self.register_extend_or_impl_methods(x.target_type, x.methods, &mut top_level_fns);
                }
                Item::Impl(i) if i.trait_path.is_none() => {
                    self.register_extend_or_impl_methods(i.target_type, i.methods, &mut top_level_fns);
                }
                _ => {}
            }
        }

        // `register_fn`/`register_method` snapshot `self.env` for the
        // closure at the moment each item is registered — which, mid-loop,
        // is *before* later top-level items have been declared. A function
        // declared early in the file (e.g. `main`) would otherwise end up
        // with a closure that can't see a function declared later in the
        // same file (e.g. `greet`), even though both are top-level and
        // should share one module scope. Backfill every top-level
        // fn/method's closure with a single snapshot taken now, after the
        // whole pre-declare pass has finished.
        let module_scope = self.env.snapshot();
        for id in top_level_fns {
            self.functions[id].closure = module_scope.clone();
        }

        let main_val = self.env.get("main").cloned()
            .ok_or_else(|| "no `main` function found".to_string())?;

        match main_val {
            Value::Function(id) => match self.call_function(id, &[]) {
                Ok(_) | Err(Signal::Return(_)) => Ok(()),
                Err(Signal::Panic(msg)) => Err(format!("panic: {}", msg)),
                Err(Signal::Fail(v))    => Err(format!("unhandled fail: {}", v)),
                Err(Signal::Break(_))   => Err("break outside loop".to_string()),
                Err(Signal::Continue)   => Err("continue outside loop".to_string()),
            },
            _ => Err("`main` is not a function".to_string()),
        }
    }

    // ── Call dispatch ─────────────────────────────────────────────

    /// Call a function by FunctionId with already-evaluated arguments.
    ///
    /// Uses a local enum to carry extracted body data so the immutable borrow
    /// on `self.functions` is fully released before any mutation of `self.env`.
    pub fn call_function(&mut self, id: FunctionId, args: &[Value]) -> EvalResult {
        // Phase 1: extract body data — immutable borrow on self.functions.
        enum BodyData<'a> {
            Builtin(BuiltinFn),
            Block(Block<'a>),
            Expr(&'a Expr<'a>),
        }

        let (body, param_names, closure) = {
            let def = &self.functions[id];
            let body = match &def.body {
                FunctionBody::Builtin(f)        => BodyData::Builtin(*f),
                FunctionBody::Ast { block }     => BodyData::Block(*block),
                FunctionBody::ExprBody { expr } => BodyData::Expr(expr),
            };
            (body, def.params.clone(), def.closure.clone())
        }; // ← immutable borrow released here

        // Phase 2: run the body — may mutate self freely.
        match body {
            BodyData::Builtin(f) => f(args),

            BodyData::Block(block) => {
                let caller_env = std::mem::replace(&mut self.env, closure);
                self.env.push();
                for (name, val) in param_names.iter().zip(args.iter()) {
                    self.env.define(name, val.clone());
                }
                let result = stmt::eval_block(self, &block);
                self.env = caller_env;
                match result {
                    Ok(v) | Err(Signal::Return(v)) => Ok(v),
                    Err(other)                     => Err(other),
                }
            }

            BodyData::Expr(e) => {
                let caller_env = std::mem::replace(&mut self.env, closure);
                self.env.push();
                for (name, val) in param_names.iter().zip(args.iter()) {
                    self.env.define(name, val.clone());
                }
                let result = expr::eval_expr(self, e);
                self.env = caller_env;
                match result {
                    Ok(v) | Err(Signal::Return(v)) => Ok(v),
                    Err(other)                     => Err(other),
                }
            }
        }
    }

    /// Call a struct method with an explicit `self` receiver.
    pub fn call_method(
        &mut self,
        id:       FunctionId,
        receiver: Value,
        args:     &[Value],
    ) -> EvalResult {
        enum BodyData<'a> {
            Builtin(BuiltinFn),
            Block(Block<'a>),
            Expr(&'a Expr<'a>),
        }

        let (body, param_names, closure) = {
            let def = &self.functions[id];
            let body = match &def.body {
                FunctionBody::Builtin(f)        => BodyData::Builtin(*f),
                FunctionBody::Ast { block }     => BodyData::Block(*block),
                FunctionBody::ExprBody { expr } => BodyData::Expr(expr),
            };
            (body, def.params.clone(), def.closure.clone())
        };

        match body {
            BodyData::Builtin(f) => {
                let mut all = vec![receiver];
                all.extend_from_slice(args);
                f(&all)
            }
            BodyData::Block(block) => {
                let caller_env = std::mem::replace(&mut self.env, closure);
                self.env.push();
                self.env.define("self", receiver);
                for (name, val) in param_names.iter().zip(args.iter()) {
                    self.env.define(name, val.clone());
                }
                let result = stmt::eval_block(self, &block);
                self.env = caller_env;
                match result {
                    Ok(v) | Err(Signal::Return(v)) => Ok(v),
                    Err(other)                     => Err(other),
                }
            }
            BodyData::Expr(e) => {
                let caller_env = std::mem::replace(&mut self.env, closure);
                self.env.push();
                self.env.define("self", receiver);
                for (name, val) in param_names.iter().zip(args.iter()) {
                    self.env.define(name, val.clone());
                }
                let result = expr::eval_expr(self, e);
                self.env = caller_env;
                match result {
                    Ok(v) | Err(Signal::Return(v)) => Ok(v),
                    Err(other)                     => Err(other),
                }
            }
        }
    }

    pub fn lookup(&self, name: &str) -> EvalResult {
        self.env.get(name)
            .cloned()
            .ok_or_else(|| Signal::Panic(format!("undefined name '{}'", name)))
    }
    }
