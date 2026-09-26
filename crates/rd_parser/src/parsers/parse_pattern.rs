// ============================================================================
// NOTICE: Full documentation, design decisions, and fix history for this file
// live in docs/ubel_stratum_rd.md, section "parsers/parse_pattern.rs"
// ============================================================================
//
// crates/rd_parser/src/parsers/parse_pattern.rs
//
// Pattern parser for Ubel Stratum.
//
// Pattern grammar (from ubel.ebnf):
//   Pattern     ::= OrPattern
//   OrPattern   ::= SinglePat ("|" SinglePat)*
//   SinglePat   ::= "_"                          (* wildcard     *)
//                 | Literal                      (* literal pat  *)
//                 | "-" NumericLit               (* neg literal  *)
//                 | "mut"? Ident                 (* binding      *)
//                 | Ident "." Ident+ (Payload?)  (* enum variant *)
//                 | Ident "{" FieldPat* "}"      (* named struct *)
//                 | "{" FieldPat* "}"            (* anon struct  *)
//                 | "(" Pat "," Pat* ")"         (* tuple        *)
//                 | "[" Pat* ("..." Ident?)? "]" (* array        *)
//                 | Literal ".." Literal         (* range        *)
//                 | Literal "..=" Literal        (* incl. range  *)
//
// Destructure patterns (for `let`, `for`, `extract`):
//   DestPat ::= Ident | "(" DestEl* ")" | "[" DestEl* "..." "]" | "{" FieldDestruct* "}"

use ubel_stratum::{
    ast::{
        common::Span,
        literals::Literal,
        patterns::{
            ArrayDestructure, DestructureElement, DestructurePattern,
            EnumPatternPayload, FieldDestructure, FieldPattern, Pattern,
            PatternKind, StructDestructure, TupleDestructure,
        },
        statements::BindingTarget,
    },
    error_management::errors::ParseContext,
    lexer::TokenType,
};

use crate::parser::Parser;

// ── Main public entry points ──────────────────────────────────────────────────

/// Parse a full match-arm pattern, including OR alternatives.
/// Handles an optional leading `|` (valid in the first arm of a match block).
pub(crate) fn parse_pattern<'ast, 'tok>(
    p: &mut Parser<'ast, 'tok>,
) -> Option<Pattern<'ast>> {
    let prev = p.enter(ParseContext::Pattern);
    // Skip a leading `|` — Ubel allows it before the first arm
    p.cursor.eat(&TokenType::Pipe);
    let result = parse_or_pattern(p);
    p.leave(prev);
    result
}

/// Parse a destructure pattern for `let`, `for`, and `extract` bindings.
pub(crate) fn parse_destructure_pattern<'ast, 'tok>(
    p: &mut Parser<'ast, 'tok>,
) -> Option<DestructurePattern<'ast>> {
    let lo = p.span();
    match p.cursor.peek().clone() {
        TokenType::Ident(ref name) if name == "_" => {
            let name = name.clone();
            p.cursor.advance();
            Some(DestructurePattern::Ident(p.intern(&name)))
        }
        TokenType::Ident(name) => {
            let name = p.intern(&name);
            p.cursor.advance();
            Some(DestructurePattern::Ident(name))
        }
        TokenType::LeftParen    => parse_tuple_destructure(p, lo).map(DestructurePattern::Tuple),
        TokenType::LeftBracket  => parse_array_destructure(p, lo).map(DestructurePattern::Array),
        TokenType::LeftBrace    => parse_struct_destructure(p, lo).map(DestructurePattern::Struct),
        _ => {
            p.expected(&["name", "'('", "'['", "'{'"]);
            None
        }
    }
}

/// Parse a binding target for `let`/`for` — either a plain name or destructure.
pub(crate) fn parse_binding_target<'ast, 'tok>(
    p: &mut Parser<'ast, 'tok>,
) -> Option<BindingTarget<'ast>> {
    match p.cursor.peek().clone() {
        TokenType::Ident(name) => {
            let name = p.intern(&name);
            p.cursor.advance();
            Some(BindingTarget::Ident(name))
        }
        TokenType::LeftParen | TokenType::LeftBracket | TokenType::LeftBrace => {
            parse_destructure_pattern(p).map(BindingTarget::Destructure)
        }
        _ => {
            p.expected(&["binding name or destructure pattern"]);
            None
        }
    }
}

// ── OR pattern ────────────────────────────────────────────────────────────────

