// ============================================================================
// NOTICE: Full documentation, design decisions, and fix history for this file
// live in docs/ubel_stratum.md, section "lexer/logos_lexer.rs"
// ============================================================================
// src/lexer/logos_lexer.rs

use logos::Logos;
use crate::lexer::{Token, TokenType, Span, IntSuffix};
use crate::error_management::{ErrorManager, errors::LexicalError};
use crate::lexer::{keywords, string_parser::StringParser, comment_parser::CommentParser};

#[derive(Logos, Debug, Clone, PartialEq)]
enum LogosToken {
    // ── Whitespace ───────────────────────────────────────────────
    // NOT a `#[logos(skip ...)]` directive. A `skip` match is consumed
    // by the lexer generator with no callback at all, so `update_position`
    // never runs for it — every token's *reported* column then silently
    // undercounts by however many spaces/tabs were skipped since the
    // last real token, compounding across a whole line. A single-line
    // fixture with one level of indentation could be off by 6+ columns
    // before this fix (see docs/DIAGNOSTICS_RULES.md, "Known limitation:
    // pre-fix column drift" for the worked example). Handling it as an
    // ordinary regex token — discarded in `handle_logos_token` right
    // alongside `Newline`/`LineComment`, but still run through
    // `update_position` first — keeps `self.column` honest.
    #[regex(r"[ \t]+")] Whitespace,

    // ── Keywords ─────────────────────────────────────────────────
    #[token("fn")]       Fn,
    #[token("let")]      Let,
    #[token("mut")]      Mut,
    #[token("const")]    Const,
    #[token("if")]       If,
    #[token("elif")]     Elif,
    #[token("else")]     Else,
    #[token("match")]    Match,
    #[token("where")]    Where,
    #[token("for")]      For,
    #[token("in")]       In,
    #[token("while")]    While,
    #[token("loop")]     Loop,
    #[token("break")]    Break,
    #[token("continue")] Continue,
    #[token("return")]   Return,
    #[token("summon")]   Summon,
    #[token("from")]     From,
    #[token("as")]       As,
    #[token("package")]  Package,
    #[token("async")]    Async,
    #[token("await")]    Await,
    #[token("Task")]     Task,
    #[token("try")]      Try,
    #[token("catch")]    Catch,
    #[token("fail")]     Fail,
    #[token("struct")]   Struct,
    #[token("enum")]     Enum,
    #[token("trait")]    Trait,
    #[token("impl")]     Impl,
    #[token("pub")]      Pub,
    #[token("edge")]     Edge,
    #[token("unsafe")]   Unsafe,
    #[token("with")]     With,
    #[token("defer")]    Defer,
    #[token("and")]      And,
    #[token("or")]       Or,
    #[token("not")]      Not,
    #[token("true")]     True,
    #[token("false")]    False,
    #[token("null")]     Null,
    #[token("self")]     SelfKw,
    #[token("getter")]   Getter,
    #[token("setter")]   Setter,
    #[token("ref")]      Ref,
    #[token("deref")]    Deref,

    // ── Declaration / statement keywords ─────────────────────────
    #[token("extend")]   Extend,
    #[token("type")]     TypeKw,
    #[token("extract")]  Extract,
    #[token("using")]    Using,
    #[token("lifetime")] Lifetime,
    #[token("tier")]     Tier,
    #[token("high")]     High,
    #[token("mid")]      Mid,
    #[token("low")]      Low,
    #[token("arena")]    Arena,
    #[token("pool")]     Pool,
    #[token("gc")]       Gc,
    #[token("heap")]     Heap,

    // ── Built-in collection type keywords ─────────────────────────
    #[token("List")]       KwList,
    #[token("Dictionary")] KwDictionary,
    #[token("Set")]        KwSet,
    #[token("Queue")]      KwQueue,
    #[token("Stack")]      KwStack,
    #[token("InlineList")] KwInlineList,

    // ── Wildcard / infer: must appear BEFORE the Ident regex so
    //    a bare `_` is tokenised as Underscore, not Ident("_").
    #[token("_", priority = 4)] Underscore,

