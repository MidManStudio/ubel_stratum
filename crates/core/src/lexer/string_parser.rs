// ============================================================================
// NOTICE: Full documentation, design decisions, and fix history for this file
// live in docs/ubel_stratum.md, section "lexer/string_parser.rs"
// ============================================================================
//! String interpolation and verbatim string parsing

use crate::lexer::{Token, TokenType, Span, InterpolationPart, LogosLexer};
use crate::error_management::errors::{LexicalError, StringType};

pub struct StringParser<'a> {
    input: &'a str,
    position: usize,
    line: usize,
    column: usize,
}

impl<'a> StringParser<'a> {
    pub fn new(input: &'a str, start_pos: usize, line: usize, column: usize) -> Self {
        StringParser {
            input,
            position: start_pos,
            line,
            column,
        }
    }

    /// Parse interpolated string: $"Hello {name}!"
    pub fn parse_interpolated_string(&mut self) -> Result<(Token, usize, usize, usize), LexicalError> {
        let start_pos = self.position;
        let start_line = self.line;
        let start_column = self.column;

        // Skip $"
        self.position += 2;
        self.column += 2;

        let mut parts = Vec::new();
        let mut current_text = String::new();
        let mut depth = 0; // Track brace nesting in expressions

        while self.position < self.input.len() {
            let ch = self.char_at(self.position);

            match ch {
                '"' if depth == 0 => {
                    // End of string
                    if !current_text.is_empty() {
                        parts.push(InterpolationPart::Text(current_text.clone()));
                    }
                    self.position += 1;
                    self.column += 1;

                    let span = Span::new(start_pos, self.position, start_line, start_column);
                    let lexeme = &self.input[start_pos..self.position];

                    return Ok((
                        Token::new(TokenType::InterpolatedString(parts), span, lexeme.to_string()),
                        self.position,
                        self.line,
                        self.column,
                    ));
                }

                '{' if depth == 0 => {
                    // Start of interpolation
                    if !current_text.is_empty() {
                        parts.push(InterpolationPart::Text(current_text.clone()));
                        current_text.clear();
                    }

                    // Parse expression
                    let expr = self.parse_interpolation_expr()?;
                    parts.push(InterpolationPart::Expr(expr));
                }

                '{' if depth > 0 => {
                    // Nested brace inside expression
                    depth += 1;
                    current_text.push(ch);
                    self.position += 1;
                    self.column += 1;
                }

                '}' if depth > 0 => {
                    depth -= 1;
                    current_text.push(ch);
                    self.position += 1;
                    self.column += 1;
                }

                '\\' => {
                    // Escape sequence
                    self.position += 1;
                    self.column += 1;

                    if self.position < self.input.len() {
                        let escaped = self.char_at(self.position);
                        current_text.push(match escaped {
                            'n' => '\n',
                            't' => '\t',
                            'r' => '\r',
                            '\\' => '\\',
                            '"' => '"',
                            '{' => '{',
                            '}' => '}',
                            _ => {
                                // Invalid escape
                                return Err(LexicalError::InvalidEscape {
                                    sequence: format!("\\{}", escaped),
                                    span: Span::new(
                                        self.position - 1,
                                        self.position + 1,
                                        self.line,
                                        self.column - 1,
                                    ),
                                    valid_escapes: vec![
                                        "\\n".to_string(),
                                        "\\t".to_string(),
                                        "\\r".to_string(),
                                        "\\\\".to_string(),
                                        "\\\"".to_string(),
                                        "\\{".to_string(),
                                        "\\}".to_string(),
                                    ],
                                });
                            }
                        });
                        self.position += 1;
                        self.column += 1;
                    }
                }

                '\n' => {
                    current_text.push(ch);
                    self.position += 1;
                    self.line += 1;
                    self.column = 1;
                }

                _ => {
                    current_text.push(ch);
                    self.position += ch.len_utf8();
                    self.column += 1;
                }
            }
        }

        // Unterminated string
        Err(LexicalError::UnterminatedString {
            span: Span::new(start_pos, self.position, start_line, start_column),
            string_type: StringType::Interpolated,
        })
    }

