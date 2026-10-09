// ============================================================================
// NOTICE: Full documentation, design decisions, and fix history for this file
// live in docs/ubel_stratum.md, section "interpreter/eval/expr.rs"
// ============================================================================
// src/interpreter/eval/expr.rs
//! Expression evaluation.

#![allow(dead_code)]

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::ast::common::{AssignOp, BinOp, UnaryOp};
use crate::ast::expressions::{
    ArgKind, Expr, ExprKind, LambdaBody, MatchArmBody, OrElseFallback,
};
use crate::ast::literals::{Align, FormatSpec, InterpolationPart, IntSuffix, Literal, NumericBase};
use crate::ast::types::{Type, TypeKind};
use crate::interpreter::eval::{stmt, pattern, FunctionBody, FunctionDef, Interpreter};
use crate::interpreter::value::{EvalResult, Signal, Value};

// ── Main entry ────────────────────────────────────────────────────

pub fn eval_expr<'ast>(interp: &mut Interpreter<'ast>, expr: &Expr<'ast>) -> EvalResult {
    match &expr.kind {

        // ── Literals ──────────────────────────────────────────────
        // An unsuffixed integer literal whose context picked a width other
        // than plain `int` (sema records it in `int_literal_types`).
        ExprKind::Lit(Literal::Int(n)) => Ok(match interp.int_literal_types.get(&expr.span) {
            Some(suffix) => sized_int_from_literal(*n, *suffix),
            None         => Value::Int(*n),
        }),
        ExprKind::Lit(lit) => eval_literal(interp, lit),

        // ── Identifier lookup ─────────────────────────────────────
        ExprKind::Ident(name) => interp.lookup(name),

        // ── self ─────────────────────────────────────────────────
        ExprKind::SelfExpr => interp.lookup("self"),

        // ── Short declaration: x := expr ──────────────────────────
        // Defines in the current scope and returns the value.
        ExprKind::ShortDecl { name, value } => {
            let val = eval_expr(interp, value)?;
            interp.env.define(name, val.clone());
            Ok(val)
        }

        // ── Assignment ────────────────────────────────────────────
        ExprKind::Assign { op, target, value } => {
            eval_assign(interp, *op, target, value)
        }

        // ── Pipe: left |> right ───────────────────────────────────
        ExprKind::Pipe { left, right } => {
            let left_val = eval_expr(interp, left)?;
            let right_val = eval_expr(interp, right)?;
            match right_val {
                Value::Function(id) => interp.call_function(id, &[left_val]),
                other => Err(Signal::Panic(format!(
                    "right side of |> must be a function, got {}", other.type_name()
                ))),
            }
        }

        // ── Binary operators ─────────────────────────────────────
        ExprKind::BinOp { op, lhs, rhs } => {
            // Short-circuit logical operators.
            match op {
                BinOp::And => {
                    let l = eval_expr(interp, lhs)?;
                    if !l.is_truthy()? { return Ok(Value::Bool(false)); }
                    let r = eval_expr(interp, rhs)?;
                    Ok(Value::Bool(r.is_truthy()?))
                }
                BinOp::Or => {
                    let l = eval_expr(interp, lhs)?;
                    if l.is_truthy()? { return Ok(Value::Bool(true)); }
                    let r = eval_expr(interp, rhs)?;
                    Ok(Value::Bool(r.is_truthy()?))
                }
                _ => {
                    let lv = eval_expr(interp, lhs)?;
                    let rv = eval_expr(interp, rhs)?;
                    eval_binop(*op, lv, rv)
                }
            }
        }

        // ── Unary operators ───────────────────────────────────────
        ExprKind::UnaryOp { op, operand } => {
            let v = eval_expr(interp, operand)?;
            match op {
                UnaryOp::Neg => match v {
                    // `wrapping_neg`, not plain `-n`: negating `i64::MIN`
                    // (reachable via `-9223372036854775808i64`, or any
                    // computed value that happens to land there) would
                    // otherwise overflow and panic in a debug build —
                    // a real, pre-existing crash this closes at the same
                    // time as wiring up wrapping for the new sized
                    // variants below, not a new gap this feature
                    // introduced. `docs/PRINT_FORMAT_RULES.md` §4
                    // already documents `Int` as two's-complement,
                    // which is exactly wrapping semantics — this was
                    // always the intent, just not what `-n` actually did.
                    Value::Int(n)    => Ok(Value::Int(n.wrapping_neg())),
                    Value::I8(n)     => Ok(Value::I8(n.wrapping_neg())),
                    Value::I16(n)    => Ok(Value::I16(n.wrapping_neg())),
                    Value::I32(n)    => Ok(Value::I32(n.wrapping_neg())),
                    Value::Float(f)  => Ok(Value::Float(-f)),
                    Value::Double(d) => Ok(Value::Double(-d)),
                    // Unsigned types don't get a `-` operator at all in
                    // Rust either (`-5u8` is a compile error there) —
                    // matched here rather than silently wrapping, same
                    // "cannot negate" panic as any other non-numeric type.
                    other => Err(Signal::Panic(format!("cannot negate {}", other.type_name()))),
                },
                UnaryOp::Not => {
                    let b = v.is_truthy()?;
                    Ok(Value::Bool(!b))
                }
                UnaryOp::BitNot => match v {
                    Value::Int(n)  => Ok(Value::Int(!n)),
                    Value::I8(n)   => Ok(Value::I8(!n)),
                    Value::I16(n)  => Ok(Value::I16(!n)),
                    Value::I32(n)  => Ok(Value::I32(!n)),
                    Value::U8(n)   => Ok(Value::U8(!n)),
                    Value::U16(n)  => Ok(Value::U16(!n)),
                    Value::U32(n)  => Ok(Value::U32(!n)),
                    Value::UInt(n) => Ok(Value::UInt(!n)),
                    other => Err(Signal::Panic(format!("~ not supported on {}", other.type_name()))),
                },
                // Tree-walker doesn't implement real async — await is a no-op here.
                UnaryOp::Await => Ok(v),
            }
        }

        // ── Function / method call ────────────────────────────────
        ExprKind::Call { callee, args } => {
            // Check for method or static call: receiver.method(args)
            if let ExprKind::Field { target: recv_expr, field: method_name } = &callee.kind {
                return eval_call_with_receiver(interp, recv_expr, method_name, args, callee.span);
            }

            // Regular function call.
            let callee_val = eval_expr(interp, callee)?;
            let eval_args  = eval_args(interp, args)?;
            match callee_val {
                Value::Function(id) => interp.call_function(id, &eval_args),
                other => Err(Signal::Panic(format!(
                    "cannot call value of type '{}'", other.type_name()
                ))),
            }
        }

        // ── Field access: obj.field ───────────────────────────────
        ExprKind::Field { target, field } => {
            // `EnumName.Variant` constructs an enum value — this has to be
            // checked before evaluating `target`, since `EnumName` is a
            // type name, not something bound in the environment. Only
            // Fieldless (which also covers Discriminant — see
            // `VariantKind`) constructs here; Tuple/Struct variants need
            // their payload, which bare field-access syntax doesn't
            // supply — those go through the Call and StructLit arms
            // instead. Sema has already rejected a bare reference to a
            // payload-carrying variant by this point (`ExprKind::Field`'s
            // `VariantArityMismatch` check), so falling through to the
            // ordinary field lookup below is unreachable in practice for
            // valid programs, not a silent behavior change.
            if let ExprKind::Ident(name) = &target.kind {
                let canon = interp.canonical_type(name);
                if let Some(variants) = interp.enum_table.get(canon) {
                    if let Some(kind) = variants.get(*field) {
                        return if *kind == crate::interpreter::eval::VariantKind::Fieldless {
                            Ok(Value::Enum {
                                type_name: canon.to_string(),
                                variant:   field.to_string(),
                                payload:   Box::new(crate::interpreter::value::EnumPayload::None),
                            })
                        } else {
                            Err(Signal::Panic(format!(
                                "'{}.{}' needs a payload — use call or `{{ }}` syntax", name, field
                            )))
                        };
                    } else {
                        return Err(Signal::Panic(format!(
                            "enum '{}' has no variant '{}'", name, field
                        )));
                    }
                }
            }
            let obj = eval_expr(interp, target)?;
            get_field(obj, field)
        }

        // ── Index: collection[index] ──────────────────────────────
        ExprKind::Index { target, index } => {
            let coll = eval_expr(interp, target)?;
            let idx  = eval_expr(interp, index)?;
            eval_index(coll, idx)
        }

        // ── Optional chain: obj?.field or obj?.method() ───────────
        ExprKind::OptionalChain { target, access } => {
            let obj = eval_expr(interp, target)?;
            if matches!(obj, Value::Null) {
                return Ok(Value::Null);
            }
            use crate::ast::expressions::OptionalAccess;
            match access {
                OptionalAccess::Field(field) => get_field(obj, field),
                OptionalAccess::Method { name, args } => {
                    let eval_args = eval_args(interp, args)?;
                    eval_method_call(interp, obj, name, &eval_args)
                }
            }
        }

        // ── Error propagation: expr? ──────────────────────────────
        // Signal::Fail propagates naturally up the call stack.
        // On Ok(val), strip Optional / Fallible wrapper and continue.
        ExprKind::Try(inner) => {
            let val = eval_expr(interp, inner)?;
            // In the tree-walker, fallible results are just values —
            // Signal::Fail would already have propagated above.
            Ok(val)
        }

        // ── Await: async is a no-op in the tree-walker ────────────
        ExprKind::Await(inner) => eval_expr(interp, inner),

        // ── Borrow: &place / ref place, &mut place / ref mut place ─
        // No runtime representation yet — same as GcRef/ArenaRef/
        // OwnedRef, tier/reference-ness is erased after sema (see
        // MEMORY_MODEL.md). Struct/List/Dict values are already
        // Rc<RefCell<_>>-backed, so a "borrow" of one of those already
        // aliases correctly for free. Plain passthrough is a real,
        // documented gap for scalar-typed borrows specifically — see
        // write_lvalue's ExprKind::Deref arm below for the write side.
        ExprKind::Borrow { place, .. } => eval_expr(interp, place),

        // ── Dereference: *place / deref place ──────────────────────
        // Read side only — see the write-through-deref note above.
        ExprKind::Deref(inner) => eval_expr(interp, inner),

        // ── Type cast: expr as Type ───────────────────────────────
        ExprKind::As { expr: inner, ty } => {
            let val = eval_expr(interp, inner)?;
            eval_cast(val, ty)
        }

        // ── Array literal: [a, b, c] ──────────────────────────────
        ExprKind::Array(elems) => {
            let items: Result<Vec<Value>, Signal> = elems.iter()
                .map(|e| eval_expr(interp, e))
                .collect();
            Ok(Value::List(Rc::new(RefCell::new(items?))))
        }

        // ── Tuple literal: (a, b, c) ─────────────────────────────
        ExprKind::Tuple(elems) => {
            let items: Result<Vec<Value>, Signal> = elems.iter()
                .map(|e| eval_expr(interp, e))
                .collect();
            Ok(Value::Tuple(items?))
        }

        // ── Dictionary literal: { key = value, ... } ─────────────
        ExprKind::Dict(entries) => {
            let mut pairs: Vec<(Value, Value)> = Vec::with_capacity(entries.len());
            for entry in entries.iter() {
                let k = eval_expr(interp, entry.key)?;
                let v = eval_expr(interp, entry.value)?;
                pairs.push((k, v));
            }
            Ok(Value::Dict(Rc::new(RefCell::new(pairs))))
        }

        // ── Anonymous object: { x = 1, y = 2 } ───────────────────
        ExprKind::AnonObject(fields) => {
            let mut map = HashMap::new();
            for f in fields.iter() {
                let v = eval_expr(interp, f.value)?;
                map.insert(f.name.to_string(), v);
            }
            Ok(Value::Struct {
                type_name: "<anon>".to_string(),
                fields:    Rc::new(RefCell::new(map)),
                // No declaration to have `@derive`d anything from.
                derives_partial_eq: false,
                derives_ord:        false,
                derives_hash:       false,
                derives_clone:      false,
                field_order:        Rc::new(Vec::new()),
            })
        }

        // ── Struct literal: Point { x = 1, y = 2 } ───────────────
        ExprKind::StructLit { path, fields } => {
            // ENUM_RULES.md — `Message.Move { x = 1, y = 2 }`, struct-
            // payload variant construction. Same signal as sema uses to
            // tell this apart from a plain struct literal: 2+ path
            // segments, first one names a known enum. Sema has already
            // validated field names/types by this point.
            if path.len() >= 2 {
                let canon_ref = interp.canonical_type(path[0]);
                if let Some(variants) = interp.enum_table.get(canon_ref) {
                    let variant = path[path.len() - 1];
                    if variants.get(variant) == Some(&crate::interpreter::eval::VariantKind::Struct) {
                        // Owned, so the borrow of `interp` ends before the
                        // field values (which need `interp` mutably) run.
                        let canon = canon_ref.to_string();
                        let mut map = HashMap::new();
                        for f in fields.iter() {
                            let v = eval_expr(interp, f.value)?;
                            map.insert(f.name.to_string(), v);
                        }
                        return Ok(Value::Enum {
                            type_name: canon,
                            variant:   variant.to_string(),
                            payload:   Box::new(crate::interpreter::value::EnumPayload::Struct(map)),
                        });
                    }
                }
            }
            let type_name = interp.canonical_type(path.last().copied().unwrap_or("")).to_string();
            let mut map = HashMap::new();
            for f in fields.iter() {
                let v = eval_expr(interp, f.value)?;
                map.insert(f.name.to_string(), v);
            }
            let derived = interp.struct_derives.get(&type_name);
            Ok(Value::Struct {
                derives_partial_eq: derived.is_some_and(|t| t.contains("PartialEq")),
                // `Ord` requires `PartialOrd` also be present (checked at
                // TYPE-117), so checking for `PartialOrd` alone already
                // catches both, see the `derives_ord` doc comment on
                // `Value::Struct` in `interpreter/value.rs`.
                derives_ord:   derived.is_some_and(|t| t.contains("PartialOrd")),
                derives_hash:  derived.is_some_and(|t| t.contains("Hash")),
                derives_clone: derived.is_some_and(|t| t.contains("Clone")),
                field_order: interp.struct_field_order
                    .get(&type_name)
                    .cloned()
                    .unwrap_or_default(),
                type_name,
                fields: Rc::new(RefCell::new(map)),
            })
        }

        // ── Lambda: fn(params) body ───────────────────────────────
        ExprKind::Lambda(lambda) => {
            let closure: crate::interpreter::env::Environment = interp.env.snapshot();
            let params: Vec<String> = lambda.params.iter()
                .map(|p| p.name.to_string())
                .collect();
            let body = match &lambda.body {
                LambdaBody::Block(b) => FunctionBody::Ast { block: *b },
                LambdaBody::Expr(e)  => FunctionBody::ExprBody { expr: e },
            };
            let id = interp.alloc_function(FunctionDef {
                name:     None,
                params,
                body,
                closure,
                tier:     crate::ast::common::TierAnnotation::High,
                is_async: false,
            });
            Ok(Value::Function(id))
        }

        // ── Block expression: { stmts } ───────────────────────────
        ExprKind::Block(b) => stmt::eval_block(interp, b),

        // ── If expression ─────────────────────────────────────────
        ExprKind::If(if_node) => {
            let cond = eval_expr(interp, if_node.condition)?;
            if cond.is_truthy()? {
                return stmt::eval_if_branch_body(interp, &if_node.then_body);
            }
            for elif in if_node.elif_branches {
                let c = eval_expr(interp, elif.condition)?;
                if c.is_truthy()? {
                    return stmt::eval_if_branch_body(interp, &elif.body);
                }
            }
            match &if_node.else_body {
                Some(b) => stmt::eval_if_branch_body(interp, b),
                None    => Ok(Value::Void),
            }
        }

        // ── Match expression ─────────────────────────────────────
        ExprKind::Match(m) => {
            let scrutinee = eval_expr(interp, m.scrutinee)?;
            for arm in m.arms.iter() {
                interp.env.push();
                let matched = pattern::match_pattern(
                    &arm.pattern, &scrutinee, &mut interp.env,
                    pattern::PatternTables { enums: &interp.enum_table, aliases: &interp.type_aliases },
                );
                if matched {
                    let guard_ok = match arm.guard {
                        Some(g) => eval_expr(interp, g)?.is_truthy()?,
                        None    => true,
                    };
                    if guard_ok {
                        let result = match &arm.body {
                            MatchArmBody::Expr(e)  => eval_expr(interp, e),
                            MatchArmBody::Block(b) => stmt::eval_block(interp, b),
                        };
                        interp.env.pop();
                        return result;
                    }
                }
                interp.env.pop();
            }
            Ok(Value::Void)
        }

        // ── Or-else: expr or fallback ─────────────────────────────
        ExprKind::OrElse { expr: inner, fallback } => {
            let val = eval_expr(interp, inner)?;
            match &val {
                Value::Null => match fallback {
                    OrElseFallback::Expr(fb) => eval_expr(interp, fb),
                    OrElseFallback::Continue => Err(Signal::Continue),
                    OrElseFallback::Break    => Err(Signal::Break(None)),
                    OrElseFallback::Return(maybe_e) => {
                        let v = match maybe_e {
                            Some(e) => eval_expr(interp, e)?,
                            None    => Value::Void,
                        };
                        Err(Signal::Return(v))
                    }
                },
                _ => Ok(val),
            }
        }
    }
}

