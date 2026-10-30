//! User-written `Display` impls (traits slice S5a).
//!
//! `println`, `print`, `log`, `to_string`, an interpolation hole `{x}` and
//! the messages of `assert` and `panic` all turn a value into text through
//! `Value`'s own formatting, which takes no interpreter and so cannot call a
//! user's `to_string`. These helpers do the one thing that formatting cannot:
//! every value, at any depth, whose type has a hand-written `Display` is
//! replaced by the string its `to_string` returns, and everything else is left
//! for the old formatting to print. A string prints as its own text, so the
//! output has exactly the shape it always had.
//!
//! `{x:?}` (Debug) is the structural form and never comes through here.
//!
//! The fast path stays fast: while no `impl Display` was registered
//! (`has_user_display` is false) none of this runs.

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;

use crate::interpreter::eval::Interpreter;
use crate::interpreter::value::{EnumPayload, InlineListData, Signal, Value};

/// Deep enough for any real structure; a cyclic one is left as it is.
const MAX_DEPTH: usize = 128;

impl<'ast> Interpreter<'ast> {
    /// `args` with each user-formatted value replaced by its text.
    pub(crate) fn display_args(&mut self, args: &[Value]) -> Result<Vec<Value>, Signal> {
        let mut shown = Vec::with_capacity(args.len());
        for a in args {
            shown.push(self.display_value(a)?);
        }
        Ok(shown)
    }

    /// `v` with each value that has a user `Display` replaced by its text.
    pub(crate) fn display_value(&mut self, v: &Value) -> Result<Value, Signal> {
        if !self.has_user_display {
            return Ok(v.clone());
        }
        Ok(self.subst_display(v, 0)?.unwrap_or_else(|| v.clone()))
    }

    /// `Some(new)` when something at or below `v` was replaced, `None` when
    /// nothing was (so an unchanged value is never copied).
    fn subst_display(&mut self, v: &Value, depth: usize) -> Result<Option<Value>, Signal> {
        if depth > MAX_DEPTH {
            return Ok(None);
        }
        if let Some(f) = self.user_trait_fn(v, "Display", "to_string") {
            let text = self.call_method(f, v.clone(), &[])?;
            return Ok(Some(match text {
                Value::Str(_) => text,
                other         => Value::str_from(other.to_string()),
            }));
        }
        let d = depth + 1;
        Ok(match v {
            Value::Struct { type_name, fields, derives_partial_eq, derives_ord, derives_hash, derives_clone, field_order } => {
                let snapshot: Vec<(String, Value)> =
                    fields.borrow().iter().map(|(k, x)| (k.clone(), x.clone())).collect();
                self.subst_pairs(snapshot, d)?.map(|map| Value::Struct {
                    type_name:          type_name.clone(),
                    fields:             Rc::new(RefCell::new(map)),
                    derives_partial_eq: *derives_partial_eq,
                    derives_ord:        *derives_ord,
                    derives_hash:       *derives_hash,
                    derives_clone:      *derives_clone,
                    field_order:        Rc::clone(field_order),
                })
            }
            Value::Enum { type_name, variant, payload } => match payload.as_ref() {
                EnumPayload::None => None,
                EnumPayload::Tuple(items) => self.subst_seq(items, d)?.map(|items| Value::Enum {
                    type_name: type_name.clone(),
                    variant:   variant.clone(),
                    payload:   Box::new(EnumPayload::Tuple(items)),
                }),
                EnumPayload::Struct(map) => {
                    let snapshot: Vec<(String, Value)> =
                        map.iter().map(|(k, x)| (k.clone(), x.clone())).collect();
                    self.subst_pairs(snapshot, d)?.map(|map| Value::Enum {
                        type_name: type_name.clone(),
                        variant:   variant.clone(),
                        payload:   Box::new(EnumPayload::Struct(map)),
                    })
                }
            },
            Value::Tuple(items) => self.subst_seq(items, d)?.map(Value::Tuple),
            Value::List(rc) => {
                let items = rc.borrow().clone();
                self.subst_seq(&items, d)?.map(|v| Value::List(Rc::new(RefCell::new(v))))
            }
            Value::Stack(rc) => {
                let items = rc.borrow().clone();
                self.subst_seq(&items, d)?.map(|v| Value::Stack(Rc::new(RefCell::new(v))))
            }
            Value::Queue(rc) => {
                let items: Vec<Value> = rc.borrow().iter().cloned().collect();
                self.subst_seq(&items, d)?
                    .map(|v| Value::Queue(Rc::new(RefCell::new(VecDeque::from(v)))))
            }
            Value::InlineList(rc) => {
                let (items, capacity) = {
                    let data = rc.borrow();
                    (data.items.clone(), data.capacity)
                };
                self.subst_seq(&items, d)?.map(|items| {
                    Value::InlineList(Rc::new(RefCell::new(InlineListData { items, capacity })))
                })
            }
            Value::Dict(rc) => {
                let entries = rc.borrow().clone();
                let mut out: Vec<(Value, Value)> = Vec::with_capacity(entries.len());
                let mut changed = false;
                for (k, x) in &entries {
                    let nk = self.subst_display(k, d)?;
                    let nx = self.subst_display(x, d)?;
                    changed |= nk.is_some() || nx.is_some();
                    out.push((nk.unwrap_or_else(|| k.clone()), nx.unwrap_or_else(|| x.clone())));
                }
                changed.then(|| Value::Dict(Rc::new(RefCell::new(out))))
            }
            Value::Unique(inner) => self.subst_display(inner, d)?.map(|x| Value::Unique(Box::new(x))),
            Value::Shared(rc) => {
                let inner = rc.borrow().clone();
                self.subst_display(&inner, d)?.map(|x| Value::Shared(Rc::new(RefCell::new(x))))
            }
            Value::SyncShared(rc) => {
                let inner = rc.borrow().clone();
                self.subst_display(&inner, d)?.map(|x| Value::SyncShared(Rc::new(RefCell::new(x))))
            }
            _ => None,
        })
    }

    /// The elements with replacements made, or `None` when none was.
    fn subst_seq(&mut self, items: &[Value], depth: usize) -> Result<Option<Vec<Value>>, Signal> {
        let mut out: Vec<Value> = Vec::with_capacity(items.len());
        let mut changed = false;
        for x in items {
            match self.subst_display(x, depth)? {
                Some(n) => { changed = true; out.push(n); }
                None    => out.push(x.clone()),
            }
        }
        Ok(changed.then_some(out))
    }

    /// The named fields with replacements made, or `None` when none was.
    fn subst_pairs(
        &mut self,
        pairs: Vec<(String, Value)>,
        depth: usize,
    ) -> Result<Option<HashMap<String, Value>>, Signal> {
        let mut out: HashMap<String, Value> = HashMap::with_capacity(pairs.len());
        let mut changed = false;
        for (k, x) in pairs {
            match self.subst_display(&x, depth)? {
                Some(n) => { changed = true; out.insert(k, n); }
                None    => { out.insert(k, x); }
            }
        }
        Ok(changed.then_some(out))
    }
}