    /// Parse expression inside { }
    ///
    /// Finds the hole's closing brace by driving a real `LogosLexer` over
    /// the remainder of the file, one token at a time, and tracking
    /// brace depth using genuine `LeftBrace`/`RightBrace` TOKENS rather
    /// than raw bytes. This is what makes it safe for a hole to contain
    /// a nested string, char literal, or comment with an unbalanced `{`
    /// or `}` inside it (e.g. `{"a { b"}`, `{x == '{'}`, `{x /* a { */ }`):
    /// anything already living inside one of those is consumed
    /// atomically by `LogosLexer`'s own string/comment sub-parsing (the
    /// same dispatch used everywhere else, including recursively for a
    /// nested `$"..."`), so it can never surface as a stray brace
    /// character the way the old byte-counting scan saw it. A nested
    /// sub-`LogosLexer` only ever scans as far as the matching close (or
    /// true end of file if the hole is genuinely unclosed); it does not
    /// eagerly tokenize the rest of the file up front.
    fn parse_interpolation_expr(&mut self) -> Result<Vec<Token>, LexicalError> {
        // Skip {
        self.position += 1;
        self.column += 1;

        let expr_start = self.position;
        let expr_start_line = self.line;
        let expr_start_column = self.column;

        let mut sub_lexer = LogosLexer::new(&self.input[expr_start..]);
        let mut depth: i32 = 1; // already inside the opening '{'
        let mut hole_tokens: Vec<Token> = Vec::new();
        // (byte offset of the closing '}' relative to expr_start, its
        // absolute end line, its absolute end column)
        let mut closed_at: Option<(usize, usize, usize)> = None;

        while let Some(mut tok) = sub_lexer.next_token() {
            // Tokens come back with spans relative to the hole's own
            // slice (starting at byte 0, line 1). Rebase them to be
            // relative to the whole file, same offsetting this function
            // already did for the old bulk re-tokenize call.
            tok.span.start += expr_start;
            tok.span.end += expr_start;
            if tok.span.line == 1 {
                tok.span.column += expr_start_column - 1;
            }
            tok.span.line += expr_start_line - 1;

            if tok.kind == TokenType::LeftBrace {
                depth += 1;
                hole_tokens.push(tok);
            } else if tok.kind == TokenType::RightBrace {
                depth -= 1;
                if depth == 0 {
                    // This brace closes OUR hole, not a nested one: don't
                    // include it in the hole's own tokens, and remember
                    // where to resume outer scanning from. It's exactly
                    // one character wide, so its own end position is
                    // simply one column past where it started.
                    let rel_end = tok.span.end - expr_start;
                    closed_at = Some((rel_end, tok.span.line, tok.span.column + 1));
                    break;
                }
                hole_tokens.push(tok);
            } else {
                hole_tokens.push(tok);
            }
        }

        let sub_errors = sub_lexer.take_lexical_errors();

        let (rel_end, end_line, end_column) = match closed_at {
            Some(v) => v,
            None => {
                // The sub-lexer ran off the true end of the file without
                // depth ever returning to 0 - a genuinely unclosed hole.
                return Err(LexicalError::InvalidInterpolation {
                    message: "Unclosed interpolation expression".to_string(),
                    span: Span::new(self.input.len(), self.input.len(), self.line, self.column),
                    suggestion: Some("Add closing }".to_string()),
                });
            }
        };

        if let Some(first_err) = sub_errors.into_iter().next() {
            return Err(LexicalError::InvalidInterpolation {
                message: format!("invalid expression in interpolation hole: {}", first_err.message()),
                span: Span::new(expr_start, expr_start + rel_end, expr_start_line, expr_start_column),
                suggestion: None,
            });
        }

        self.position = expr_start + rel_end;
        self.line = end_line;
        self.column = end_column;

        // `rd_parser::Cursor` clamps its index access on the assumption
        // that every token slice it's handed ends with `Eof` (see its own
        // `peek_token` doc comment): a hole's tokens get fed straight
        // into a fresh `Cursor` at the parser level (parse_expr.rs's
        // `parse_interp`), so this isn't optional bookkeeping, it's a
        // real invariant a downstream consumer relies on. `next_token`
        // itself never yields one (only `tokenize`'s own wrapper does,
        // deliberately, since `next_token` is the shared primitive both
        // paths pull from), so it has to be added here explicitly.
        hole_tokens.push(Token::new(
            TokenType::Eof,
            Span::new(self.position, self.position, self.line, self.column),
            String::new(),
        ));

        Ok(hole_tokens)
    }