// ── Literal evaluation ────────────────────────────────────────────

/// The value of an unsuffixed integer literal that sema resolved to the
/// width `suffix` names. Sema has already range-checked `n` against that
/// width (`TYPE-120`), so the `as` casts are mechanical narrowing, not a
/// second check. A literal directly under `-` reaches here with its
/// positive magnitude and is negated afterwards by `UnaryOp::Neg`, whose
/// `wrapping_neg` makes the most-negative value (`-128` for `i8`) work.
fn sized_int_from_literal(n: i64, suffix: IntSuffix) -> Value {
    match suffix {
        IntSuffix::I8    => Value::I8(n as i8),
        IntSuffix::I16   => Value::I16(n as i16),
        IntSuffix::I32   => Value::I32(n as i32),
        IntSuffix::I64   => Value::Int(n),
        IntSuffix::Isize => Value::Int(n),
        IntSuffix::U8    => Value::U8(n as u8),
        IntSuffix::U16   => Value::U16(n as u16),
        IntSuffix::U32   => Value::U32(n as u32),
        IntSuffix::U64   => Value::UInt(n as u64),
        IntSuffix::Usize => Value::UInt(n as u64),
    }
}

fn eval_literal<'ast>(interp: &mut Interpreter<'ast>, lit: &Literal<'ast>) -> EvalResult {
    match lit {
        Literal::Int(n)    => Ok(Value::Int(*n)),
        // Sema (`TYPE-120`) already confirmed `raw` fits `suffix`'s range
        // before this ever runs; the `as` truncations below are just the
        // mechanical width-narrowing from the wide `u64` token payload
        // down to each suffix's real Rust type, not a second range check.
        Literal::TypedInt { raw, suffix } => Ok(match suffix {
            IntSuffix::I8    => Value::I8(*raw as i8),
            IntSuffix::I16   => Value::I16(*raw as i16),
            IntSuffix::I32   => Value::I32(*raw as i32),
            IntSuffix::I64   => Value::Int(*raw as i64),
            IntSuffix::Isize => Value::Int(*raw as i64),
            IntSuffix::U8    => Value::U8(*raw as u8),
            IntSuffix::U16   => Value::U16(*raw as u16),
            IntSuffix::U32   => Value::U32(*raw as u32),
            IntSuffix::U64   => Value::UInt(*raw),
            IntSuffix::Usize => Value::UInt(*raw),
        }),
        Literal::Float(f)  => Ok(Value::Float(*f)),
        Literal::Double(d) => Ok(Value::Double(*d)),
        Literal::Bool(b)   => Ok(Value::Bool(*b)),
        Literal::Char(c)   => Ok(Value::Char(*c)),
        Literal::Null      => Ok(Value::Null),

        Literal::Str(s)        => Ok(Value::str_from(*s)),
        Literal::VerbatimStr(s) => Ok(Value::str_from(*s)),

        // Interpolation holes are fully parsed Expr nodes by the time the
        // interpreter sees them (parsed by rd_parser, alongside everything
        // else) — evaluating one is no different from evaluating any other
        // expression in the language.
        Literal::InterpolatedStr(parts)
        | Literal::InterpolatedVerbatimStr(parts) => {
            let mut result = String::new();
            for part in parts.iter() {
                match part {
                    InterpolationPart::Text(t) => result.push_str(t),
                    InterpolationPart::Expr { expr, spec } => {
                        let val = eval_expr(interp, expr)?;
                        result.push_str(&apply_format_spec(&val, spec.as_ref()));
                    }
                }
            }
            Ok(Value::str_from(result))
        }
    }
}

