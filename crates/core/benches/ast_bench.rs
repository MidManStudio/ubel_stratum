// benches/ast_bench.rs
use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use std::time::Duration;

use ubel_stratum::ast::arena::AstArena;
use ubel_stratum::ast::common::{BinOp, Span, TierAnnotation, Visibility};
use ubel_stratum::ast::expressions::{
    ElifBranch, Expr, ExprKind, FieldInit, IfBranchBody, IfExpr, MatchArm, MatchArmBody, MatchExpr,
};
use ubel_stratum::ast::literals::Literal;
use ubel_stratum::ast::patterns::{Pattern, PatternKind};
use ubel_stratum::ast::statements::{BindingTarget, Block, Stmt, StmtKind};
use ubel_stratum::ast::declarations::{
    ConstDecl, FieldDecl, FunctionDecl, MethodDecl, Param, ParamKind, ReturnType,
    StructDecl, StructMember,
};
use ubel_stratum::ast::root::{Import, ImportItems, ImportKind, Item, PackageDecl, Program};
use ubel_stratum::ast::types::{Type, TypeKind};

// ── Shared helpers ────────────────────────────────────────────────

const DUMMY: Span = Span { start: 0, end: 0, line: 1, column: 1 };

fn int_type<'ast>(arena: &'ast AstArena) -> &'ast Type<'ast> {
    arena.alloc(Type { kind: TypeKind::Int, span: DUMMY })
}

fn string_type<'ast>(arena: &'ast AstArena) -> &'ast Type<'ast> {
    arena.alloc(Type { kind: TypeKind::Str, span: DUMMY })
}

fn int_lit<'ast>(arena: &'ast AstArena, n: i64) -> &'ast Expr<'ast> {
    arena.alloc(Expr { kind: ExprKind::Lit(Literal::Int(n)), span: DUMMY })
}

fn ident_expr<'ast>(arena: &'ast AstArena, name: &str) -> &'ast Expr<'ast> {
    let s = arena.alloc_str(name);
    arena.alloc(Expr { kind: ExprKind::Ident(s), span: DUMMY })
}

fn make_binop<'ast>(
    arena: &'ast AstArena,
    op: BinOp,
    lhs: &'ast Expr<'ast>,
    rhs: &'ast Expr<'ast>,
) -> &'ast Expr<'ast> {
    arena.alloc(Expr {
        kind: ExprKind::BinOp { op, lhs, rhs },
        span: DUMMY,
    })
}

// Standalone function so the 'ast lifetime can be expressed properly.
fn make_branch<'ast>(
    arena: &'ast AstArena,
    cond_name: &str,
    ret_val: i64,
) -> (&'ast Expr<'ast>, IfBranchBody<'ast>) {
    let cond = ident_expr(arena, cond_name);
    let stmt = arena.alloc(Stmt {
        kind: StmtKind::Return(Some(int_lit(arena, ret_val))),
        span: DUMMY,
    });
    let stmts = arena.alloc_slice_copy(&[*stmt]);
    (cond, IfBranchBody::Block(Block { stmts, span: DUMMY }))
}

// Standalone function so the 'ast lifetime can be expressed properly.
fn make_named_param<'ast>(
    arena: &'ast AstArena,
    name: &str,
    ty: &'ast Type<'ast>,
) -> Param<'ast> {
    Param {
        kind: ParamKind::Named {
            mutable: false,
            name: arena.alloc_str(name),
            ty: Some(ty),
            default: None,
        },
        span: DUMMY,
    }
}

// ── 1. Leaf node allocation ───────────────────────────────────────

fn bench_alloc_leaf(c: &mut Criterion) {
    let mut group = c.benchmark_group("ast/alloc_leaf");
    group.measurement_time(Duration::from_secs(4));

    group.bench_function("int_literal", |b| {
        b.iter(|| {
            let arena = AstArena::new();
            let node = arena.alloc(Expr {
                kind: ExprKind::Lit(Literal::Int(black_box(42))),
                span: DUMMY,
            });
            black_box(node);
        });
    });

    group.bench_function("string_intern_short", |b| {
        b.iter(|| {
            let arena = AstArena::new();
            let s = arena.alloc_str(black_box("hello"));
            black_box(s);
        });
    });

    group.bench_function("string_intern_long", |b| {
        let long = "handle_request_with_very_long_function_name_that_exceeds_cache";
        b.iter(|| {
            let arena = AstArena::new();
            let s = arena.alloc_str(black_box(long));
            black_box(s);
        });
    });

    group.bench_function("type_node", |b| {
        b.iter(|| {
            let arena = AstArena::new();
            let ty = arena.alloc(Type { kind: TypeKind::Int, span: DUMMY });
            black_box(ty);
        });
    });

    group.finish();
}

