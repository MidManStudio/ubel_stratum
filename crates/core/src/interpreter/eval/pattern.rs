// src/interpreter/eval/pattern.rs
//! Pattern matching for match arms and destructuring.

#![allow(dead_code)]

use std::collections::HashMap;

use crate::ast::literals::{IntSuffix, Literal};
use crate::ast::patterns::{
    DestructureElement, DestructurePattern, EnumPatternPayload,
    FieldPattern, Pattern, PatternKind,
};
use crate::interpreter::env::Environment;
use crate::interpreter::eval::VariantKind;
use crate::interpreter::value::{EnumPayload, Value};

/// enum_type_name → variant_name → payload kind — the same table
/// `Interpreter::enum_table` holds, threaded through so `PatternKind::Ident`
/// and `PatternKind::Struct` can tell "this name is definitely one of this
/// enum's variants, just not the one we have" (no match, try the next arm)
/// apart from "this name isn't any variant at all" (genuine catch-all
/// binding) — see those two arms' own comments in `match_inner`.
type EnumTable = HashMap<String, HashMap<String, VariantKind>>;

/// `type` alias name -> the type it stands for (`Interpreter::type_aliases`),
/// so a struct pattern written with an alias (`P { x, y }`) matches a value
/// of the real struct, whose `type_name` is always the real name.
type AliasTable = HashMap<String, String>;

/// Everything a pattern needs to resolve a NAME: the enum table and the
/// alias table, bundled so the recursive matchers take one extra parameter
/// rather than two.
#[derive(Clone, Copy)]
pub struct PatternTables<'a> {
    pub enums:   &'a EnumTable,
    pub aliases: &'a AliasTable,
}

/// The real type name behind `name`, following aliases (hop-limited; sema
/// rejects an alias cycle, this only guards sema-less unit tests).
fn canonical_type<'a>(aliases: &'a AliasTable, name: &'a str) -> &'a str {
    let mut cur = name;
    for _ in 0..64 {
        match aliases.get(cur) {
            Some(next) => cur = next.as_str(),
            None       => break,
        }
    }
    cur
}

// ── Public API ────────────────────────────────────────────────────

/// Try to match `value` against `pattern`, defining any bound names into `env`
/// on success. Returns `true` if the pattern matched.
///
/// Bindings are committed atomically — either all succeed (pattern matched,
/// all names defined) or none are (pattern failed, env is unchanged).
/// This invariant is maintained by the internal `try_match` helper.
pub fn match_pattern(
    pattern:    &Pattern<'_>,
    value:      &Value,
    env:        &mut Environment,
    enum_table: PatternTables<'_>,
) -> bool {
    match try_match(pattern, value, enum_table) {
        Some(bindings) => {
            for (name, val) in bindings { env.define(&name, val); }
            true
        }
        None => false,
    }
}

/// Bind a destructuring pattern (from `extract` or destructuring `let`) into env.
/// Unlike `match_pattern`, this always binds — failure is a runtime panic since
/// the type-checker should have caught arity mismatches.
pub fn bind_destructure_pattern(
    pattern: &DestructurePattern<'_>,
    value:   Value,
    env:     &mut Environment,
) {
    match pattern {
        DestructurePattern::Ident(name) => {
            env.define(name, value);
        }
        DestructurePattern::Tuple(t) => {
            let items = match value {
                Value::Tuple(v) => v,
                Value::List(rc) => rc.borrow().clone(),
                other => {
                    env.define("_", other); // graceful fallback
                    return;
                }
            };
            for (elem, val) in t.elements.iter().zip(items.into_iter()) {
                bind_destructure_elem(elem, val, env);
            }
        }
        DestructurePattern::Array(a) => {
            let items = match value {
                Value::List(rc) => rc.borrow().clone(),
                Value::Tuple(v) => v,
                other => {
                    env.define("_", other);
                    return;
                }
            };
            let n = a.elements.len().min(items.len());
            for (elem, val) in a.elements[..n].iter().zip(items[..n].iter()) {
                bind_destructure_elem(elem, val.clone(), env);
            }
            // Bind rest if present.
            if let Some(rest_name) = a.rest.flatten() {
                let rest_items = items[n..].to_vec();
                env.define(rest_name, Value::List(
                    std::rc::Rc::new(std::cell::RefCell::new(rest_items))
                ));
            }
        }
        DestructurePattern::Struct(s) => {
            let fields = match value {
                Value::Struct { fields, .. } => fields.borrow().clone(),
                _ => return,
            };
            for fd in s.fields.iter() {
                let field_val = fields.get(fd.field).cloned().unwrap_or(Value::Null);
                if let Some(sub_pat) = &fd.pattern {
                    bind_destructure_pattern(sub_pat, field_val, env);
                } else {
                    env.define(fd.field, field_val);
                }
            }
        }
    }
}

