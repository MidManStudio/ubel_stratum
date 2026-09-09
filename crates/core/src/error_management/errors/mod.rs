// src/error_management/errors/mod.rs
//!
//! Replaces the old flat `error_types/` folder — see
//! docs/DIAGNOSTICS_RULES.md §9. Organized by phase, same as before
//! (`lexical` = lex phase, `parse` = parse phase, `naming` = name
//! resolution, `types` = ordinary type checking); `tier` is new here,
//! physically split out of what used to be `types::TypeError`'s
//! TYPE-2xx range.

pub mod lexical;
pub mod parse;
pub mod naming;
pub mod types;
pub mod tier;
pub mod borrow;
pub mod move_check;
pub mod lifetime;

pub use lexical::{LexicalError, StringType};
pub use parse::{ParseContext, ParseError};
pub use naming::NameError;
pub use types::TypeError;
pub use tier::TierError;
pub use borrow::BorrowError;
pub use move_check::MoveError;
pub use lifetime::LifetimeError;