fn parse_or_pattern<'ast, 'tok>(p: &mut Parser<'ast, 'tok>) -> Option<Pattern<'ast>> {
    let first = parse_single_pattern(p)?;

    if !p.cursor.is_at(&TokenType::Pipe) {
        return Some(first);
    }

    let lo = first.span;
    let mut alts: Vec<Pattern<'ast>> = Vec::with_capacity(4);
    alts.push(first);

    while p.cursor.eat(&TokenType::Pipe) {
        alts.push(parse_single_pattern(p)?);
    }

    let hi   = alts.last().unwrap().span;
    let span = lo.merge(&hi);
    let alts = p.arena.alloc_slice_clone(&alts);
    Some(Pattern { kind: PatternKind::Or(alts), span })
}

// ── Single non-OR pattern ─────────────────────────────────────────────────────

fn parse_single_pattern<'ast, 'tok>(p: &mut Parser<'ast, 'tok>) -> Option<Pattern<'ast>> {
    let lo = p.span();

    match p.cursor.peek().clone() {

        // Wildcard `_`. This is TokenType::Underscore, not Ident("_").
        TokenType::Underscore => {
            p.cursor.advance();
            Some(Pattern { kind: PatternKind::Wildcard, span: lo })
        }

        // ── Numeric literals (+ optional range) ───────────────────────────────
        TokenType::IntLit(n) => {
            p.cursor.advance();
            parse_range_or_literal(p, Literal::Int(n), lo)
        }
        // Suffixed literal pattern (`5u8 => ...`). Negative suffixed
        // literals (`-128i8`) aren't supported as patterns yet — `raw`
        // is an unsigned magnitude with no representation for "negate
        // this", unlike plain `Literal::Int` which just flips an `i64`
        // sign bit below. Falls through to the same "expected integer
        // or float literal after '-'" error as any other unsupported
        // token there until that's designed properly.
        TokenType::TypedIntLit(raw, suffix) => {
            p.cursor.advance();
            parse_range_or_literal(p, Literal::TypedInt { raw, suffix }, lo)
        }
        TokenType::FloatLit(f) => {
            p.cursor.advance();
            parse_range_or_literal(p, Literal::Float(f), lo)
        }
        TokenType::CharLit(c) => {
            p.cursor.advance();
            parse_range_or_literal(p, Literal::Char(c), lo)
        }

        // ── String literals (no range) ────────────────────────────────────────
        TokenType::StringLit(s) => {
            let s = s.clone();
            p.cursor.advance();
            let s = p.intern(&s);
            Some(Pattern { kind: PatternKind::Literal(Literal::Str(s)), span: lo })
        }

        // ── Boolean literals ──────────────────────────────────────────────────
        TokenType::True  => {
            p.cursor.advance();
            Some(Pattern { kind: PatternKind::Literal(Literal::Bool(true)), span: lo })
        }
        TokenType::False => {
            p.cursor.advance();
            Some(Pattern { kind: PatternKind::Literal(Literal::Bool(false)), span: lo })
        }
        TokenType::Null  => {
            p.cursor.advance();
            Some(Pattern { kind: PatternKind::Literal(Literal::Null), span: lo })
        }

        // ── Negative numeric literal: `-42`, `-3.14` ──────────────────────────
        TokenType::Minus => {
            p.cursor.advance();
            let neg_lo = p.span();
            match p.cursor.peek().clone() {
                TokenType::IntLit(n) => {
                    p.cursor.advance();
                    let span = lo.merge(&neg_lo);
                    parse_range_or_literal(p, Literal::Int(-n), span)
                }
                TokenType::FloatLit(f) => {
                    p.cursor.advance();
                    let span = lo.merge(&neg_lo);
                    parse_range_or_literal(p, Literal::Float(-f), span)
                }
                _ => {
                    p.expected(&["integer or float literal after '-'"]);
                    None
                }
            }
        }

        // ── Mutable binding `mut x` ───────────────────────────────────────────
        TokenType::Mut => {
            p.cursor.advance();
            let (name, nspan) = p.expect_ident()?;
            let span = lo.merge(&nspan);
            Some(Pattern { kind: PatternKind::Ident { name, mutable: true }, span })
        }

        // ── Identifier: binding / enum variant / named struct ─────────────────
        TokenType::Ident(name) => {
            parse_ident_pattern(p, name, lo)
        }

        // ── Tuple `(a, b, c)` ────────────────────────────────────────────────
        TokenType::LeftParen => {
            parse_tuple_pattern(p, lo)
        }

        // ── Array `[a, b, ...rest]` ───────────────────────────────────────────
        TokenType::LeftBracket => {
            parse_array_pattern(p, lo)
        }

        // ── Anonymous struct `{ field, name = pat }` ──────────────────────────
        TokenType::LeftBrace => {
            let fields = parse_struct_fields_pattern(p)?;
            let span   = lo.merge(&p.span());
            Some(Pattern { kind: PatternKind::Struct { name: None, fields }, span })
        }

        _ => {
            p.expected(&[
                "pattern", "'_'", "literal", "identifier",
                "'mut'", "'('", "'['", "'{'"
            ]);
            None
        }
    }
}

