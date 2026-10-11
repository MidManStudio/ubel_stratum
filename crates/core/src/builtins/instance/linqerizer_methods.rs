// crates/core/src/builtins/instance/linqerizer_methods.rs
//! Instance methods on `Value::Linqerizer` — called as
//! `someLinqerizer.method(...)`. See the `Value::Linqerizer` doc comment
//! (`interpreter/value.rs`) for the lazy-pipeline design; see
//! `docs/DATASTRUCTURES.md` §6 for the full design writeup.
//!
//! `.query()` (the only way to *get* a `Linqerizer<T>`) lives on `List`
//! in `list_methods.rs`, not here — it's `HIGH_ONLY`-gated there. Every
//! method in this file is deliberately *not* individually HIGH-only:
//! by the time you're holding a `Linqerizer<T>` value at all, you're
//! already past the one real gate (`.query()`'s own), same principle as
//! `List`'s own methods never being individually tier-gated beyond
//! `List.new()`'s own construction rules.

use std::rc::Rc;
use crate::interpreter::eval::Interpreter;
use crate::interpreter::value::{EvalResult, LinqOp, LinqPipeline, Signal, Value};

pub const METHOD_NAMES: &[&str] = &[
    "where", "select", "order_by", "order_by_desc",
    "to_list", "first", "count", "group_by",
];

/// Empty — see the module doc comment above for why. Real, consulted
/// registry, not a stub (`instance::is_high_only`), same as every other
/// `HIGH_ONLY` here.
pub const HIGH_ONLY: &[&str] = &[];

/// `(return shape, arity)` for sema — see `instance::MethodReturn`.
///
/// `select` and `group_by`'s `MethodReturn` values here are never
/// actually consulted — both are intercepted in `type_infer.rs` *before*
/// `method_return_type` runs at all, because their real return type
/// depends on an argument's own inferred type (the selector/keyselector
/// function's return type), which `method_return_type`'s
/// `(MethodReturn, ReceiverWrap, TypeId)` signature has no way to see.
/// `NewSelf` is just a valid placeholder so this function stays
/// exhaustive and honest about arity — the actual arity numbers here
/// *are* real and consulted, for the generic `ArgumentCountMismatch`
/// check that runs before the interception point.
pub fn signature(name: &str) -> Option<(crate::builtins::instance::MethodReturn, usize)> {
    use crate::builtins::instance::MethodReturn as R;
    Some(match name {
        "where"         => (R::NewSelf, 1),
        "select"        => (R::NewSelf, 1), // intercepted — see doc comment above
        "order_by"      => (R::NewSelf, 1),
        "order_by_desc" => (R::NewSelf, 1),
        "to_list"       => (R::NewListOfLinqElem, 0),
        "first"         => (R::Elem, 0),
        "count"         => (R::Int, 0),
        "group_by"      => (R::NewSelf, 1), // intercepted — see doc comment above
        _ => return None,
    })
}

fn expect_predicate(args: &[Value], method: &str) -> Result<usize, Signal> {
    match args.first() {
        Some(Value::Function(id)) => Ok(*id),
        _ => Err(Signal::Panic(format!("{}() needs 1 argument: a function", method))),
    }
}

fn push_op(pipeline: &Rc<LinqPipeline>, op: LinqOp) -> Value {
    let mut ops = pipeline.ops.clone();
    ops.push(op);
    Value::Linqerizer(Rc::new(LinqPipeline { source: Rc::clone(&pipeline.source), ops }))
}

pub fn where_(pipeline: &Rc<LinqPipeline>, args: &[Value]) -> EvalResult {
    let pred_id = expect_predicate(args, "where")?;
    Ok(push_op(pipeline, LinqOp::Where(pred_id)))
}

pub fn select(pipeline: &Rc<LinqPipeline>, args: &[Value]) -> EvalResult {
    let sel_id = expect_predicate(args, "select")?;
    Ok(push_op(pipeline, LinqOp::Select(sel_id)))
}

pub fn order_by(pipeline: &Rc<LinqPipeline>, args: &[Value]) -> EvalResult {
    let key_id = expect_predicate(args, "order_by")?;
    Ok(push_op(pipeline, LinqOp::OrderBy(key_id, false)))
}

pub fn order_by_desc(pipeline: &Rc<LinqPipeline>, args: &[Value]) -> EvalResult {
    let key_id = expect_predicate(args, "order_by_desc")?;
    Ok(push_op(pipeline, LinqOp::OrderBy(key_id, true)))
}