    // ── Operators — ORDER MATTERS: longer tokens first ────────────
    #[token("<<=")] LeftShiftEqual,
    #[token(">>=")] RightShiftEqual,
    #[token("|>")]  PipeArrow,
    #[token("<<")] LeftShift,
    #[token(">>")] RightShift,
    #[token("==")] EqualEqual,
    #[token("!=")] BangEqual,
    #[token("<=")] LessEqual,
    #[token(">=")] GreaterEqual,
    #[token("&&")] AmpAmp,
    #[token("||")] PipePipe,
    #[token("?.")] QuestionDot,
    #[token("=>")] FatArrow,
    #[token(":=")] ColonEqual,
    #[token("+=")] PlusEqual,
    #[token("-=")] MinusEqual,
    #[token("*=")] StarEqual,
    #[token("/=")] SlashEqual,
    #[token("%=")] PercentEqual,
    #[token("&=")] AmpEqual,
    #[token("|=")] PipeEqual,
    #[token("^=")] CaretEqual,

    #[token("..=")] DotDotEqual,
    #[token("...")] DotDotDot,
    #[token("..")] DotDot,

    #[token("+")] Plus,
    #[token("-")] Minus,
    #[token("*")] Star,
    #[token("/")] Slash,
    #[token("%")] Percent,
    #[token("&")] Amp,
    #[token("|")] Pipe,
    #[token("^")] Caret,
    #[token("~")] Tilde,
    #[token("<")] Less,
    #[token(">")] Greater,
    #[token("!")] Bang,
    #[token("?")] Question,
    #[token("=")] Equal,

    // ── Delimiters ────────────────────────────────────────────────
    #[token("(")] LeftParen,
    #[token(")")] RightParen,
    #[token("{")] LeftBrace,
    #[token("}")] RightBrace,
    #[token("[")] LeftBracket,
    #[token("]")] RightBracket,
    #[token(",")] Comma,
    #[token(".")] Dot,
    #[token(":")] Colon,
    #[token(";")] Semicolon,
    #[token("@")] At,
    #[token("#")] Hash,

    // ── Literals ──────────────────────────────────────────────────
    // Optional trailing Rust-style width suffix (`255u8`, `5000i64`, …)
    // folded straight into these three regexes rather than a separate
    // token, same approach `FloatLit`'s trailing `f`/`F` already uses
    // just below. Parsed and returned as `u64` (not `i64`) so the digit
    // part alone can hold the full unsigned range -- needed both for an
    // explicit `u64` suffix and for a bare, unsuffixed literal in the
    // top half of that range (auto-promoted to an implicit `u64`, see
    // `handle_logos_token` below; there is no other way for such a value
    // to exist as a literal at all). Suffix text itself is re-read from
    // the raw lexeme in `handle_logos_token`, exactly like the float
    // suffix is, rather than threaded back out of these callbacks.
    #[regex(r"[0-9][0-9_]*(u8|i8|u16|i16|u32|i32|u64|i64|usize|isize)?", parse_decimal)]
    #[regex(r"0x[0-9a-fA-F][0-9a-fA-F_]*(u8|i8|u16|i16|u32|i32|u64|i64|usize|isize)?", parse_hex)]
    #[regex(r"0b[01][01_]*(u8|i8|u16|i16|u32|i32|u64|i64|usize|isize)?", parse_binary)]
    IntLit(u64),

    #[regex(r"[0-9][0-9_]*\.[0-9_]*[fF]?", parse_float)]
    #[regex(r"[0-9][0-9_]*\.[0-9_]*[eE][+-]?[0-9][0-9_]*[fF]?", parse_float)]
    #[regex(r"[0-9][0-9_]*[eE][+-]?[0-9][0-9_]*[fF]?", parse_float)]
    FloatLit(f64),

    #[regex(r#""([^"\\]|\\["\\nrt])*""#, parse_simple_string)]
    StringLit(String),

    #[regex(r"'([^'\\]|\\['\\nrt])'", parse_char_literal)]
    CharLit(char),

    // Ident comes AFTER the bare `_` token so `_` alone is Underscore.
    #[regex(r"[a-zA-Z_][a-zA-Z0-9_]*", priority = 3)]
    Ident,

    // ── Hand-written parser triggers ──────────────────────────────
    #[regex(r#"\$@""#)] InterpolatedVerbatimStart,
    #[regex(r#"\$""#)]  InterpolatedStringStart,
    #[regex(r#"@""#)]   VerbatimStringStart,

    #[regex(r"//[^\n]*")]   LineComment,
    #[regex(r"/\*\*")]      DocCommentStar,
    #[regex(r"/\*!")]       DocCommentBang,
    #[regex(r"/\*")]        BlockCommentStart,