/// Renders `val` per `spec`: width/precision/alignment/fill/sign/
/// alternate-form/zero-padding/numeric bases (docs/PRINT_FORMAT_RULES.md
/// §4). `spec == None` is exactly the old `val.to_string()` behavior,
/// unchanged.
fn apply_format_spec(val: &Value, spec: Option<&FormatSpec>) -> String {
    let Some(spec) = spec else { return val.to_string(); };

    // A numeric base only ever applies to Int (TYPE-115), and handles
    // its own sign/alternate-prefix/zero-pad together, since those three
    // all land between the sign and the digits, not around an
    // already-rendered decimal string the way width/align do for
    // everything else.
    if let (Value::Int(n), Some(base)) = (val, spec.base) {
        return render_int_with_base(*n, base, spec);
    }

    // Precision: sema already rejected this combination for anything
    // that isn't Float/Double/Str (TypeError::InvalidFormatSpec,
    // TYPE-1xx — see type_infer.rs), so reaching here with a precision
    // on some other type would mean sema has a bug, not that this code
    // needs its own fallback story for it.
    //
    // `spec.debug` picks Value::debug_string() over Display as the base
    // formatter. For Str+precision specifically, precision truncates the
    // raw content FIRST and debug-quoting (if requested) wraps the
    // already-truncated result second — truncating a quoted/escaped
    // string by character count would risk cutting an escape sequence
    // in half or leaving an unbalanced quote.
    let base = match (val, spec.precision) {
        (Value::Float(f), Some(p))  => format!("{:.*}", p as usize, f),
        (Value::Double(f), Some(p)) => format!("{:.*}", p as usize, f),
        (Value::Str(s), Some(p)) => {
            let p = p as usize;
            let truncated: String =
                if s.chars().count() <= p { s.to_string() } else { s.chars().take(p).collect() };
            if spec.debug { format!("{:?}", truncated) } else { truncated }
        }
        _ if spec.debug => val.debug_string(),
        _ => val.to_string(),
    };

    // Sign forcing: a negative Int/Float/Double already renders its own
    // `-` via `to_string()`/the precision branch above; `+` only ever
    // needs adding for a non-negative one (TYPE-115 already rejects
    // sign_plus on anything non-numeric, so the wildcard arm below never
    // needs to worry about e.g. a Str starting with a digit).
    let base = if spec.sign_plus && is_non_negative_number(val) {
        format!("+{base}")
    } else {
        base
    };

    pad_to_width(&base, spec.width, spec.zero_pad, spec.fill, spec.align)
}

fn is_non_negative_number(val: &Value) -> bool {
    match val {
        Value::Int(n)    => *n >= 0,
        Value::Float(f)  => *f >= 0.0,
        Value::Double(f) => *f >= 0.0,
        _ => false,
    }
}

/// Shared width/fill/align padding, used both for the general case above
/// and for `render_int_with_base` below. Zero-padding is handled
/// separately from fill/align: it always pads immediately before the
/// digits (after any sign), never around the outside the way a custom
/// fill character does, and ignores `align` entirely, matching the
/// well-established convention this feature is modeled on (Rust's own
/// `format!`), not something invented for this one.
fn pad_to_width(s: &str, width: Option<u32>, zero_pad: bool, fill: Option<char>, align: Option<Align>) -> String {
    let Some(width) = width else { return s.to_string(); };
    let width = width as usize;
    let len = s.chars().count();
    if len >= width { return s.to_string(); }
    let pad = width - len;

    if zero_pad {
        let (sign, rest) = if s.starts_with('-') || s.starts_with('+') {
            (&s[..1], &s[1..])
        } else {
            ("", s)
        };
        return format!("{sign}{}{rest}", "0".repeat(pad));
    }

    // No explicit align marker: left-align, matching "text flows left by
    // default" rather than Rust's type-dependent default (right for
    // numbers, left for strings), a deliberate simplification since
    // applying that here would need type info this function doesn't
    // have. Use `>` explicitly for right-aligned numbers. `fill`
    // defaults to space, same as it always implicitly did before this
    // delivery, now just an explicit default rather than the only option.
    let fill = fill.unwrap_or(' ');
    match align.unwrap_or(Align::Left) {
        Align::Left   => format!("{s}{}", fill.to_string().repeat(pad)),
        Align::Right  => format!("{}{s}", fill.to_string().repeat(pad)),
        Align::Center => {
            let left  = pad / 2;
            let right = pad - left;
            format!("{}{s}{}", fill.to_string().repeat(left), fill.to_string().repeat(right))
        }
    }
}

/// `Int` rendered in a non-decimal base: the sign, the `#` alternate-form
/// prefix (`0x`/`0X`/`0o`/`0b`), and zero-padding all need to land between
/// each other in a specific order (sign, then prefix, then zero-fill,
/// then digits: `-0x00ff`, not `00-0xff`), so this builds the whole thing
/// directly rather than reusing `pad_to_width`'s generic sign-stripping.
/// A negative value renders as its 64-bit two's-complement bit pattern in
/// the chosen base (matching Rust's own `{:x}` on a signed integer), not
/// a `-` sign plus the magnitude's digits.
fn render_int_with_base(n: i64, base: NumericBase, spec: &FormatSpec) -> String {
    let digits = match base {
        NumericBase::Hex      => format!("{:x}", n),
        NumericBase::HexUpper => format!("{:X}", n),
        NumericBase::Octal    => format!("{:o}", n),
        NumericBase::Binary   => format!("{:b}", n),
    };
    let sign = if spec.sign_plus && n >= 0 { "+" } else { "" };
    let prefix = if spec.alternate {
        match base {
            NumericBase::Hex      => "0x",
            NumericBase::HexUpper => "0X",
            NumericBase::Octal    => "0o",
            NumericBase::Binary   => "0b",
        }
    } else {
        ""
    };
    let core = format!("{sign}{prefix}{digits}");

    let Some(width) = spec.width else { return core; };
    let width = width as usize;
    let len = core.chars().count();
    if len >= width { return core; }
    let pad = width - len;

    if spec.zero_pad {
        format!("{sign}{prefix}{}{digits}", "0".repeat(pad))
    } else {
        let fill = spec.fill.unwrap_or(' ');
        match spec.align.unwrap_or(Align::Left) {
            Align::Left   => format!("{core}{}", fill.to_string().repeat(pad)),
            Align::Right  => format!("{}{core}", fill.to_string().repeat(pad)),
            Align::Center => {
                let left  = pad / 2;
                let right = pad - left;
                format!("{}{core}{}", fill.to_string().repeat(left), fill.to_string().repeat(right))
            }
        }
    }
}

