// crates/rd_parser/src/cursor.rs
//! Token cursor — the backbone of the recursive-descent parser.
//!
//! `Cursor<'tok>` is a read-only view over a pre-tokenised `&'tok [Token]`
//! slice.  It never owns the token data, never allocates, and can be cloned
//! cheaply (it's just a `(slice_ref, usize)` pair) for speculative parsing.
//!
//! # Invariant
//!
//! The token slice MUST end with at least one `TokenType::Eof` token.
//! `ubel_stratum::lexer::tokenize` always appends one.
//! This guarantees `peek()` and `peek_token()` never index out of bounds
//! (we clamp `pos` at `len - 1` which is always the `Eof`).

use ubel_stratum::lexer::{Span, Token, TokenType};

// ── Cursor ────────────────────────────────────────────────────────────────────

/// A zero-copy view into a token slice with arbitrary look-ahead.
#[derive(Clone)]
pub struct Cursor<'tok> {
    tokens: &'tok [Token],
    /// Index of the *next* token to be consumed.
    pos:    usize,
    /// Position of a `>>` token whose first `>` has already been consumed
    /// as the close of an inner generic argument list, or `NO_SPLIT`. See
    /// `eat_generic_close`.
    split_at: usize,
}

/// `Cursor::split_at` value meaning no `>>` token is half consumed.
const NO_SPLIT: usize = usize::MAX;

impl<'tok> Cursor<'tok> {
    /// Create a cursor at the beginning of `tokens`.
    ///
    /// # Panics
    ///
    /// Panics in debug builds if `tokens` is empty (no `Eof` sentinel).
    pub fn new(tokens: &'tok [Token]) -> Self {
        debug_assert!(!tokens.is_empty(), "token slice must contain at least an Eof sentinel");
        Cursor { tokens, pos: 0, split_at: NO_SPLIT }
    }

    // ── Peeking (non-mutating) ────────────────────────────────────────────────

    /// The `TokenType` of the current (not-yet-consumed) token.
    ///
    /// Returns `&TokenType::Eof` if the cursor is at or past the end.
    #[inline]
    pub fn peek(&self) -> &TokenType {
        &self.peek_token().kind
    }

