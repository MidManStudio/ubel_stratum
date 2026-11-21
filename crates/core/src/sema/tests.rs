// crates/core/src/sema/tests.rs
//! Sema unit tests — AST built directly from the arena, no parser needed.
//!
//! NOTE: sema enforces return-type annotation when a value is returned.
//! Tests that want to verify expressions work use StmtKind::Expr (expression
//! statements) rather than StmtKind::Return(Some(...)) to avoid triggering
//! the return-type check on functions with no declared return type.

use crate::ast::arena::AstArena;
use crate::ast::common::{AssignOp, Span, TierAnnotation, Visibility};
use crate::ast::declarations::{FunctionDecl, Param, ParamKind, ReturnType, StructDecl};
use crate::ast::expressions::{Arg, ArgKind, Expr, ExprKind};
use crate::ast::literals::{FormatSpec, InterpolationPart, Literal};
use crate::ast::root::{Program, Item};
use crate::ast::statements::{
    AllocatorKind, Block, SizeExpr, SizeUnit, Stmt, StmtKind, BindingTarget,
};
use crate::ast::types::{Type, TypeKind};
use crate::error_management::errors::{TierError, TypeError};
use crate::sema;
use crate::sema::type_table::SemaType;

// ── Helpers ───────────────────────────────────────────────────────────────────

const Z: Span = Span { start: 0, end: 0, line: 0, column: 0 };

/// `name_resolution`'s `resolutions: HashMap<Span, DefId>` and
/// `type_infer`'s `expr_types: HashMap<Span, TypeId>` are both keyed
/// purely by `Span` — real source always gives every token a distinct
/// span, but hand-built AST nodes using the shared `Z` constant above
/// don't. That's invisible for tests with only one meaningfully-resolved
/// identifier, but a test with *several* distinct identifier references
/// (e.g. a function name and a builtin namespace name in the same body)
/// will have later ones silently overwrite earlier ones' slot in both
/// maps, corrupting lookups in a way that has nothing to do with
/// whatever the test is actually trying to check. Use this for any
/// `Ident`/call-target node beyond the first in a single test.
fn span_n(n: usize) -> Span { Span { start: n, end: n + 1, line: 0, column: n } }

fn empty_prog(arena: &AstArena) -> Program<'_> {
    Program { package: None, imports: &[], items: arena.alloc_slice_copy(&[]), span: Z }
}

fn make_fn<'a>(arena: &'a AstArena, name: &str, body: Block<'a>) -> FunctionDecl<'a> {
    FunctionDecl {
        tier:            TierAnnotation::default(),
        attributes:      &[],
        visibility:      Visibility::default(),
        is_async:        false,
        name:            arena.alloc_str(name),
        lifetime_params: &[],
        generic_params:  &[],
        params:          &[],
        return_type:     None,
        body,
        span:            Z,
    }
}

fn prog_with<'a>(arena: &'a AstArena, items: &[Item<'a>]) -> Program<'a> {
    Program { package: None, imports: &[], items: arena.alloc_slice_copy(items), span: Z }
}

fn assert_ok(arena: &AstArena, prog: &Program<'_>) {
    assert!(
        sema::analyse(prog, arena, String::new()).is_ok(),
        "sema should succeed but returned an error"
    );
}

fn expr_stmt<'a>(arena: &'a AstArena, kind: ExprKind<'a>) -> Stmt<'a> {
    let e = arena.alloc(Expr { kind, span: Z });
    Stmt { kind: StmtKind::Expr(e), span: Z }
}

/// Like `make_fn`, but with an explicit tier — needed for `with arena(...)`
/// tests, since `with arena` is only legal inside `@tier(mid)`.
fn make_fn_tiered<'a>(
    arena: &'a AstArena, name: &str, tier: TierAnnotation, body: Block<'a>,
) -> FunctionDecl<'a> {
    FunctionDecl {
        tier,
        attributes:      &[],
        visibility:      Visibility::default(),
        is_async:        false,
        name:            arena.alloc_str(name),
        lifetime_params: &[],
        generic_params:  &[],
        params:          &[],
        return_type:     None,
        body,
        span:            Z,
    }
}

/// Build `<ns>.new()` as a Call expression — e.g.
/// `builtin_ctor_call(&arena, "List", CALL_SPAN)` for `List.new()`.
/// Only the outer Call gets `call_span`; the callee/target sub-expressions
/// keep span `Z`, since the test only ever looks up `call_span`.
fn builtin_ctor_call<'a>(arena: &'a AstArena, ns: &str, call_span: Span) -> &'a Expr<'a> {
    let target = arena.alloc(Expr { kind: ExprKind::Ident(arena.alloc_str(ns)), span: Z });
    let callee = arena.alloc(Expr {
        kind: ExprKind::Field { target, field: arena.alloc_str("new") },
        span: Z,
    });
    let args: &[Arg] = &[];
    arena.alloc(Expr { kind: ExprKind::Call { callee, args }, span: call_span })
}

/// A single-type-argument named type node, e.g. `named1(arena, "Unique",
/// int_ty)` builds `Unique<int>`. Used for the ownership-model axis
/// (`Unique`/`Shared`/`SyncShared`) tests below — these go through the
/// generic `TypeKind::Named` path (no dedicated `TypeKind` variant), same
/// as any other user-named generic type.
fn named1<'a>(arena: &'a AstArena, name: &'static str, inner: &'a Type<'a>) -> &'a Type<'a> {
    let path = arena.alloc_slice_copy(&[name]);
    let args = arena.alloc_slice_copy(&[inner]);
    arena.alloc(Type { kind: TypeKind::Named { path, args }, span: Z })
}

/// A zero-type-argument named type node, e.g. `named0(arena, "Unique")`
/// builds bare `Unique` — used to exercise the arg-count check.
fn named0<'a>(arena: &'a AstArena, name: &'static str) -> &'a Type<'a> {
    let path = arena.alloc_slice_copy(&[name]);
    arena.alloc(Type { kind: TypeKind::Named { path, args: &[] }, span: Z })
}

fn int_type(arena: &AstArena) -> &Type<'_> {
    arena.alloc(Type { kind: TypeKind::Int, span: Z })
}

fn str_type(arena: &AstArena) -> &Type<'_> {
    arena.alloc(Type { kind: TypeKind::Str, span: Z })
}

fn bool_type(arena: &AstArena) -> &Type<'_> {
    arena.alloc(Type { kind: TypeKind::Bool, span: Z })
}

fn param_named<'a>(arena: &'a AstArena, name: &str, ty: &'a Type<'a>) -> Param<'a> {
    Param {
        kind: ParamKind::Named { mutable: false, name: arena.alloc_str(name), ty: Some(ty), default: None },
        span: Z,
    }
}