// ── Binary operator evaluation ────────────────────────────────────

fn eval_binop(op: BinOp, lhs: Value, rhs: Value) -> EvalResult {
    // String concatenation.
    if let BinOp::Add = op {
        if let (Value::Str(a), Value::Str(b)) = (&lhs, &rhs) {
            return Ok(Value::str_from(format!("{}{}", a, b)));
        }
    }

    // Range operators → produce a list of integers.
    match op {
        BinOp::Range => {
            if let (Value::Int(lo), Value::Int(hi)) = (&lhs, &rhs) {
                let items: Vec<Value> = (*lo..*hi).map(Value::Int).collect();
                return Ok(Value::List(Rc::new(RefCell::new(items))));
            }
            return Err(Signal::Panic(".. requires integer operands".into()));
        }
        BinOp::RangeIncl => {
            if let (Value::Int(lo), Value::Int(hi)) = (&lhs, &rhs) {
                let items: Vec<Value> = (*lo..=*hi).map(Value::Int).collect();
                return Ok(Value::List(Rc::new(RefCell::new(items))));
            }
            return Err(Signal::Panic("..= requires integer operands".into()));
        }
        _ => {}
    }

    // Equality — works on any type.
    match op {
        BinOp::Eq => return Ok(Value::Bool(lhs.equals(&rhs))),
        BinOp::Ne => return Ok(Value::Bool(!lhs.equals(&rhs))),
        _ => {}
    }

    // Ordering on Str/Struct/Unique/Shared/SyncShared, new this
    // delivery, via `Value::partial_cmp` (TYPE-118 has already gated
    // this at sema time for well-formed programs; a bare interpreter
    // test that skips sema still ends up here safely, since
    // `partial_cmp` itself returns `None` for anything not actually
    // comparable, same fallback the `None` arm below reaches). Int/
    // Float/Double keep using the existing numeric path below,
    // unchanged, not folded into `partial_cmp` here, since it already
    // works and touching it isn't this delivery's job.
    match op {
        BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge
            if matches!(lhs, Value::Str(_) | Value::Struct { .. }
                | Value::Unique(_) | Value::Shared(_) | Value::SyncShared(_))
            || matches!(rhs, Value::Str(_) | Value::Struct { .. }
                | Value::Unique(_) | Value::Shared(_) | Value::SyncShared(_)) =>
        {
            return match (op, lhs.partial_cmp(&rhs)) {
                (BinOp::Lt, Some(o)) => Ok(Value::Bool(o.is_lt())),
                (BinOp::Le, Some(o)) => Ok(Value::Bool(o.is_le())),
                (BinOp::Gt, Some(o)) => Ok(Value::Bool(o.is_gt())),
                (BinOp::Ge, Some(o)) => Ok(Value::Bool(o.is_ge())),
                (_, None) => Err(Signal::Panic("comparison between incomparable values".into())),
                _ => unreachable!(),
            };
        }
        _ => {}
    }

    // Sized integers (i8/u8/i16/u16/i32/u32/u64) — real, width-correct
    // wrapping arithmetic, comparisons, and bitwise ops, entirely
    // separate from the Int/Float/Double path below (which promotes
    // through f64 and so both loses precision above 2^53 and has no
    // notion of a type narrower than i64 to wrap at in the first
    // place — neither is fit for purpose here).
    if let Some(result) = sized_int_binop(op, &lhs, &rhs) {
        return result;
    }

    // Numeric operations with implicit promotion.
    // Promotion ladder: Int → Float → Double.
    let (lv, rv, is_double, is_float) = promote_numeric(&lhs, &rhs)?;

    let result = match op {
        BinOp::Add  => lv + rv,
        BinOp::Sub  => lv - rv,
        BinOp::Mul  => lv * rv,
        BinOp::Div  => {
            if rv == 0.0 { return Err(Signal::Panic("division by zero".into())); }
            lv / rv
        }
        BinOp::Rem  => {
            if rv == 0.0 { return Err(Signal::Panic("modulo by zero".into())); }
            lv % rv
        }
        BinOp::Lt   => return Ok(Value::Bool(lv < rv)),
        BinOp::Le   => return Ok(Value::Bool(lv <= rv)),
        BinOp::Gt   => return Ok(Value::Bool(lv > rv)),
        BinOp::Ge   => return Ok(Value::Bool(lv >= rv)),

        BinOp::BitAnd => return bitwise_op(op, &lhs, &rhs),
        BinOp::BitOr  => return bitwise_op(op, &lhs, &rhs),
        BinOp::BitXor => return bitwise_op(op, &lhs, &rhs),
        BinOp::Shl    => return bitwise_op(op, &lhs, &rhs),
        BinOp::Shr    => return bitwise_op(op, &lhs, &rhs),

        _ => return Err(Signal::Panic(format!("unsupported binary op: {:?}", op))),
    };

    if is_double {
        Ok(Value::Double(result))
    } else if is_float {
        Ok(Value::Float(result as f32))
    } else {
        Ok(Value::Int(result as i64))
    }
}

/// Real, width-correct arithmetic/comparison/bitwise ops for the sized
/// integer `Value` variants (`I8`/`I16`/`I32`/`U8`/`U16`/`U32`/`UInt`).
/// Both operands must already be the *same* variant — sema's own
/// `unify` requires matching operand types for a well-typed binary op,
/// so a mismatch reaching here means an ill-typed AST got to the
/// interpreter some other way (a hand-built test bypassing sema, most
/// likely); either way a clear panic beats silently picking a width or
/// promoting one side, which this language doesn't otherwise do.
///
/// `Some(_)` short-circuits the caller straight back out; `None` means
/// neither side is one of these variants at all, so the caller falls
/// through to the existing `Int`/`Float`/`Double` path unchanged.
fn sized_int_binop(op: BinOp, lhs: &Value, rhs: &Value) -> Option<EvalResult> {
    macro_rules! width {
        ($variant:ident, $mask:expr) => {
            if let (Value::$variant(a), Value::$variant(b)) = (lhs, rhs) {
                let (a, b) = (*a, *b);
                return Some(match op {
                    BinOp::Add => Ok(Value::$variant(a.wrapping_add(b))),
                    BinOp::Sub => Ok(Value::$variant(a.wrapping_sub(b))),
                    BinOp::Mul => Ok(Value::$variant(a.wrapping_mul(b))),
                    BinOp::Div =>
                        if b == 0 { Err(Signal::Panic("division by zero".into())) }
                        else { Ok(Value::$variant(a.wrapping_div(b))) },
                    BinOp::Rem =>
                        if b == 0 { Err(Signal::Panic("modulo by zero".into())) }
                        else { Ok(Value::$variant(a.wrapping_rem(b))) },
                    BinOp::Lt => Ok(Value::Bool(a < b)),
                    BinOp::Le => Ok(Value::Bool(a <= b)),
                    BinOp::Gt => Ok(Value::Bool(a > b)),
                    BinOp::Ge => Ok(Value::Bool(a >= b)),
                    BinOp::BitAnd => Ok(Value::$variant(a & b)),
                    BinOp::BitOr  => Ok(Value::$variant(a | b)),
                    BinOp::BitXor => Ok(Value::$variant(a ^ b)),
                    // Shift amount masked to the type's own bit width,
                    // same convention the existing i64 path already
                    // uses (`a << (b & 63)` below) — defined for any
                    // `b`, including a negative one, rather than
                    // matching Rust's own panic-on-out-of-range-shift.
                    BinOp::Shl => Ok(Value::$variant(a.wrapping_shl((b as u32) & $mask))),
                    BinOp::Shr => Ok(Value::$variant(a.wrapping_shr((b as u32) & $mask))),
                    _ => Err(Signal::Panic(format!("unsupported binary op: {:?}", op))),
                });
            }
        };
    }
    width!(I8,   7);
    width!(I16,  15);
    width!(I32,  31);
    width!(U8,   7);
    width!(U16,  15);
    width!(U32,  31);
    width!(UInt, 63);

    let is_sized = |v: &Value| matches!(v,
        Value::I8(_) | Value::I16(_) | Value::I32(_)
        | Value::U8(_) | Value::U16(_) | Value::U32(_) | Value::UInt(_));
    if is_sized(lhs) || is_sized(rhs) {
        return Some(Err(Signal::Panic(format!(
            "type mismatch in binary op: {} and {}", lhs.type_name(), rhs.type_name()
        ))));
    }
    None
}

