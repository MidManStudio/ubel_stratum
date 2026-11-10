// src/lib.rs
// Core library — lexer, AST, semantic analysis, interpreter.
// Parser lives in the `ubel_stratum_parser` workspace crate so that
// `cargo test -p ubel_stratum` never triggers lalrpop compilation.

pub mod lexer;
pub mod error_management;
pub mod ast;
pub mod builtins;
pub mod sema;
pub mod interpreter;
pub mod stack;

pub use lexer::{Token, TokenType, tokenize};