/// `_: Type` — see `ParamKind::Discard`'s own doc comment for why it
/// carries no name and no default.
fn param_discard<'a>(ty: &'a Type<'a>) -> Param<'a> {
    Param { kind: ParamKind::Discard { ty: Some(ty) }, span: Z }
}

fn call_expr_multi<'a>(arena: &'a AstArena, callee_name: &str, args: &[&'a Expr<'a>]) -> &'a Expr<'a> {
    let callee = arena.alloc(Expr { kind: ExprKind::Ident(arena.alloc_str(callee_name)), span: Z });
    let arg_nodes: Vec<Arg> = args.iter().map(|a| Arg { kind: ArgKind::Positional(a), span: Z }).collect();
    let args_slice: &[Arg] = arena.alloc_slice_copy(&arg_nodes);
    arena.alloc(Expr { kind: ExprKind::Call { callee, args: args_slice }, span: Z })
}

/// Like `make_fn`/`make_fn_tiered`, but with real params and an optional
/// return type — needed for the ownership-model tests below, which (with
/// no construction syntax yet for `Unique<T>`/`Shared<T>`/`SyncShared<T>`)
/// can only get a value of one of these types via a *parameter's own
/// declared type*, not a constructed expression.
fn make_fn_full<'a>(
    arena: &'a AstArena, name: &str, params: &[Param<'a>],
    return_type: Option<ReturnType<'a>>, body: Block<'a>,
) -> FunctionDecl<'a> {
    FunctionDecl {
        tier:            TierAnnotation::default(),
        attributes:      &[],
        visibility:      Visibility::default(),
        is_async:        false,
        name:            arena.alloc_str(name),
        lifetime_params: &[],
        generic_params:  &[],
        params:          arena.alloc_slice_copy(params),
        return_type,
        body,
        span:            Z,
    }
}

fn call_expr<'a>(arena: &'a AstArena, callee_name: &str, arg: &'a Expr<'a>) -> &'a Expr<'a> {
    let callee = arena.alloc(Expr { kind: ExprKind::Ident(arena.alloc_str(callee_name)), span: Z });
    let args: &[Arg] = arena.alloc_slice_copy(&[Arg { kind: ArgKind::Positional(arg), span: Z }]);
    arena.alloc(Expr { kind: ExprKind::Call { callee, args }, span: Z })
}

/// `Namespace.new(args...)` — the construction idiom every builtin
/// wrapper in this language uses (`List.new()`, `Pool.new()`, and now
/// `Unique.new(v)`/`Shared.new(v)`/`SyncShared.new(v)`).
fn namespace_new_call<'a>(arena: &'a AstArena, namespace: &str, args: &[&'a Expr<'a>]) -> &'a Expr<'a> {
    let target = arena.alloc(Expr { kind: ExprKind::Ident(arena.alloc_str(namespace)), span: Z });
    let callee = arena.alloc(Expr { kind: ExprKind::Field { target, field: "new" }, span: Z });
    let arg_nodes: Vec<Arg> = args.iter().map(|a| Arg { kind: ArgKind::Positional(a), span: Z }).collect();
    let args_slice: &[Arg] = arena.alloc_slice_copy(&arg_nodes);
    arena.alloc(Expr { kind: ExprKind::Call { callee, args: args_slice }, span: Z })
}