/// Promote both values to f64 for arithmetic.
/// Returns (lv, rv, is_double, is_float).
fn promote_numeric(lhs: &Value, rhs: &Value) -> Result<(f64, f64, bool, bool), Signal> {
    let to_f64 = |v: &Value| -> Option<(f64, bool, bool)> {
        match v {
            Value::Int(n)    => Some((*n as f64, false, false)),
            Value::Float(f)  => Some((*f as f64, false, true)),
            Value::Double(d) => Some((*d, true, false)),
            _ => None,
        }
    };
    let (lv, ld, lf) = to_f64(lhs).ok_or_else(|| Signal::Panic(format!(
        "arithmetic not supported on {}", lhs.type_name()
    )))?;
    let (rv, rd, rf) = to_f64(rhs).ok_or_else(|| Signal::Panic(format!(
        "arithmetic not supported on {}", rhs.type_name()
    )))?;
    Ok((lv, rv, ld || rd, lf || rf))
}

fn bitwise_op(op: BinOp, lhs: &Value, rhs: &Value) -> EvalResult {
    match (lhs, rhs) {
        (Value::Int(a), Value::Int(b)) => Ok(Value::Int(match op {
            BinOp::BitAnd => a & b,
            BinOp::BitOr  => a | b,
            BinOp::BitXor => a ^ b,
            BinOp::Shl    => a << (b & 63),
            BinOp::Shr    => a >> (b & 63),
            _ => unreachable!(),
        })),
        _ => Err(Signal::Panic(format!(
            "bitwise op requires int operands, got {} and {}",
            lhs.type_name(), rhs.type_name()
        ))),
    }
}

// ── Assignment ────────────────────────────────────────────────────

fn eval_assign<'ast>(
    interp: &mut Interpreter<'ast>,
    op:     AssignOp,
    target: &'ast Expr<'ast>,
    value:  &'ast Expr<'ast>,
) -> EvalResult {
    let rhs = eval_expr(interp, value)?;

    if let AssignOp::Assign = op {
        return write_lvalue(interp, target, rhs);
    }

    // Compound assignment: read → binop → write.
    let current = read_lvalue(interp, target)?;
    let binop   = assign_op_to_binop(op);
    let new_val = eval_binop(binop, current, rhs)?;
    write_lvalue(interp, target, new_val)
}

fn assign_op_to_binop(op: AssignOp) -> BinOp {
    match op {
        AssignOp::Assign     => BinOp::Add, // unreachable in compound path
        AssignOp::AddAssign  => BinOp::Add,
        AssignOp::SubAssign  => BinOp::Sub,
        AssignOp::MulAssign  => BinOp::Mul,
        AssignOp::DivAssign  => BinOp::Div,
        AssignOp::RemAssign  => BinOp::Rem,
        AssignOp::BitAndAssign => BinOp::BitAnd,
        AssignOp::BitOrAssign  => BinOp::BitOr,
        AssignOp::BitXorAssign => BinOp::BitXor,
        AssignOp::ShlAssign    => BinOp::Shl,
        AssignOp::ShrAssign    => BinOp::Shr,
    }
}

/// Read the current value of an lvalue expression without consuming it.
fn read_lvalue<'ast>(interp: &mut Interpreter<'ast>, target: &'ast Expr<'ast>) -> EvalResult {
    eval_expr(interp, target)
}

/// Write a value to an lvalue expression.
fn write_lvalue<'ast>(
    interp: &mut Interpreter<'ast>,
    target: &'ast Expr<'ast>,
    value:  Value,
) -> EvalResult {
    match &target.kind {
        ExprKind::Ident(name) => {
            // Update an existing local first (a local shadows a static of
            // the same name), then a static, and only define a new binding
            // if the name is neither.
            if !interp.env.set(name, value.clone()) {
                if let Some(slot) = interp.statics.get_mut(*name) {
                    *slot = value;
                } else {
                    interp.env.define(name, value);
                }
            }
            Ok(Value::Void)
        }
        ExprKind::Field { target: obj_expr, field } => {
            let obj = eval_expr(interp, obj_expr)?;
            match obj {
                Value::Struct { fields, .. } => {
                    fields.borrow_mut().insert(field.to_string(), value);
                    Ok(Value::Void)
                }
                other => Err(Signal::Panic(format!(
                    "cannot assign to field '{}' on {}", field, other.type_name()
                ))),
            }
        }
        ExprKind::Index { target: coll_expr, index: idx_expr } => {
            let coll = eval_expr(interp, coll_expr)?;
            let idx  = eval_expr(interp, idx_expr)?;
            match (coll, idx) {
                (Value::List(rc), Value::Int(i)) => {
                    let mut list = rc.borrow_mut();
                    let i = i as usize;
                    if i < list.len() {
                        list[i] = value;
                        Ok(Value::Void)
                    } else {
                        Err(Signal::Panic(format!(
                            "index {} out of bounds (len {})", i, list.len()
                        )))
                    }
                }
                (Value::Dict(rc), key) => {
                    let mut dict = rc.borrow_mut();
                    if let Some(entry) = dict.iter_mut().find(|(k, _)| k.equals(&key)) {
                        entry.1 = value;
                    } else {
                        dict.push((key, value));
                    }
                    Ok(Value::Void)
                }
                _ => Err(Signal::Panic("invalid index assignment target".into())),
            }
        }
        // Assignment THROUGH a dereferenced reference (`*p = v` /
        // `deref p = v`) needs a real persisted place — a runtime
        // reference-cell representation (`Value::Ref(Rc<RefCell<Value>>)`
        // or similar) that plain-value Borrow/Deref passthrough doesn't
        // have yet. Rather than silently writing to the wrong place
        // (e.g. collapsing to the pointer variable itself), this is a
        // clear, loud "not yet" until that representation exists —
        // naturally pairs with the CFG/loan-tracking work ahead, not
        // separate follow-up.
        ExprKind::Deref(_) => Err(Signal::Panic(
            "assignment through a dereferenced reference isn't implemented yet \
             — needs the borrow checker's runtime reference-cell representation".into()
        )),
        _ => Err(Signal::Panic("invalid assignment target".into())),
    }
}

// ── Field access ──────────────────────────────────────────────────

fn get_field(obj: Value, field: &str) -> EvalResult {
    match obj {
        Value::Struct { ref fields, .. } => {
            fields.borrow().get(field)
                .cloned()
                .ok_or_else(|| Signal::Panic(format!(
                    "no field '{}' on {}", field, obj.type_name()
                )))
        }
        Value::Enum { ref type_name, ref variant, payload: _ } => {
            // Allow accessing discriminant metadata fields.
            match field {
                "type_name" => Ok(Value::str_from(type_name.as_str())),
                "variant"   => Ok(Value::str_from(variant.as_str())),
                _ => Err(Signal::Panic(format!("no field '{}' on enum", field))),
            }
        }
        other => Err(Signal::Panic(format!(
            "cannot access field '{}' on {}", field, other.type_name()
        ))),
    }
}

// ── Index access ──────────────────────────────────────────────────

fn eval_index(coll: Value, idx: Value) -> EvalResult {
    match (coll, idx) {
        (Value::List(rc), Value::Int(i)) => {
            let list = rc.borrow();
            let i = if i < 0 {
                // Negative indexing: -1 = last element.
                let len = list.len() as i64;
                (len + i) as usize
            } else {
                i as usize
            };
            list.get(i)
                .cloned()
                .ok_or_else(|| Signal::Panic(format!("list index {} out of bounds", i)))
        }
        (Value::Tuple(elems), Value::Int(i)) => {
            let i = i as usize;
            elems.get(i)
                .cloned()
                .ok_or_else(|| Signal::Panic(format!("tuple index {} out of bounds", i)))
        }
        (Value::Dict(rc), key) => {
            let dict = rc.borrow();
            dict.iter()
                .find(|(k, _)| k.equals(&key))
                .map(|(_, v)| v.clone())
                .ok_or_else(|| Signal::Panic("key not found in dictionary".into()))
        }
        (Value::Str(s), Value::Int(i)) => {
            let i = i as usize;
            s.chars().nth(i)
                .map(Value::Char)
                .ok_or_else(|| Signal::Panic(format!("string index {} out of bounds", i)))
        }
        (coll, idx) => Err(Signal::Panic(format!(
            "cannot index {} with {}", coll.type_name(), idx.type_name()
        ))),
    }
}

// ── Call dispatch helpers ─────────────────────────────────────────