// ── Literal / range helper ────────────────────────────────────────────────────

/// After parsing a literal, check for `..` or `..=` to form a range pattern.
fn parse_range_or_literal<'ast, 'tok>(
    p:    &mut Parser<'ast, 'tok>,
    lo_lit: Literal<'ast>,
    span: Span,
) -> Option<Pattern<'ast>> {
    if p.cursor.eat(&TokenType::DotDotEqual) {
        let hi_lit = parse_bare_literal(p)?;
        let hi_span = p.span();
        return Some(Pattern {
            kind: PatternKind::Range { lo: lo_lit, hi: hi_lit, inclusive: true },
            span: span.merge(&hi_span),
        });
    }
    if p.cursor.eat(&TokenType::DotDot) {
        let hi_lit  = parse_bare_literal(p)?;
        let hi_span = p.span();
        return Some(Pattern {
            kind: PatternKind::Range { lo: lo_lit, hi: hi_lit, inclusive: false },
            span: span.merge(&hi_span),
        });
    }
    Some(Pattern { kind: PatternKind::Literal(lo_lit), span })
}

/// Parse just a literal value — used as the high bound in a range pattern.
fn parse_bare_literal<'ast, 'tok>(p: &mut Parser<'ast, 'tok>) -> Option<Literal<'ast>> {
    match p.cursor.peek().clone() {
        TokenType::IntLit(n)    => { p.cursor.advance(); Some(Literal::Int(n)) }
        TokenType::TypedIntLit(raw, suffix) => { p.cursor.advance(); Some(Literal::TypedInt { raw, suffix }) }
        TokenType::FloatLit(f)  => { p.cursor.advance(); Some(Literal::Float(f)) }
        TokenType::CharLit(c)   => { p.cursor.advance(); Some(Literal::Char(c)) }
        TokenType::StringLit(s) => {
            let s = p.intern(&s); p.cursor.advance(); Some(Literal::Str(s))
        }
        TokenType::Minus => {
            p.cursor.advance();
            match p.cursor.peek().clone() {
                TokenType::IntLit(n)   => { p.cursor.advance(); Some(Literal::Int(-n)) }
                TokenType::FloatLit(f) => { p.cursor.advance(); Some(Literal::Float(-f)) }
                _ => { p.expected(&["number for range upper bound"]); None }
            }
        }
        _ => {
            p.expected(&["literal for range upper bound"]);
            None
        }
    }
}

// ── Identifier pattern ────────────────────────────────────────────────────────