/// `.order_by()`/`.order_by_desc()`'s comparison now goes straight
/// through `Value::partial_cmp` — the single comparison implementation
/// this interpreter has, the same relationship every other comparison
/// already has with `Value::equals`. Retires this module's own,
/// narrower `compare_values` (Int/Float/Double/Str/Bool only); a struct
/// with a derived ordering now sorts correctly too, as a direct
/// consequence rather than something built specifically for this call
/// site. Falls back to treating an incomparable pair as equal (stable
/// sort leaves their relative order alone) rather than panicking on,
/// say, a key selector that returns `List`s — unchanged behavior, just
/// re-hung off `partial_cmp` instead of a local match.

/// Run every pending op against the snapshot, in order. The one real
/// piece of shared logic every terminal method needs — where the actual
/// interpretation happens, since chaining (`push_op` above) never
/// touches an `Interpreter` at all, only terminal calls do.
fn materialize(interp: &mut Interpreter<'_>, pipeline: &LinqPipeline) -> Result<Vec<Value>, Signal> {
    let mut current: Vec<Value> = (*pipeline.source).clone();
    for op in &pipeline.ops {
        match op {
            LinqOp::Where(pred_id) => {
                let mut kept = Vec::with_capacity(current.len());
                for item in current {
                    if interp.call_function(*pred_id, &[item.clone()])?.is_truthy()? {
                        kept.push(item);
                    }
                }
                current = kept;
            }
            LinqOp::Select(sel_id) => {
                let mut projected = Vec::with_capacity(current.len());
                for item in current {
                    projected.push(interp.call_function(*sel_id, &[item])?);
                }
                current = projected;
            }
            LinqOp::OrderBy(key_id, descending) => {
                let mut keyed = Vec::with_capacity(current.len());
                for item in current {
                    let key = interp.call_function(*key_id, &[item.clone()])?;
                    keyed.push((key, item));
                }
                // Keys compare through the interpreter, so a user's `partial_cmp`
                // orders them; the first error a comparison raises is kept.
                let mut failure: Option<Signal> = None;
                keyed.sort_by(|(ka, _), (kb, _)| {
                    match interp.values_cmp(ka, kb) {
                        Ok(o)  => o.unwrap_or(std::cmp::Ordering::Equal),
                        Err(e) => { if failure.is_none() { failure = Some(e); } std::cmp::Ordering::Equal }
                    }
                });
                if let Some(e) = failure { return Err(e); }
                if *descending {
                    keyed.reverse();
                }
                current = keyed.into_iter().map(|(_, v)| v).collect();
            }
        }
    }
    Ok(current)
}

pub fn to_list(interp: &mut Interpreter<'_>, pipeline: &LinqPipeline) -> EvalResult {
    let result = materialize(interp, pipeline)?;
    Ok(Value::List(Rc::new(std::cell::RefCell::new(result))))
}

/// First element after every pending op is applied, or `Null` — same
/// "nothing there" convention `pop`/`first`/`last`/`get` already use.
pub fn first(interp: &mut Interpreter<'_>, pipeline: &LinqPipeline) -> EvalResult {
    let result = materialize(interp, pipeline)?;
    Ok(result.into_iter().next().unwrap_or(Value::Null))
}

pub fn count(interp: &mut Interpreter<'_>, pipeline: &LinqPipeline) -> EvalResult {
    let result = materialize(interp, pipeline)?;
    Ok(Value::Int(result.len() as i64))
}

/// `Dictionary<Key, List<Value>>` — every distinct key produced by
/// `key_selector`, mapped to a `List` of every element that produced it,
/// in first-seen order. The real, decided replacement for the old
/// `eval_linq`'s `groupby` clause, which was a literal `// TODO` no-op —
/// this actually groups.
pub fn group_by(interp: &mut Interpreter<'_>, pipeline: &LinqPipeline, args: &[Value]) -> EvalResult {
    let key_id = expect_predicate(args, "group_by")?;
    let result = materialize(interp, pipeline)?;
    let mut groups: Vec<(Value, Value)> = Vec::new();
    for item in result {
        let key = interp.call_function(key_id, &[item.clone()])?;
        let mut at: Option<usize> = None;
        for (i, (k, _)) in groups.iter().enumerate() {
            if interp.values_equal(k, &key)? { at = Some(i); break; }
        }
        match at.map(|i| &mut groups[i]) {
            Some((_, Value::List(list_rc))) => {
                list_rc.borrow_mut().push(item);
            }
            _ => {
                groups.push((key, Value::List(Rc::new(std::cell::RefCell::new(vec![item])))));
            }
        }
    }
    Ok(Value::Dict(Rc::new(std::cell::RefCell::new(groups))))
}
