// crates/rd_parser/src/lib.rs

pub mod check;
pub mod cursor;
pub mod error;
pub mod keywords;
pub mod estimates;

pub(crate) mod parser;
pub(crate) mod parsers;

pub use check::{check_source, CheckReport, Stage};
pub use parser::Parser;

use ubel_stratum::{
    ast::{arena::AstArena, expressions::Expr, root::Program},
    error_management::ErrorManager,
    lexer::Token,
};

/// Parse a complete `.ubl` source file into a `Program<'ast>`.
pub fn parse<'ast>(
    arena:  &'ast AstArena,
    tokens: &[Token],
    source: String,
) -> Result<Program<'ast>, ErrorManager> {
    Parser::new(arena, tokens, source).parse_program()
}

/// Parse a single expression — used for string interpolation evaluation.
pub fn parse_expr_str<'ast>(
    arena:  &'ast AstArena,
    source: &str,
) -> Option<&'ast Expr<'ast>> {
    let tokens = ubel_stratum::lexer::tokenize(source).ok()?;
    Parser::new(arena, &tokens, source.to_string()).parse_single_expr()
}
