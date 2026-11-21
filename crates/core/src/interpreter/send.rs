//! The copy at a task boundary (docs/ASYNC_SYNC_DESIGN.md, A7 and AS3a).
//!
//! Every task runs in an interpreter of its own, and the values of one interpreter are built of
//! `Rc`, which cannot cross to another thread. A value that goes into a task, or comes back out of
//! one, is therefore copied: `encode` turns it into a `Sent`, which owns plain data and can be
//! moved to any thread, and `Sent::decode` builds fresh `Rc` values from it in the receiving
//! interpreter.
//!
//! The copy keeps the shape of what it copies. Two places that pointed at one list still point at
//! one list afterwards, and a value that contains itself still does. Each heap value (a list, a
//! dictionary, a queue, a stack, a struct, an inline list) becomes one node of a table, and a
//! reference to it becomes the index of the node.
//!
//! A value that is not plain data cannot be copied: a `Shared`, a `SyncShared`, a task, a pool, a
//! handle, a query pipeline, and a function that exists only in the interpreter that made it (a
//! lambda). `encode` reports these with a message that names the kind. Until the sendable rule of
//! AS4 exists at compile time, this is where a program that sends one finds out.

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;

use super::value::{EnumPayload, FunctionId, InlineListData, Value};

/// One value in a `Sent`. Plain data, no `Rc`, so it is `Send`.
#[derive(Debug, Clone)]
pub enum SendValue {
    Null,
    Void,
    Bool(bool),
    Int(i64),
    I8(i8),
    I16(i16),
    I32(i32),
    U8(u8),
    U16(u16),
    U32(u32),
    UInt(u64),
    Float(f32),
    Double(f64),
    Char(char),
    Str(String),
    Tuple(Vec<SendValue>),
    Enum { type_name: String, variant: String, payload: SendPayload },
    Unique(Box<SendValue>),
    /// A function declared in the program. The same program registers its functions in the same
    /// order in every interpreter, so the id means the same function everywhere.
    Function(FunctionId),
    /// A heap value: an index into the node table of the `Sent`.
    Node(usize),
}

#[derive(Debug, Clone)]
pub enum SendPayload {
    None,
    Tuple(Vec<SendValue>),
    Struct(Vec<(String, SendValue)>),
}

#[derive(Debug, Clone)]
enum SendNode {
    List(Vec<SendValue>),
    Dict(Vec<(SendValue, SendValue)>),
    Queue(Vec<SendValue>),
    Stack(Vec<SendValue>),
    InlineList { items: Vec<SendValue>, capacity: usize },
    Struct {
        type_name:          String,
        fields:             Vec<(String, SendValue)>,
        derives_partial_eq: bool,
        derives_ord:        bool,
        derives_hash:       bool,
        derives_clone:      bool,
        field_order:        Vec<String>,
    },
}

/// A group of values copied together, so that what they share stays shared.
#[derive(Debug, Clone)]
pub struct Sent {
    nodes: Vec<SendNode>,
    roots: Vec<SendValue>,
}

/// Copy `values` out of the interpreter that owns them. `declared_functions` is the number of
/// functions the program declares: a function value below it is sendable, one above it is a
/// lambda and is not.
pub fn encode(values: &[Value], declared_functions: usize) -> Result<Sent, String> {
    let mut enc = Encoder { declared: declared_functions, nodes: Vec::new(), seen: HashMap::new() };
    let mut roots = Vec::with_capacity(values.len());
    for v in values {
        roots.push(enc.value(v)?);
    }
    Ok(Sent { nodes: enc.nodes, roots })
}

impl Sent {
    /// Build the values again, as fresh `Rc` values of the interpreter that calls this.
    pub fn decode(self) -> Vec<Value> {
        let Sent { nodes, roots } = self;

        // First every heap value exists, empty, so that a reference to it can be made before the
        // value is filled. Then each one is filled. That is what lets a cycle come back as a cycle.
        let shells: Vec<Value> = nodes.iter().map(shell).collect();
        for (shell, node) in shells.iter().zip(nodes) {
            fill(shell, node, &shells);
        }
        roots.into_iter().map(|r| build(r, &shells)).collect()
    }