    #[regex(r"\n")] Newline,
}

// ── Parse helpers ─────────────────────────────────────────────────

/// Known Rust-style integer literal suffixes, longest-first purely out of
/// habit -- none of these is actually a suffix of another (`u8` never
/// collides with `usize`, etc.), so strip order doesn't matter for
/// correctness, only for readability.
const INT_SUFFIXES: &[&str] = &[
    "usize", "isize",
    "u8", "i8", "u16", "i16", "u32", "i32", "u64", "i64",
];

/// Strip a trailing integer-literal suffix off a numeric lexeme's digit
/// text, if one is present. `handle_logos_token` re-detects *which*
/// suffix it was (if any) straight from the raw lexeme afterward, the
/// same two-step split `FloatLit`'s `f`/`F` suffix already uses below --
/// this only needs to know where the digits end.
/// The inverse of `strip_int_suffix`: which suffix (if any) a full
/// lexeme -- including any `0x`/`0b` prefix, digits, and underscores --
/// ends with. Used once per int literal in `handle_logos_token`, on the
/// raw lexeme rather than inside the `logos` callback, same split
/// `FloatLit`'s `f`/`F` detection already uses.
fn int_suffix_of(lexeme: &str) -> Option<IntSuffix> {
    // Longest-first: "usize"/"isize" must be checked before nothing
    // shorter could ever falsely match them (they don't share a suffix
    // with any entry below, but checking wide-to-narrow is the safer
    // habit regardless of that).
    if lexeme.ends_with("usize") { return Some(IntSuffix::Usize); }
    if lexeme.ends_with("isize") { return Some(IntSuffix::Isize); }
    if lexeme.ends_with("u8")  { return Some(IntSuffix::U8); }
    if lexeme.ends_with("i8")  { return Some(IntSuffix::I8); }
    if lexeme.ends_with("u16") { return Some(IntSuffix::U16); }
    if lexeme.ends_with("i16") { return Some(IntSuffix::I16); }
    if lexeme.ends_with("u32") { return Some(IntSuffix::U32); }
    if lexeme.ends_with("i32") { return Some(IntSuffix::I32); }
    if lexeme.ends_with("u64") { return Some(IntSuffix::U64); }
    if lexeme.ends_with("i64") { return Some(IntSuffix::I64); }
    None
}

fn strip_int_suffix(slice: &str) -> &str {
    for suf in INT_SUFFIXES {
        if let Some(stripped) = slice.strip_suffix(suf) {
            return stripped;
        }
    }
    slice
}

// Digit text is parsed as `u64`, not `i64`: wide enough to hold the full
// unsigned range, needed both for an explicit `u64`/`usize` suffix and for
// a bare, unsuffixed literal past `i64::MAX` (auto-promoted to an implicit
// `u64` in `handle_logos_token` below -- there's no other way such a value
// could ever be written as a literal). `LEX-001` on overflow still fires
// exactly as before, just at the wider `u64::MAX` ceiling instead of
// `i64::MAX` -- a real range increase, not merely a type change: `parse`
// still returns `None` (and this stays a lex error) past `u64::MAX`.
fn parse_decimal(lex: &mut logos::Lexer<LogosToken>) -> Option<u64> {
    strip_int_suffix(lex.slice()).replace('_', "").parse().ok()
}

fn parse_hex(lex: &mut logos::Lexer<LogosToken>) -> Option<u64> {
    u64::from_str_radix(&strip_int_suffix(&lex.slice()[2..]).replace('_', ""), 16).ok()
}

fn parse_binary(lex: &mut logos::Lexer<LogosToken>) -> Option<u64> {
    u64::from_str_radix(&strip_int_suffix(&lex.slice()[2..]).replace('_', ""), 2).ok()
}

fn parse_float(lex: &mut logos::Lexer<LogosToken>) -> Option<f64> {
    let binding = lex.slice().replace('_', "");
    let cleaned = binding.trim_end_matches('f').trim_end_matches('F');
    cleaned.parse().ok()
}

fn parse_simple_string(lex: &mut logos::Lexer<LogosToken>) -> Option<String> {
    let slice = lex.slice();
    let content = &slice[1..slice.len() - 1];
    let mut result = String::new();
    let mut chars = content.chars();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            match chars.next() {
                Some('n')  => result.push('\n'),
                Some('t')  => result.push('\t'),
                Some('r')  => result.push('\r'),
                Some('\\') => result.push('\\'),
                Some('"')  => result.push('"'),
                Some(c)    => { result.push('\\'); result.push(c); }
                None       => result.push('\\'),
            }
        } else {
            result.push(ch);
        }
    }
    Some(result)
}