fn parse_ident_pattern<'ast, 'tok>(
    p:    &mut Parser<'ast, 'tok>,
    name: String,
    lo:   Span,
) -> Option<Pattern<'ast>> {
    let name = p.intern(&name);
    p.cursor.advance();

    // Build a dotted path: `Status.Active`, `std.Option.Some`
    let mut path: Vec<&'ast str> = Vec::with_capacity(p.estimates.path_segs);
    path.push(name);

    while p.cursor.eat(&TokenType::Dot) {
        let (seg, _) = p.expect_ident()?;
        path.push(seg);
    }

    let hi = p.span();

    match p.cursor.peek().clone() {
        // `Name(pat, pat)` — enum variant with tuple payload
        TokenType::LeftParen => {
            let open_span = p.span();
            p.cursor.advance();
            let mut elems: Vec<Pattern<'ast>> = Vec::with_capacity(4);
            while !p.cursor.is_at(&TokenType::RightParen) && !p.cursor.is_eof() {
                elems.push(parse_or_pattern(p)?);
                p.eat_sep();
            }
            if !p.cursor.eat(&TokenType::RightParen) {
                p.emit(crate::error::unclosed('(', open_span, None, p.span()));
            }
            let path  = p.arena.alloc_slice_clone(&path);
            let elems = p.arena.alloc_slice_clone(&elems);
            let span  = lo.merge(&p.span());
            Some(Pattern {
                kind: PatternKind::Enum { path, payload: EnumPatternPayload::Tuple(elems) },
                span,
            })
        }

        // `Name { field, other = pat }` — struct pattern (named or enum-struct)
        TokenType::LeftBrace => {
            if path.len() == 1 {
                // Named struct pattern: `Point { x, y }`
                let fields = parse_struct_fields_pattern(p)?;
                let span   = lo.merge(&p.span());
                Some(Pattern {
                    kind: PatternKind::Struct { name: Some(path[0]), fields },
                    span,
                })
            } else {
                // Enum variant with struct payload: `Result.Err { code }`
                let path   = p.arena.alloc_slice_clone(&path);
                let fields = parse_struct_fields_pattern(p)?;
                let span   = lo.merge(&p.span());
                Some(Pattern {
                    kind: PatternKind::Enum { path, payload: EnumPatternPayload::Struct(fields) },
                    span,
                })
            }
        }

        // No payload or just an ident
        _ => {
            let span = lo.merge(&hi);
            if path.len() == 1 {
                // Simple binding: `x`
                Some(Pattern {
                    kind: PatternKind::Ident { name: path[0], mutable: false },
                    span,
                })
            } else {
                // Multi-segment enum variant: `Status.Active`
                let path = p.arena.alloc_slice_clone(&path);
                Some(Pattern {
                    kind: PatternKind::Enum { path, payload: EnumPatternPayload::None },
                    span,
                })
            }
        }
    }
}

// ── Struct pattern fields ─────────────────────────────────────────────────────

fn parse_struct_fields_pattern<'ast, 'tok>(
    p: &mut Parser<'ast, 'tok>,
) -> Option<&'ast [FieldPattern<'ast>]> {
    let open_span = p.span();
    if let Err(e) = p.cursor.expect(&TokenType::LeftBrace) {
        p.emit(crate::error::from_cursor(e, ParseContext::Pattern));
        return None;
    }
    let mut fields: Vec<FieldPattern<'ast>> = Vec::with_capacity(p.estimates.struct_fields);

    while !p.cursor.is_at(&TokenType::RightBrace) && !p.cursor.is_eof() {
        let flo           = p.span();
        let (field, _)    = p.expect_ident()?;
        // `field = pat` for renamed; bare `field` for shorthand
        let pattern = if p.cursor.eat(&TokenType::Equal) {
            Some(parse_or_pattern(p)?)
        } else {
            None
        };
        let span = flo.merge(&p.span());
        fields.push(FieldPattern { field, pattern, span });
        p.eat_sep();
    }

    if !p.cursor.eat(&TokenType::RightBrace) {
        p.emit(crate::error::unclosed('{', open_span, None, p.span()));
    }
    Some(p.arena.alloc_slice_clone(&fields))
}

// ── Tuple pattern ─────────────────────────────────────────────────────────────

fn parse_tuple_pattern<'ast, 'tok>(
    p:  &mut Parser<'ast, 'tok>,
    lo: Span,
) -> Option<Pattern<'ast>> {
    let open_span = p.span();
    p.cursor.advance(); // `(`
    let mut elems: Vec<Pattern<'ast>> = Vec::with_capacity(4);

    while !p.cursor.is_at(&TokenType::RightParen) && !p.cursor.is_eof() {
        elems.push(parse_or_pattern(p)?);
        p.eat_sep();
    }

    if !p.cursor.eat(&TokenType::RightParen) {
        p.emit(crate::error::unclosed('(', open_span, None, p.span()));
    }
    let span  = lo.merge(&p.span());
    let elems = p.arena.alloc_slice_clone(&elems);
    Some(Pattern { kind: PatternKind::Tuple(elems), span })
}

// ── Array pattern ─────────────────────────────────────────────────────────────

fn parse_array_pattern<'ast, 'tok>(
    p:  &mut Parser<'ast, 'tok>,
    lo: Span,
) -> Option<Pattern<'ast>> {
    let open_span = p.span();
    p.cursor.advance(); // `[`
    let mut elements: Vec<Pattern<'ast>> = Vec::with_capacity(4);
    let mut rest: Option<Option<&'ast str>> = None;

    while !p.cursor.is_at(&TokenType::RightBracket) && !p.cursor.is_eof() {
        // `...rest` or `...` (spread / rest element, must be last)
        if p.cursor.eat(&TokenType::DotDotDot) {
            rest = match p.eat_ident() {
                Some((name, _)) => Some(Some(name)), // `...name`
                None            => Some(None),        // discard `...`
            };
            break;
        }
        elements.push(parse_or_pattern(p)?);
        p.eat_sep();
    }

    if !p.cursor.eat(&TokenType::RightBracket) {
        p.emit(crate::error::unclosed('[', open_span, None, p.span()));
    }
    let span     = lo.merge(&p.span());
    let elements = p.arena.alloc_slice_clone(&elements);
    Some(Pattern { kind: PatternKind::Array { elements, rest }, span })
}

