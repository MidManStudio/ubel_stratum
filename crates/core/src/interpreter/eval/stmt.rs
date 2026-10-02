// src/interpreter/eval/stmt.rs
//! Statement and block evaluation.

#![allow(dead_code)]

use crate::ast::expressions::{Expr, IfBranchBody};
use crate::ast::statements::{AllocatorKind, BindingTarget, Block, Stmt, StmtKind};
use crate::interpreter::eval::{expr, pattern, Interpreter};
use crate::interpreter::value::{EvalResult, Signal, Value};

// ── Block entry points ────────────────────────────────────────────

/// Evaluate a block: push scope, run all stmts in order, run deferred
/// expressions in LIFO order on exit, pop scope.
///
/// This is the standard entry point used everywhere except loop bodies
/// where the loop variable must share the block's scope.
pub fn eval_block<'ast>(interp: &mut Interpreter<'ast>, block: &Block<'ast>) -> EvalResult {
    let mut deferred: Vec<&'ast Expr<'ast>> = Vec::new();
    interp.env.push();
    let mut last = Value::Void;

    for &stmt in block.stmts {
        match eval_stmt(interp, &stmt, &mut deferred) {
            Ok(v)  => last = v,
            Err(sig) => {
                run_deferred(interp, &deferred);
                interp.env.pop();
                return Err(sig);
            }
        }
    }

    run_deferred(interp, &deferred);
    interp.env.pop();
    Ok(last)
}

/// Run a block's statements in the *current* scope without pushing a new one.
///
/// Used for:
/// - Loop bodies (where the iteration variable lives in the same scope)
/// - `try` catch bodies (scope is already pushed by the caller)
/// - `using` bodies (bindings pushed before entering)
pub fn eval_block_in_scope<'ast>(
    interp: &mut Interpreter<'ast>,
    block:  &Block<'ast>,
) -> EvalResult {
    let mut deferred: Vec<&'ast Expr<'ast>> = Vec::new();
    let mut last = Value::Void;

    for &stmt in block.stmts {
        match eval_stmt(interp, &stmt, &mut deferred) {
            Ok(v)    => last = v,
            Err(sig) => {
                run_deferred(interp, &deferred);
                return Err(sig);
            }
        }
    }

    run_deferred(interp, &deferred);
    Ok(last)
}

/// Evaluate an `if`/`elif`/`else` branch body — either a `{ block }`
/// (via `eval_block`, its own scope) or the single-line `then expr`
/// form. Shared by `StmtKind::If` (below) and `ExprKind::If`
/// (eval/expr.rs), same relationship `eval_block` already has to its
/// two callers.
pub(crate) fn eval_if_branch_body<'ast>(
    interp: &mut Interpreter<'ast>,
    body: &IfBranchBody<'ast>,
) -> EvalResult {
    match body {
        IfBranchBody::Expr(e)  => expr::eval_expr(interp, e),
        IfBranchBody::Block(b) => eval_block(interp, b),
    }
}

/// Run deferred expressions in LIFO order.
/// Errors are silently swallowed so they don't mask the signal that caused
/// scope exit.
fn run_deferred<'ast>(interp: &mut Interpreter<'ast>, deferred: &[&'ast Expr<'ast>]) {
    for &e in deferred.iter().rev() {
        let _ = expr::eval_expr(interp, e);
    }
}

// ── Statement dispatch ────────────────────────────────────────────

