// ============================================================================
// NOTICE: Full documentation, design decisions, and fix history for this file
// live in docs/ubel_stratum.md, section "lexer/logos_lexer.rs"
// ============================================================================
// src/lexer/logos_lexer.rs

use logos::Logos;
use crate::lexer::{Token, TokenType, Span};
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
    #[regex(r"[0-9][0-9_]*", parse_decimal)]
    #[regex(r"0x[0-9a-fA-F][0-9a-fA-F_]*", parse_hex)]
    #[regex(r"0b[01][01_]*", parse_binary)]
    IntLit(i64),

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

fn parse_decimal(lex: &mut logos::Lexer<LogosToken>) -> Option<i64> {
    lex.slice().replace('_', "").parse().ok()
}

fn parse_hex(lex: &mut logos::Lexer<LogosToken>) -> Option<i64> {
    i64::from_str_radix(&lex.slice()[2..].replace('_', ""), 16).ok()
}

fn parse_binary(lex: &mut logos::Lexer<LogosToken>) -> Option<i64> {
    i64::from_str_radix(&lex.slice()[2..].replace('_', ""), 2).ok()
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
        while let Some(token_result) = self.logos_lex.next() {
            let span_range = self.logos_lex.span();
            let lexeme = self.logos_lex.slice().to_string();
            match token_result {
                Ok(logos_token) => self.handle_logos_token(logos_token, span_range, lexeme),
                Err(_)          => self.handle_error(span_range, lexeme),
            }
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

    fn handle_logos_token(
        &mut self,
        logos_token: LogosToken,
        span_range: std::ops::Range<usize>,
        lexeme: String,
    ) {
        // `span_range` (from `self.logos_lex.span()`) is relative to
        // whichever slice `self.logos_lex` currently starts from — and
        // every branch below rebases it (`LogosToken::lexer(&self.input
        // [pos..])`) after a string/comment, so after the FIRST rebase,
        // `span_range` is no longer relative to `self.input` at all.
        // `self.position`, by contrast, is a plain running byte counter
        // (`update_position`) that's never reset by a rebase — it's the
        // one value here that's always absolute. This was the actual
        // root cause of the documented "a second interpolated string
        // anywhere later currently breaks the lexer" gap (see
        // ok_collections_full.ubl's own header comment): a SECOND
        // InterpolatedStringStart's sub-parser was being told to start
        // scanning from a rebase-relative offset as if it were absolute,
        // landing it somewhere else in the file entirely.
        let abs_start = self.position;
        match logos_token {
            LogosToken::InterpolatedStringStart => {
                let mut parser = StringParser::new(self.input, abs_start, self.line, self.column);
                match parser.parse_interpolated_string() {
                    Ok((token, pos, line, col)) => {
                        self.tokens.push(token);
                        self.position = pos; self.line = line; self.column = col;
                        self.logos_lex = LogosToken::lexer(&self.input[pos..]);
                    }
                    Err(err) => self.error_manager.add_lexical_error(err),
                }
                return;
            }
            LogosToken::VerbatimStringStart => {
                let mut parser = StringParser::new(self.input, abs_start, self.line, self.column);
                match parser.parse_verbatim_string() {
                    Ok((token, pos, line, col)) => {
                        self.tokens.push(token);
                        self.position = pos; self.line = line; self.column = col;
                        self.logos_lex = LogosToken::lexer(&self.input[pos..]);
                    }
                    Err(err) => self.error_manager.add_lexical_error(err),
                }
                return;
            }
            LogosToken::InterpolatedVerbatimStart => {
                let mut parser = StringParser::new(self.input, abs_start, self.line, self.column);
                match parser.parse_interpolated_verbatim_string() {
                    Ok((token, pos, line, col)) => {
                        self.tokens.push(token);
                        self.position = pos; self.line = line; self.column = col;
                        self.logos_lex = LogosToken::lexer(&self.input[pos..]);
                    }
                    Err(err) => self.error_manager.add_lexical_error(err),
                }
                return;
            }
            LogosToken::BlockCommentStart => {
                let mut parser = CommentParser::new(self.input, abs_start, self.line, self.column);
                match parser.parse_block_comment() {
                    Ok((_token, pos, line, col)) => {
                        self.position = pos; self.line = line; self.column = col;
                        self.logos_lex = LogosToken::lexer(&self.input[pos..]);
                    }
                    Err(err) => self.error_manager.add_lexical_error(err),
                }
                return;
            }
            LogosToken::DocCommentStar | LogosToken::DocCommentBang => {
                let marker = if matches!(logos_token, LogosToken::DocCommentStar) { "/**" } else { "/*!" };
                let mut parser = CommentParser::new(self.input, abs_start, self.line, self.column);
                match parser.parse_doc_comment(marker) {
                    Ok((token, pos, line, col)) => {
                        self.tokens.push(token);
                        self.position = pos; self.line = line; self.column = col;
                        self.logos_lex = LogosToken::lexer(&self.input[pos..]);
                    }
                    Err(err) => self.error_manager.add_lexical_error(err),
                }
                return;
            }
            LogosToken::LineComment | LogosToken::Newline | LogosToken::Whitespace => {
                self.update_position(&lexeme);
                return;
            }
            _ => {}
        }

        let span = Span::new(abs_start, abs_start + (span_range.end - span_range.start), self.line, self.column);
        self.update_position(&lexeme);
        let token_type = self.map_logos_token(logos_token, &lexeme);
        self.tokens.push(Token::new(token_type, span, lexeme));
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
            LogosToken::IntLit(n)    => TokenType::IntLit(n),
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

    fn handle_error(&mut self, span_range: std::ops::Range<usize>, lexeme: String) {
        let span = Span::new(span_range.start, span_range.end, self.line, self.column);
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