/// Evaluate a call where the callee is `recv_expr.method_name(args)`.
///
/// Three cases in priority order:
///   1. `TypeName.method(args)` — static method (type name in method_table).
///   2. `obj.method(args)` — instance method on a struct.
///   3. `obj.method(args)` — built-in method on List, Str, Dict, Tuple.
fn eval_call_with_receiver<'ast>(
    interp:       &mut Interpreter<'ast>,
    recv_expr:    &'ast Expr<'ast>,
    method_name:  &str,
    raw_args:     &'ast [crate::ast::expressions::Arg<'ast>],
    callee_span:  crate::ast::common::Span,
) -> EvalResult {
    // Case 1: static call — receiver is a bare identifier naming a type.
    if let ExprKind::Ident(type_name) = &recv_expr.kind {
        // `Pool.new()` needs the interpreter's own ambient
        // `pool_capacity_stack` (MEMORY_MODEL.md §11) — it can't be a
        // normal `BuiltinFn` (`fn(&[Value]) -> EvalResult`), since that
        // signature has no way to reach interpreter state. Checked
        // before `is_builtin_namespace` for that reason, not folded
        // into `constructors.rs`/`resolve_namespace_member` like
        // `List.new()` etc.
        if *type_name == "Pool" && method_name == "new" {
            let cap = interp.pool_capacity_stack.last().copied().ok_or_else(|| {
                Signal::Panic(
                    "Pool.new() requires an enclosing with pool<T>(count) block".into(),
                )
            })?;
            return Ok(Value::new_pool(cap));
        }
        // ENUM_RULES.md — `Result.Ok(5)`, tuple-payload variant
        // construction. Sema has already validated arity/types by this
        // point, so this just evaluates the args and wraps them —
        // no re-checking here.
        // `Trait.method(value, ..)`: the explicit trait-qualified call.
        if interp.trait_names.contains(*type_name) {
            return eval_qualified_trait_call(interp, type_name, method_name, raw_args);
        }
        let canon = interp.canonical_type(type_name);
        if let Some(variants) = interp.enum_table.get(canon) {
            if variants.get(method_name) == Some(&crate::interpreter::eval::VariantKind::Tuple) {
                let enum_name = canon.to_string();
                let args = eval_args(interp, raw_args)?;
                return Ok(Value::Enum {
                    type_name: enum_name,
                    variant:   method_name.to_string(),
                    payload:   Box::new(crate::interpreter::value::EnumPayload::Tuple(args)),
                });
            }
        }
        if crate::builtins::is_builtin_namespace(type_name) {
            let func = crate::builtins::resolve_namespace_member(type_name, method_name)
                .ok_or_else(|| Signal::Panic(format!(
                    "'{}' has no member '{}'", type_name, method_name
                )))?;
            let args = eval_args(interp, raw_args)?;
            return func(&args);
        }
        if interp.method_table.contains_key(canon) {
            let fn_id = interp.method_table
                .get(canon)
                .and_then(|m| m.get(method_name))
                .copied()
                .ok_or_else(|| Signal::Panic(format!(
                    "no static method '{}' on '{}'", method_name, canon
                )))?;
            let args = eval_args(interp, raw_args)?;
            return interp.call_function(fn_id, &args);
        }
    }

    // Case 2 & 3: evaluate the receiver, then dispatch.
    let receiver = eval_expr(interp, recv_expr)?;
    let args     = eval_args(interp, raw_args)?;

    // A call sema resolved THROUGH A TRAIT (a bound, or `Self` in a default
    // method) runs the trait's method for the receiver's type, never an
    // inherent method of the same name that only the concrete type sees.
    if let Some(trait_name) = interp.trait_call_sites.get(&callee_span) {
        let type_name = match &receiver {
            Value::Struct { type_name, .. } | Value::Enum { type_name, .. } => Some(type_name.clone()),
            _ => None,
        };
        if let Some(type_name) = type_name {
            let fn_id = interp.trait_method_table
                .get(&(type_name, trait_name.clone()))
                .and_then(|m| m.get(method_name))
                .copied();
            if let Some(fn_id) = fn_id {
                return interp.call_method(fn_id, receiver, &args);
            }
        }
        // A prelude trait has no impl block to find: what a built-in or
        // derived type does for `eq`, `cmp`, `clone` and the rest is native.
        if is_prelude_trait(trait_name) {
            return native_prelude_method(receiver, method_name, &args);
        }
    }
    eval_method_call(interp, receiver, method_name, &args)
}

/// The traits declared in the prelude (`ubel_stratum_rd::prelude`).
fn is_prelude_trait(name: &str) -> bool {
    matches!(name, "PartialEq" | "Eq" | "PartialOrd" | "Ord" | "Clone" | "Hash")
}

/// Strip the ownership wrappers (`Unique`, `Shared`, `SyncShared`) so a
/// value is compared, ordered and cloned as the value it holds.
fn unwrap_ownership(value: Value) -> Value {
    match value {
        Value::Unique(inner)                      => unwrap_ownership((*inner).clone()),
        Value::Shared(rc) | Value::SyncShared(rc) => unwrap_ownership(rc.borrow().clone()),
        other                                     => other,
    }
}

/// `Less`, `Equal` or `Greater`, the prelude enum.
fn ordering_value(variant: &str) -> Value {
    Value::Enum {
        type_name: "Ordering".to_string(),
        variant:   variant.to_string(),
        payload:   Box::new(crate::interpreter::value::EnumPayload::None),
    }
}

/// The order of two values, through the same `==` and `<` the operators use
/// (so every numeric width, string and derived struct behaves as it does
/// there). `None` when they are unordered (a NaN).
fn order_of(a: &Value, b: &Value) -> Result<Option<std::cmp::Ordering>, Signal> {
    let truth = |op: BinOp| -> Result<bool, Signal> {
        Ok(matches!(eval_binop(op, a.clone(), b.clone())?, Value::Bool(true)))
    };
    if truth(BinOp::Lt)? { return Ok(Some(std::cmp::Ordering::Less)); }
    if truth(BinOp::Gt)? { return Ok(Some(std::cmp::Ordering::Greater)); }
    if truth(BinOp::Eq)? { return Ok(Some(std::cmp::Ordering::Equal)); }
    Ok(None)
}

/// `value.hash(state)`: mix the value's hash into the prelude `Hasher`'s
/// `state` field. Order matters (the old state is rotated before the new
/// hash is folded in), so hashing `a` then `b` differs from `b` then `a`,
/// and the same values in the same order always give the same `finish()`.
fn native_hash_into(value: &Value, args: &[Value]) -> EvalResult {
    let Some(Value::Struct { fields, .. }) = args.first() else {
        return Err(Signal::Panic("'hash' needs a Hasher argument".into()));
    };
    let mut fields = fields.borrow_mut();
    let old = match fields.get("state") {
        Some(Value::UInt(s)) => *s,
        _ => return Err(Signal::Panic("'hash' needs a Hasher argument".into())),
    };
    let next = (old.rotate_left(5) ^ value.compute_hash()).wrapping_mul(0x517c_c1b7_2722_0a95);
    fields.insert("state".to_string(), Value::UInt(next));
    Ok(Value::Void)
}

/// The methods of the prelude traits on a built-in or derived value:
/// `eq`, `ne`, `lt`, `le`, `gt`, `ge`, `partial_cmp`, `cmp`, `clone`.
fn native_prelude_method(receiver: Value, method: &str, args: &[Value]) -> EvalResult {
    let receiver = unwrap_ownership(receiver);
    if method == "clone" {
        return Ok(receiver.deep_clone());
    }
    if method == "hash" {
        return native_hash_into(&receiver, args);
    }
    let other = match args.first() {
        Some(v) => unwrap_ownership(v.clone()),
        None => return Err(Signal::Panic(format!("'{}' needs one argument", method))),
    };
    let as_bool = |v: Value| matches!(v, Value::Bool(true));
    match method {
        "eq" => eval_binop(BinOp::Eq, receiver, other),
        "ne" => Ok(Value::Bool(!as_bool(eval_binop(BinOp::Eq, receiver, other)?))),
        "lt" => eval_binop(BinOp::Lt, receiver, other),
        "le" => eval_binop(BinOp::Le, receiver, other),
        "gt" => eval_binop(BinOp::Gt, receiver, other),
        "ge" => eval_binop(BinOp::Ge, receiver, other),
        "partial_cmp" | "cmp" => {
            use std::cmp::Ordering;
            match order_of(&receiver, &other)? {
                Some(Ordering::Less)    => Ok(ordering_value("Less")),
                Some(Ordering::Equal)   => Ok(ordering_value("Equal")),
                Some(Ordering::Greater) => Ok(ordering_value("Greater")),
                None if method == "partial_cmp" => Ok(Value::Null),
                None => Err(Signal::Panic("cmp on values that are not ordered".into())),
            }
        }
        _ => Err(Signal::Panic(format!("no prelude method '{}'", method))),
    }
}