    /// Parse verbatim string: @"C:\path\to\file"
    pub fn parse_verbatim_string(&mut self) -> Result<(Token, usize, usize, usize), LexicalError> {
        let start_pos = self.position;
        let start_line = self.line;
        let start_column = self.column;

        // Skip @"
        self.position += 2;
        self.column += 2;

        let mut content = String::new();

        while self.position < self.input.len() {
            let ch = self.char_at(self.position);

            match ch {
                '"' => {
                    // Check for doubled quote ""
                    if self.position + 1 < self.input.len()
                        && self.char_at(self.position + 1) == '"' {
                        // Escaped quote
                        content.push('"');
                        self.position += 2;
                        self.column += 2;
                    } else {
                        // End of string
                        self.position += 1;
                        self.column += 1;

                        let span = Span::new(start_pos, self.position, start_line, start_column);
                        let lexeme = &self.input[start_pos..self.position];

                        return Ok((
                            Token::new(TokenType::VerbatimString(content), span, lexeme.to_string()),
                            self.position,
                            self.line,
                            self.column,
                        ));
                    }
                }

                '\n' => {
                    content.push(ch);
                    self.position += 1;
                    self.line += 1;
                    self.column = 1;
                }

                _ => {
                    content.push(ch);
                    self.position += ch.len_utf8();
                    self.column += 1;
                }
            }
        }

        // Unterminated string
        Err(LexicalError::UnterminatedString {
            span: Span::new(start_pos, self.position, start_line, start_column),
            string_type: StringType::Verbatim,
        })
    }

    /// Parse interpolated verbatim string: $@"C:\path\{file}"
    pub fn parse_interpolated_verbatim_string(&mut self) -> Result<(Token, usize, usize, usize), LexicalError> {
        let start_pos = self.position;
        let start_line = self.line;
        let start_column = self.column;

        // Skip $@"
        self.position += 3;
        self.column += 3;

        let mut parts = Vec::new();
        let mut current_text = String::new();

        while self.position < self.input.len() {
            let ch = self.char_at(self.position);

            match ch {
                '"' => {
                    // Check for doubled quote
                    if self.position + 1 < self.input.len()
                        && self.char_at(self.position + 1) == '"' {
                        // Escaped quote
                        current_text.push('"');
                        self.position += 2;
                        self.column += 2;
                    } else {
                        // End of string
                        if !current_text.is_empty() {
                            parts.push(InterpolationPart::Text(current_text));
                        }
                        self.position += 1;
                        self.column += 1;

                        let span = Span::new(start_pos, self.position, start_line, start_column);
                        let lexeme = &self.input[start_pos..self.position];

                        return Ok((
                            Token::new(TokenType::InterpolatedString(parts), span, lexeme.to_string()),
                            self.position,
                            self.line,
                            self.column,
                        ));
                    }
                }

                '{' => {
                    // Start of interpolation
                    if !current_text.is_empty() {
                        parts.push(InterpolationPart::Text(current_text.clone()));
                        current_text.clear();
                    }

                    let expr = self.parse_interpolation_expr()?;
                    parts.push(InterpolationPart::Expr(expr));
                }

                '\n' => {
                    current_text.push(ch);
                    self.position += 1;
                    self.line += 1;
                    self.column = 1;
                }

                _ => {
                    current_text.push(ch);
                    self.position += ch.len_utf8();
                    self.column += 1;
                }
            }
        }

        Err(LexicalError::UnterminatedString {
            span: Span::new(start_pos, self.position, start_line, start_column),
            string_type: StringType::InterpolatedVerbatim,
        })
    }

    #[inline]
    fn char_at(&self, pos: usize) -> char {
        self.input[pos..].chars().next().unwrap_or('\0')
    }
                        }
