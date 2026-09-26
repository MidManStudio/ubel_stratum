// ============================================================================
// NOTICE: Full documentation, design decisions, and fix history for this file
// live in docs/ubel_stratum.md, section "ast/literals.rs"
// ============================================================================
// src/ast/literals.rs
//! Literal value nodes.
//!
//! `Literal<'ast>` is `Copy` — all variants are either primitive values
//! or fat pointers into the arena, so copying is always cheap.


#![allow(dead_code)]




use crate::ast::expressions::Expr;
pub use crate::lexer::token::IntSuffix;

/// A parsed `{expr:spec}` format specifier: the part after the `:` in an
/// interpolation hole. Covers `[[fill]align][sign]['#']['0']width?
/// ['.'precision]?['?' | base]?`: fill/align/width/precision/debug landed
/// first; sign forcing, the alternate form, zero-padding, and numeric
/// bases (`x`/`X`/`o`/`b`) followed later (docs/PRINT_FORMAT_RULES.md §4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FormatSpec {
    /// Custom fill character, e.g. the `*` in `{v:*>10}`. Only ever
    /// `Some` alongside `align`. A fill character with no alignment to
    /// pad around is meaningless, and the parser never produces one
    /// (`parse_format_spec`, `rd_parser`): it only looks for a fill
    /// character at all when the token right after it is an align
    /// marker. `None` means the default space fill.
    pub fill:      Option<char>,
    pub align:     Option<Align>,
    /// `+`: force a sign on positive numbers too, not just negative
    /// ones. `Int`/`Float`/`Double` only (`TYPE-115`, same family as
    /// `.precision`'s existing type restriction below).
    pub sign_plus: bool,
    /// `#`: the alternate form. Today this means exactly one thing:
    /// prefix a numeric-base rendering with `0x`/`0X`/`0o`/`0b`. Requires
    /// `base` also be set (`TYPE-119`). There's nothing else in this
    /// spec for "alternate" to alter.
    pub alternate: bool,
    /// A literal leading `0` immediately followed by more digits in the
    /// width position, e.g. the first `0` in `{v:010}` (width 10),
    /// distinguished from an ordinary `width: Some(0)` by reading the
    /// integer token's own source lexeme, not its parsed value, the same
    /// technique `width`/`precision`'s existing token-collision recovery
    /// already uses just above this struct's parser. Pads with `0`
    /// between the sign and the digits instead of spaces around the
    /// whole thing. `Int`/`Float`/`Double` only (`TYPE-115`).
    pub zero_pad:  bool,
    pub width:     Option<u32>,
    pub precision: Option<u32>,
    /// Trailing `?`: selects `Value::debug_string()` over `Display` as
    /// the base formatter (see `interpreter::eval::expr::apply_format_spec`
    /// and `Value::debug_string` doc comments for exactly what differs:
    /// quoted/escaped `Str`/`Char`, and live `Rc::strong_count` on
    /// `Shared`/`SyncShared`). Mutually exclusive with `base`: an `Int`
    /// (the only type `base` accepts) never diverges between `Display`
    /// and `debug_string` in the first place, so a hole can't sensibly
    /// ask for both at once.
    pub debug:     bool,
    /// Trailing `x`/`X`/`o`/`b`: render an `Int` in hex/hex-uppercase/
    /// octal/binary instead of decimal. `Int` only (`TYPE-115`); a
    /// negative value renders as its 64-bit two's-complement bit pattern
    /// in the chosen base, matching Rust's own `{:x}` on a signed integer
    /// rather than a `-` sign plus the magnitude's digits, since that's
    /// the well-established, unsurprising convention for this exact
    /// feature in every systems-adjacent language, not something new
    /// invented for this one.
    pub base:      Option<NumericBase>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NumericBase {
    Hex,
    HexUpper,
    Octal,
    Binary,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Left,
    Right,
    Center,
}

/// One segment of an interpolated string `$"...{...}..."`.
#[derive(Debug, Clone, Copy)]
pub enum InterpolationPart<'ast> {
    /// A plain text run: `"Hello, "` in `$"Hello, {name}"`.
    Text(&'ast str),
    /// A fully parsed expression hole: `name` in `{name}`, with an
    /// optional format spec: `{name:>10}`. Parsed by rd_parser at the
    /// same time as everything else -- no re-parsing happens later, in
    /// sema or the interpreter.
    Expr {
        expr: &'ast Expr<'ast>,
        spec: Option<FormatSpec>,
    },
}


/// Every form of literal that Ubel Stratum supports.
#[derive(Debug, Clone, Copy)]
pub enum Literal<'ast> {
    /// Integer literal: `42`, `0xFF`, `0b1010`, `1_000_000`. Always fits
    /// `i64` -- infers as `SemaType::Int`, exactly as before this feature
    /// existed. Zero change in behavior for any literal without a suffix
    /// that also fits `i64`.
    Int(i64),
    /// Explicitly-suffixed integer literal (`255u8`, `1024u32`, `5000i64`),
    /// OR an unsuffixed literal whose value overflows `i64::MAX` but fits
    /// `u64::MAX` -- the lexer treats that case as an implicit `u64`
    /// suffix (auto-promotion) rather than a lex error, since there is no
    /// other way for a value in the top half of `u64`'s range to exist as
    /// a literal at all. `raw` holds the exact bit pattern as `u64`;
    /// `suffix` says how sema/the interpreter should interpret it.
    TypedInt { raw: u64, suffix: IntSuffix },
    /// 32-bit float: `3.14f`
    Float(f32),
    /// 64-bit float (default when no suffix): `3.14`
    Double(f64),
    /// Plain string: `"hello\nworld"`
    Str(&'ast str),
    /// Interpolated string: `$"Hello, {name}!"`
    InterpolatedStr(&'ast [InterpolationPart<'ast>]),
    /// Verbatim string (no escapes, `""` to embed a quote): `@"C:\path"`
    VerbatimStr(&'ast str),
    /// Interpolated verbatim: `$@"C:\Users\{name}"`
    InterpolatedVerbatimStr(&'ast [InterpolationPart<'ast>]),
    /// Character: `'a'`, `'\n'`
    Char(char),
    /// Boolean: `true` / `false`
    Bool(bool),
    /// The `null` literal
    Null,
}