fn let_stmt<'a>(arena: &'a AstArena, name: &str, value: &'a Expr<'a>) -> Stmt<'a> {
    Stmt {
        kind: StmtKind::Let {
            mutable: false,
            binding: BindingTarget::Ident(arena.alloc_str(name)),
            ty:      None,
            value,
        },
        span: Z,
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[test]
fn test_sema_empty_program_ok() {
    let arena = AstArena::new();
    assert_ok(&arena, &empty_prog(&arena));
}

#[test]
fn test_sema_fn_empty_body_ok() {
    let arena = AstArena::new();
    let f = make_fn(&arena, "helper", Block::empty(Z));
    assert_ok(&arena, &prog_with(&arena, &[Item::Function(f)]));
}

#[test]
fn test_sema_fn_explicit_return_unit_ok() {
    // fn foo() { return }  — no value, matches void return type
    let arena = AstArena::new();
    let stmts = arena.alloc_slice_copy(&[Stmt { kind: StmtKind::Return(None), span: Z }]);
    let f     = make_fn(&arena, "foo", Block { stmts, span: Z });
    assert_ok(&arena, &prog_with(&arena, &[Item::Function(f)]));
}

#[test]
fn test_sema_fn_expr_int_ok() {
    // fn foo() { 42 }  — int literal as an expression statement (no return)
    let arena = AstArena::new();
    let stmts = arena.alloc_slice_copy(&[expr_stmt(&arena, ExprKind::Lit(Literal::Int(42)))]);
    let f     = make_fn(&arena, "foo", Block { stmts, span: Z });
    assert_ok(&arena, &prog_with(&arena, &[Item::Function(f)]));
}

#[test]
fn test_sema_fn_expr_bool_ok() {
    let arena = AstArena::new();
    let stmts = arena.alloc_slice_copy(&[expr_stmt(&arena, ExprKind::Lit(Literal::Bool(true)))]);
    let f     = make_fn(&arena, "foo", Block { stmts, span: Z });
    assert_ok(&arena, &prog_with(&arena, &[Item::Function(f)]));
}

#[test]
fn test_sema_fn_expr_float_ok() {
    let arena = AstArena::new();
    let stmts = arena.alloc_slice_copy(&[expr_stmt(&arena, ExprKind::Lit(Literal::Double(3.14)))]);
    let f     = make_fn(&arena, "foo", Block { stmts, span: Z });
    assert_ok(&arena, &prog_with(&arena, &[Item::Function(f)]));
}

#[test]
fn test_sema_fn_expr_str_ok() {
    let arena = AstArena::new();
    let s     = arena.alloc_str("hello");
    let stmts = arena.alloc_slice_copy(&[expr_stmt(&arena, ExprKind::Lit(Literal::Str(s)))]);
    let f     = make_fn(&arena, "foo", Block { stmts, span: Z });
    assert_ok(&arena, &prog_with(&arena, &[Item::Function(f)]));
}

#[test]
fn test_sema_fn_expr_null_ok() {
    let arena = AstArena::new();
    let stmts = arena.alloc_slice_copy(&[expr_stmt(&arena, ExprKind::Lit(Literal::Null))]);
    let f     = make_fn(&arena, "foo", Block { stmts, span: Z });
    assert_ok(&arena, &prog_with(&arena, &[Item::Function(f)]));
}

#[test]
fn test_sema_fn_let_binding_ok() {
    let arena = AstArena::new();
    let val   = arena.alloc(Expr { kind: ExprKind::Lit(Literal::Int(10)), span: Z });
    let stmts = arena.alloc_slice_copy(&[Stmt {
        kind: StmtKind::Let {
            mutable: false,
            binding: BindingTarget::Ident(arena.alloc_str("x")),
            ty:      None,
            value:   val,
        },
        span: Z,
    }]);
    let f = make_fn(&arena, "with_let", Block { stmts, span: Z });
    assert_ok(&arena, &prog_with(&arena, &[Item::Function(f)]));
}

#[test]
fn test_sema_fn_let_then_read_ok() {
    // fn foo() { let x = 1; x }  — bind then use the name as an expr statement
    let arena = AstArena::new();
    let val   = arena.alloc(Expr { kind: ExprKind::Lit(Literal::Int(1)), span: Z });
    let let_s = Stmt {
        kind: StmtKind::Let {
            mutable: false,
            binding: BindingTarget::Ident(arena.alloc_str("x")),
            ty:      None,
            value:   val,
        },
        span: Z,
    };
    let read_s = expr_stmt(&arena, ExprKind::Ident(arena.alloc_str("x")));
    let stmts  = arena.alloc_slice_copy(&[let_s, read_s]);
    let f      = make_fn(&arena, "foo", Block { stmts, span: Z });
    assert_ok(&arena, &prog_with(&arena, &[Item::Function(f)]));
}

#[test]
fn test_sema_two_fns_ok() {
    let arena = AstArena::new();
    let a = make_fn(&arena, "foo", Block::empty(Z));
    let b = make_fn(&arena, "bar", Block::empty(Z));
    assert_ok(&arena, &prog_with(&arena, &[Item::Function(a), Item::Function(b)]));
}

#[test]
fn test_sema_fn_ref_sibling_as_expr_ok() {
    // fn helper() {}
    // fn caller()  { helper }  — reference sibling as an expression statement
    let arena  = AstArena::new();
    let helper = make_fn(&arena, "helper", Block::empty(Z));
    let stmts  = arena.alloc_slice_copy(&[
        expr_stmt(&arena, ExprKind::Ident(arena.alloc_str("helper")))
    ]);
    let caller = make_fn(&arena, "caller", Block { stmts, span: Z });
    assert_ok(&arena, &prog_with(&arena, &[
        Item::Function(helper),
        Item::Function(caller),
    ]));
}

#[test]
fn test_sema_empty_struct_ok() {
    let arena = AstArena::new();
    let s = StructDecl {
        attributes:      &[],
        visibility:      Visibility::default(),
        is_edge:         false,
        name:            arena.alloc_str("Point"),
        lifetime_params: &[],
        generic_params:  &[],
        members:         &[],
        span:            Z,
    };
    assert_ok(&arena, &prog_with(&arena, &[Item::Struct(s)]));
}

#[test]
fn test_sema_struct_and_fn_ok() {
    let arena = AstArena::new();
    let s = StructDecl {
        attributes:      &[],
        visibility:      Visibility::default(),
        is_edge:         false,
        name:            arena.alloc_str("Vec3"),
        lifetime_params: &[],
        generic_params:  &[],
        members:         &[],
        span:            Z,
    };
    let f = make_fn(&arena, "new_vec3", Block::empty(Z));
    assert_ok(&arena, &prog_with(&arena, &[Item::Struct(s), Item::Function(f)]));
}

#[test]
fn test_sema_many_fns_ok() {
    let arena = AstArena::new();
    let fns: Vec<Item> = ["alpha", "beta", "gamma", "delta", "epsilon"]
        .iter()
        .map(|n| Item::Function(make_fn(&arena, n, Block::empty(Z))))
        .collect();
    assert_ok(&arena, &prog_with(&arena, &fns));
}

// ── Gap 1: builtin constructor calls + arena coloring ──────────────────────
//
// Before this fix, `List.new()` etc. were parsed as
// Call { callee: Field { target: Ident(ns), field: "new" } }, and the
// generic Field arm has no field table yet — it always inferred Unknown,
// which also meant these calls silently skipped arena tagging inside a
// `with arena(...)` block (only array/tuple/dict/struct literals were
// tagged). See docs/MEMORY_MODEL.md §5.

#[test]
fn test_sema_list_new_outside_arena_is_plain_list() {
    // fn foo() { List.new() }  — no `with arena` in scope: must stay a
    // plain SemaType::List, NOT wrapped in ArenaRef. Negative-case guard —
    // constructor calls should only be arena-tagged when they're actually
    // inside a `with arena(...)` block.
    let arena = AstArena::new();
    const CALL_SPAN: Span = Span { start: 1, end: 1, line: 0, column: 0 };

    let call  = builtin_ctor_call(&arena, "List", CALL_SPAN);
    let stmts = arena.alloc_slice_copy(&[Stmt { kind: StmtKind::Expr(call), span: Z }]);
    let f     = make_fn(&arena, "foo", Block { stmts, span: Z });

    let prog = prog_with(&arena, &[Item::Function(f)]);
    let ctx  = sema::analyse(&prog, &arena, String::new())
        .expect("sema should succeed for a plain List.new() call");

    let ty_id = ctx.expr_type(CALL_SPAN).expect("List.new() should have an inferred type");
    match ctx.types.get(ty_id) {
        SemaType::List(_) => {} // expected
        other => panic!("expected plain SemaType::List outside any arena, got {:?}", other),
    }
}

#[test]
fn test_sema_list_new_in_arena_is_arena_ref() {
    // @tier(mid) fn foo() { with arena(1MB) { List.new() } }
    // Inside the with-arena block, List.new() must come back wrapped in
    // ArenaRef, exactly like an array literal already does.
    let arena = AstArena::new();
    const CALL_SPAN: Span = Span { start: 2, end: 2, line: 0, column: 0 };

    let call        = builtin_ctor_call(&arena, "List", CALL_SPAN);
    let inner_stmts = arena.alloc_slice_copy(&[Stmt { kind: StmtKind::Expr(call), span: Z }]);
    let inner_block = arena.alloc(Block { stmts: inner_stmts, span: Z });
    let with_stmt   = Stmt {
        kind: StmtKind::With {
            allocator: AllocatorKind::Arena(SizeExpr::WithUnit { value: 1, unit: SizeUnit::MB }),
            body: inner_block,
        },
        span: Z,
    };
    let stmts = arena.alloc_slice_copy(&[with_stmt]);
    let f     = make_fn_tiered(&arena, "foo", TierAnnotation::Mid, Block { stmts, span: Z });

    let prog = prog_with(&arena, &[Item::Function(f)]);
    let ctx  = sema::analyse(&prog, &arena, String::new())
        .expect("sema should succeed for a well-formed @tier(mid) with-arena block");

    let ty_id = ctx.expr_type(CALL_SPAN).expect("List.new() should have an inferred type");
    match ctx.types.get(ty_id) {
        SemaType::ArenaRef { inner, .. } => match ctx.types.get(*inner) {
            SemaType::List(_) => {} // expected
            other => panic!("expected ArenaRef<List<_>>, inner was {:?}", other),
        },
        other => panic!("expected List.new() inside `with arena` to be ArenaRef-wrapped, got {:?}", other),
    }
}

#[test]
fn test_sema_dictionary_new_in_arena_is_arena_ref() {
    let arena = AstArena::new();
    const CALL_SPAN: Span = Span { start: 3, end: 3, line: 0, column: 0 };

    let call        = builtin_ctor_call(&arena, "Dictionary", CALL_SPAN);
    let inner_stmts = arena.alloc_slice_copy(&[Stmt { kind: StmtKind::Expr(call), span: Z }]);
    let inner_block = arena.alloc(Block { stmts: inner_stmts, span: Z });
    let with_stmt   = Stmt {
        kind: StmtKind::With {
            allocator: AllocatorKind::Arena(SizeExpr::WithUnit { value: 1, unit: SizeUnit::MB }),
            body: inner_block,
        },
        span: Z,
    };
    let stmts = arena.alloc_slice_copy(&[with_stmt]);
    let f     = make_fn_tiered(&arena, "foo", TierAnnotation::Mid, Block { stmts, span: Z });

    let prog = prog_with(&arena, &[Item::Function(f)]);
    let ctx  = sema::analyse(&prog, &arena, String::new())
        .expect("sema should succeed for a well-formed @tier(mid) with-arena block");

    let ty_id = ctx.expr_type(CALL_SPAN).expect("Dictionary.new() should have an inferred type");
    match ctx.types.get(ty_id) {
        SemaType::ArenaRef { inner, .. } => match ctx.types.get(*inner) {
            SemaType::Dictionary(_, _) => {} // expected
            other => panic!("expected ArenaRef<Dictionary<_,_>>, inner was {:?}", other),
        },
        other => panic!("expected Dictionary.new() inside `with arena` to be ArenaRef-wrapped, got {:?}", other),
    }
}

#[test]
fn test_sema_queue_new_in_arena_is_arena_ref() {
    let arena = AstArena::new();
    const CALL_SPAN: Span = Span { start: 4, end: 4, line: 0, column: 0 };

    let call        = builtin_ctor_call(&arena, "Queue", CALL_SPAN);
    let inner_stmts = arena.alloc_slice_copy(&[Stmt { kind: StmtKind::Expr(call), span: Z }]);
    let inner_block = arena.alloc(Block { stmts: inner_stmts, span: Z });
    let with_stmt   = Stmt {
        kind: StmtKind::With {
            allocator: AllocatorKind::Arena(SizeExpr::WithUnit { value: 1, unit: SizeUnit::MB }),
            body: inner_block,
        },
        span: Z,
    };
    let stmts = arena.alloc_slice_copy(&[with_stmt]);
    let f     = make_fn_tiered(&arena, "foo", TierAnnotation::Mid, Block { stmts, span: Z });

    let prog = prog_with(&arena, &[Item::Function(f)]);
    let ctx  = sema::analyse(&prog, &arena, String::new())
        .expect("sema should succeed for a well-formed @tier(mid) with-arena block");

    let ty_id = ctx.expr_type(CALL_SPAN).expect("Queue.new() should have an inferred type");
    match ctx.types.get(ty_id) {
        SemaType::ArenaRef { inner, .. } => match ctx.types.get(*inner) {
            SemaType::Queue(_) => {} // expected
            other => panic!("expected ArenaRef<Queue<_>>, inner was {:?}", other),
        },
        other => panic!("expected Queue.new() inside `with arena` to be ArenaRef-wrapped, got {:?}", other),
    }
}

#[test]
fn test_sema_stack_new_in_arena_is_arena_ref() {
    let arena = AstArena::new();
    const CALL_SPAN: Span = Span { start: 5, end: 5, line: 0, column: 0 };

    let call        = builtin_ctor_call(&arena, "Stack", CALL_SPAN);
    let inner_stmts = arena.alloc_slice_copy(&[Stmt { kind: StmtKind::Expr(call), span: Z }]);
    let inner_block = arena.alloc(Block { stmts: inner_stmts, span: Z });
    let with_stmt   = Stmt {
        kind: StmtKind::With {
            allocator: AllocatorKind::Arena(SizeExpr::WithUnit { value: 1, unit: SizeUnit::MB }),
            body: inner_block,
        },
        span: Z,
    };
    let stmts = arena.alloc_slice_copy(&[with_stmt]);
    let f     = make_fn_tiered(&arena, "foo", TierAnnotation::Mid, Block { stmts, span: Z });

    let prog = prog_with(&arena, &[Item::Function(f)]);
    let ctx  = sema::analyse(&prog, &arena, String::new())
        .expect("sema should succeed for a well-formed @tier(mid) with-arena block");

    let ty_id = ctx.expr_type(CALL_SPAN).expect("Stack.new() should have an inferred type");
    match ctx.types.get(ty_id) {
        SemaType::ArenaRef { inner, .. } => match ctx.types.get(*inner) {
            SemaType::Stack(_) => {} // expected
            other => panic!("expected ArenaRef<Stack<_>>, inner was {:?}", other),
        },
        other => panic!("expected Stack.new() inside `with arena` to be ArenaRef-wrapped, got {:?}", other),
    }
}

// ── Gap 2: escape boundary enforcement ──────────────────────────────────────
//
// See docs/MEMORY_MODEL.md §6. These two are the core mechanism
// (assignment-target-vs-value ArenaId comparison in `unify` /
// `check_assign_arena_escape`); the struct-field, nested-arena-mismatch,
// and closure-capture cases are covered end-to-end through the real
// parser by the tests/fixtures/err_arena_escapes_*.ubl fixtures instead,
// since building nested struct/lambda AST by hand here would mostly be
// re-testing the parser's own shape rather than the sema mechanism.

#[test]
fn test_sema_arena_escape_outer_binding_rejected() {
    // @tier(mid) fn foo() {
    //     let mut outer = List.new()     // outside any arena — plain List
    //     with arena(1MB) {
    //         outer = List.new()         // GAP 2: outer's home arena is
    //     }                              // `None`, this value's is `Some`
    // }                                  // — must be rejected.
    let arena = AstArena::new();
    const OUTER_LET_CALL_SPAN:  Span = Span { start: 10, end: 10, line: 0, column: 0 };
    const OUTER_LET_STMT_SPAN:  Span = Span { start: 15, end: 15, line: 0, column: 0 };
    const ASSIGN_TARGET_SPAN:   Span = Span { start: 20, end: 20, line: 0, column: 0 };
    const INNER_CALL_SPAN:      Span = Span { start: 30, end: 30, line: 0, column: 0 };

    let outer_ctor = builtin_ctor_call(&arena, "List", OUTER_LET_CALL_SPAN);
    let let_outer = Stmt {
        kind: StmtKind::Let {
            mutable: true,
            binding: BindingTarget::Ident(arena.alloc_str("outer")),
            ty:      None,
            value:   outer_ctor,
        },
        // `record_binding` looks up the DefId this statement declared via
        // `ctx.resolutions.get(stmt.span)` — this span MUST be unique, or
        // Pass 1's declaration write collides with some other node's
        // resolution write at the same key (e.g. `builtin_ctor_call`'s
        // internal "List" ident, which deliberately reuses span `Z`) and
        // whichever happens later in the Pass-1 walk silently wins,
        // leaving `outer`'s own DefId never actually assigned a type.
        span: OUTER_LET_STMT_SPAN,
    };

    let target      = arena.alloc(Expr {
        kind: ExprKind::Ident(arena.alloc_str("outer")),
        span: ASSIGN_TARGET_SPAN,
    });
    let inner_ctor  = builtin_ctor_call(&arena, "List", INNER_CALL_SPAN);
    let assign      = arena.alloc(Expr {
        kind: ExprKind::Assign { op: AssignOp::Assign, target, value: inner_ctor },
        span: Z,
    });
    let inner_stmts = arena.alloc_slice_copy(&[Stmt { kind: StmtKind::Expr(assign), span: Z }]);
    let inner_block = arena.alloc(Block { stmts: inner_stmts, span: Z });
    let with_stmt   = Stmt {
        kind: StmtKind::With {
            allocator: AllocatorKind::Arena(SizeExpr::WithUnit { value: 1, unit: SizeUnit::MB }),
            body: inner_block,
        },
        span: Z,
    };

    let stmts = arena.alloc_slice_copy(&[let_outer, with_stmt]);
    let f     = make_fn_tiered(&arena, "foo", TierAnnotation::Mid, Block { stmts, span: Z });
    let prog  = prog_with(&arena, &[Item::Function(f)]);

    match sema::analyse(&prog, &arena, String::new()) {
        Ok(_) => panic!("expected sema to reject an arena value escaping to an outer binding"),
        Err(mut errors) => {
            let tier_errs = errors.take_tier_errors();
            assert!(
                tier_errs.iter().any(|e| matches!(e, TierError::ArenaRefEscapesBoundary { .. })),
                "expected ArenaRefEscapesBoundary, got {:?}", tier_errs
            );
        }
    }
}

#[test]
fn test_sema_arena_same_arena_reassign_ok() {
    // @tier(mid) fn foo() {
    //     with arena(1MB) {
    //         let mut items = List.new()
    //         items = List.new()         // same arena as `items` itself —
    //     }                              // legitimate, must NOT be rejected.
    // }
    //
    // Regression guard for the `structurally_compatible` fix: before Gap 2,
    // ArenaRef<->ArenaRef had no dedicated arm at all, so this would have
    // spuriously failed with a generic TypeMismatch even though both sides
    // share the exact same ArenaId.
    let arena = AstArena::new();
    const FIRST_CALL_SPAN:    Span = Span { start: 40, end: 40, line: 0, column: 0 };
    const LET_STMT_SPAN:      Span = Span { start: 45, end: 45, line: 0, column: 0 };
    const ASSIGN_TARGET_SPAN: Span = Span { start: 50, end: 50, line: 0, column: 0 };
    const SECOND_CALL_SPAN:   Span = Span { start: 60, end: 60, line: 0, column: 0 };

    let first_ctor = builtin_ctor_call(&arena, "List", FIRST_CALL_SPAN);
    let let_items = Stmt {
        kind: StmtKind::Let {
            mutable: true,
            binding: BindingTarget::Ident(arena.alloc_str("items")),
            ty:      None,
            value:   first_ctor,
        },
        // See the matching comment in test_sema_arena_escape_outer_binding_rejected —
        // this span must be unique for `record_binding`'s DefId lookup to
        // find the right declaration.
        span: LET_STMT_SPAN,
    };

    let target       = arena.alloc(Expr {
        kind: ExprKind::Ident(arena.alloc_str("items")),
        span: ASSIGN_TARGET_SPAN,
    });
    let second_ctor  = builtin_ctor_call(&arena, "List", SECOND_CALL_SPAN);
    let assign       = arena.alloc(Expr {
        kind: ExprKind::Assign { op: AssignOp::Assign, target, value: second_ctor },
        span: Z,
    });

    let inner_stmts = arena.alloc_slice_copy(&[let_items, Stmt { kind: StmtKind::Expr(assign), span: Z }]);
    let inner_block = arena.alloc(Block { stmts: inner_stmts, span: Z });
    let with_stmt   = Stmt {
        kind: StmtKind::With {
            allocator: AllocatorKind::Arena(SizeExpr::WithUnit { value: 1, unit: SizeUnit::MB }),
            body: inner_block,
        },
        span: Z,
    };

    let stmts = arena.alloc_slice_copy(&[with_stmt]);
    let f     = make_fn_tiered(&arena, "foo", TierAnnotation::Mid, Block { stmts, span: Z });
    assert_ok(&arena, &prog_with(&arena, &[Item::Function(f)]));
}

// ── Ownership model: Unique<T>/Shared<T>/SyncShared<T> ─────────────
//
// MEMORY_MODEL.md §9, DATASTRUCTURES.md "Decisions locked in" — landed as
// a new SemaType axis orthogonal to the tier wrappers, the same
// relationship `Reference` already has to them. No construction syntax
// exists yet for any of the three (nothing in the language can produce a
// *value* of one of these types), so every test below reaches a
// real-typed binding only through a function *parameter's* own declared
// type, never a constructed expression — that's enough to exercise
// `ast_type_to_sema`, `substitute`, `structurally_compatible`, and
// `display` for real without depending on move-tracking or runtime
// semantics, neither of which exist yet.

#[test]
fn test_sema_unique_param_and_return_type_round_trip_ok() {
    // fn identity_unique(x: Unique<int>) Unique<int> { return x }
    //
    // The parameter's declared type and the return type are two
    // independently-built `Unique<int>` annotations — same shape as the
    // `Option<int>` regression `structurally_compatible`'s own `Named`
    // arm comment already documents — must unify as compatible.
    let arena = AstArena::new();
    let param_ty = named1(&arena, "Unique", int_type(&arena));
    let ret_ty   = named1(&arena, "Unique", int_type(&arena));

    let x_ident  = arena.alloc(Expr { kind: ExprKind::Ident(arena.alloc_str("x")), span: Z });
    let ret_stmt = Stmt { kind: StmtKind::Return(Some(x_ident)), span: Z };

    let params = [param_named(&arena, "x", param_ty)];
    let f = make_fn_full(
        &arena, "identity_unique", &params,
        Some(ReturnType { ty: ret_ty, is_fallible: false }),
        Block { stmts: arena.alloc_slice_copy(&[ret_stmt]), span: Z },
    );
    assert_ok(&arena, &prog_with(&arena, &[Item::Function(f)]));
}

#[test]
fn test_sema_shared_and_sync_shared_param_types_ok() {
    // fn take_shared(x: Shared<string>) void {}
    // fn take_sync_shared(x: SyncShared<bool>) void {}
    //
    // Round out coverage beyond `Unique` alone — all three wrappers
    // resolve, with different inner types, in the same program.
    let arena = AstArena::new();
    let str_ty  = arena.alloc(Type { kind: TypeKind::Str,  span: Z });
    let bool_ty = arena.alloc(Type { kind: TypeKind::Bool, span: Z });
    let shared_param      = named1(&arena, "Shared",     str_ty);
    let sync_shared_param = named1(&arena, "SyncShared", bool_ty);

    let take_shared = make_fn_full(
        &arena, "take_shared", &[param_named(&arena, "x", shared_param)],
        None, Block { stmts: &[], span: Z },
    );
    let take_sync_shared = make_fn_full(
        &arena, "take_sync_shared", &[param_named(&arena, "x", sync_shared_param)],
        None, Block { stmts: &[], span: Z },
    );
    assert_ok(&arena, &prog_with(&arena, &[
        Item::Function(take_shared), Item::Function(take_sync_shared),
    ]));
}

#[test]
fn test_sema_unique_and_shared_call_arg_type_mismatch() {
    // fn take_shared(x: Shared<int>) void {}
    // fn wrong(x: Unique<int>) void { take_shared(x) }
    //
    // The actual enforcement of "new orthogonal axis": Unique<int> and
    // Shared<int> must NOT be interchangeable even though their inner
    // type is identical — same as `&T` vs `&mut T` already aren't.
    let arena = AstArena::new();
    let shared_param_ty = named1(&arena, "Shared", int_type(&arena));
    let unique_param_ty = named1(&arena, "Unique", int_type(&arena));

    let take_shared = make_fn_full(
        &arena, "take_shared", &[param_named(&arena, "x", shared_param_ty)],
        None, Block { stmts: &[], span: Z },
    );

    let x_arg    = arena.alloc(Expr { kind: ExprKind::Ident(arena.alloc_str("x")), span: Z });
    let call     = call_expr(&arena, "take_shared", x_arg);
    let call_stmt = Stmt { kind: StmtKind::Expr(call), span: Z };

    let wrong = make_fn_full(
        &arena, "wrong", &[param_named(&arena, "x", unique_param_ty)],
        None, Block { stmts: arena.alloc_slice_copy(&[call_stmt]), span: Z },
    );

    match sema::analyse(
        &prog_with(&arena, &[Item::Function(take_shared), Item::Function(wrong)]),
        &arena, String::new(),
    ) {
        Ok(_) => panic!("expected sema to reject Unique<int> passed where Shared<int> is expected"),
        Err(mut errors) => {
            let type_errs = errors.take_type_errors();
            assert!(!type_errs.is_empty(), "expected at least one type error, got none");
        }
    }
}

#[test]
fn test_sema_unique_missing_type_argument_reports_error() {
    // fn bad(x: Unique) void {}
    //
    // Unique<T>/Shared<T>/SyncShared<T> take exactly one type argument —
    // same arity check every other single-arg generic wrapper gets,
    // reusing the existing GenericArgCountMismatch diagnostic rather
    // than a bespoke error path.
    let arena = AstArena::new();
    let bare_unique = named0(&arena, "Unique");
    let f = make_fn_full(
        &arena, "bad", &[param_named(&arena, "x", bare_unique)],
        None, Block { stmts: &[], span: Z },
    );
    match sema::analyse(&prog_with(&arena, &[Item::Function(f)]), &arena, String::new()) {
        Ok(_) => panic!("expected sema to reject Unique with zero type arguments"),
        Err(mut errors) => {
            let type_errs = errors.take_type_errors();
            assert!(
                type_errs.iter().any(|e| matches!(e, TypeError::GenericArgCountMismatch { .. })),
                "expected GenericArgCountMismatch, got {:?}", type_errs
            );
        }
    }
}

// ── Ownership model construction: Unique.new()/Shared.new()/SyncShared.new() ──
//
// MEMORY_MODEL.md §9 — `Namespace.new(value)`, same idiom every builtin
// wrapper uses. Restricted to `@tier(low)` (Open Decision #5, resolved):
// the inverse of `CollectionConstructionInLowTier` — List/Dictionary/
// Queue/Stack are banned *inside* LOW tier because LOW has no memory
// model of its own; Unique/Shared/SyncShared now *are* that model, so
// construction is banned everywhere *except* LOW tier.

#[test]
fn test_sema_unique_shared_sync_shared_construction_in_low_tier_ok() {
    // @tier(low) fn f() int { let u = Unique.new(42); let s = Shared.new("x");
    //                          let ss = SyncShared.new(true); return 1 }
    let arena = AstArena::new();
    let int_lit = arena.alloc(Expr { kind: ExprKind::Lit(Literal::Int(42)), span: Z });
    let str_lit = arena.alloc(Expr { kind: ExprKind::Lit(Literal::Str(arena.alloc_str("x"))), span: Z });
    let bool_lit = arena.alloc(Expr { kind: ExprKind::Lit(Literal::Bool(true)), span: Z });

    let u_call  = namespace_new_call(&arena, "Unique", &[int_lit]);
    let s_call  = namespace_new_call(&arena, "Shared", &[str_lit]);
    let ss_call = namespace_new_call(&arena, "SyncShared", &[bool_lit]);

    // make_fn_tiered always builds a `void`-returning function — these
    // are let-bindings only, no `return`, matching every other
    // make_fn/make_fn_tiered-based test's style in this file.
    let stmts = arena.alloc_slice_copy(&[
        let_stmt(&arena, "u", u_call),
        let_stmt(&arena, "s", s_call),
        let_stmt(&arena, "ss", ss_call),
    ]);
    let f = make_fn_tiered(&arena, "f", TierAnnotation::Low, Block { stmts, span: Z });
    assert_ok(&arena, &prog_with(&arena, &[Item::Function(f)]));
}

#[test]
fn test_sema_unique_construction_outside_low_tier_rejected() {
    // fn f() int { let u = Unique.new(42); return 1 }  -- default tier, not LOW
    let arena = AstArena::new();
    let int_lit = arena.alloc(Expr { kind: ExprKind::Lit(Literal::Int(42)), span: Z });
    let u_call  = namespace_new_call(&arena, "Unique", &[int_lit]);
    let one = arena.alloc(Expr { kind: ExprKind::Lit(Literal::Int(1)), span: Z });
    let stmts = arena.alloc_slice_copy(&[
        let_stmt(&arena, "u", u_call),
        Stmt { kind: StmtKind::Return(Some(one)), span: Z },
    ]);
    let f = make_fn(&arena, "f", Block { stmts, span: Z });
    match sema::analyse(&prog_with(&arena, &[Item::Function(f)]), &arena, String::new()) {
        Ok(_) => panic!("expected sema to reject Unique.new() outside @tier(low)"),
        Err(mut errors) => {
            let tier_errs = errors.take_tier_errors();
            assert!(
                tier_errs.iter().any(|e| matches!(e, TierError::OwnershipWrapperOutsideLowTier { .. })),
                "expected OwnershipWrapperOutsideLowTier, got {:?}", tier_errs
            );
        }
    }
}

#[test]
fn test_sema_unique_new_wrong_arg_count_reports_error() {
    // @tier(low) fn f() int { let u = Unique.new(); return 1 }
    let arena = AstArena::new();
    let u_call = namespace_new_call(&arena, "Unique", &[]);
    let one = arena.alloc(Expr { kind: ExprKind::Lit(Literal::Int(1)), span: Z });
    let stmts = arena.alloc_slice_copy(&[
        let_stmt(&arena, "u", u_call),
        Stmt { kind: StmtKind::Return(Some(one)), span: Z },
    ]);
    let f = make_fn_tiered(&arena, "f", TierAnnotation::Low, Block { stmts, span: Z });
    match sema::analyse(&prog_with(&arena, &[Item::Function(f)]), &arena, String::new()) {
        Ok(_) => panic!("expected sema to reject Unique.new() with zero arguments"),
        Err(mut errors) => {
            let type_errs = errors.take_type_errors();
            assert!(
                type_errs.iter().any(|e| matches!(e, TypeError::ArgumentCountMismatch { expected: 1, found: 0, .. })),
                "expected ArgumentCountMismatch{{expected:1,found:0}}, got {:?}", type_errs
            );
        }
    }
}

#[test]
fn test_sema_unique_new_inner_type_matches_argument() {
    // @tier(low) fn f(x: Unique<int>) void {}
    // @tier(low) fn g() void { f(Unique.new(42)) }
    //
    // Proves `Unique.new(value)`'s inferred type isn't just "some Unique"
    // — its inner type comes directly from the argument (`int` here) and
    // must actually unify against a real `Unique<int>`-typed parameter.
    //
    // Two distinct identifier references here (`Unique`, `f`) — needs
    // span_n, not the shared Z constant (see span_n's own doc comment).
    let arena = AstArena::new();
    let unique_int_ty = named1(&arena, "Unique", int_type(&arena));
    let f_decl = make_fn_full(
        &arena, "f", &[param_named(&arena, "x", unique_int_ty)],
        None, Block { stmts: &[], span: Z },
    );
    let f_decl = FunctionDecl { tier: TierAnnotation::Low, ..f_decl };

    let int_lit = arena.alloc(Expr { kind: ExprKind::Lit(Literal::Int(42)), span: Z });
    let unique_target = arena.alloc(Expr { kind: ExprKind::Ident(arena.alloc_str("Unique")), span: span_n(1) });
    let unique_callee  = arena.alloc(Expr { kind: ExprKind::Field { target: unique_target, field: "new" }, span: Z });
    let u_call = arena.alloc(Expr {
        kind: ExprKind::Call {
            callee: unique_callee,
            args:   arena.alloc_slice_copy(&[Arg { kind: ArgKind::Positional(int_lit), span: Z }]),
        },
        span: Z,
    });
    let f_target = arena.alloc(Expr { kind: ExprKind::Ident(arena.alloc_str("f")), span: span_n(2) });
    let f_call = arena.alloc(Expr {
        kind: ExprKind::Call {
            callee: f_target,
            args:   arena.alloc_slice_copy(&[Arg { kind: ArgKind::Positional(u_call), span: Z }]),
        },
        span: Z,
    });
    let call_stmt = Stmt { kind: StmtKind::Expr(f_call), span: Z };
    let g_decl = make_fn_tiered(&arena, "g", TierAnnotation::Low, Block {
        stmts: arena.alloc_slice_copy(&[call_stmt]), span: Z,
    });

    assert_ok(&arena, &prog_with(&arena, &[Item::Function(f_decl), Item::Function(g_decl)]));
}



// ── Format specs: {expr:spec} inside interpolation holes ───────────
//
// docs/PRINT_FORMAT_RULES.md. Hand-building `Literal::InterpolatedStr`
// directly here rather than going through the parser -- these are
// sema-level checks (does `infer_literal` walk the hole and validate
// its spec correctly), not parser tests; parser-level coverage lives in
// the real .ubl fixtures instead (docs/TESTING_RULES.md §1).

#[test]
fn test_sema_format_spec_precision_on_double_ok() {
    // let pi = 3.14 ; $"{pi:.2}"
    let arena = AstArena::new();
    let pi_lit = arena.alloc(Expr { kind: ExprKind::Lit(Literal::Double(3.14)), span: Z });
    let pi_ident = arena.alloc(Expr { kind: ExprKind::Ident(arena.alloc_str("pi")), span: span_n(1) });
    let spec = FormatSpec { fill: None, align: None, sign_plus: false, alternate: false, zero_pad: false, width: None, precision: Some(2), debug: false, base: None };
    let parts = arena.alloc_slice_copy(&[InterpolationPart::Expr { expr: pi_ident, spec: Some(spec) }]);
    let interp = arena.alloc(Expr { kind: ExprKind::Lit(Literal::InterpolatedStr(parts)), span: Z });

    let stmts = arena.alloc_slice_copy(&[
        let_stmt(&arena, "pi", pi_lit),
        Stmt { kind: StmtKind::Expr(interp), span: Z },
    ]);
    let f = make_fn(&arena, "f", Block { stmts, span: Z });
    assert_ok(&arena, &prog_with(&arena, &[Item::Function(f)]));
}

#[test]
fn test_sema_format_spec_precision_on_int_rejected() {
    // let n = 42 ; $"{n:.2}"  -- precision only applies to Float/Double/Str
    let arena = AstArena::new();
    let n_lit = arena.alloc(Expr { kind: ExprKind::Lit(Literal::Int(42)), span: Z });
    let n_ident = arena.alloc(Expr { kind: ExprKind::Ident(arena.alloc_str("n")), span: span_n(1) });
    let spec = FormatSpec { fill: None, align: None, sign_plus: false, alternate: false, zero_pad: false, width: None, precision: Some(2), debug: false, base: None };
    let parts = arena.alloc_slice_copy(&[InterpolationPart::Expr { expr: n_ident, spec: Some(spec) }]);
    let interp = arena.alloc(Expr { kind: ExprKind::Lit(Literal::InterpolatedStr(parts)), span: Z });

    let stmts = arena.alloc_slice_copy(&[
        let_stmt(&arena, "n", n_lit),
        Stmt { kind: StmtKind::Expr(interp), span: Z },
    ]);
    let f = make_fn(&arena, "f", Block { stmts, span: Z });
    match sema::analyse(&prog_with(&arena, &[Item::Function(f)]), &arena, String::new()) {
        Ok(_) => panic!("expected sema to reject `.precision` on Int"),
        Err(mut errors) => {
            let type_errs = errors.take_type_errors();
            assert!(
                type_errs.iter().any(|e| matches!(e, TypeError::InvalidFormatSpec { spec_part, .. } if spec_part == "precision")),
                "expected InvalidFormatSpec, got {:?}", type_errs
            );
        }
    }
}

#[test]
fn test_sema_interpolation_hole_undefined_name_rejected() {
    // $"{does_not_exist}" -- proves interpolation holes are now actually
    // walked by name resolution (they weren't at all before this
    // delivery -- see PRINT_FORMAT_RULES.md §5).
    let arena = AstArena::new();
    let bad_ident = arena.alloc(Expr { kind: ExprKind::Ident(arena.alloc_str("does_not_exist")), span: Z });
    let parts = arena.alloc_slice_copy(&[InterpolationPart::Expr { expr: bad_ident, spec: None }]);
    let interp = arena.alloc(Expr { kind: ExprKind::Lit(Literal::InterpolatedStr(parts)), span: Z });
    let f = make_fn(&arena, "f", Block {
        stmts: arena.alloc_slice_copy(&[Stmt { kind: StmtKind::Expr(interp), span: Z }]),
        span: Z,
    });
    match sema::analyse(&prog_with(&arena, &[Item::Function(f)]), &arena, String::new()) {
        Ok(_) => panic!("expected sema to reject an undefined name inside an interpolation hole"),
        Err(mut errors) => {
            let name_errs = errors.take_name_errors();
            assert!(!name_errs.is_empty(), "expected at least one name error, got none");
        }
    }
}

#[test]
fn test_sema_discard_param_on_free_function_does_not_report_self_outside_method() {
    // fn f(_: int) void {} -- regression test for a real bug found
    // while wiring ParamKind::Discard in: resolve_param's catch-all
    // (name_resolution.rs) was written with ONLY the self-family
    // variants in mind, and Discard fell into it too, incorrectly
    // firing NameError::SelfOutsideMethod for a plain free function
    // that has nothing to do with `self` at all.
    let arena = AstArena::new();
    let f = make_fn_full(
        &arena, "f", &[param_discard(int_type(&arena))],
        None, Block { stmts: &[], span: Z },
    );
    assert_ok(&arena, &prog_with(&arena, &[Item::Function(f)]));
}

#[test]
fn test_sema_discard_param_type_is_enforced_at_call_site() {
    // fn f(a: int, _: string, b: bool) void {}, called as f(1, 2, true)
    // -- the middle argument is an Int where the discarded slot expects
    // a Str. Regression test for a real bug found while wiring
    // ParamKind::Discard in: three separate signature-collection sites
    // in type_infer.rs used `ParamKind::Named { ty, .. } => ..., _ =>
    // None` with filter_map, silently DROPPING a discard param's type
    // from the computed signature entirely rather than just skipping
    // its (nonexistent) name -- an arity/type-checking gap, not merely
    // an unused value. If that's still broken, this call would
    // type-check cleanly since the signature would only expect 2
    // arguments' worth of real type constraints; it must not.
    let arena = AstArena::new();
    let f = make_fn_full(
        &arena, "f",
        &[param_named(&arena, "a", int_type(&arena)),
          param_discard(str_type(&arena)),
          param_named(&arena, "b", bool_type(&arena))],
        None, Block { stmts: &[], span: Z },
    );

    let arg_a = arena.alloc(Expr { kind: ExprKind::Lit(Literal::Int(1)), span: Z });
    let arg_wrong = arena.alloc(Expr { kind: ExprKind::Lit(Literal::Int(2)), span: Z }); // should be a Str
    let arg_b = arena.alloc(Expr { kind: ExprKind::Lit(Literal::Bool(true)), span: Z });
    let call = call_expr_multi(&arena, "f", &[arg_a, arg_wrong, arg_b]);
    let call_stmt = Stmt { kind: StmtKind::Expr(call), span: Z };
    let main = make_fn(&arena, "main", Block { stmts: arena.alloc_slice_copy(&[call_stmt]), span: Z });

    match sema::analyse(&prog_with(&arena, &[Item::Function(f), Item::Function(main)]), &arena, String::new()) {
        Ok(_) => panic!("expected sema to reject an Int argument where the discarded param expects a Str"),
        Err(mut errors) => {
            let type_errs = errors.take_type_errors();
            assert!(!type_errs.is_empty(), "expected at least one type error, got none -- the discard param's type may have been silently dropped from the signature again");
        }
    }
}


// -- Inference variables in expr_types are resolved at the end of inference --

#[test]
fn test_sema_expr_types_have_their_inference_variables_resolved() {
    // fn foo() { let xs: List<int> = List.new() }
    // While the call is checked it is `List<?T>`; the annotation fixes `?T` only afterwards. The type recorded for
    // the call must be `List<int>` when inference is over (the native backend reads these types).
    let arena = AstArena::new();
    const CALL_SPAN: Span = Span { start: 1, end: 1, line: 0, column: 0 };
    const LET_SPAN:  Span = Span { start: 2, end: 2, line: 0, column: 0 };

    let call = builtin_ctor_call(&arena, "List", CALL_SPAN);
    let let_xs = Stmt {
        kind: StmtKind::Let {
            mutable: false,
            binding: BindingTarget::Ident(arena.alloc_str("xs")),
            ty:      Some(arena.alloc(Type { kind: TypeKind::List(Some(int_type(&arena))), span: Z })),
            value:   call,
        },
        span: LET_SPAN,
    };
    let stmts = arena.alloc_slice_copy(&[let_xs]);
    let f     = make_fn(&arena, "foo", Block { stmts, span: Z });

    let prog = prog_with(&arena, &[Item::Function(f)]);
    let ctx  = sema::analyse(&prog, &arena, String::new())
        .expect("sema should accept an annotated List.new()");

    let ty_id = ctx.expr_type(CALL_SPAN).expect("List.new() should have an inferred type");
    match ctx.types.get(ty_id) {
        SemaType::List(elem) => match ctx.types.get(*elem) {
            SemaType::Int => {}
            other => panic!("expected List<int> with its element resolved, got an element of {:?}", other),
        },
        other => panic!("expected SemaType::List, got {:?}", other),
    }
}