/// Evaluate a single statement, appending any `defer` expressions to
/// `deferred` for the enclosing block to execute on scope exit.
pub fn eval_stmt<'ast>(
    interp:   &mut Interpreter<'ast>,
    stmt:     &Stmt<'ast>,
    deferred: &mut Vec<&'ast Expr<'ast>>,
) -> EvalResult {
    use crate::ast::expressions::MatchArmBody;

    match &stmt.kind {

        // ── Variable binding ──────────────────────────────────────
        StmtKind::Let { binding, ty: _, value, .. } => {
            let val = expr::eval_expr(interp, value)?;
            bind_target(interp, binding, val);
            Ok(Value::Void)
        }

        // ── Expression statement ───────────────────────────────────
        StmtKind::Expr(e) => expr::eval_expr(interp, e),

        // ── Return ────────────────────────────────────────────────
        StmtKind::Return(maybe_e) => {
            let val = match maybe_e {
                Some(e) => expr::eval_expr(interp, e)?,
                None    => Value::Void,
            };
            Err(Signal::Return(val))
        }

        // ── Fail (typed error raise) ───────────────────────────────
        StmtKind::Fail(e) => {
            let val = expr::eval_expr(interp, e)?;
            Err(Signal::Fail(val))
        }

        // ── Break / Continue ──────────────────────────────────────
        StmtKind::Break(maybe_e) => {
            let val = match maybe_e {
                Some(e) => Some(expr::eval_expr(interp, e)?),
                None    => None,
            };
            Err(Signal::Break(val))
        }

        StmtKind::Continue => Err(Signal::Continue),

        // ── Defer ─────────────────────────────────────────────────
        // The expression is not evaluated now — it is pushed onto the
        // defer list and run when the enclosing block exits.
        StmtKind::Defer(e) => {
            deferred.push(e);
            Ok(Value::Void)
        }

        // ── If / elif / else ──────────────────────────────────────
        StmtKind::If(if_node) => {
            let cond = expr::eval_expr(interp, if_node.condition)?;
            if cond.is_truthy()? {
                return eval_if_branch_body(interp, &if_node.then_body);
            }
            for elif in if_node.elif_branches {
                let c = expr::eval_expr(interp, elif.condition)?;
                if c.is_truthy()? {
                    return eval_if_branch_body(interp, &elif.body);
                }
            }
            match &if_node.else_body {
                Some(b) => eval_if_branch_body(interp, b),
                None    => Ok(Value::Void),
            }
        }

        // ── Match ─────────────────────────────────────────────────
        StmtKind::Match { scrutinee, arms } => {
            let scrutinee_val = expr::eval_expr(interp, scrutinee)?;
            for arm in arms.iter() {
                // Push a scope for pattern bindings.
                interp.env.push();
                let matched = pattern::match_pattern(
                    &arm.pattern, &scrutinee_val, &mut interp.env,
                    pattern::PatternTables { enums: &interp.enum_table, aliases: &interp.type_aliases },
                );
                if matched {
                    // Check guard (can reference pattern bindings).
                    let guard_ok = match arm.guard {
                        Some(g) => expr::eval_expr(interp, g)?.is_truthy()?,
                        None    => true,
                    };
                    if guard_ok {
                        let result = match &arm.body {
                            MatchArmBody::Expr(e)  => expr::eval_expr(interp, e),
                            MatchArmBody::Block(b) => eval_block_in_scope(interp, b),
                        };
                        interp.env.pop();
                        return result;
                    }
                }
                interp.env.pop(); // pop if pattern didn't match or guard failed
            }
            // No arm matched — return Void (exhaustiveness is a sema concern).
            Ok(Value::Void)
        }

        // ── For loop ──────────────────────────────────────────────
        // The loop variable lives in the same scope as the body stmts
        // (no double-push). `eval_block_in_scope` is used for the body.
        StmtKind::For { binding, iter, body } => {
            let coll  = expr::eval_expr(interp, iter)?;
            let items = value_to_iter_vec(coll)?;

            for item in items {
                interp.env.push();
                bind_target(interp, binding, item);

                let res = eval_block_in_scope(interp, body);
                interp.env.pop();

                match res {
                    Ok(_) | Err(Signal::Continue) => {}
                    Err(Signal::Break(v)) => return Ok(v.unwrap_or(Value::Void)),
                    Err(sig)              => return Err(sig),
                }
            }
            Ok(Value::Void)
        }

        // ── While loop ────────────────────────────────────────────
        StmtKind::While { condition, body } => {
            loop {
                let cond = expr::eval_expr(interp, condition)?;
                if !cond.is_truthy()? { break; }

                match eval_block(interp, body) {
                    Ok(_) | Err(Signal::Continue) => {}
                    Err(Signal::Break(v)) => return Ok(v.unwrap_or(Value::Void)),
                    Err(sig)              => return Err(sig),
                }
            }
            Ok(Value::Void)
        }

        // ── Infinite loop ─────────────────────────────────────────
        StmtKind::Loop(body) => {
            loop {
                match eval_block(interp, body) {
                    Ok(_) | Err(Signal::Continue) => {}
                    Err(Signal::Break(v)) => return Ok(v.unwrap_or(Value::Void)),
                    Err(sig)              => return Err(sig),
                }
            }
        }

        // ── With block (arena / allocator) ────────────────────────
        // Tier enforcer already validated the tier constraint. Arena,
        // Gc, and Heap are all transparent blocks in the tree-walker —
        // real bump-allocation lands with the LLVM backend. Pool is the
        // one exception: `Pool.new()` (crate::builtins::instance::
        // pool_methods) needs a real capacity to size its slot table,
        // and unlike `List.new()` it has no generic argument of its own
        // to supply one — see `Interpreter::pool_capacity_stack`.
        StmtKind::With { allocator, body } => {
            if let AllocatorKind::Pool { count, .. } = allocator {
                let cap_val = expr::eval_expr(interp, count)?;
                let cap = match cap_val {
                    Value::Int(n) if n >= 0 => n as usize,
                    other => return Err(Signal::Panic(format!(
                        "pool capacity must be a non-negative int, got {}", other.type_name()
                    ))),
                };
                interp.pool_capacity_stack.push(cap);
                let result = eval_block(interp, body);
                interp.pool_capacity_stack.pop();
                result
            } else {
                eval_block(interp, body)
            }
        }

        // ── Using (RAII resource management) ─────────────────────
        StmtKind::Using { bindings, body } => {
            interp.env.push();
            for b in bindings.iter() {
                let val = expr::eval_expr(interp, b.value)?;
                interp.env.define(b.name, val);
            }
            // Note: cleanup (e.g., file.close()) would be called here on exit.
            // For the tree-walker MVP, Rust's Drop handles Rc cleanup.
            let result = eval_block_in_scope(interp, body);
            interp.env.pop();
            result
        }

        // ── Destructuring (`extract`) ──────────────────────────────
        StmtKind::Extract { pattern, value } => {
            let val = expr::eval_expr(interp, value)?;
            pattern::bind_destructure_pattern(pattern, val, &mut interp.env);
            Ok(Value::Void)
        }

        // ── Try / catch ───────────────────────────────────────────
        StmtKind::Try { body, catch_binding, catch_body } => {
            match eval_block(interp, body) {
                Ok(v) => Ok(v),
                Err(Signal::Fail(err_val)) => match catch_body {
                    Some(cb) => {
                        interp.env.push();
                        if let Some(name) = catch_binding {
                            interp.env.define(name, err_val);
                        }
                        let r = eval_block_in_scope(interp, cb);
                        interp.env.pop();
                        r
                    }
                    // `fail` with no catch body is swallowed — returns Null.
                    None => Ok(Value::Null),
                },
                // Non-Fail signals (Return, Break, Continue, Panic) propagate.
                Err(other) => Err(other),
            }
        }

        // ── Unsafe block ──────────────────────────────────────────
        // No unsafe boundary in the tree-walker — treat as a normal block.
        StmtKind::Unsafe(body) => eval_block(interp, body),
    }
}

