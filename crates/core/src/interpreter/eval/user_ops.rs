//! Hand-written impls of the prelude traits, called from the places the
//! runtime compares, orders and looks things up (traits slice S2b).
//!
//! `==`, `!=`, `<`, `<=`, `>`, `>=`, `contains` on a list, queue, stack or
//! inline list, the dictionary operations (`get`, `set`, `contains_key`,
//! indexing) and the Linqerizer's `group_by` and `order_by` all used to
//! compare through `Value::equals` and `Value::partial_cmp`, which take no
//! interpreter and so cannot call a user's `eq` or `partial_cmp`. These
//! helpers do, and fall back to the structural rules for everything else.
//!
//! The fast path stays fast: only a struct, enum, tuple or `Unique` operand
//! takes these paths; integers, strings and the rest go straight to the old
//! code.

use std::cell::RefCell;
use std::cmp::Ordering;
use std::rc::Rc;

use crate::ast::common::BinOp;
use crate::interpreter::eval::Interpreter;
use crate::interpreter::value::{EvalResult, FunctionId, Signal, Value};

/// Whether comparing this value could involve a user's impl.
#[inline]
pub(crate) fn may_have_user_ops(v: &Value) -> bool {
    matches!(v, Value::Struct { .. } | Value::Enum { .. } | Value::Tuple(_) | Value::Unique(_))
}

/// `Less`, `Equal`, `Greater` or `Null` (unordered) back to an `Ordering`.
fn ordering_from_value(v: &Value) -> Option<Ordering> {
    match v {
        Value::Enum { variant, .. } => match variant.as_str() {
            "Less"    => Some(Ordering::Less),
            "Equal"   => Some(Ordering::Equal),
            "Greater" => Some(Ordering::Greater),
            _         => None,
        },
        _ => None,
    }
}

impl<'ast> Interpreter<'ast> {
    /// The function a type's impl of `trait_name` registered for `method`.
    pub(crate) fn user_trait_fn(&self, value: &Value, trait_name: &str, method: &str) -> Option<FunctionId> {
        let type_name = match value {
            Value::Struct { type_name, .. } | Value::Enum { type_name, .. } => type_name,
            _ => return None,
        };
        self.trait_method_table
            .get(&(type_name.clone(), trait_name.to_string()))
            .and_then(|m| m.get(method))
            .copied()
    }

    fn call_user_bool(&mut self, f: FunctionId, a: &Value, b: &Value) -> Result<bool, Signal> {
        Ok(matches!(self.call_method(f, a.clone(), &[b.clone()])?, Value::Bool(true)))
    }

    /// `a == b`, calling a hand-written `eq` when `a`'s type has one and
    /// otherwise comparing as `Value::equals` does, except that tuple
    /// elements, derived-struct fields and a `Unique`'s contents are
    /// compared by this same rule, so a user's `eq` is honored inside them.
    pub(crate) fn values_equal(&mut self, a: &Value, b: &Value) -> Result<bool, Signal> {
        if let Some(f) = self.user_trait_fn(a, "PartialEq", "eq") {
            return self.call_user_bool(f, a, b);
        }
        match (a, b) {
            (Value::Tuple(x), Value::Tuple(y)) => {
                if x.len() != y.len() { return Ok(false); }
                for (p, q) in x.iter().zip(y.iter()) {
                    if !self.values_equal(p, q)? { return Ok(false); }
                }
                Ok(true)
            }
            (Value::Struct { type_name: ta, fields: fa, derives_partial_eq: da, .. },
             Value::Struct { type_name: tb, fields: fb, derives_partial_eq: db, .. }) if *da || *db => {
                if ta != tb { return Ok(false); }
                let pairs: Vec<(Value, Value)> = {
                    let (fa, fb) = (fa.borrow(), fb.borrow());
                    if fa.len() != fb.len() { return Ok(false); }
                    let mut pairs = Vec::with_capacity(fa.len());
                    for (k, v) in fa.iter() {
                        match fb.get(k) {
                            Some(w) => pairs.push((v.clone(), w.clone())),
                            None    => return Ok(false),
                        }
                    }
                    pairs
                };
                for (p, q) in &pairs {
                    if !self.values_equal(p, q)? { return Ok(false); }
                }
                Ok(true)
            }
            (Value::Unique(x), Value::Unique(y)) => self.values_equal(x, y),
            _ => Ok(a.equals(b)),
        }
    }