// ── 2. Vec → slice (the primary list-building pattern) ────────────

fn bench_vec_to_slice(c: &mut Criterion) {
    let mut group = c.benchmark_group("ast/vec_to_slice");
    group.measurement_time(Duration::from_secs(4));

    for count in [4usize, 16, 64, 256] {
        group.throughput(Throughput::Elements(count as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(count),
            &count,
            |b, &n| {
                b.iter(|| {
                    let arena = AstArena::new();
                    let mut v = arena.vec::<&Expr>();
                    for i in 0..n {
                        let expr = arena.alloc(Expr {
                            kind: ExprKind::Lit(Literal::Int(i as i64)),
                            span: DUMMY,
                        });
                        v.push(expr);
                    }
                    let slice = v.into_bump_slice();
                    black_box(slice);
                });
            },
        );
    }

    group.finish();
}

// ── 3. Binary expression tree ─────────────────────────────────────

fn bench_binop_tree(c: &mut Criterion) {
    let mut group = c.benchmark_group("ast/binop_tree");
    group.measurement_time(Duration::from_secs(4));

    group.bench_function("depth_3", |b| {
        b.iter(|| {
            let arena = AstArena::new();
            let l1 = make_binop(&arena, BinOp::Add, int_lit(&arena, 1), int_lit(&arena, 2));
            let l2 = make_binop(&arena, BinOp::Sub, int_lit(&arena, 3), int_lit(&arena, 4));
            let l3 = make_binop(&arena, BinOp::Div, int_lit(&arena, 5), int_lit(&arena, 6));
            let m1 = make_binop(&arena, BinOp::Mul, l1, l2);
            let root = make_binop(&arena, BinOp::Add, m1, l3);
            black_box(root);
        });
    });

    group.bench_function("chain_128", |b| {
        b.iter(|| {
            let arena = AstArena::new();
            let mut acc = int_lit(&arena, 0);
            for i in (1i64..=128).rev() {
                acc = make_binop(&arena, BinOp::Add, int_lit(&arena, i), acc);
            }
            black_box(acc);
        });
    });

    group.finish();
}

// ── 4. If / elif / else expression ───────────────────────────────

fn bench_if_expr(c: &mut Criterion) {
    let mut group = c.benchmark_group("ast/if_expr");
    group.measurement_time(Duration::from_secs(4));

    group.bench_function("simple_if_else", |b| {
        b.iter(|| {
            let arena = AstArena::new();

            let cond = ident_expr(&arena, "condition");

            let then_stmt = arena.alloc(Stmt {
                kind: StmtKind::Return(Some(int_lit(&arena, 1))),
                span: DUMMY,
            });
            let then_body = IfBranchBody::Block(Block {
                stmts: arena.alloc_slice_copy(&[*then_stmt]),
                span: DUMMY,
            });

            let else_stmt = arena.alloc(Stmt {
                kind: StmtKind::Return(Some(int_lit(&arena, 0))),
                span: DUMMY,
            });
            let else_body = IfBranchBody::Block(Block {
                stmts: arena.alloc_slice_copy(&[*else_stmt]),
                span: DUMMY,
            });

            let if_expr = arena.alloc(IfExpr {
                condition: cond,
                then_body,
                elif_branches: &[],
                else_body: Some(else_body),
                span: DUMMY,
            });

            black_box(if_expr);
        });
    });

    group.bench_function("if_3_elif_else", |b| {
        b.iter(|| {
            let arena = AstArena::new();

            let (cond, then_body) = make_branch(&arena, "is_admin", 3);
            let (c1, b1) = make_branch(&arena, "is_mod", 2);
            let (c2, b2) = make_branch(&arena, "is_user", 1);

            let mut elifs = arena.vec::<ElifBranch>();
            elifs.push(ElifBranch { condition: c1, body: b1, span: DUMMY });
            elifs.push(ElifBranch { condition: c2, body: b2, span: DUMMY });
            let elif_branches = elifs.into_bump_slice();

            let (_, else_body) = make_branch(&arena, "_unused", 0);

            let if_node = arena.alloc(IfExpr {
                condition: cond,
                then_body,
                elif_branches,
                else_body: Some(else_body),
                span: DUMMY,
            });

            black_box(if_node);
        });
    });

    group.finish();
}

// ── 5. Match expression ───────────────────────────────────────────

fn bench_match_expr(c: &mut Criterion) {
    let mut group = c.benchmark_group("ast/match_expr");
    group.measurement_time(Duration::from_secs(4));

    for arm_count in [4usize, 8, 16] {
        group.bench_with_input(
            BenchmarkId::from_parameter(format!("{}_arms", arm_count)),
            &arm_count,
            |b, &n| {
                b.iter(|| {
                    let arena = AstArena::new();
                    let scrutinee = ident_expr(&arena, "status");

                    let mut arms = arena.vec::<MatchArm>();
                    for i in 0..n {
                        let pat = Pattern {
                            kind: PatternKind::Literal(Literal::Int(i as i64)),
                            span: DUMMY,
                        };
                        let body = MatchArmBody::Expr(int_lit(&arena, i as i64 * 10));
                        arms.push(MatchArm {
                            pattern: pat,
                            guard: None,
                            body,
                            span: DUMMY,
                        });
                    }
                    let arms = arms.into_bump_slice();

                    let match_node = arena.alloc(MatchExpr {
                        scrutinee,
                        arms,
                        span: DUMMY,
                    });
                    black_box(match_node);
                });
            },
        );
    }

    group.finish();
}

// ── 6. Function declaration ───────────────────────────────────────

fn bench_function_decl(c: &mut Criterion) {
    let mut group = c.benchmark_group("ast/function_decl");
    group.measurement_time(Duration::from_secs(4));

    group.bench_function("simple_2_params", |b| {
        b.iter(|| {
            let arena = AstArena::new();
            let int_ty = int_type(&arena);

            let mut params = arena.vec::<Param>();
            params.push(make_named_param(&arena, "x", int_ty));
            params.push(make_named_param(&arena, "y", int_ty));
            let params = params.into_bump_slice();

            let add = make_binop(
                &arena,
                BinOp::Add,
                ident_expr(&arena, "x"),
                ident_expr(&arena, "y"),
            );
            let ret_stmt = arena.alloc(Stmt {
                kind: StmtKind::Return(Some(add)),
                span: DUMMY,
            });
            let body = Block {
                stmts: arena.alloc_slice_copy(&[*ret_stmt]),
                span: DUMMY,
            };

            let decl = FunctionDecl {
                tier: TierAnnotation::High,
                attributes: &[],
                visibility: Visibility::Public,
                is_async: false,
                name: arena.alloc_str("add"),
                lifetime_params: &[],
                generic_params: &[],
                params,
                return_type: Some(ReturnType { ty: int_ty, is_fallible: false }),
                body,
                span: DUMMY,
            };
            black_box(decl);
        });
    });

    group.bench_function("async_handler_5_params", |b| {
        b.iter(|| {
            let arena = AstArena::new();
            let str_ty = string_type(&arena);

            let mut params = arena.vec::<Param>();
            for name in &["req", "auth", "db", "cache", "logger"] {
                params.push(make_named_param(&arena, name, str_ty));
            }
            let params = params.into_bump_slice();

            let let_stmt = arena.alloc(Stmt {
                kind: StmtKind::Let {
                    mutable: false,
                    binding: BindingTarget::Ident(arena.alloc_str("result")),
                    ty: None,
                    value: ident_expr(&arena, "req"),
                },
                span: DUMMY,
            });
            let ret_stmt = arena.alloc(Stmt {
                kind: StmtKind::Return(Some(ident_expr(&arena, "result"))),
                span: DUMMY,
            });
            let body = Block {
                stmts: arena.alloc_slice_copy(&[*let_stmt, *ret_stmt]),
                span: DUMMY,
            };

            let task_inner = arena.alloc(Type { kind: TypeKind::Str, span: DUMMY });
            let task_ty = arena.alloc(Type {
                kind: TypeKind::Task(Some(task_inner)),
                span: DUMMY,
            });

            let decl = FunctionDecl {
                tier: TierAnnotation::High,
                attributes: &[],
                visibility: Visibility::Public,
                is_async: true,
                name: arena.alloc_str("handle_request"),
                lifetime_params: &[],
                generic_params: &[],
                params,
                return_type: Some(ReturnType { ty: task_ty, is_fallible: true }),
                body,
                span: DUMMY,
            };
            black_box(decl);
        });
    });

    group.finish();
}

// ── 7. Struct declaration ─────────────────────────────────────────

fn bench_struct_decl(c: &mut Criterion) {
    let mut group = c.benchmark_group("ast/struct_decl");
    group.measurement_time(Duration::from_secs(4));

    for field_count in [4usize, 8, 16] {
        group.bench_with_input(
            BenchmarkId::from_parameter(format!("{}_fields", field_count)),
            &field_count,
            |b, &n| {
                b.iter(|| {
                    let arena = AstArena::new();
                    let int_ty = int_type(&arena);
                    let mut members = arena.vec::<StructMember>();

                    for i in 0..n {
                        // We can't format into the arena directly, so
                        // build the string on the stack first.
                        let name_buf = format!("field_{}", i);
                        members.push(StructMember::Field(FieldDecl {
                            visibility: Visibility::Public,
                            name: arena.alloc_str(&name_buf),
                            ty: int_ty,
                            span: DUMMY,
                        }));
                    }

                    let ret_stmt = arena.alloc(Stmt {
                        kind: StmtKind::Return(Some(int_lit(&arena, 0))),
                        span: DUMMY,
                    });
                    let body = Block {
                        stmts: arena.alloc_slice_copy(&[*ret_stmt]),
                        span: DUMMY,
                    };
                    members.push(StructMember::Method(MethodDecl {
                        tier: TierAnnotation::High,
                        attributes: &[],
                        visibility: Visibility::Public,
                        is_async: false,
                        name: arena.alloc_str("total"),
                        generic_params: &[],
                        params: &[],
                        return_type: Some(ReturnType {
                            ty: int_ty,
                            is_fallible: false,
                        }),
                        body,
                        span: DUMMY,
                    }));

                    let decl = StructDecl {
                        attributes: &[],
                        visibility: Visibility::Public,
                        is_edge: false,
                        name: arena.alloc_str("MyStruct"),
                        lifetime_params: &[],
                        generic_params: &[],
                        members: members.into_bump_slice(),
                        span: DUMMY,
                    };
                    black_box(decl);
                });
            },
        );
    }

    group.finish();
}

// ── 8. Arena reuse — simulate parsing multiple files ──────────────

fn bench_arena_reuse(c: &mut Criterion) {
    let mut group = c.benchmark_group("ast/arena_reuse");
    group.measurement_time(Duration::from_secs(5));

    group.bench_function("fresh_arena_per_file", |b| {
        b.iter(|| {
            let arena = AstArena::with_capacity(64 * 1024);
            let int_ty = int_type(&arena);

            let mut items = arena.vec::<Item>();
            for i in 0..20usize {
                let name_buf = format!("fn_{}", i);

                let mut params = arena.vec::<Param>();
                for j in 0..3usize {
                    let p_buf = format!("p{}", j);
                    params.push(make_named_param(&arena, &p_buf, int_ty));
                }
                let params = params.into_bump_slice();

                let mut stmts = arena.vec::<Stmt>();
                for k in 0..4usize {
                    let v_buf = format!("v{}", k);
                    stmts.push(Stmt {
                        kind: StmtKind::Let {
                            mutable: false,
                            binding: BindingTarget::Ident(arena.alloc_str(&v_buf)),
                            ty: None,
                            value: int_lit(&arena, k as i64),
                        },
                        span: DUMMY,
                    });
                }
                let body = Block {
                    stmts: stmts.into_bump_slice(),
                    span: DUMMY,
                };

                items.push(Item::Function(FunctionDecl {
                    tier: TierAnnotation::High,
                    attributes: &[],
                    visibility: Visibility::Public,
                    is_async: false,
                    name: arena.alloc_str(&name_buf),
                    lifetime_params: &[],
                    generic_params: &[],
                    params,
                    return_type: Some(ReturnType {
                        ty: int_ty,
                        is_fallible: false,
                    }),
                    body,
                    span: DUMMY,
                }));
            }

            let prog = Program {
                package: Some(PackageDecl {
                    path: arena.alloc_slice_clone(&["my_package"]),
                    span: DUMMY,
                }),
                imports: &[],
                items: items.into_bump_slice(),
                span: DUMMY,
            };
            black_box(prog);
            black_box(arena.allocated_bytes());
        });
    });

    group.finish();
}

// ── 9. Full small program AST ─────────────────────────────────────

fn bench_full_small_program(c: &mut Criterion) {
    let mut group = c.benchmark_group("ast/full_program");
    group.measurement_time(Duration::from_secs(5));

    group.bench_function("small_program", |b| {
        b.iter(|| {
            let arena = AstArena::new();
            let int_ty = int_type(&arena);

            // package demo
            let package = PackageDecl {
                path: arena.alloc_slice_clone(&["demo"]),
                span: DUMMY,
            };

            // summon std.io
            let mut imports = arena.vec::<Import>();
            imports.push(Import {
                kind: ImportKind::Summon {
                    path: arena.alloc_slice_clone(&["std", "io"]),
                    alias: None,
                },
                span: DUMMY,
            });
            let imports = imports.into_bump_slice();

            let mut items = arena.vec::<Item>();

            // const MAX: int = 100
            items.push(Item::Const(ConstDecl {
                attributes: &[],
                visibility: Visibility::Private,
                name: arena.alloc_str("MAX"),
                ty: Some(int_ty),
                value: int_lit(&arena, 100),
                span: DUMMY,
            }));

            // struct Point { x: int, y: int }
            let mut fields = arena.vec::<StructMember>();
            for name in &["x", "y"] {
                fields.push(StructMember::Field(FieldDecl {
                    visibility: Visibility::Public,
                    name: arena.alloc_str(name),
                    ty: int_ty,
                    span: DUMMY,
                }));
            }
            items.push(Item::Struct(StructDecl {
                attributes: &[],
                visibility: Visibility::Public,
                is_edge: false,
                name: arena.alloc_str("Point"),
                lifetime_params: &[],
                generic_params: &[],
                members: fields.into_bump_slice(),
                span: DUMMY,
            }));

            // fn main() { let p = Point { x = 0, y = 0 }; return }
            let mut field_inits = arena.vec::<FieldInit>();
            field_inits.push(FieldInit {
                name: arena.alloc_str("x"),
                value: int_lit(&arena, 0),
                span: DUMMY,
            });
            field_inits.push(FieldInit {
                name: arena.alloc_str("y"),
                value: int_lit(&arena, 0),
                span: DUMMY,
            });
            let point_lit = arena.alloc(Expr {
                kind: ExprKind::StructLit {
                    path: arena.alloc_slice_clone(&["Point"]),
                    fields: field_inits.into_bump_slice(),
                },
                span: DUMMY,
            });

            let let_p = arena.alloc(Stmt {
                kind: StmtKind::Let {
                    mutable: false,
                    binding: BindingTarget::Ident(arena.alloc_str("p")),
                    ty: None,
                    value: point_lit,
                },
                span: DUMMY,
            });
            let ret = arena.alloc(Stmt {
                kind: StmtKind::Return(None),
                span: DUMMY,
            });
            let main_body = Block {
                stmts: arena.alloc_slice_copy(&[*let_p, *ret]),
                span: DUMMY,
            };

            items.push(Item::Function(FunctionDecl {
                tier: TierAnnotation::High,
                attributes: &[],
                visibility: Visibility::Public,
                is_async: false,
                name: arena.alloc_str("main"),
                lifetime_params: &[],
                generic_params: &[],
                params: &[],
                return_type: None,
                body: main_body,
                span: DUMMY,
            }));

            let prog = Program {
                package: Some(package),
                imports,
                items: items.into_bump_slice(),
                span: DUMMY,
            };
            black_box(prog);
        });
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_alloc_leaf,
    bench_vec_to_slice,
    bench_binop_tree,
    bench_if_expr,
    bench_match_expr,
    bench_function_decl,
    bench_struct_decl,
    bench_arena_reuse,
    bench_full_small_program,
);
criterion_main!(benches);