// ── Destructure helpers ───────────────────────────────────────────────────────

fn parse_tuple_destructure<'ast, 'tok>(
    p:  &mut Parser<'ast, 'tok>,
    lo: Span,
) -> Option<TupleDestructure<'ast>> {
    let open_span = p.span();
    p.cursor.advance(); // `(`
    let mut elements: Vec<DestructureElement<'ast>> = Vec::with_capacity(4);

    while !p.cursor.is_at(&TokenType::RightParen) && !p.cursor.is_eof() {
        elements.push(parse_destructure_element(p)?);
        p.eat_sep();
    }

    if !p.cursor.eat(&TokenType::RightParen) {
        p.emit(crate::error::unclosed('(', open_span, None, p.span()));
    }
    let span     = lo.merge(&p.span());
    let elements = p.arena.alloc_slice_clone(&elements);
    Some(TupleDestructure { elements, span })
}

fn parse_array_destructure<'ast, 'tok>(
    p:  &mut Parser<'ast, 'tok>,
    lo: Span,
) -> Option<ArrayDestructure<'ast>> {
    let open_span = p.span();
    p.cursor.advance(); // `[`
    let mut elements: Vec<DestructureElement<'ast>> = Vec::with_capacity(4);
    let mut rest: Option<Option<&'ast str>> = None;

    while !p.cursor.is_at(&TokenType::RightBracket) && !p.cursor.is_eof() {
        if p.cursor.eat(&TokenType::DotDotDot) {
            rest = match p.eat_ident() {
                Some((name, _)) => Some(Some(name)),
                None            => Some(None),
            };
            break;
        }
        elements.push(parse_destructure_element(p)?);
        p.eat_sep();
    }

    if !p.cursor.eat(&TokenType::RightBracket) {
        p.emit(crate::error::unclosed('[', open_span, None, p.span()));
    }
    let span     = lo.merge(&p.span());
    let elements = p.arena.alloc_slice_clone(&elements);
    Some(ArrayDestructure { elements, rest, span })
}

fn parse_struct_destructure<'ast, 'tok>(
    p:  &mut Parser<'ast, 'tok>,
    lo: Span,
) -> Option<StructDestructure<'ast>> {
    let open_span = p.span();
    p.cursor.advance(); // `{`
    let mut fields: Vec<FieldDestructure<'ast>> = Vec::with_capacity(p.estimates.struct_fields);

    while !p.cursor.is_at(&TokenType::RightBrace) && !p.cursor.is_eof() {
        let flo        = p.span();
        let (field, _) = p.expect_ident()?;
        // `field = DestPat` for remapped; bare `field` = bind to same name
        let pattern = if p.cursor.eat(&TokenType::Equal) {
            Some(parse_destructure_pattern(p)?)
        } else {
            None
        };
        let span = flo.merge(&p.span());
        fields.push(FieldDestructure { field, pattern, span });
        p.eat_sep();
    }

    if !p.cursor.eat(&TokenType::RightBrace) {
        p.emit(crate::error::unclosed('{', open_span, None, p.span()));
    }
    let span   = lo.merge(&p.span());
    let fields = p.arena.alloc_slice_clone(&fields);
    Some(StructDestructure { fields, span })
}

fn parse_destructure_element<'ast, 'tok>(
    p: &mut Parser<'ast, 'tok>,
) -> Option<DestructureElement<'ast>> {
    match p.cursor.peek().clone() {
        // Wildcard `_`. This is TokenType::Underscore, not Ident("_").
        TokenType::Underscore => {
            p.cursor.advance();
            Some(DestructureElement::Wildcard)
        }
        TokenType::Ident(name) => {
            let name = p.intern(&name);
            p.cursor.advance();
            Some(DestructureElement::Ident(name))
        }
        TokenType::LeftParen | TokenType::LeftBracket | TokenType::LeftBrace => {
            parse_destructure_pattern(p).map(DestructureElement::Nested)
        }
        _ => {
            p.expected(&["'_'", "identifier", "nested destructure pattern"]);
            None
        }
    }
    }