/// Dispatch a method call given an already-evaluated receiver.
fn eval_method_call(
    interp:      &mut Interpreter<'_>,
    receiver:    Value,
    method_name: &str,
    args:        &[Value],
) -> EvalResult {
    // Peel off at most one ownership-model wrapper before dispatching,
    // mirroring `resolve_receiver` (`builtins/instance.rs`) on the sema
    // side. Cloning the inner `Value` out is O(1) for every collection/
    // struct variant here, since each already keeps its own mutable
    // state behind its own `Rc<RefCell<...>>`; the clone just shares
    // that same inner `Rc`, so a mutating method (`.push()` etc.) still
    // mutates the one real storage location, whether reached through
    // `Unique`, `Shared`, `SyncShared`, or no wrapper at all.
    match &receiver {
        Value::Unique(inner) => {
            let inner_val = (**inner).clone();
            return eval_method_call(interp, inner_val, method_name, args);
        }
        Value::Shared(rc) | Value::SyncShared(rc) => {
            let inner_val = rc.borrow().clone();
            return eval_method_call(interp, inner_val, method_name, args);
        }
        _ => {}
    }

    match &receiver {
        // ── Built-in List methods ──────────────────────────────────
        Value::List(rc) => {
            use crate::builtins::instance::list_methods as m;
            match method_name {
                "len"      => return Ok(m::len(rc)),
                "push"     => return m::push(rc, args),
                "pop"      => return Ok(m::pop(rc)),
                "contains" => return m::contains(rc, args),
                "first"    => return Ok(m::first(rc)),
                "last"     => return Ok(m::last(rc)),
                "is_empty" => return Ok(m::is_empty(rc)),
                "reverse"  => return Ok(m::reverse(rc)),
                "get"      => return m::get(rc, args),
                "set"      => return m::set(rc, args),
                "find"     => return m::find(interp, rc, args),
                "find_all" => return m::find_all(interp, rc, args),
                "query"    => return Ok(m::query(rc)),
                _ => {}
            }
        }

        // ── Built-in Queue methods ──────────────────────────────────
        Value::Queue(rc) => {
            use crate::builtins::instance::queue_methods as m;
            match method_name {
                "len"      => return Ok(m::len(rc)),
                "is_empty" => return Ok(m::is_empty(rc)),
                "enqueue"  => return m::enqueue(rc, args),
                "dequeue"  => return Ok(m::dequeue(rc)),
                "peek"     => return Ok(m::peek(rc)),
                "contains" => return m::contains(rc, args),
                "clear"    => return Ok(m::clear(rc)),
                _ => {}
            }
        }

        // ── Built-in Stack methods ──────────────────────────────────
        Value::Stack(rc) => {
            use crate::builtins::instance::stack_methods as m;
            match method_name {
                "len"      => return Ok(m::len(rc)),
                "is_empty" => return Ok(m::is_empty(rc)),
                "push"     => return m::push(rc, args),
                "pop"      => return Ok(m::pop(rc)),
                "peek"     => return Ok(m::peek(rc)),
                "contains" => return m::contains(rc, args),
                "clear"    => return Ok(m::clear(rc)),
                _ => {}
            }
        }

        // ── Built-in Pool methods (MEMORY_MODEL.md §11, DATASTRUCTURES.md §1) ──
        Value::Pool(rc) => {
            use crate::builtins::instance::pool_methods as m;
            match method_name {
                "acquire"  => return m::acquire(rc, args),
                "release"  => return m::release(rc, args),
                "get"      => return m::get(rc, args),
                "growable" => return m::growable(rc, args),
                "fifo"     => return m::fifo(rc, args),
                _ => {}
            }
        }

        // ── Built-in InlineList methods (DATASTRUCTURES.md §5) ──────
        Value::InlineList(rc) => {
            use crate::builtins::instance::inline_list_methods as m;
            match method_name {
                "len"      => return Ok(m::len(rc)),
                "push"     => return m::push(rc, args),
                "pop"      => return Ok(m::pop(rc)),
                "contains" => return m::contains(rc, args),
                "first"    => return Ok(m::first(rc)),
                "last"     => return Ok(m::last(rc)),
                "is_empty" => return Ok(m::is_empty(rc)),
                "reverse"  => return Ok(m::reverse(rc)),
                "capacity" => return Ok(m::capacity(rc)),
                _ => {}
            }
        }

        // ── Built-in Linqerizer methods ────────────────────────────
        // Chainable (`where`/`select`/`order_by`/`order_by_desc`) never
        // touch `interp` — appending an op doesn't run anything.
        // Terminal (`to_list`/`first`/`count`/`group_by`) all do —
        // that's the one place any actual interpretation happens.
        Value::Linqerizer(pipeline) => {
            use crate::builtins::instance::linqerizer_methods as m;
            match method_name {
                "where"         => return m::where_(pipeline, args),
                "select"        => return m::select(pipeline, args),
                "order_by"      => return m::order_by(pipeline, args),
                "order_by_desc" => return m::order_by_desc(pipeline, args),
                "to_list"       => return m::to_list(interp, pipeline),
                "first"         => return m::first(interp, pipeline),
                "count"         => return m::count(interp, pipeline),
                "group_by"      => return m::group_by(interp, pipeline, args),
                _ => {}
            }
        }

        // ── Built-in String methods ────────────────────────────────
        Value::Str(s) => {
            let s = s.clone(); // Rc clone so we don't hold borrow across returns
            match method_name {
                "len"        => return Ok(Value::Int(s.len() as i64)),
                "is_empty"   => return Ok(Value::Bool(s.is_empty())),
                "to_upper"   => return Ok(Value::str_from(s.to_uppercase())),
                "to_lower"   => return Ok(Value::str_from(s.to_lowercase())),
                "trim"       => return Ok(Value::str_from(s.trim())),
                "trim_start" => return Ok(Value::str_from(s.trim_start())),
                "trim_end"   => return Ok(Value::str_from(s.trim_end())),
                "chars"      => {
                    let chars: Vec<Value> = s.chars().map(Value::Char).collect();
                    return Ok(Value::List(Rc::new(RefCell::new(chars))));
                }
                "contains" => {
                    let sub = args.first().ok_or_else(|| Signal::Panic("contains() needs 1 arg".into()))?;
                    if let Value::Str(sub_str) = sub {
                        return Ok(Value::Bool(s.contains(sub_str.as_str())));
                    }
                    return Ok(Value::Bool(false));
                }
                "starts_with" => {
                    let sub = args.first().ok_or_else(|| Signal::Panic("starts_with() needs 1 arg".into()))?;
                    if let Value::Str(sub_str) = sub {
                        return Ok(Value::Bool(s.starts_with(sub_str.as_str())));
                    }
                    return Ok(Value::Bool(false));
                }
                "ends_with" => {
                    let sub = args.first().ok_or_else(|| Signal::Panic("ends_with() needs 1 arg".into()))?;
                    if let Value::Str(sub_str) = sub {
                        return Ok(Value::Bool(s.ends_with(sub_str.as_str())));
                    }
                    return Ok(Value::Bool(false));
                }
                "split" => {
                    let delim = args.first().ok_or_else(|| Signal::Panic("split() needs 1 arg".into()))?;
                    if let Value::Str(d) = delim {
                        let parts: Vec<Value> = s.split(d.as_str())
                            .map(Value::str_from)
                            .collect();
                        return Ok(Value::List(Rc::new(RefCell::new(parts))));
                    }
                    return Err(Signal::Panic("split() delimiter must be a string".into()));
                }
                "replace" => {
                    if args.len() < 2 { return Err(Signal::Panic("replace() needs 2 arguments".into())); }
                    if let (Value::Str(from), Value::Str(to)) = (&args[0], &args[1]) {
                        return Ok(Value::str_from(s.replace(from.as_str(), to.as_str())));
                    }
                    return Err(Signal::Panic("replace() arguments must be strings".into()));
                }
                _ => {}
            }
        }

        // ── Built-in Dict methods ──────────────────────────────────
        Value::Dict(rc) => {
            use crate::builtins::instance::dict_methods as m;
            match method_name {
                "len"          => return Ok(m::len(rc)),
                "is_empty"     => return Ok(m::is_empty(rc)),
                "contains_key" => return m::contains_key(rc, args),
                "keys"         => return Ok(m::keys(rc)),
                "values"       => return Ok(m::values(rc)),
                "set"          => return m::set(rc, args),
                "get"          => return m::get(rc, args),
                _ => {}
            }
        }

        // ── Built-in Tuple methods ─────────────────────────────────
        Value::Tuple(elems) => match method_name {
            "len" => return Ok(Value::Int(elems.len() as i64)),
            _ => {}
        },

        _ => {}
    }

    // User-defined instance method on a struct.
    //
    // An `enum` value dispatches through the same `method_table` as a
    // struct (`extend Color { fn is_red(self) .. }`); it has no derives, so
    // no `.clone()` pseudo-method.
    let (type_name, derives_clone) = match &receiver {
        Value::Struct { type_name, derives_clone, .. } => (type_name.clone(), *derives_clone),
        Value::Enum   { type_name, .. }                => (type_name.clone(), false),
        other => return Err(Signal::Panic(format!(
            "no method '{}' on {}", method_name, other.type_name()
        ))),
    };
    if let Some(fn_id) = interp.method_table
        .get(&type_name)
        .and_then(|m| m.get(method_name))
        .copied()
    {
        return interp.call_method(fn_id, receiver, args);
    }
    // `c.cb(4)` where `cb` is a struct FIELD holding a function. A real
    // method of the same name was already tried above, the same order sema
    // uses.
    if let Value::Struct { fields, .. } = &receiver {
        let field_val = fields.borrow().get(method_name).cloned();
        if let Some(Value::Function(id)) = field_val {
            return interp.call_function(id, args);
        }
    }
    // `.clone()` is a derive-gated pseudo-method (`Value::deep_clone`),
    // not a real `method_table` entry, checked only once no
    // user-defined method by that name exists, so an explicit `fn
    // clone(&self)` a person writes themselves still wins (mirrors
    // sema's own resolution order, `type_infer.rs`'s struct-instance-
    // method arm, and for the same reason: explicit beats implicit).
    if method_name == "clone" && derives_clone {
        return Ok(receiver.deep_clone());
    }
    Err(Signal::Panic(format!(
        "no method '{}' on '{}'", method_name, type_name
    )))
}

