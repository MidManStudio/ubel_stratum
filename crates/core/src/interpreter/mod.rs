// src/interpreter/mod.rs
//! Tree-walking interpreter for Ubel Stratum.
//!
//! # Module layout
//!
//! | Module     | Contents                                               |
//! |------------|-------------------------------------------------------|
//! | `value`    | `Value` enum, `Signal`, `EvalResult`, `FunctionId`    |
//! | `env`      | `Environment` — lexical scope stack                   |
//! | `eval`     | `Interpreter` struct, expression/statement evaluation  |
//!
//! Native builtins (`println`, `sqrt`, instance methods, ...) live in the
//! top-level `crate::builtins` module, not here — they're shared with
//! `sema::name_resolution`, which needs to know about them too.
//!
//! # Value and lifetime design
//!
//! `Value` is free of AST lifetimes — functions are stored as `FunctionId`
//! indices into a table on `Interpreter<'ast>`. The table entries hold
//! `Block<'ast>` (which is `Copy`) so AST references are contained to the
//! `Interpreter` itself and don't infect `Value` or `Environment`.
//!
//! # Control flow
//!
//! Non-local exits (`return`, `break`, `continue`, `fail`) are signalled
//! as `Err(Signal)` bubbling up the call stack, caught at the right boundary
//! in `eval/stmt.rs` and `eval/mod.rs`.
//!
//! # Tier model in the tree-walker
//!
//! HIGH: default — heap allocation via `Rc`.
//! MID: `with arena(…)` blocks are marker scopes (tier_check already
//!      validated them); values still use `Rc` in the interpreter.
//!      Real bump-allocation lands with the LLVM backend.
//! LOW: called like any other function; borrow-check enforcement is Phase 4.

pub mod value;
pub mod send;
pub mod pool;
pub mod executor;
pub mod env;
pub mod eval;

pub use value::{EnumPayload, EvalResult, FunctionId, Signal, Value};
pub use env::Environment;
pub use eval::Interpreter;
pub use executor::{DefaultExecutor, Executor, Lend};