fn parse_char_literal(lex: &mut logos::Lexer<LogosToken>) -> Option<char> {
    let slice = lex.slice();
    let content = &slice[1..slice.len() - 1];
    if content.starts_with('\\') {
        match content.chars().nth(1) {
            Some('n')  => Some('\n'),
            Some('t')  => Some('\t'),
            Some('r')  => Some('\r'),
            Some('\\') => Some('\\'),
            Some('\'') => Some('\''),
            _          => None,
        }
    } else {
        content.chars().next()
    }
}

// ── LogosLexer ────────────────────────────────────────────────────

pub struct LogosLexer<'a> {
    input: &'a str,
    logos_lex: logos::Lexer<'a, LogosToken>,
    error_manager: ErrorManager,
    position: usize,
    line: usize,
    column: usize,
    tokens: Vec<Token>,
}

impl<'a> LogosLexer<'a> {
    pub fn new(input: &'a str) -> Self {
        LogosLexer {
            logos_lex: LogosToken::lexer(input),
            error_manager: ErrorManager::new(input.to_string()),
            input,
            position: 0,
            line: 1,
            column: 1,
            tokens: Vec::new(),
        }
    }

    pub fn tokenize(mut self) -> Result<Vec<Token>, ErrorManager> {
        while let Some(token) = self.next_token() {
            self.tokens.push(token);
        }
        self.tokens.push(Token::new(
            TokenType::Eof,
            Span::new(self.position, self.position, self.line, self.column),
            String::new(),
        ));
        if self.error_manager.has_errors() {
            Err(self.error_manager)
        } else {
            Ok(self.tokens)
        }
    }