// ── Evaluate argument list ────────────────────────────────────────

/// `Trait.method(value, args..)`: the first argument is the receiver, and
/// the method is looked up in the per-trait table for the receiver's type,
/// so it picks the right function even when two traits supply the same name.
fn eval_qualified_trait_call<'ast>(
    interp:     &mut Interpreter<'ast>,
    trait_name: &str,
    method:     &str,
    raw_args:   &'ast [crate::ast::expressions::Arg<'ast>],
) -> EvalResult {
    let mut values = eval_args(interp, raw_args)?;
    if values.is_empty() {
        return Err(Signal::Panic(format!(
            "{}.{}(..) needs a receiver as its first argument", trait_name, method
        )));
    }
    let receiver = values.remove(0);
    // A prelude trait has no impl block to find (see `native_prelude_method`).
    if is_prelude_trait(trait_name) {
        let has_table_entry = match &receiver {
            Value::Struct { type_name, .. } | Value::Enum { type_name, .. } => interp.trait_method_table
                .get(&(type_name.clone(), trait_name.to_string()))
                .is_some_and(|m| m.contains_key(method)),
            _ => false,
        };
        if !has_table_entry {
            return native_prelude_method(receiver, method, &values);
        }
    }
    let type_name = match &receiver {
        Value::Struct { type_name, .. } | Value::Enum { type_name, .. } => type_name.clone(),
        other => return Err(Signal::Panic(format!(
            "{}.{}(..) on {}, which implements no trait", trait_name, method, other.type_name()
        ))),
    };
    let fn_id = interp.trait_method_table
        .get(&(type_name.clone(), trait_name.to_string()))
        .and_then(|m| m.get(method))
        .copied()
        .ok_or_else(|| Signal::Panic(format!(
            "'{}' does not implement trait '{}' (or the trait has no method '{}')",
            type_name, trait_name, method
        )))?;
    interp.call_method(fn_id, receiver, &values)
}

fn eval_args<'ast>(
    interp: &mut Interpreter<'ast>,
    args:   &'ast [crate::ast::expressions::Arg<'ast>],
) -> Result<Vec<Value>, Signal> {
    args.iter()
        .map(|a| match &a.kind {
            ArgKind::Positional(e)       => eval_expr(interp, e),
            ArgKind::Named { value, .. } => eval_expr(interp, value),
        })
        .collect()
}

// ── Type cast ─────────────────────────────────────────────────────

/// One arm of `eval_cast`'s numeric targets: cast `val` to `$ty`,
/// wrapped in `Value::$Variant`. Every source variant gets its *own*
/// direct `as $ty` from Rust — an integer source truncates/extends per
/// Rust's normal two's-complement `as` rules, and (critically) a float
/// source saturates per Rust's own `as`-from-float rules (stable since
/// 1.45) rather than being routed through a shared wide intermediate
/// first, which would have silently turned that saturation into a
/// wraparound instead (`300.5 as u8` must land on `255`, not `44`).
macro_rules! int_cast_arm {
    ($val:expr, $Variant:ident, $ty:ty, $name:literal) => {
        match $val {
            Value::Int(n)    => Ok(Value::$Variant(n as $ty)),
            Value::I8(n)     => Ok(Value::$Variant(n as $ty)),
            Value::I16(n)    => Ok(Value::$Variant(n as $ty)),
            Value::I32(n)    => Ok(Value::$Variant(n as $ty)),
            Value::U8(n)     => Ok(Value::$Variant(n as $ty)),
            Value::U16(n)    => Ok(Value::$Variant(n as $ty)),
            Value::U32(n)    => Ok(Value::$Variant(n as $ty)),
            Value::UInt(n)   => Ok(Value::$Variant(n as $ty)),
            Value::Float(f)  => Ok(Value::$Variant(f as $ty)),
            Value::Double(d) => Ok(Value::$Variant(d as $ty)),
            Value::Bool(b)   => Ok(Value::$Variant(if b { 1 as $ty } else { 0 as $ty })),
            Value::Str(ref s) => s.parse::<$ty>()
                .map(Value::$Variant)
                .map_err(|_| Signal::Panic(format!("cannot cast '{}' to {}", s, $name))),
            ref other => Err(Signal::Panic(format!("cannot cast {} to {}", other.type_name(), $name))),
        }
    };
}

fn eval_cast<'ast>(val: Value, ty: &'ast Type<'ast>) -> EvalResult {
    match ty.kind {
        // 64-bit signed family: `int`/`long`/`i64`/`isize` all share the
        // same `Value::Int(i64)` runtime width (see `Value`'s own doc
        // comment on why), so casting between any of them and an
        // already-integer source is exactly the no-op it always was.
        // `short` widens to `SemaType::Int` at the sema level
        // (`ast_type_to_sema`) and now gets the matching runtime
        // treatment here — previously fell all the way through to the
        // unconditional pass-through below, a real pre-existing gap
        // (`5.9 as short` silently stayed a `Double`) this closes too.
        TypeKind::Int | TypeKind::Long | TypeKind::I64 | TypeKind::Isize | TypeKind::Short =>
            int_cast_arm!(val, Int, i64, "int"),

        // 32-bit unsigned family: `uint`/`u32`/`ushort` (the last via
        // the same sema-level widening `short` gets above). Real 32-bit
        // wraparound now, where this and `Uint`/`Ushort` both
        // previously either had no truncation (`I32`, wrongly grouped
        // with `Int` before this) or no handling at all (`Uint`/
        // `Ushort`, falling to the pass-through).
        TypeKind::Uint | TypeKind::U32 | TypeKind::Ushort =>
            int_cast_arm!(val, U32, u32, "u32"),

        // 64-bit unsigned family: `ulong`/`u64`/`usize`. The one width
        // that genuinely cannot be represented by `Value::Int` at all
        // above `i64::MAX` — see `Value::UInt`'s own doc comment.
        TypeKind::Ulong | TypeKind::U64 | TypeKind::Usize =>
            int_cast_arm!(val, UInt, u64, "u64"),

        // `byte` intentionally stays signed (`I8`) per this session's
        // own decision to leave it as-is rather than flip it to match
        // C#'s unsigned convention; `i8` shares the same runtime type.
        TypeKind::Byte | TypeKind::I8 => int_cast_arm!(val, I8, i8, "i8"),
        TypeKind::Ubyte | TypeKind::U8 => int_cast_arm!(val, U8, u8, "u8"),
        TypeKind::I16 => int_cast_arm!(val, I16, i16, "i16"),
        TypeKind::I32 => int_cast_arm!(val, I32, i32, "i32"),
        TypeKind::U16 => int_cast_arm!(val, U16, u16, "u16"),

        TypeKind::Float | TypeKind::F32 => match val {
            Value::Float(f)  => Ok(Value::Float(f)),
            Value::Double(d) => Ok(Value::Float(d as f32)),
            Value::Int(n)    => Ok(Value::Float(n as f32)),
            Value::I8(n)     => Ok(Value::Float(n as f32)),
            Value::I16(n)    => Ok(Value::Float(n as f32)),
            Value::I32(n)    => Ok(Value::Float(n as f32)),
            Value::U8(n)     => Ok(Value::Float(n as f32)),
            Value::U16(n)    => Ok(Value::Float(n as f32)),
            Value::U32(n)    => Ok(Value::Float(n as f32)),
            Value::UInt(n)   => Ok(Value::Float(n as f32)),
            other => Err(Signal::Panic(format!("cannot cast {} to float", other.type_name()))),
        },
        TypeKind::Double | TypeKind::F64 => match val {
            Value::Double(d) => Ok(Value::Double(d)),
            Value::Float(f)  => Ok(Value::Double(f as f64)),
            Value::Int(n)    => Ok(Value::Double(n as f64)),
            Value::I8(n)     => Ok(Value::Double(n as f64)),
            Value::I16(n)    => Ok(Value::Double(n as f64)),
            Value::I32(n)    => Ok(Value::Double(n as f64)),
            Value::U8(n)     => Ok(Value::Double(n as f64)),
            Value::U16(n)    => Ok(Value::Double(n as f64)),
            Value::U32(n)    => Ok(Value::Double(n as f64)),
            Value::UInt(n)   => Ok(Value::Double(n as f64)),
            other => Err(Signal::Panic(format!("cannot cast {} to double", other.type_name()))),
        },
        TypeKind::Str => Ok(Value::str_from(val.to_string())),
        TypeKind::Bool => {
            let b = val.is_truthy()?;
            Ok(Value::Bool(b))
        }
        _ => Ok(val), // Unknown cast — pass through for now.
    }
}