    /// The number of values in the group.
    pub fn len(&self) -> usize {
        self.roots.len()
    }

    pub fn is_empty(&self) -> bool {
        self.roots.is_empty()
    }
}

// ── Encoding ─────────────────────────────────────────────────────

struct Encoder {
    declared: usize,
    nodes:    Vec<SendNode>,
    /// Address of an `Rc` already copied, to the node it became.
    seen:     HashMap<*const (), usize>,
}

impl Encoder {
    fn value(&mut self, v: &Value) -> Result<SendValue, String> {
        Ok(match v {
            Value::Null      => SendValue::Null,
            Value::Void      => SendValue::Void,
            Value::Bool(b)   => SendValue::Bool(*b),
            Value::Int(n)    => SendValue::Int(*n),
            Value::I8(n)     => SendValue::I8(*n),
            Value::I16(n)    => SendValue::I16(*n),
            Value::I32(n)    => SendValue::I32(*n),
            Value::U8(n)     => SendValue::U8(*n),
            Value::U16(n)    => SendValue::U16(*n),
            Value::U32(n)    => SendValue::U32(*n),
            Value::UInt(n)   => SendValue::UInt(*n),
            Value::Float(n)  => SendValue::Float(*n),
            Value::Double(n) => SendValue::Double(*n),
            Value::Char(c)   => SendValue::Char(*c),
            Value::Str(s)    => SendValue::Str(s.as_ref().clone()),
            Value::Tuple(items) => SendValue::Tuple(self.values(items)?),
            Value::Enum { type_name, variant, payload } => SendValue::Enum {
                type_name: type_name.clone(),
                variant:   variant.clone(),
                payload:   match payload.as_ref() {
                    EnumPayload::None          => SendPayload::None,
                    EnumPayload::Tuple(items)  => SendPayload::Tuple(self.values(items)?),
                    EnumPayload::Struct(map)   => SendPayload::Struct(self.fields(map)?),
                },
            },
            Value::Unique(inner) => SendValue::Unique(Box::new(self.value(inner)?)),
            Value::Function(id) => {
                if *id < self.declared {
                    SendValue::Function(*id)
                } else {
                    return Err("a closure cannot be sent to another task".to_string());
                }
            }

            Value::List(rc) => {
                if let Some(i) = self.seen_node(Rc::as_ptr(rc) as *const ()) { return Ok(SendValue::Node(i)); }
                let i = self.reserve(Rc::as_ptr(rc) as *const ());
                let items = self.values(&rc.borrow())?;
                self.nodes[i] = SendNode::List(items);
                SendValue::Node(i)
            }
            Value::Stack(rc) => {
                if let Some(i) = self.seen_node(Rc::as_ptr(rc) as *const ()) { return Ok(SendValue::Node(i)); }
                let i = self.reserve(Rc::as_ptr(rc) as *const ());
                let items = self.values(&rc.borrow())?;
                self.nodes[i] = SendNode::Stack(items);
                SendValue::Node(i)
            }
            Value::Queue(rc) => {
                if let Some(i) = self.seen_node(Rc::as_ptr(rc) as *const ()) { return Ok(SendValue::Node(i)); }
                let i = self.reserve(Rc::as_ptr(rc) as *const ());
                let mut items = Vec::with_capacity(rc.borrow().len());
                for item in rc.borrow().iter() { items.push(self.value(item)?); }
                self.nodes[i] = SendNode::Queue(items);
                SendValue::Node(i)
            }
            Value::Dict(rc) => {
                if let Some(i) = self.seen_node(Rc::as_ptr(rc) as *const ()) { return Ok(SendValue::Node(i)); }
                let i = self.reserve(Rc::as_ptr(rc) as *const ());
                let mut pairs = Vec::with_capacity(rc.borrow().len());
                for (k, v) in rc.borrow().iter() { pairs.push((self.value(k)?, self.value(v)?)); }
                self.nodes[i] = SendNode::Dict(pairs);
                SendValue::Node(i)
            }
            Value::InlineList(rc) => {
                if let Some(i) = self.seen_node(Rc::as_ptr(rc) as *const ()) { return Ok(SendValue::Node(i)); }
                let i = self.reserve(Rc::as_ptr(rc) as *const ());
                let items = self.values(&rc.borrow().items)?;
                let capacity = rc.borrow().capacity;
                self.nodes[i] = SendNode::InlineList { items, capacity };
                SendValue::Node(i)
            }
            Value::Struct {
                type_name, fields, derives_partial_eq, derives_ord, derives_hash, derives_clone, field_order,
            } => {
                if let Some(i) = self.seen_node(Rc::as_ptr(fields) as *const ()) { return Ok(SendValue::Node(i)); }
                let i = self.reserve(Rc::as_ptr(fields) as *const ());
                let fields = self.fields(&fields.borrow())?;
                self.nodes[i] = SendNode::Struct {
                    type_name:          type_name.clone(),
                    fields,
                    derives_partial_eq: *derives_partial_eq,
                    derives_ord:        *derives_ord,
                    derives_hash:       *derives_hash,
                    derives_clone:      *derives_clone,
                    field_order:        field_order.as_ref().clone(),
                };
                SendValue::Node(i)
            }

            Value::Shared(_)     => return Err(cannot("Shared")),
            Value::Task(_)       => return Err(cannot("task")),
            Value::Pool(_)       => return Err(cannot("Pool")),
            Value::Handle { .. } => return Err(cannot("pool handle")),
            Value::Linqerizer(_) => return Err(cannot("query")),
            #[allow(unreachable_patterns)]
            other                => return Err(cannot(other.type_name())),
        })
    }