// ── Internal helpers ──────────────────────────────────────────────

/// Try to match `pattern` against `value`. Returns `Some(bindings)` on success
/// where `bindings` is the ordered list of `(name, value)` pairs to define,
/// or `None` if the pattern didn't match.
///
/// Collecting bindings before committing them means OR patterns can try each
/// alternative cleanly without partially polluting the environment.
fn try_match(
    pattern:    &Pattern<'_>,
    value:      &Value,
    enum_table: PatternTables<'_>,
) -> Option<Vec<(String, Value)>> {
    let mut bindings = Vec::new();
    if match_inner(pattern, value, &mut bindings, enum_table) {
        Some(bindings)
    } else {
        None
    }
}

/// Recursive matching core. Accumulates name bindings into `out`.
/// Returns `true` if the pattern matched.
fn match_inner(
    pattern:    &Pattern<'_>,
    value:      &Value,
    out:        &mut Vec<(String, Value)>,
    enum_table: PatternTables<'_>,
) -> bool {
    match &pattern.kind {
        // ── Wildcard `_` — always matches, binds nothing ──────────
        PatternKind::Wildcard => true,

        // ── Literal — value must equal the literal ────────────────
        PatternKind::Literal(lit) => match_literal(lit, value),

        // ── Binding: `x` or `mut x` — always matches, binds name ─
        // ENUM_RULES.md fix: EXCEPT when `value` is itself an enum and
        // `name` names one of THAT SAME ENUM TYPE's variants — then this
        // is a bare unqualified variant pattern (`North => ...`, not
        // `Direction.North => ...`), which the parser can't tell apart
        // from a fresh binding at parse time (no type info yet). Sema
        // resolves this ambiguity statically via the scrutinee's known
        // type (`InferCtx::check_pattern`'s Ident arm); this mirrors the
        // same call dynamically, off the concrete runtime value instead.
        //
        // Consulting `enum_table` (not just comparing `name` against
        // THIS value's own `variant`) matters: without it, an arm whose
        // pattern name happens not to equal the CURRENT value's variant
        // would fall through to "always matches, binds name" — exactly
        // the original bug, just one comparison later. `North => ...`
        // tried against a `South` value must FAIL and let the next arm
        // try, not silently bind `North` to the whole South value.
        PatternKind::Ident { name, .. } => {
            if let Value::Enum { type_name, variant, payload } = value {
                if matches!(payload.as_ref(), EnumPayload::None) {
                    if let Some(kind) = enum_table.enums.get(type_name.as_str()).and_then(|v| v.get(*name)) {
                        return *kind == VariantKind::Fieldless && variant == name;
                    }
                }
            }
            out.push((name.to_string(), value.clone()));
            true
        }

        // ── Tuple: (a, b, c) ──────────────────────────────────────
        PatternKind::Tuple(pats) => {
            let items = match value {
                Value::Tuple(v) => v.as_slice(),
                // Allow matching a list as a tuple pattern (flexible MVP behaviour).
                Value::List(rc) => {
                    // Can't easily return a borrow here; clone.
                    let cloned: Vec<Value> = rc.borrow().clone();
                    return match_tuple_slice(pats, &cloned, out, enum_table);
                }
                _ => return false,
            };
            match_tuple_slice(pats, items, out, enum_table)
        }

        // ── Array: [a, b, ...rest] ────────────────────────────────
        PatternKind::Array { elements, rest } => {
            let items: Vec<Value> = match value {
                Value::List(rc)  => rc.borrow().clone(),
                Value::Tuple(v)  => v.clone(),
                _ => return false,
            };
            // Without a rest pattern, length must match exactly.
            if rest.is_none() && items.len() != elements.len() {
                return false;
            }
            // With a rest pattern, we need at least as many items as fixed elements.
            if rest.is_some() && items.len() < elements.len() {
                return false;
            }
            let mut trial = Vec::new();
            for (pat, item) in elements.iter().zip(items.iter()) {
                if !match_inner(pat, item, &mut trial, enum_table) { return false; }
            }
            // Bind the rest if named.
            if let Some(Some(rest_name)) = rest {
                let rest_items = items[elements.len()..].to_vec();
                trial.push((rest_name.to_string(), Value::List(
                    std::rc::Rc::new(std::cell::RefCell::new(rest_items))
                )));
            }
            out.extend(trial);
            true
        }

        // ── Struct: Point { x, y } ────────────────────────────────
        // ENUM_RULES.md fix: also matches Value::Enum when
        // `expected_name` names one of its variants — `Move { x, y }`
        // parses as this same PatternKind::Struct (not
        // PatternKind::Enum) whenever it's written with a 1-segment
        // name, since the parser can't tell a struct-payload enum
        // variant apart from a plain struct pattern without type info
        // (`parse_pattern.rs` only treats 2+-segment names like
        // `Result.Err { code }` as unambiguously enum). Sema resolves
        // this the same way, statically, in `InferCtx::check_pattern`'s
        // mirroring `PatternKind::Struct { name: Some(n), .. }` arm.
        PatternKind::Struct { name: expected_name, fields } => {
            if let Value::Enum { type_name: _, variant, payload } = value {
                if let Some(n) = expected_name {
                    if n == variant {
                        if let EnumPayload::Struct(field_map) = payload.as_ref() {
                            let mut trial = Vec::new();
                            for fp in fields.iter() {
                                let field_val = field_map.get(fp.field).cloned().unwrap_or(Value::Null);
                                if let Some(sub_pat) = &fp.pattern {
                                    if !match_inner(sub_pat, &field_val, &mut trial, enum_table) { return false; }
                                } else {
                                    trial.push((fp.field.to_string(), field_val));
                                }
                            }
                            out.extend(trial);
                            return true;
                        }
                    }
                }
                return false;
            }
            let (type_name, field_map) = match value {
                Value::Struct { type_name, fields, .. } => {
                    (type_name.as_str(), fields.borrow().clone())
                }
                _ => return false,
            };
            // If the pattern names a type, check it matches.
            if let Some(n) = expected_name {
                if canonical_type(enum_table.aliases, n) != type_name && *n != "<anon>" { return false; }
            }
            let mut trial = Vec::new();
            for fp in fields.iter() {
                let field_val = field_map.get(fp.field).cloned().unwrap_or(Value::Null);
                if let Some(sub_pat) = &fp.pattern {
                    if !match_inner(sub_pat, &field_val, &mut trial, enum_table) { return false; }
                } else {
                    // Shorthand `{ name }` — bind the field name directly.
                    trial.push((fp.field.to_string(), field_val));
                }
            }
            out.extend(trial);
            true
        }

        // ── Enum: Status.Active or Ok(x) or Err { code, msg } ────
        PatternKind::Enum { path, payload } => {
            // Extract expected type and variant from the path.
            // `path = ["Status", "Active"]` → type = "Status", variant = "Active"
            // `path = ["Ok"]`               → variant = "Ok" (any type)
            let (expected_type, expected_variant) = if path.len() >= 2 {
                (Some(path[path.len() - 2]), path[path.len() - 1])
            } else {
                (None, path[0])
            };

            match value {
                Value::Enum { type_name, variant, payload: val_payload } => {
                    // Check type name if provided.
                    if let Some(et) = expected_type {
                        if canonical_type(enum_table.aliases, et) != type_name.as_str() { return false; }
                    }
                    if expected_variant != variant.as_str() { return false; }

                    // Match payload.
                    let mut trial = Vec::new();
                    let matched = match (payload, val_payload.as_ref()) {
                        (EnumPatternPayload::None, EnumPayload::None) => true,
                        (EnumPatternPayload::Tuple(pats), EnumPayload::Tuple(vals)) => {
                            if pats.len() != vals.len() { return false; }
                            pats.iter().zip(vals.iter()).all(|(p, v)| {
                                match_inner(p, v, &mut trial, enum_table)
                            })
                        }
                        (EnumPatternPayload::Struct(fps), EnumPayload::Struct(fields)) => {
                            match_struct_payload(fps, fields, &mut trial, enum_table)
                        }
                        // Tolerant: pattern expects None but value has payload — no match.
                        _ => false,
                    };
                    if matched {
                        out.extend(trial);
                        true
                    } else {
                        false
                    }
                }
                // Allow matching integers against discriminant-valued enums
                // when the expected variant is a bare name (e.g. 0 matches Active = 0).
                _ => false,
            }
        }

        // ── Range: lo..hi (exclusive) or lo..=hi (inclusive) ─────
        PatternKind::Range { lo, hi, inclusive } => {
            match value {
                Value::Int(n) => {
                    let lo_val = match literal_to_i64(lo) { Some(v) => v, None => return false };
                    let hi_val = match literal_to_i64(hi) { Some(v) => v, None => return false };
                    if *inclusive {
                        *n >= lo_val && *n <= hi_val
                    } else {
                        *n >= lo_val && *n < hi_val
                    }
                }
                Value::Char(c) => {
                    let lo_char = match literal_to_char(lo) { Some(v) => v, None => return false };
                    let hi_char = match literal_to_char(hi) { Some(v) => v, None => return false };
                    if *inclusive {
                        *c >= lo_char && *c <= hi_char
                    } else {
                        *c >= lo_char && *c < hi_char
                    }
                }
                _ => false,
            }
        }

        // ── OR pattern: A | B | C ─────────────────────────────────
        // Try each alternative; commit bindings from the first that matches.
        PatternKind::Or(pats) => {
            for pat in pats.iter() {
                let mut trial = Vec::new();
                if match_inner(pat, value, &mut trial, enum_table) {
                    out.extend(trial);
                    return true;
                }
            }
            false
        }

        // ── Extract: extract { field, ... } ──────────────────────
        PatternKind::Extract(fields) => {
            let field_map: HashMap<String, Value> = match value {
                Value::Struct { fields: f, .. } => f.borrow().clone(),
                _ => return false,
            };
            let mut trial = Vec::new();
            for fp in fields.iter() {
                let fv = field_map.get(fp.field).cloned().unwrap_or(Value::Null);
                if let Some(sub_pat) = &fp.pattern {
                    if !match_inner(sub_pat, &fv, &mut trial, enum_table) { return false; }
                } else {
                    trial.push((fp.field.to_string(), fv));
                }
            }
            out.extend(trial);
            true
        }
    }
}