    /// Pulls the next single real token, or `None` at true end of input.
    /// This is `tokenize`'s bulk loop's own primitive (below); it is also
    /// driven directly by `StringParser::parse_interpolation_expr` on a
    /// fresh `LogosLexer` over the remainder of the file, so an
    /// interpolation hole's closing brace can be found by tracking real
    /// `LeftBrace`/`RightBrace` TOKENS instead of raw bytes: anything
    /// already living inside a nested string, char literal, or comment
    /// is consumed atomically here exactly as it is everywhere else, so
    /// it can never be mistaken for a hole boundary. One dispatch path,
    /// used both ways, instead of a second copy of it.
    ///
    /// On a bad character, the error is recorded on `self.error_manager`
    /// and scanning continues with the next raw token rather than
    /// stopping, matching this lexer's existing resilient-on-error
    /// design elsewhere (a single bad character shouldn't hide every
    /// token after it).
    pub(crate) fn next_token(&mut self) -> Option<Token> {
        loop {
            let token_result = self.logos_lex.next()?;
            let span_range = self.logos_lex.span();
            let lexeme = self.logos_lex.slice().to_string();
            // `span_range` (from `self.logos_lex.span()`) is relative to
            // whichever slice `self.logos_lex` currently starts from,
            // and every branch below rebases it (`LogosToken::lexer(
            // &self.input[pos..])`) after a string/comment, so after the
            // FIRST rebase, `span_range` is no longer relative to
            // `self.input` at all. `self.position`, by contrast, is a
            // plain running byte counter (`update_position`) that's
            // never reset by a rebase: it's the one value here that's
            // always absolute. This was the actual root cause of the
            // documented "a second interpolated string anywhere later
            // currently breaks the lexer" gap (see ok_collections_full
            // .ubl's own header comment): a SECOND InterpolatedStringStart's
            // sub-parser was being told to start scanning from a
            // rebase-relative offset as if it were absolute, landing it
            // somewhere else in the file entirely.
            let abs_start = self.position;
            match token_result {
                Ok(LogosToken::InterpolatedStringStart) => {
                    let mut parser = StringParser::new(self.input, abs_start, self.line, self.column);
                    match parser.parse_interpolated_string() {
                        Ok((token, pos, line, col)) => {
                            self.position = pos; self.line = line; self.column = col;
                            self.logos_lex = LogosToken::lexer(&self.input[pos..]);
                            return Some(token);
                        }
                        Err(err) => { self.error_manager.add_lexical_error(err); continue; }
                    }
                }
                Ok(LogosToken::VerbatimStringStart) => {
                    let mut parser = StringParser::new(self.input, abs_start, self.line, self.column);
                    match parser.parse_verbatim_string() {
                        Ok((token, pos, line, col)) => {
                            self.position = pos; self.line = line; self.column = col;
                            self.logos_lex = LogosToken::lexer(&self.input[pos..]);
                            return Some(token);
                        }
                        Err(err) => { self.error_manager.add_lexical_error(err); continue; }
                    }
                }
                Ok(LogosToken::InterpolatedVerbatimStart) => {
                    let mut parser = StringParser::new(self.input, abs_start, self.line, self.column);
                    match parser.parse_interpolated_verbatim_string() {
                        Ok((token, pos, line, col)) => {
                            self.position = pos; self.line = line; self.column = col;
                            self.logos_lex = LogosToken::lexer(&self.input[pos..]);
                            return Some(token);
                        }
                        Err(err) => { self.error_manager.add_lexical_error(err); continue; }
                    }
                }
                Ok(LogosToken::BlockCommentStart) => {
                    let mut parser = CommentParser::new(self.input, abs_start, self.line, self.column);
                    match parser.parse_block_comment() {
                        Ok((_token, pos, line, col)) => {
                            self.position = pos; self.line = line; self.column = col;
                            self.logos_lex = LogosToken::lexer(&self.input[pos..]);
                            continue;
                        }
                        Err(err) => { self.error_manager.add_lexical_error(err); continue; }
                    }
                }
                Ok(logos_token @ (LogosToken::DocCommentStar | LogosToken::DocCommentBang)) => {
                    let marker = if matches!(logos_token, LogosToken::DocCommentStar) { "/**" } else { "/*!" };
                    let mut parser = CommentParser::new(self.input, abs_start, self.line, self.column);
                    match parser.parse_doc_comment(marker) {
                        Ok((token, pos, line, col)) => {
                            self.position = pos; self.line = line; self.column = col;
                            self.logos_lex = LogosToken::lexer(&self.input[pos..]);
                            return Some(token);
                        }
                        Err(err) => { self.error_manager.add_lexical_error(err); continue; }
                    }
                }
                Ok(LogosToken::LineComment | LogosToken::Newline | LogosToken::Whitespace) => {
                    self.update_position(&lexeme);
                    continue;
                }
                Ok(logos_token) => {
                    let span = Span::new(abs_start, abs_start + (span_range.end - span_range.start), self.line, self.column);
                    self.update_position(&lexeme);
                    let token_type = self.map_logos_token(logos_token, &lexeme);
                    return Some(Token::new(token_type, span, lexeme));
                }
                Err(_) => {
                    self.handle_error(abs_start, span_range, lexeme);
                    continue;
                }
            }
        }
    }

    /// Drains any lexical errors recorded by `next_token` calls made on
    /// this lexer so far. Used by `StringParser::parse_interpolation_expr`
    /// after driving a sub-`LogosLexer` over a hole's contents, to learn
    /// whether anything inside the hole failed to tokenize cleanly.
    pub(crate) fn take_lexical_errors(&mut self) -> Vec<LexicalError> {
        self.error_manager.take_lexical_errors()
    }