    /// The order of `a` and `b`: a hand-written `partial_cmp` when `a`'s type
    /// has one (`Null` is unordered), a derived struct field by field through
    /// this same rule, otherwise `Value::partial_cmp`.
    pub(crate) fn values_cmp(&mut self, a: &Value, b: &Value) -> Result<Option<Ordering>, Signal> {
        if let Some(f) = self.user_trait_fn(a, "PartialOrd", "partial_cmp") {
            let r = self.call_method(f, a.clone(), &[b.clone()])?;
            return Ok(ordering_from_value(&r));
        }
        match (a, b) {
            (Value::Struct { type_name: ta, fields: fa, derives_ord: true, field_order, .. },
             Value::Struct { type_name: tb, fields: fb, derives_ord: true, .. }) if ta == tb => {
                let pairs: Vec<(Value, Value)> = {
                    let (fa, fb) = (fa.borrow(), fb.borrow());
                    let mut pairs = Vec::with_capacity(field_order.len());
                    for name in field_order.iter() {
                        match (fa.get(name), fb.get(name)) {
                            (Some(x), Some(y)) => pairs.push((x.clone(), y.clone())),
                            _ => return Ok(None),
                        }
                    }
                    pairs
                };
                for (x, y) in &pairs {
                    match self.values_cmp(x, y)? {
                        Some(Ordering::Equal) => continue,
                        other => return Ok(other),
                    }
                }
                Ok(Some(Ordering::Equal))
            }
            (Value::Unique(x), Value::Unique(y)) => self.values_cmp(x, y),
            _ => Ok(a.partial_cmp(b)),
        }
    }

    /// A comparison operator where an operand may carry a user's impl.
    /// `Ok(None)` means "not mine": the operands are plain values and the
    /// ordinary operator code runs.
    pub(crate) fn user_comparison(&mut self, op: BinOp, lhs: &Value, rhs: &Value) -> Result<Option<Value>, Signal> {
        if !(may_have_user_ops(lhs) || may_have_user_ops(rhs)) { return Ok(None); }
        match op {
            BinOp::Eq => Ok(Some(Value::Bool(self.values_equal(lhs, rhs)?))),
            BinOp::Ne => {
                // The impl's own `ne` (its override, or the prelude default).
                if let Some(f) = self.user_trait_fn(lhs, "PartialEq", "ne") {
                    return Ok(Some(Value::Bool(self.call_user_bool(f, lhs, rhs)?)));
                }
                Ok(Some(Value::Bool(!self.values_equal(lhs, rhs)?)))
            }
            BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
                let method = match op {
                    BinOp::Lt => "lt", BinOp::Le => "le", BinOp::Gt => "gt", _ => "ge",
                };
                if let Some(f) = self.user_trait_fn(lhs, "PartialOrd", method) {
                    return Ok(Some(Value::Bool(self.call_user_bool(f, lhs, rhs)?)));
                }
                // A derived struct: field by field, honoring a field's own impl.
                if matches!(lhs, Value::Struct { derives_ord: true, .. }) {
                    return match (op, self.values_cmp(lhs, rhs)?) {
                        (BinOp::Lt, Some(o)) => Ok(Some(Value::Bool(o.is_lt()))),
                        (BinOp::Le, Some(o)) => Ok(Some(Value::Bool(o.is_le()))),
                        (BinOp::Gt, Some(o)) => Ok(Some(Value::Bool(o.is_gt()))),
                        (BinOp::Ge, Some(o)) => Ok(Some(Value::Bool(o.is_ge()))),
                        (_, None) => Err(Signal::Panic("comparison between incomparable values".into())),
                        _ => unreachable!(),
                    };
                }
                Ok(None)
            }
            _ => Ok(None),
        }
    }

    /// Whether `needle` is in `items`, by `values_equal`.
    pub(crate) fn contains_value(&mut self, items: &[Value], needle: &Value) -> Result<bool, Signal> {
        for item in items {
            if self.values_equal(item, needle)? { return Ok(true); }
        }
        Ok(false)
    }

    /// The position of the entry whose key equals `key`, by `values_equal`.
    pub(crate) fn dict_position(&mut self, dict: &Rc<RefCell<Vec<(Value, Value)>>>, key: &Value) -> Result<Option<usize>, Signal> {
        let keys: Vec<Value> = dict.borrow().iter().map(|(k, _)| k.clone()).collect();
        for (i, k) in keys.iter().enumerate() {
            if self.values_equal(k, key)? { return Ok(Some(i)); }
        }
        Ok(None)
    }

    /// `contains_key`, `get` and `set` on a dictionary whose key may carry a
    /// user's `eq`.
    pub(crate) fn dict_op_user(&mut self, dict: &Rc<RefCell<Vec<(Value, Value)>>>, method: &str, args: &[Value]) -> EvalResult {
        let key = args.first().cloned()
            .ok_or_else(|| Signal::Panic(format!("{}() needs a key", method)))?;
        let at = self.dict_position(dict, &key)?;
        match method {
            "contains_key" => Ok(Value::Bool(at.is_some())),
            "get" => Ok(at.map(|i| dict.borrow()[i].1.clone()).unwrap_or(Value::Null)),
            "set" => {
                let val = args.get(1).cloned()
                    .ok_or_else(|| Signal::Panic("set() needs 2 arguments (key, value)".into()))?;
                match at {
                    Some(i) => dict.borrow_mut()[i].1 = val,
                    None    => dict.borrow_mut().push((key, val)),
                }
                Ok(Value::Void)
            }
            other => Err(Signal::Panic(format!("no dictionary method '{}'", other))),
        }
    }
}