// ── Literal matching helpers ──────────────────────────────────────

fn match_literal(lit: &Literal<'_>, value: &Value) -> bool {
    match (lit, value) {
        (Literal::Null,      Value::Null)       => true,
        (Literal::Bool(b),   Value::Bool(v))    => b == v,
        (Literal::Int(n),    Value::Int(v))     => n == v,
        // An unsuffixed literal pattern against a sized-integer scrutinee
        // (`match byte { 0 => .., 255 => .. }`): sema unified the literal
        // with the scrutinee's type and range-checked it, so comparing the
        // mathematical values is exact. `i128` holds every `i64`/`u64`.
        (Literal::Int(n),    Value::I8(v))      => *n as i128 == *v as i128,
        (Literal::Int(n),    Value::I16(v))     => *n as i128 == *v as i128,
        (Literal::Int(n),    Value::I32(v))     => *n as i128 == *v as i128,
        (Literal::Int(n),    Value::U8(v))      => *n as i128 == *v as i128,
        (Literal::Int(n),    Value::U16(v))     => *n as i128 == *v as i128,
        (Literal::Int(n),    Value::U32(v))     => *n as i128 == *v as i128,
        (Literal::Int(n),    Value::UInt(v))    => *n as i128 == *v as i128,
        (Literal::TypedInt { raw, suffix }, _) => match_typed_int(*raw, *suffix, value),
        (Literal::Float(f),  Value::Float(v))   => f == v,
        (Literal::Double(d), Value::Double(v))  => d == v,
        (Literal::Char(c),   Value::Char(v))    => c == v,
        (Literal::Str(s),    Value::Str(v))     => *s == v.as_str(),
        // Allow int literal to match float/double (common in range patterns).
        (Literal::Int(n),    Value::Float(v))   => (*n as f32) == *v,
        (Literal::Int(n),    Value::Double(v))  => (*n as f64) == *v,
        _ => false,
    }
}

