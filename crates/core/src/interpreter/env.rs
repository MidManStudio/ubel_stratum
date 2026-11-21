// src/interpreter/env.rs
//! Lexical environment — a stack of scopes.

#![allow(dead_code)]

use std::collections::HashMap;
use crate::interpreter::value::Value;

/// A stack of lexical scopes. The last element is the innermost scope.
///
/// Closures call `snapshot()` at definition time to capture the current
/// environment. Since `Value` heap types are `Rc`-wrapped, the snapshot
/// shares mutable state with the parent — mutations to a captured `List`
/// are visible through all references, which is the correct HIGH-tier
/// semantics.
#[derive(Debug, Clone, Default)]
pub struct Environment {
    scopes: Vec<HashMap<String, Value>>,
}

impl Environment {
    /// Create a fresh environment with one global scope.
    pub fn new() -> Self {
        let mut env = Environment { scopes: Vec::new() };
        env.push();
        env
    }

    /// Enter a new scope.
    pub fn push(&mut self) {
        self.scopes.push(HashMap::new());
    }

    /// Exit the current scope.
    pub fn pop(&mut self) {
        self.scopes.pop();
    }

    /// Define a name in the current (innermost) scope.
    /// Shadows any outer binding with the same name.
    pub fn define(&mut self, name: &str, value: Value) {
        if let Some(scope) = self.scopes.last_mut() {
            scope.insert(name.to_string(), value);
        }
    }

    /// Look up a name, walking outward from the innermost scope.
    /// A binding made inside a function: the innermost one with this name in any scope but the
    /// first, which holds the program's top-level names.
    pub(crate) fn local_get(&self, name: &str) -> Option<&Value> {
        for scope in self.scopes.iter().skip(1).rev() {
            if let Some(v) = scope.get(name) {
                return Some(v);
            }
        }
        None
    }

    pub fn get(&self, name: &str) -> Option<&Value> {
        for scope in self.scopes.iter().rev() {
            if let Some(v) = scope.get(name) {
                return Some(v);
            }
        }
        None
    }

    /// Assign to an existing binding (walks outward to find it).
    /// Returns `false` if the name doesn't exist in any scope.
    /// Does NOT create a new binding — use `define` for that.
    pub fn set(&mut self, name: &str, value: Value) -> bool {
        for scope in self.scopes.iter_mut().rev() {
            if scope.contains_key(name) {
                scope.insert(name.to_string(), value);
                return true;
            }
        }
        false
    }

    /// Clone the environment to capture a closure snapshot.
    /// O(n) in the number of live bindings. Rc-wrapped heap values
    /// are shared (not deep-copied) so mutations remain visible.
    pub fn snapshot(&self) -> Self {
        self.clone()
    }

    /// Number of currently active scopes. Useful for debugging.
    pub fn depth(&self) -> usize {
        self.scopes.len()
    }
          }