// ── Binding helpers ───────────────────────────────────────────────

/// Bind a value to a BindingTarget in the current (innermost) scope.
pub fn bind_target<'ast>(
    interp: &mut Interpreter<'ast>,
    target: &BindingTarget<'ast>,
    value:  Value,
) {
    match target {
        BindingTarget::Ident(name) => interp.env.define(name, value),
        BindingTarget::Destructure(pat) => {
            pattern::bind_destructure_pattern(pat, value, &mut interp.env);
        }
    }
}

// ── Iterator helper ───────────────────────────────────────────────

/// Convert a runtime Value into a Vec for use as a for-loop iterator.
///
/// Supports:
/// - `Value::List`  — elements in order
/// - `Value::Tuple` — elements in order
/// - `Value::Str`   — Unicode scalar values as `Value::Char`
///
/// Range expressions (`0..10`) are evaluated to `Value::List` by
/// the BinOp evaluator in `eval_expr.rs`, so they arrive here already
/// as lists.
/// `for x in <expr> { }`'s runtime side — DATASTRUCTURES.md §1 for the
/// `Value::Pool` arm specifically: walks every currently-occupied slot
/// in index order (via `PoolData::iter_occupied`), holes skipped — the
/// actual skipfield behavior. Deliberately bare values, not `(Handle, T)`
/// pairs: pairing would need `for (h, v) in pool { }` to work, which
/// needs per-name destructure-binding typing that `record_binding`
/// doesn't actually do yet (`BindingTarget::Destructure` records one
/// type for the whole pattern's span, not a type per bound name — see
/// that function's own doc comment) — a real, separate gap, not solved
/// here. Bare-value iteration doesn't depend on it and covers the
/// "process every live entry" case; handle-per-item is a genuine
/// follow-up once destructure-binding typing exists for real.
///
/// Note for whoever eventually wires up `Value::Queue`/`Value::Stack`/
/// `Value::Set`/arrays here too: `element_type_of` (sema) already treats
/// all of those as directly iterable, but this function doesn't handle
/// any of them yet — a pre-existing gap, found while adding the `Pool`
/// arm, not introduced by it and not fixed here (out of scope for this
/// change; every current fixture that iterates only ever iterates a
/// `List`/`Tuple`/`Str`).
fn value_to_iter_vec(val: Value) -> Result<Vec<Value>, Signal> {
    match val {
        Value::List(rc)  => Ok(rc.borrow().clone()),
        Value::Tuple(v)  => Ok(v),
        Value::Str(s)    => Ok(s.chars().map(Value::Char).collect()),
        Value::Pool(rc)  => Ok(rc.borrow().iter_occupied().cloned().collect()),
        Value::InlineList(rc) => Ok(rc.borrow().items.clone()),
        other => Err(Signal::Panic(format!(
            "cannot iterate over value of type '{}'", other.type_name()
        ))),
    }
    }