/// `Literal::TypedInt`'s half of `match_literal`: `raw` (an unsigned
/// magnitude — see that variant's own doc comment) compared against
/// whichever sized `Value` variant `suffix` says it should be. Only
/// ever `true` against that one matching variant; a suffixed literal
/// pattern doesn't loosely match other numeric types the way a plain
/// `Literal::Int` does against `Float`/`Double` above — the suffix is
/// exactly the annotation that makes the intended width unambiguous,
/// so there's no ambiguity left to be lenient about.
fn match_typed_int(raw: u64, suffix: IntSuffix, value: &Value) -> bool {
    match (suffix, value) {
        (IntSuffix::I8,    Value::I8(v))   => raw as i8  == *v,
        (IntSuffix::I16,   Value::I16(v))  => raw as i16 == *v,
        (IntSuffix::I32,   Value::I32(v))  => raw as i32 == *v,
        (IntSuffix::I64,   Value::Int(v))  => raw as i64 == *v,
        (IntSuffix::Isize, Value::Int(v))  => raw as i64 == *v,
        (IntSuffix::U8,    Value::U8(v))   => raw as u8  == *v,
        (IntSuffix::U16,   Value::U16(v))  => raw as u16 == *v,
        (IntSuffix::U32,   Value::U32(v))  => raw as u32 == *v,
        (IntSuffix::U64,   Value::UInt(v)) => raw == *v,
        (IntSuffix::Usize, Value::UInt(v)) => raw == *v,
        _ => false,
    }
}