    fn map_logos_token(&self, logos_token: LogosToken, lexeme: &str) -> TokenType {
        match logos_token {
            LogosToken::Fn        => TokenType::Fn,
            LogosToken::Let       => TokenType::Let,
            LogosToken::Mut       => TokenType::Mut,
            LogosToken::Const     => TokenType::Const,
            LogosToken::If        => TokenType::If,
            LogosToken::Elif      => TokenType::Elif,
            LogosToken::Else      => TokenType::Else,
            LogosToken::Match     => TokenType::Match,
            LogosToken::Where     => TokenType::Where,
            LogosToken::For       => TokenType::For,
            LogosToken::In        => TokenType::In,
            LogosToken::While     => TokenType::While,
            LogosToken::Loop      => TokenType::Loop,
            LogosToken::Break     => TokenType::Break,
            LogosToken::Continue  => TokenType::Continue,
            LogosToken::Return    => TokenType::Return,
            LogosToken::Summon    => TokenType::Summon,
            LogosToken::From      => TokenType::From,
            LogosToken::As        => TokenType::As,
            LogosToken::Package   => TokenType::Package,
            LogosToken::Async     => TokenType::Async,
            LogosToken::Await     => TokenType::Await,
            LogosToken::Task      => TokenType::Task,
            LogosToken::Try       => TokenType::Try,
            LogosToken::Catch     => TokenType::Catch,
            LogosToken::Fail      => TokenType::Fail,
            LogosToken::Struct    => TokenType::Struct,
            LogosToken::Enum      => TokenType::Enum,
            LogosToken::Trait     => TokenType::Trait,
            LogosToken::Impl      => TokenType::Impl,
            LogosToken::Pub       => TokenType::Pub,
            LogosToken::Edge      => TokenType::Edge,
            LogosToken::Unsafe    => TokenType::Unsafe,
            LogosToken::With      => TokenType::With,
            LogosToken::Defer     => TokenType::Defer,
            LogosToken::And       => TokenType::And,
            LogosToken::Or        => TokenType::Or,
            LogosToken::Not       => TokenType::Not,
            LogosToken::True      => TokenType::True,
            LogosToken::False     => TokenType::False,
            LogosToken::Null      => TokenType::Null,
            LogosToken::SelfKw    => TokenType::SelfKw,
            LogosToken::Getter    => TokenType::Getter,
            LogosToken::Setter    => TokenType::Setter,
            LogosToken::Ref       => TokenType::Ref,
            LogosToken::Deref     => TokenType::Deref,
            LogosToken::Extend    => TokenType::Extend,
            LogosToken::TypeKw    => TokenType::TypeKw,
            LogosToken::Extract   => TokenType::Extract,
            LogosToken::Using     => TokenType::Using,
            LogosToken::Lifetime  => TokenType::Lifetime,
            LogosToken::Tier      => TokenType::Tier,
            LogosToken::High      => TokenType::High,
            LogosToken::Mid       => TokenType::Mid,
            LogosToken::Low       => TokenType::Low,
            LogosToken::Arena     => TokenType::Arena,
            LogosToken::Pool      => TokenType::Pool,
            LogosToken::Gc        => TokenType::Gc,
            LogosToken::Heap      => TokenType::Heap,
            LogosToken::KwList       => TokenType::KwList,
            LogosToken::KwDictionary => TokenType::KwDictionary,
            LogosToken::KwSet        => TokenType::KwSet,
            LogosToken::KwQueue      => TokenType::KwQueue,
            LogosToken::KwStack      => TokenType::KwStack,
            LogosToken::KwInlineList => TokenType::KwInlineList,
            LogosToken::Underscore   => TokenType::Underscore,
            LogosToken::Plus          => TokenType::Plus,
            LogosToken::Minus         => TokenType::Minus,
            LogosToken::Star          => TokenType::Star,
            LogosToken::Slash         => TokenType::Slash,
            LogosToken::Percent       => TokenType::Percent,
            LogosToken::Amp           => TokenType::Amp,
            LogosToken::Pipe          => TokenType::Pipe,
            LogosToken::Caret         => TokenType::Caret,
            LogosToken::Tilde         => TokenType::Tilde,
            LogosToken::LeftShift     => TokenType::LeftShift,
            LogosToken::RightShift    => TokenType::RightShift,
            LogosToken::EqualEqual    => TokenType::EqualEqual,
            LogosToken::BangEqual     => TokenType::BangEqual,
            LogosToken::Less          => TokenType::Less,
            LogosToken::Greater       => TokenType::Greater,
            LogosToken::LessEqual     => TokenType::LessEqual,
            LogosToken::GreaterEqual  => TokenType::GreaterEqual,
            LogosToken::Bang          => TokenType::Bang,
            LogosToken::AmpAmp        => TokenType::AmpAmp,
            LogosToken::PipePipe      => TokenType::PipePipe,
            LogosToken::Equal         => TokenType::Equal,
            LogosToken::PlusEqual     => TokenType::PlusEqual,
            LogosToken::MinusEqual    => TokenType::MinusEqual,
            LogosToken::StarEqual     => TokenType::StarEqual,
            LogosToken::SlashEqual    => TokenType::SlashEqual,
            LogosToken::PercentEqual  => TokenType::PercentEqual,
            LogosToken::AmpEqual      => TokenType::AmpEqual,
            LogosToken::PipeEqual     => TokenType::PipeEqual,
            LogosToken::CaretEqual    => TokenType::CaretEqual,
            LogosToken::LeftShiftEqual  => TokenType::LeftShiftEqual,
            LogosToken::RightShiftEqual => TokenType::RightShiftEqual,
            LogosToken::Question      => TokenType::Question,
            LogosToken::QuestionDot   => TokenType::QuestionDot,
            LogosToken::FatArrow      => TokenType::FatArrow,
            LogosToken::ColonEqual    => TokenType::ColonEqual,
            LogosToken::PipeArrow     => TokenType::PipeArrow,
            LogosToken::DotDot        => TokenType::DotDot,
            LogosToken::DotDotEqual   => TokenType::DotDotEqual,
            LogosToken::DotDotDot     => TokenType::DotDotDot,
            LogosToken::LeftParen    => TokenType::LeftParen,
            LogosToken::RightParen   => TokenType::RightParen,
            LogosToken::LeftBrace    => TokenType::LeftBrace,
            LogosToken::RightBrace   => TokenType::RightBrace,
            LogosToken::LeftBracket  => TokenType::LeftBracket,
            LogosToken::RightBracket => TokenType::RightBracket,
            LogosToken::Comma        => TokenType::Comma,
            LogosToken::Dot          => TokenType::Dot,
            LogosToken::Colon        => TokenType::Colon,
            LogosToken::Semicolon    => TokenType::Semicolon,
            LogosToken::At           => TokenType::At,
            LogosToken::Hash         => TokenType::Hash,
            LogosToken::IntLit(n)    => match int_suffix_of(lexeme) {
                // Explicit suffix -- always a TypedIntLit, whatever the
                // value, even if it happens to fit i64 too (`5i64` stays
                // TypedIntLit rather than collapsing to a plain IntLit;
                // sema still needs to see the explicit `i64` intent to
                // give it that exact type rather than defaulting to Int).
                Some(suffix) => TokenType::TypedIntLit(n, suffix),
                // No suffix, but the digits alone already overflow i64 --
                // this is the auto-promotion case (see `parse_decimal`'s
                // doc comment): treat it as an implicit `u64` suffix
                // rather than a lex error, since that's the only way a
                // value in the top half of u64's range can be written.
                None if n > i64::MAX as u64 => TokenType::TypedIntLit(n, IntSuffix::U64),
                // The common case, unchanged: fits i64, no suffix.
                None => TokenType::IntLit(n as i64),
            },
            LogosToken::FloatLit(f)  => {
                if lexeme.ends_with('f') || lexeme.ends_with('F') {
                    TokenType::FloatLit(f as f32)
                } else {
                    TokenType::DoubleLit(f)
                }
            }
            LogosToken::StringLit(s) => TokenType::StringLit(s),
            LogosToken::CharLit(c)   => TokenType::CharLit(c),
            LogosToken::Ident        => {
                keywords::get_keyword(lexeme)
                    .unwrap_or_else(|| TokenType::Ident(lexeme.to_string()))
            }
            _ => TokenType::Error(format!("Unhandled token: {:?}", logos_token)),
        }
    }

