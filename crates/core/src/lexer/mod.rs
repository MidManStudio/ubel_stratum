//! Lexer module - Tokenization

pub mod token;
pub mod keywords;
pub mod logos_lexer;
pub mod string_parser;
pub mod comment_parser;

pub use token::{Token, TokenType, Span, InterpolationPart, IntSuffix};
pub use logos_lexer::LogosLexer;

/// Main tokenization entry point
pub fn tokenize(input: &str) -> Result<Vec<Token>, crate::error_management::ErrorManager> {
    LogosLexer::new(input).tokenize()
}

#[cfg(test)]
mod tests;