    fn values(&mut self, items: &[Value]) -> Result<Vec<SendValue>, String> {
        let mut out = Vec::with_capacity(items.len());
        for v in items { out.push(self.value(v)?); }
        Ok(out)
    }

    /// A map's entries in name order, so that the same value always encodes the same way.
    fn fields(&mut self, map: &HashMap<String, Value>) -> Result<Vec<(String, SendValue)>, String> {
        let mut names: Vec<&String> = map.keys().collect();
        names.sort();
        let mut out = Vec::with_capacity(names.len());
        for name in names { out.push((name.clone(), self.value(&map[name])?)); }
        Ok(out)
    }

    fn seen_node(&self, key: *const ()) -> Option<usize> {
        self.seen.get(&key).copied()
    }

    /// Give a heap value its place in the table before its contents are copied, so that a
    /// reference back to it from inside finds the place.
    fn reserve(&mut self, key: *const ()) -> usize {
        let i = self.nodes.len();
        self.nodes.push(SendNode::List(Vec::new()));
        self.seen.insert(key, i);
        i
    }
}

fn cannot(kind: &str) -> String {
    format!("a {kind} cannot be sent to another task")
}

// ── Decoding ─────────────────────────────────────────────────────

/// The empty heap value a node stands for.
fn shell(node: &SendNode) -> Value {
    match node {
        SendNode::List(_)  => Value::List(Rc::new(RefCell::new(Vec::new()))),
        SendNode::Dict(_)  => Value::Dict(Rc::new(RefCell::new(Vec::new()))),
        SendNode::Queue(_) => Value::Queue(Rc::new(RefCell::new(VecDeque::new()))),
        SendNode::Stack(_) => Value::Stack(Rc::new(RefCell::new(Vec::new()))),
        SendNode::InlineList { capacity, .. } => Value::InlineList(Rc::new(RefCell::new(
            InlineListData { items: Vec::new(), capacity: *capacity },
        ))),
        SendNode::Struct {
            type_name, derives_partial_eq, derives_ord, derives_hash, derives_clone, field_order, ..
        } => Value::Struct {
            type_name:          type_name.clone(),
            fields:             Rc::new(RefCell::new(HashMap::new())),
            derives_partial_eq: *derives_partial_eq,
            derives_ord:        *derives_ord,
            derives_hash:       *derives_hash,
            derives_clone:      *derives_clone,
            field_order:        Rc::new(field_order.clone()),
        },
    }
}