    fn handle_error(&mut self, abs_start: usize, span_range: std::ops::Range<usize>, lexeme: String) {
        // `span_range` is relative to whatever `self.logos_lex` currently
        // wraps, which is rebase-relative (not absolute) after the first
        // string/comment in the file; see `next_token`'s comment on
        // `abs_start` above. Using `span_range` directly here (as this
        // used to) is the same class of bug that was already found and
        // fixed for ordinary tokens; it just hadn't been mirrored into
        // this error path yet. Length is safe to take from `span_range`
        // either way, since a rebase only shifts the start, not the
        // width of the current match.
        let span = Span::new(abs_start, abs_start + (span_range.end - span_range.start), self.line, self.column);
        let ch = lexeme.chars().next().unwrap_or('\0');
        self.error_manager.add_lexical_error(LexicalError::UnexpectedChar {
            ch,
            span,
            suggestion: Some("Remove this character or check for typos".to_string()),
        });
        self.tokens.push(Token::error(format!("Unexpected character: '{}'", ch), span));
        self.update_position(&lexeme);
    }

    fn update_position(&mut self, lexeme: &str) {
        for ch in lexeme.chars() {
            if ch == '\n' {
                self.line += 1;
                self.column = 1;
            } else {
                self.column += 1;
            }
            self.position += ch.len_utf8();
        }
    }
}