    /// The full `Token` at the current position (including span + lexeme).
    #[inline]
    pub fn peek_token(&self) -> &'tok Token {
        // Clamped access — `tokens` always ends with `Eof`, so this is safe.
        &self.tokens[self.pos.min(self.tokens.len() - 1)]
    }

    /// Look `n` tokens ahead without consuming anything.
    ///
    /// `peek_nth(0)` is the same as `peek()`.
    /// Returns `&TokenType::Eof` if the look-ahead exceeds the slice.
    #[inline]
    pub fn peek_nth(&self, n: usize) -> &TokenType {
        let idx = (self.pos + n).min(self.tokens.len() - 1);
        &self.tokens[idx].kind
    }

    /// The full `Token` `n` positions ahead.
    #[inline]
    pub fn peek_token_nth(&self, n: usize) -> &'tok Token {
        let idx = (self.pos + n).min(self.tokens.len() - 1);
        &self.tokens[idx]
    }

    // ── Querying ──────────────────────────────────────────────────────────────

    /// Returns `true` if the current token's kind equals `tt`.
    #[inline]
    pub fn is_at(&self, tt: &TokenType) -> bool {
        self.peek() == tt
    }

    /// Returns `true` if the cursor has reached (or passed) `TokenType::Eof`.
    #[inline]
    pub fn is_eof(&self) -> bool {
        matches!(self.peek(), TokenType::Eof)
    }

    /// The source `Span` of the current token (useful for error reporting).
    #[inline]
    pub fn current_span(&self) -> Span {
        self.peek_token().span
    }

    /// Raw position index — useful for save/restore during speculative parsing.
    #[inline]
    pub fn position(&self) -> usize {
        self.pos
    }

    // ── Advancing (mutating) ──────────────────────────────────────────────────

    /// Consume and return the current token.
    ///
    /// Advancing past `Eof` is a no-op: the cursor stays pinned at the last
    /// token (which is `Eof`) and returns it every time.
    #[inline]
    pub fn advance(&mut self) -> &'tok Token {
        let idx = self.pos.min(self.tokens.len() - 1);
        let tok = &self.tokens[idx];
        // Don't advance past the final Eof.
        if self.pos < self.tokens.len() - 1 {
            self.pos += 1;
        }
        tok
    }

    /// Consume the current token only if its kind matches `tt`.
    ///
    /// Returns `true` on a match (and advances), `false` otherwise (no-op).
    #[inline]
    pub fn eat(&mut self, tt: &TokenType) -> bool {
        if self.peek() == tt {
            self.pos = (self.pos + 1).min(self.tokens.len() - 1);
            true
        } else {
            false
        }
    }

    /// Consume and return the current token, or produce a `CursorError` if it
    /// does not match `expected`.
    ///
    /// The returned reference has the `'tok` lifetime, so it is safe to use
    /// after the cursor has advanced further.
    pub fn expect(&mut self, expected: &TokenType) -> Result<&'tok Token, CursorError> {
        if self.peek() == expected {
            Ok(self.advance())
        } else {
            let found = self.peek_token();
            if matches!(found.kind, TokenType::Eof) {
                Err(CursorError::UnexpectedEof {
                    expected: format!("{:?}", expected),
                    span:     found.span,
                })
            } else {
                Err(CursorError::UnexpectedToken {
                    expected: format!("{:?}", expected),
                    found:    found.kind.clone(),
                    span:     found.span,
                })
            }
        }
    }

    /// Restore the cursor to a saved position.
    ///
    /// Used after failed speculative parses:
    /// ```ignore
    /// let saved = cursor.position();
    /// if try_parse_something(&mut cursor).is_none() {
    ///     cursor.restore(saved);
    /// }
    /// ```
    #[inline]
    pub fn restore(&mut self, pos: usize) {
        self.pos = pos.min(self.tokens.len() - 1);
        // A restore can land before a `>>` that was half consumed on the
        // abandoned attempt; that half consumption must not leak into the
        // re-parse.
        self.split_at = NO_SPLIT;
    }

    // ── Generic-closing `>` (splits `>>`) ─────────────────────────────────────

    /// Is the current token a `>` that can close a generic argument list?
    ///
    /// True for `Greater`, and for `RightShift` (`>>`), which closes two
    /// nested lists at once (`List<List<int>>`) and is tokenized as one
    /// token because the lexer cannot know it is in a type position.
    #[inline]
    pub fn at_generic_close(&self) -> bool {
        matches!(self.peek(), TokenType::Greater | TokenType::RightShift)
    }

    /// Consume one `>` toward closing a generic argument list.
    ///
    /// A `Greater` token is consumed whole. A `RightShift` token is split:
    /// the first call marks its first `>` consumed and leaves the cursor on
    /// the token, the second call consumes the token. Returns `false`, and
    /// consumes nothing, when the current token is neither.
    #[inline]
    pub fn eat_generic_close(&mut self) -> bool {
        match self.peek() {
            TokenType::Greater => {
                self.advance();
                true
            }
            TokenType::RightShift => {
                if self.split_at == self.pos {
                    self.split_at = NO_SPLIT;
                    self.advance();
                } else {
                    self.split_at = self.pos;
                }
                true
            }
            _ => false,
        }
    }

    /// `eat_generic_close`, or the same `CursorError` `expect(&Greater)`
    /// would produce.
    pub fn expect_generic_close(&mut self) -> Result<(), CursorError> {
        if self.eat_generic_close() {
            Ok(())
        } else {
            self.expect(&TokenType::Greater).map(|_| ())
        }
    }

    // ── Error recovery (sync / skip) ──────────────────────────────────────────

    /// Skip tokens until the current token matches `tt` or we hit `Eof`.
    ///
    /// The matching token is **not** consumed — the caller decides whether to
    /// eat it or treat it as the next statement boundary.
    pub fn skip_until(&mut self, tt: &TokenType) {
        while !self.is_eof() && self.peek() != tt {
            self.pos += 1;
        }
    }

    /// Skip tokens until any token in `sync_set` is found, or we hit `Eof`.
    ///
    /// Useful for "panic-mode" recovery: skip to the next statement or
    /// declaration boundary.
    ///
    /// # Example sync sets
    ///
    /// - Statement boundary: `[Semicolon, RightBrace, Eof]`
    /// - Declaration boundary: `[Fn, Struct, Enum, Trait, Impl, Extend, Const, TypeKw, Eof]`
    pub fn skip_until_any(&mut self, sync_set: &[TokenType]) {
        'outer: while !self.is_eof() {
            let cur = self.peek();
            for tt in sync_set {
                if cur == tt {
                    break 'outer;
                }
            }
            self.pos += 1;
        }
    }

    /// Skip past the current token and any following tokens up to and including
    /// the matching closing delimiter.
    ///
    /// Tracks nesting depth so `skip_balanced('{', '}')` skips an entire block
    /// including nested blocks.  Stops (and returns `false`) if `Eof` is
    /// reached before the delimiter is closed.
    pub fn skip_balanced(&mut self, open: &TokenType, close: &TokenType) -> bool {
        let mut depth: usize = 0;
        loop {
            let cur = self.peek();
            if matches!(cur, TokenType::Eof) {
                return false; // unclosed — report error upstream
            }
            if cur == open  { depth += 1; }
            if cur == close {
                if depth == 0 {
                    self.advance(); // consume the final close token
                    return true;
                }
                depth -= 1;
            }
            self.advance();
        }
    }
}

// ── Cursor-level error ────────────────────────────────────────────────────────

/// A lightweight error produced by `Cursor::expect`.
///
/// The parser lifts these into `ubel_stratum::error_management::errors::ParseError`
/// via the helpers in `crate::error`.
#[derive(Debug, Clone)]
pub enum CursorError {
    UnexpectedToken {
        expected: String,
        found:    TokenType,
        span:     Span,
    },
    UnexpectedEof {
        expected: String,
        span:     Span,
    },
}

impl CursorError {
    pub fn span(&self) -> Span {
        match self {
            CursorError::UnexpectedToken { span, .. } => *span,
            CursorError::UnexpectedEof   { span, .. } => *span,
        }
    }
  }