fn fill(shell: &Value, node: SendNode, shells: &[Value]) {
    match (shell, node) {
        (Value::List(rc), SendNode::List(items)) => {
            *rc.borrow_mut() = items.into_iter().map(|v| build(v, shells)).collect();
        }
        (Value::Stack(rc), SendNode::Stack(items)) => {
            *rc.borrow_mut() = items.into_iter().map(|v| build(v, shells)).collect();
        }
        (Value::Queue(rc), SendNode::Queue(items)) => {
            *rc.borrow_mut() = items.into_iter().map(|v| build(v, shells)).collect();
        }
        (Value::Dict(rc), SendNode::Dict(pairs)) => {
            *rc.borrow_mut() = pairs.into_iter().map(|(k, v)| (build(k, shells), build(v, shells))).collect();
        }
        (Value::InlineList(rc), SendNode::InlineList { items, .. }) => {
            rc.borrow_mut().items = items.into_iter().map(|v| build(v, shells)).collect();
        }
        (Value::Struct { fields, .. }, SendNode::Struct { fields: sent, .. }) => {
            *fields.borrow_mut() = sent.into_iter().map(|(k, v)| (k, build(v, shells))).collect();
        }
        _ => unreachable!("a node is filled into the shell made from it"),
    }
}

fn build(v: SendValue, shells: &[Value]) -> Value {
    match v {
        SendValue::Null      => Value::Null,
        SendValue::Void      => Value::Void,
        SendValue::Bool(b)   => Value::Bool(b),
        SendValue::Int(n)    => Value::Int(n),
        SendValue::I8(n)     => Value::I8(n),
        SendValue::I16(n)    => Value::I16(n),
        SendValue::I32(n)    => Value::I32(n),
        SendValue::U8(n)     => Value::U8(n),
        SendValue::U16(n)    => Value::U16(n),
        SendValue::U32(n)    => Value::U32(n),
        SendValue::UInt(n)   => Value::UInt(n),
        SendValue::Float(n)  => Value::Float(n),
        SendValue::Double(n) => Value::Double(n),
        SendValue::Char(c)   => Value::Char(c),
        SendValue::Str(s)    => Value::Str(Rc::new(s)),
        SendValue::Tuple(items) => Value::Tuple(items.into_iter().map(|v| build(v, shells)).collect()),
        SendValue::Enum { type_name, variant, payload } => Value::Enum {
            type_name,
            variant,
            payload: Box::new(match payload {
                SendPayload::None         => EnumPayload::None,
                SendPayload::Tuple(items) => EnumPayload::Tuple(items.into_iter().map(|v| build(v, shells)).collect()),
                SendPayload::Struct(map)  => EnumPayload::Struct(map.into_iter().map(|(k, v)| (k, build(v, shells))).collect()),
            }),
        },
        SendValue::Unique(inner) => Value::Unique(Box::new(build(*inner, shells))),
        SendValue::Function(id)  => Value::Function(id),
        SendValue::Node(i)       => shells[i].clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(items: Vec<Value>) -> Value {
        Value::List(Rc::new(RefCell::new(items)))
    }

    fn int_list(items: &[i64]) -> Value {
        list(items.iter().map(|n| Value::Int(*n)).collect())
    }

    fn round_trip(values: &[Value]) -> Vec<Value> {
        encode(values, 10).expect("these values can be sent").decode()
    }

    fn ints(v: &Value) -> Vec<i64> {
        match v {
            Value::List(rc) => rc.borrow().iter().map(|x| match x { Value::Int(n) => *n, _ => panic!("not an int") }).collect(),
            _ => panic!("not a list"),
        }
    }

    #[test]
    fn scalars_and_strings_come_back_equal() {
        let back = round_trip(&[
            Value::Null, Value::Void, Value::Bool(true), Value::Int(-7), Value::I8(-1), Value::I16(300),
            Value::I32(-5), Value::U8(200), Value::U16(60000), Value::U32(4_000_000_000), Value::UInt(u64::MAX),
            Value::Float(1.5), Value::Double(-2.25), Value::Char('x'), Value::Str(Rc::new("héllo".to_string())),
        ]);
        assert_eq!(back.len(), 15);
        assert!(matches!(back[2], Value::Bool(true)));
        assert!(matches!(back[3], Value::Int(-7)));
        assert!(matches!(back[6], Value::I32(-5)));
        assert!(matches!(back[8], Value::U16(60000)));
        assert!(matches!(back[10], Value::UInt(u64::MAX)));
        assert!(matches!(back[11], Value::Float(f) if f == 1.5));
        assert!(matches!(&back[14], Value::Str(s) if s.as_str() == "héllo"));
    }

    #[test]
    fn a_list_comes_back_as_a_new_list_with_the_same_items() {
        let original = int_list(&[1, 2, 3]);
        let back = round_trip(&[original.clone()]).pop().unwrap();
        assert_eq!(ints(&back), vec![1, 2, 3]);
        match (&original, &back) {
            (Value::List(a), Value::List(b)) => assert!(!Rc::ptr_eq(a, b)),
            _ => unreachable!(),
        }
        // Changing the copy leaves the original alone.
        if let Value::List(rc) = &back { rc.borrow_mut().push(Value::Int(4)); }
        assert_eq!(ints(&original), vec![1, 2, 3]);
    }

    #[test]
    fn one_list_reached_twice_is_still_one_list() {
        let shared = int_list(&[1]);
        let back = round_trip(&[shared.clone(), Value::Tuple(vec![shared.clone(), shared])]);
        let (a, b, c) = match (&back[0], &back[1]) {
            (Value::List(a), Value::Tuple(items)) => match (&items[0], &items[1]) {
                (Value::List(b), Value::List(c)) => (a.clone(), b.clone(), c.clone()),
                _ => panic!("tuple items should be lists"),
            },
            _ => panic!("unexpected shapes"),
        };
        assert!(Rc::ptr_eq(&a, &b));
        assert!(Rc::ptr_eq(&b, &c));
    }

    #[test]
    fn two_equal_lists_stay_two_lists() {
        let back = round_trip(&[int_list(&[1]), int_list(&[1])]);
        match (&back[0], &back[1]) {
            (Value::List(a), Value::List(b)) => assert!(!Rc::ptr_eq(a, b)),
            _ => unreachable!(),
        }
    }

    #[test]
    fn a_list_that_contains_itself_comes_back_containing_itself() {
        let cell = Rc::new(RefCell::new(Vec::new()));
        let l = Value::List(cell.clone());
        cell.borrow_mut().push(Value::Int(1));
        cell.borrow_mut().push(l.clone());
        let back = round_trip(&[l]).pop().unwrap();
        let Value::List(outer) = &back else { panic!("not a list") };
        let inner = match &outer.borrow()[1] {
            Value::List(rc) => rc.clone(),
            _ => panic!("second item should be the list itself"),
        };
        assert!(Rc::ptr_eq(outer, &inner));
        // Break the cycle so the test does not leak.
        cell.borrow_mut().clear();
        outer.borrow_mut().clear();
    }

    fn struct_value(name: &str, fields: Vec<(&str, Value)>) -> Value {
        let order: Vec<String> = fields.iter().map(|(k, _)| k.to_string()).collect();
        Value::Struct {
            type_name:          name.to_string(),
            fields:             Rc::new(RefCell::new(fields.into_iter().map(|(k, v)| (k.to_string(), v)).collect())),
            derives_partial_eq: true,
            derives_ord:        false,
            derives_hash:       true,
            derives_clone:      false,
            field_order:        Rc::new(order),
        }
    }

    #[test]
    fn a_struct_keeps_its_name_fields_flags_and_field_order() {
        let s = struct_value("Point", vec![("x", Value::Int(1)), ("y", Value::Int(2))]);
        let back = round_trip(&[s]).pop().unwrap();
        match back {
            Value::Struct { type_name, fields, derives_partial_eq, derives_ord, derives_hash, derives_clone, field_order } => {
                assert_eq!(type_name, "Point");
                assert_eq!(fields.borrow().len(), 2);
                assert!(matches!(fields.borrow()["y"], Value::Int(2)));
                assert!(derives_partial_eq && !derives_ord && derives_hash && !derives_clone);
                assert_eq!(field_order.as_ref(), &vec!["x".to_string(), "y".to_string()]);
            }
            _ => panic!("not a struct"),
        }
    }

    #[test]
    fn a_struct_that_holds_itself_comes_back_holding_itself() {
        let s = struct_value("Node", vec![("n", Value::Int(1))]);
        if let Value::Struct { fields, .. } = &s {
            fields.borrow_mut().insert("me".to_string(), s.clone());
        }
        let back = round_trip(&[s.clone()]).pop().unwrap();
        let Value::Struct { fields, .. } = &back else { panic!("not a struct") };
        let me = fields.borrow()["me"].clone();
        let Value::Struct { fields: inner, .. } = &me else { panic!("not a struct") };
        assert!(Rc::ptr_eq(fields, inner));
        fields.borrow_mut().clear();
        if let Value::Struct { fields, .. } = &s { fields.borrow_mut().clear(); }
    }

    #[test]
    fn two_equal_structs_encode_the_same_way() {
        // Fields live in a `HashMap`, whose order differs from one map to the next. The copy lists
        // them by name, so equal structs give equal encodings (eight fields: the chance that two
        // maps agree by luck is one in forty thousand).
        let make = || struct_value("S", ["h", "b", "g", "d", "a", "f", "c", "e"]
            .iter().map(|n| (*n, Value::Int(n.len() as i64))).collect());
        let a = encode(&[make()], 0).unwrap();
        let b = encode(&[make()], 0).unwrap();
        assert_eq!(format!("{a:?}"), format!("{b:?}"));
    }

    #[test]
    fn enums_come_back_with_every_kind_of_payload() {
        let none = Value::Enum { type_name: "E".into(), variant: "A".into(), payload: Box::new(EnumPayload::None) };
        let tuple = Value::Enum {
            type_name: "E".into(), variant: "B".into(),
            payload: Box::new(EnumPayload::Tuple(vec![Value::Int(1), int_list(&[2])])),
        };
        let mut map = HashMap::new();
        map.insert("w".to_string(), Value::Int(3));
        let strukt = Value::Enum { type_name: "E".into(), variant: "C".into(), payload: Box::new(EnumPayload::Struct(map)) };
        let back = round_trip(&[none, tuple, strukt]);
        assert!(matches!(&back[0], Value::Enum { variant, payload, .. } if variant == "A" && matches!(**payload, EnumPayload::None)));
        match &back[1] {
            Value::Enum { payload, .. } => match payload.as_ref() {
                EnumPayload::Tuple(items) => {
                    assert!(matches!(items[0], Value::Int(1)));
                    assert_eq!(ints(&items[1]), vec![2]);
                }
                _ => panic!("tuple payload expected"),
            },
            _ => panic!("enum expected"),
        }
        match &back[2] {
            Value::Enum { payload, .. } => match payload.as_ref() {
                EnumPayload::Struct(m) => assert!(matches!(m["w"], Value::Int(3))),
                _ => panic!("struct payload expected"),
            },
            _ => panic!("enum expected"),
        }
    }

    #[test]
    fn dictionaries_queues_stacks_and_inline_lists_come_back() {
        let dict = Value::Dict(Rc::new(RefCell::new(vec![(Value::Int(1), Value::Int(10)), (Value::Int(2), Value::Int(20))])));
        let queue = Value::Queue(Rc::new(RefCell::new(VecDeque::from(vec![Value::Int(5), Value::Int(6)]))));
        let stack = Value::Stack(Rc::new(RefCell::new(vec![Value::Int(7), Value::Int(8)])));
        let inline = Value::InlineList(Rc::new(RefCell::new(InlineListData { items: vec![Value::Int(9)], capacity: 4 })));
        let back = round_trip(&[dict, queue, stack, inline]);
        match &back[0] { Value::Dict(rc) => { let d = rc.borrow(); assert_eq!(d.len(), 2); assert!(matches!(d[1], (Value::Int(2), Value::Int(20)))); } _ => panic!("dict") }
        match &back[1] { Value::Queue(rc) => { let q = rc.borrow(); assert_eq!(q.len(), 2); assert!(matches!(q[0], Value::Int(5))); } _ => panic!("queue") }
        match &back[2] { Value::Stack(rc) => assert!(matches!(rc.borrow()[1], Value::Int(8))), _ => panic!("stack") }
        match &back[3] { Value::InlineList(rc) => { assert_eq!(rc.borrow().capacity, 4); assert_eq!(rc.borrow().items.len(), 1); } _ => panic!("inline list") }
    }

    #[test]
    fn a_unique_comes_back_as_a_unique() {
        let back = round_trip(&[Value::Unique(Box::new(int_list(&[1])))]).pop().unwrap();
        match back {
            Value::Unique(inner) => assert_eq!(ints(&inner), vec![1]),
            _ => panic!("not a unique"),
        }
    }

    #[test]
    fn a_function_of_the_program_can_be_sent_and_a_lambda_cannot() {
        let back = encode(&[Value::Function(3)], 10).unwrap().decode();
        assert!(matches!(back[0], Value::Function(3)));
        // The last declared function has an id one below the count; the first lambda is the count itself.
        assert!(encode(&[Value::Function(9)], 10).is_ok());
        assert_eq!(encode(&[Value::Function(10)], 10).unwrap_err(), "a closure cannot be sent to another task");
        assert_eq!(encode(&[list(vec![Value::Function(11)])], 10).unwrap_err(), "a closure cannot be sent to another task");
    }

    #[test]
    fn a_value_that_is_not_plain_data_says_what_it_is() {
        let shared = Value::Shared(Rc::new(RefCell::new(Value::Int(1))));
        assert_eq!(encode(&[shared.clone()], 0).unwrap_err(), "a Shared cannot be sent to another task");
        assert_eq!(encode(&[list(vec![shared])], 0).unwrap_err(), "a Shared cannot be sent to another task");
        let task = Value::Task(Rc::new(RefCell::new(super::super::value::TaskState::Awaited)));
        assert_eq!(encode(&[task], 0).unwrap_err(), "a task cannot be sent to another task");
        assert_eq!(encode(&[Value::Handle { index: 0, generation: 0 }], 0).unwrap_err(), "a pool handle cannot be sent to another task");
    }

    #[test]
    fn the_number_of_values_is_kept() {
        let sent = encode(&[Value::Int(1), Value::Int(2), Value::Int(3)], 0).unwrap();
        assert_eq!(sent.len(), 3);
        assert!(!sent.is_empty());
        assert!(encode(&[], 0).unwrap().is_empty());
    }
}