fn literal_to_i64(lit: &Literal<'_>) -> Option<i64> {
    match lit {
        Literal::Int(n) => Some(*n),
        _ => None,
    }
}

fn literal_to_char(lit: &Literal<'_>) -> Option<char> {
    match lit {
        Literal::Char(c) => Some(*c),
        _ => None,
    }
}

// ── Tuple slice matching ──────────────────────────────────────────

fn match_tuple_slice(
    pats:       &[Pattern<'_>],
    items:      &[Value],
    out:        &mut Vec<(String, Value)>,
    enum_table: PatternTables<'_>,
) -> bool {
    if pats.len() != items.len() { return false; }
    let mut trial = Vec::new();
    for (pat, val) in pats.iter().zip(items.iter()) {
        if !match_inner(pat, val, &mut trial, enum_table) { return false; }
    }
    out.extend(trial);
    true
}

// ── Struct payload matching ───────────────────────────────────────

fn match_struct_payload(
    fps:        &[FieldPattern<'_>],
    fields:     &HashMap<String, Value>,
    out:        &mut Vec<(String, Value)>,
    enum_table: PatternTables<'_>,
) -> bool {
    let mut trial = Vec::new();
    for fp in fps.iter() {
        let fv = fields.get(fp.field).cloned().unwrap_or(Value::Null);
        if let Some(sub_pat) = &fp.pattern {
            if !match_inner(sub_pat, &fv, &mut trial, enum_table) { return false; }
        } else {
            trial.push((fp.field.to_string(), fv));
        }
    }
    out.extend(trial);
    true
}

// ── Destructure element helper ────────────────────────────────────

fn bind_destructure_elem(
    elem:  &DestructureElement<'_>,
    value: Value,
    env:   &mut Environment,
) {
    match elem {
        DestructureElement::Ident(name) => env.define(name, value),
        DestructureElement::Wildcard    => {}
        DestructureElement::Nested(pat) => bind_destructure_pattern(pat, value, env),
    }
}
