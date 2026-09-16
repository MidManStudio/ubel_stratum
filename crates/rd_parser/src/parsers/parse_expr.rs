// ============================================================================
// NOTICE: Full documentation, design decisions, and fix history for this file
// live in docs/ubel_stratum_rd.md, section "parsers/parse_expr.rs"
// ============================================================================
// crates/rd_parser/src/parsers/parse_expr.rs
//! Expression parser — Pratt / Top-Down Operator Precedence.
//!
//! Binding power table (L = left bp, R = right bp):
//!   Assignment  = += -= …      L=1  R=2   right-assoc
//!   Range .. ..=               L=3  R=4
//!   OrElse `or return/break`   handled before Or
//!   Or / ||                    L=5  R=6
//!   And / &&                   L=7  R=8
//!   Equality == !=             L=9  R=10
//!   Comparison < > <= >=       L=11 R=12
//!   Pipe |>                    L=13 R=14
//!   BitOr |                    L=15 R=16
//!   BitXor ^                   L=17 R=18
//!   BitAnd &                   L=19 R=20
//!   Shift << >>                L=21 R=22
//!   Add/Sub + -                L=23 R=24
//!   Mul/Div/Mod * / %          L=25 R=26
//!   As (type coercion)         L=27 R=— (RHS is a Type, not an Expr)
//!   Unary prefix               R=28
//!   Postfix . () [] ? ?.       L=29 R=30

use ubel_stratum::{
    ast::{
        common::{AssignOp, BinOp, TierAnnotation, UnaryOp},
        expressions::{
            Arg, ArgKind, DictEntry, ElifBranch, Expr, ExprKind,
            FieldInit, IfBranchBody, IfExpr, Lambda, LambdaBody, LambdaParam,
            MatchArm, MatchExpr,
            ObjectField, OptionalAccess, OrElseFallback,
        },
        literals::{Align, FormatSpec, InterpolationPart, Literal, NumericBase},
    },
    error_management::errors::ParseContext,
    lexer::{InterpolationPart as LexPart, Span as LSpan, TokenType},
};

use crate::parser::Parser;

// ── Public entry point ────────────────────────────────────────────────────────

pub(crate) fn parse_expr<'ast, 'tok>(
    p: &mut Parser<'ast, 'tok>,
) -> Option<&'ast Expr<'ast>> {
    parse_bp(p, 0)
}

// ── Pratt core ────────────────────────────────────────────────────────────────

fn parse_bp<'ast, 'tok>(
    p: &mut Parser<'ast, 'tok>,
    min_bp: u8,
) -> Option<&'ast Expr<'ast>> {
    let lo = p.span();
    let mut lhs = parse_prefix(p)?;

    loop {
        let op = p.cursor.peek().clone();

        // ── Special: `or return/break/continue` → OrElse ─────────────────────
        if matches!(op, TokenType::Or) {
            let nxt = p.cursor.peek_nth(1).clone();
            if matches!(nxt, TokenType::Return | TokenType::Break | TokenType::Continue) {
                if 5 < min_bp { break; }
                p.cursor.advance(); // `or`
                let fallback = parse_or_else_rhs(p)?;
                let span = lo.merge(&p.span());
                lhs = p.alloc(Expr { kind: ExprKind::OrElse { expr: lhs, fallback }, span });
                continue;
            }
        }

        // ── Struct literal: `TypeName { field = val }` ────────────────────────
        if matches!(op, TokenType::LeftBrace) && !p.no_struct_lit && is_struct_prefix(lhs) && is_struct_open(p) {
            lhs = parse_struct_lit(p, lhs, lo)?;
            continue;
        }

        let (l_bp, r_bp) = match infix_bp(&op) {
            Some(v) => v,
            None    => break,
        };
        if l_bp < min_bp { break; }

        let op_span = p.cursor.current_span();
        p.cursor.advance();

        // ── Postfix (no RHS recursion) ────────────────────────────────────────
        match &op {
            TokenType::Dot => { lhs = parse_dot(p, lhs, op_span, lo)?; continue; }
            TokenType::LeftParen => { lhs = parse_call(p, lhs, op_span, lo)?; continue; }
            TokenType::LeftBracket => { lhs = parse_index(p, lhs, op_span, lo)?; continue; }
            TokenType::Question => {
                let span = lo.merge(&op_span);
                lhs = p.alloc(Expr { kind: ExprKind::Try(lhs), span });
                continue;
            }
            TokenType::QuestionDot => { lhs = parse_opt_chain(p, lhs, op_span, lo)?; continue; }
            _ => {}
        }

        // ── `as` type-coercion: RHS is a Type ────────────────────────────────
        if matches!(op, TokenType::As) {
            let ty   = p.parse_type_expr()?;
            let span = lo.merge(&ty.span);
            lhs = p.alloc(Expr { kind: ExprKind::As { expr: lhs, ty }, span });
            continue;
        }

        // ── Assignment ────────────────────────────────────────────────────────
        if let Some(aop) = to_assign_op(&op) {
            let value = parse_bp(p, r_bp)?;
            let span  = lo.merge(&value.span);
            lhs = p.alloc(Expr { kind: ExprKind::Assign { op: aop, target: lhs, value }, span });
            continue;
        }

        // ── Pipe |> ───────────────────────────────────────────────────────────
        if matches!(op, TokenType::PipeArrow) {
            let right = parse_bp(p, r_bp)?;
            let span  = lo.merge(&right.span);
            lhs = p.alloc(Expr { kind: ExprKind::Pipe { left: lhs, right }, span });
            continue;
        }

        // ── Binary operators ──────────────────────────────────────────────────
        if let Some(bop) = to_bin_op(&op) {
            let rhs  = parse_bp(p, r_bp)?;
            let span = lo.merge(&rhs.span);
            lhs = p.alloc(Expr { kind: ExprKind::BinOp { op: bop, lhs, rhs }, span });
            continue;
        }

        break;
    }
    Some(lhs)
}

// ── Prefix ────────────────────────────────────────────────────────────────────

fn parse_prefix<'ast, 'tok>(p: &mut Parser<'ast, 'tok>) -> Option<&'ast Expr<'ast>> {
    let lo = p.span();
    match p.cursor.peek().clone() {
        TokenType::IntLit(n)    => { p.cursor.advance(); Some(p.alloc(Expr { kind: ExprKind::Lit(Literal::Int(n)),    span: lo })) }
        TokenType::FloatLit(f)  => { p.cursor.advance(); Some(p.alloc(Expr { kind: ExprKind::Lit(Literal::Float(f)),  span: lo })) }
        TokenType::DoubleLit(d) => { p.cursor.advance(); Some(p.alloc(Expr { kind: ExprKind::Lit(Literal::Double(d)), span: lo })) }
        TokenType::StringLit(s) => { let s = p.intern(&s); p.cursor.advance(); Some(p.alloc(Expr { kind: ExprKind::Lit(Literal::Str(s)), span: lo })) }
        TokenType::VerbatimString(s) => { let s = p.intern(&s); p.cursor.advance(); Some(p.alloc(Expr { kind: ExprKind::Lit(Literal::VerbatimStr(s)), span: lo })) }
        TokenType::CharLit(c)   => { p.cursor.advance(); Some(p.alloc(Expr { kind: ExprKind::Lit(Literal::Char(c)),   span: lo })) }
        TokenType::True         => { p.cursor.advance(); Some(p.alloc(Expr { kind: ExprKind::Lit(Literal::Bool(true)),  span: lo })) }
        TokenType::False        => { p.cursor.advance(); Some(p.alloc(Expr { kind: ExprKind::Lit(Literal::Bool(false)), span: lo })) }
        TokenType::Null         => { p.cursor.advance(); Some(p.alloc(Expr { kind: ExprKind::Lit(Literal::Null),        span: lo })) }
        TokenType::InterpolatedString(parts) => parse_interp(p, parts, lo),
        TokenType::SelfKw       => { p.cursor.advance(); Some(p.alloc(Expr { kind: ExprKind::SelfExpr, span: lo })) }
        TokenType::Ident(name)  => parse_ident_expr(p, name, lo),
        // Built-in collection type names (List, Dictionary, Set, Queue,
        // Stack) are dedicated keyword tokens for parse_type.rs's benefit,
        // but they're also ordinary names in expression position —
        // `List.new()`, `Dictionary.new()`. Route through the same path
        // as a plain identifier, using the canonical spelling.
        TokenType::KwList | TokenType::KwDictionary | TokenType::KwSet
        | TokenType::KwQueue | TokenType::KwStack | TokenType::KwInlineList => {
            let name = p.cursor.peek().to_string();
            parse_ident_expr(p, name, lo)
        }
        TokenType::Minus => { p.cursor.advance(); let o = parse_bp(p, 28)?; let span = lo.merge(&o.span); Some(p.alloc(Expr { kind: ExprKind::UnaryOp { op: UnaryOp::Neg,    operand: o }, span })) }
        TokenType::Bang  => { p.cursor.advance(); let o = parse_bp(p, 28)?; let span = lo.merge(&o.span); Some(p.alloc(Expr { kind: ExprKind::UnaryOp { op: UnaryOp::Not,    operand: o }, span })) }
        TokenType::Not   => { p.cursor.advance(); let o = parse_bp(p, 28)?; let span = lo.merge(&o.span); Some(p.alloc(Expr { kind: ExprKind::UnaryOp { op: UnaryOp::Not,    operand: o }, span })) }
        TokenType::Tilde => { p.cursor.advance(); let o = parse_bp(p, 28)?; let span = lo.merge(&o.span); Some(p.alloc(Expr { kind: ExprKind::UnaryOp { op: UnaryOp::BitNot, operand: o }, span })) }
        // Borrow: `&place`/`ref place`, `&mut place`/`ref mut place`.
        // `&`/`ref` are dual spellings, same as the type-position arm in
        // parse_type.rs (docs/PARSER_RULES.md §5.6). Prefix position only
        // — Amp's infix arm (bitwise-and) is untouched, same disambiguation
        // trick already used for Minus/Bang/Tilde above.
        TokenType::Amp | TokenType::Ref => {
            p.cursor.advance();
            let mutable = p.cursor.eat(&TokenType::Mut);
            let o = parse_bp(p, 28)?; let span = lo.merge(&o.span);
            Some(p.alloc(Expr { kind: ExprKind::Borrow { mutable, place: o }, span }))
        }
        // Dereference: `*place`/`deref place`. Same dual-spelling pattern.
        TokenType::Star | TokenType::Deref => {
            p.cursor.advance();
            let o = parse_bp(p, 28)?; let span = lo.merge(&o.span);
            Some(p.alloc(Expr { kind: ExprKind::Deref(o), span }))
        }
        TokenType::Await => {
            if p.tier != TierAnnotation::High {
                p.emit(crate::error::illegal_here("await", "await is only valid in @tier(high)", lo, Some("use callback pattern for MID/LOW")));
            }
            p.cursor.advance();
            let o = parse_bp(p, 28)?; let span = lo.merge(&o.span);
            Some(p.alloc(Expr { kind: ExprKind::Await(o), span }))
        }
        TokenType::LeftParen    => parse_paren_or_tuple(p, lo),
        TokenType::LeftBracket  => parse_array_lit(p, lo),
        TokenType::LeftBrace    => parse_brace_expr(p, lo),
        TokenType::If           => parse_if_expr(p, lo),
        TokenType::Match        => parse_match_expr(p, lo),
        TokenType::Fn           => parse_lambda(p, lo),
        TokenType::Async        => {
            // async block (not async fn — that's a declaration)
            p.cursor.advance();
            let block = crate::parsers::parse_stmt::parse_block_inner(p)?;
            let block = p.alloc(block);
            let span  = lo.merge(&block.span);
            Some(p.alloc(Expr { kind: ExprKind::Block(block), span }))
        }
        TokenType::Unsafe       => {
            p.cursor.advance();
            let block = crate::parsers::parse_stmt::parse_block_inner(p)?;
            let block = p.alloc(block);
            let span  = lo.merge(&block.span);
            Some(p.alloc(Expr { kind: ExprKind::Block(block), span }))
        }
        _ => { p.expected(&["expression"]); None }
    }
}

// ── Ident / path / short-decl ─────────────────────────────────────────────────

fn parse_ident_expr<'ast, 'tok>(
    p: &mut Parser<'ast, 'tok>, name: String, lo: LSpan,
) -> Option<&'ast Expr<'ast>> {
    let name = p.intern(&name);
    p.cursor.advance();

    // Short declaration: `name := expr`
    if p.cursor.is_at(&TokenType::ColonEqual) {
        p.cursor.advance();
        let value = parse_expr(p)?;
        let span  = lo.merge(&value.span);
        return Some(p.alloc(Expr { kind: ExprKind::ShortDecl { name, value }, span }));
    }

    // Build dotted path: `std.io.File`
    let mut segs: Vec<&'ast str> = Vec::with_capacity(p.estimates.path_segs);
    segs.push(name);
    let mut hi = lo;

    while p.cursor.eat(&TokenType::Dot) {
        // Peek: if Ident (or a built-in collection keyword, which is also
        // valid as an ordinary name here) followed by `(` it's a method
        // call — break path, let postfix handle it. `where` is included
        // for the same reason `parse_field_name` (below, used by the
        // postfix `.` handler) accepts it — it's a real reserved token
        // (match-arm guards need it), but needs to double as a method
        // name too (`Linqerizer<T>.where(...)`); match-guard parsing
        // checks the raw token directly and is unaffected by this.
        let seg_name = match p.cursor.peek() {
            TokenType::Ident(s) => Some(s.clone()),
            TokenType::KwList | TokenType::KwDictionary | TokenType::KwSet
            | TokenType::KwQueue | TokenType::KwStack | TokenType::KwInlineList
            | TokenType::Where => Some(p.cursor.peek().to_string()),
            _ => None,
        };
        if let Some(seg) = seg_name {
            let seg = p.intern(&seg);
            if matches!(p.cursor.peek_nth(1), TokenType::LeftParen) {
                // Field access result; postfix loop will handle `(`
                p.cursor.advance();
                hi = p.span();
                let base = build_path(p, &segs, lo);
                let span = lo.merge(&hi);
                return Some(p.alloc(Expr { kind: ExprKind::Field { target: base, field: seg }, span }));
            }
            p.cursor.advance();
            hi = p.span();
            segs.push(seg);
        } else {
            p.expected(&["identifier after '.'"]);
            break;
        }
    }

    Some(build_path(p, &segs, lo.merge(&hi)))
}

fn build_path<'ast>(p: &Parser<'ast, '_>, segs: &[&'ast str], span: LSpan) -> &'ast Expr<'ast> {
    if segs.len() == 1 {
        p.alloc(Expr { kind: ExprKind::Ident(segs[0]), span })
    } else {
        let mut e = p.alloc(Expr { kind: ExprKind::Ident(segs[0]), span });
        for seg in &segs[1..] {
            e = p.alloc(Expr { kind: ExprKind::Field { target: e, field: seg }, span });
        }
        e
    }
}

// ── Struct literal ────────────────────────────────────────────────────────────

fn is_struct_prefix(lhs: &Expr<'_>) -> bool {
    matches!(lhs.kind, ExprKind::Ident(_) | ExprKind::Field { .. })
}

fn is_struct_open(p: &Parser<'_, '_>) -> bool {
    debug_assert!(matches!(p.cursor.peek(), TokenType::LeftBrace));
    matches!(
        (p.cursor.peek_nth(1), p.cursor.peek_nth(2)),
        (TokenType::RightBrace, _) | (TokenType::Ident(_), TokenType::Equal)
    )
}

fn extract_path_from_expr<'ast>(_p: &Parser<'ast, '_>, e: &'ast Expr<'ast>) -> Vec<&'ast str> {
    let mut segs = Vec::new();
    fn collect<'a>(e: &'a Expr<'a>, segs: &mut Vec<&'a str>) {
        match e.kind {
            ExprKind::Ident(n) => segs.push(n),
            ExprKind::Field { target, field } => { collect(target, segs); segs.push(field); }
            _ => {}
        }
    }
    collect(e, &mut segs);
    segs
}

fn parse_struct_lit<'ast, 'tok>(
    p: &mut Parser<'ast, 'tok>, lhs: &'ast Expr<'ast>, lo: LSpan,
) -> Option<&'ast Expr<'ast>> {
    let path_vec = extract_path_from_expr(p, lhs);
    let path     = p.arena.alloc_slice_copy(path_vec.as_slice());
    let open     = p.span();
    p.cursor.advance(); // `{`
    let mut fields: Vec<FieldInit<'ast>> = Vec::with_capacity(p.estimates.struct_fields);
    while !p.cursor.is_at(&TokenType::RightBrace) && !p.cursor.is_eof() {
        let flo = p.span();
        let (name, _) = p.expect_ident()?;
        if let Err(e) = p.cursor.expect(&TokenType::Equal) { p.emit(crate::error::from_cursor(e, ParseContext::Expr)); break; }
        let value = parse_expr(p)?;
        fields.push(FieldInit { name, value, span: flo.merge(&value.span) });
        p.eat_sep();
    }
    if !p.cursor.eat(&TokenType::RightBrace) { p.emit(crate::error::unclosed('{', open, None, p.span())); }
    let span   = lo.merge(&p.span());
    let fields = p.arena.alloc_slice_copy(fields.as_slice());
    Some(p.alloc(Expr { kind: ExprKind::StructLit { path, fields }, span }))
}

// ── Brace expr: Dict / AnonObject / Block ─────────────────────────────────────

fn parse_brace_expr<'ast, 'tok>(p: &mut Parser<'ast, 'tok>, lo: LSpan) -> Option<&'ast Expr<'ast>> {
    debug_assert!(matches!(p.cursor.peek(), TokenType::LeftBrace));
    let t1 = p.cursor.peek_nth(1).clone();
    let t2 = p.cursor.peek_nth(2).clone();
    match (&t1, &t2) {
        // `{ Ident = ...}` → AnonObject
        (TokenType::Ident(_), TokenType::Equal) => parse_anon_object(p, lo),
        // `{ }` → empty AnonObject
        (TokenType::RightBrace, _) => {
            p.cursor.advance(); p.cursor.advance(); // { }
            let span = lo.merge(&p.span());
            Some(p.alloc(Expr { kind: ExprKind::AnonObject(&[]), span }))
        }
        // `{ StringLit = ...}` or `{ IntLit = ...}` → Dict
        (TokenType::StringLit(_), TokenType::Equal) |
        (TokenType::IntLit(_),    TokenType::Equal) |
        (TokenType::DoubleLit(_), TokenType::Equal) |
        (TokenType::FloatLit(_),  TokenType::Equal) |
        (TokenType::True,         TokenType::Equal) |
        (TokenType::False,        TokenType::Equal) => parse_dict(p, lo),
        // Everything else → block expression
        _ => {
            let block = crate::parsers::parse_stmt::parse_block_inner(p)?;
            let block = p.alloc(block);
            let span  = lo.merge(&block.span);
            Some(p.alloc(Expr { kind: ExprKind::Block(block), span }))
        }
    }
}

fn parse_dict<'ast, 'tok>(p: &mut Parser<'ast, 'tok>, lo: LSpan) -> Option<&'ast Expr<'ast>> {
    let open = p.span(); p.cursor.advance(); // `{`
    let mut entries: Vec<DictEntry<'ast>> = Vec::with_capacity(p.estimates.call_args);
    while !p.cursor.is_at(&TokenType::RightBrace) && !p.cursor.is_eof() {
        let elo = p.span();
        // min_bp = 3, not the usual 0 (i.e. not the public parse_expr(p)).
        // Assignment `=` is a valid infix operator in the general
        // expression grammar — parsing the key with min_bp=0 would let it
        // greedily consume `key = value` as one Assign expression before
        // control ever reached the explicit expect(Equal) below, which is
        // exactly what made single- and multi-entry dict literals fail to
        // parse before this fix (see docs/MEMORY_MODEL.md). Note this
        // file's own infix_bp table gives assignment (l_bp=2, r_bp=1) —
        // reversed from the header comment's documented "L=1 R=2" for
        // right-associativity — so min_bp needs to be 3, one above
        // assignment's *actual* l_bp of 2, not 2. Range (l_bp=3) and
        // everything looser still works fine in key position.
        let key = parse_bp(p, 3)?;
        if let Err(e) = p.cursor.expect(&TokenType::Equal) { p.emit(crate::error::from_cursor(e, ParseContext::Expr)); break; }
        let value = parse_expr(p)?;
        entries.push(DictEntry { key, value, span: elo.merge(&value.span) });
        p.eat_sep();
    }
    if !p.cursor.eat(&TokenType::RightBrace) { p.emit(crate::error::unclosed('{', open, None, p.span())); }
    let span    = lo.merge(&p.span());
    let entries = p.arena.alloc_slice_copy(entries.as_slice());
    Some(p.alloc(Expr { kind: ExprKind::Dict(entries), span }))
}

fn parse_anon_object<'ast, 'tok>(p: &mut Parser<'ast, 'tok>, lo: LSpan) -> Option<&'ast Expr<'ast>> {
    let open = p.span(); p.cursor.advance(); // `{`
    let mut fields: Vec<ObjectField<'ast>> = Vec::with_capacity(p.estimates.struct_fields);
    while !p.cursor.is_at(&TokenType::RightBrace) && !p.cursor.is_eof() {
        let flo = p.span();
        let (name, _) = p.expect_ident()?;
        if let Err(e) = p.cursor.expect(&TokenType::Equal) { p.emit(crate::error::from_cursor(e, ParseContext::Expr)); break; }
        let value = parse_expr(p)?;
        fields.push(ObjectField { name, value, span: flo.merge(&value.span) });
        p.eat_sep();
    }
    if !p.cursor.eat(&TokenType::RightBrace) { p.emit(crate::error::unclosed('{', open, None, p.span())); }
    let span   = lo.merge(&p.span());
    let fields = p.arena.alloc_slice_copy(fields.as_slice());
    Some(p.alloc(Expr { kind: ExprKind::AnonObject(fields), span }))
}

// ── Paren / tuple ─────────────────────────────────────────────────────────────

fn parse_paren_or_tuple<'ast, 'tok>(p: &mut Parser<'ast, 'tok>, lo: LSpan) -> Option<&'ast Expr<'ast>> {
    let open = p.span(); p.cursor.advance(); // `(`
    if p.cursor.eat(&TokenType::RightParen) {
        let span = lo.merge(&p.span());
        return Some(p.alloc(Expr { kind: ExprKind::Tuple(&[]), span }));
    }
    let prev_nsl = p.clear_no_struct_lit();
    let result = parse_paren_or_tuple_inner(p, lo, open);
    p.leave_no_struct_lit(prev_nsl);
    result
}

fn parse_paren_or_tuple_inner<'ast, 'tok>(
    p: &mut Parser<'ast, 'tok>, lo: LSpan, open: LSpan,
) -> Option<&'ast Expr<'ast>> {
    let first = parse_expr(p)?;
    if p.cursor.eat(&TokenType::RightParen) { return Some(first); } // grouped
    let mut elems: Vec<&'ast Expr<'ast>> = Vec::with_capacity(4);
    elems.push(first);
    while p.cursor.eat(&TokenType::Comma) {
        if p.cursor.is_at(&TokenType::RightParen) { break; }
        elems.push(parse_expr(p)?);
    }
    if !p.cursor.eat(&TokenType::RightParen) { p.emit(crate::error::unclosed('(', open, None, p.span())); }
    let span  = lo.merge(&p.span());
    let elems = p.arena.alloc_slice_copy(elems.as_slice());
    Some(p.alloc(Expr { kind: ExprKind::Tuple(elems), span }))
}

// ── Array literal ─────────────────────────────────────────────────────────────

fn parse_array_lit<'ast, 'tok>(p: &mut Parser<'ast, 'tok>, lo: LSpan) -> Option<&'ast Expr<'ast>> {
    let open = p.span(); p.cursor.advance(); // `[`
    let prev_nsl = p.clear_no_struct_lit();
    let mut elems: Vec<&'ast Expr<'ast>> = Vec::with_capacity(p.estimates.call_args);
    let mut ok = true;
    while !p.cursor.is_at(&TokenType::RightBracket) && !p.cursor.is_eof() {
        match parse_expr(p) {
            Some(e) => { elems.push(e); p.eat_sep(); }
            None => { ok = false; break; }
        }
    }
    p.leave_no_struct_lit(prev_nsl);
    if !ok { return None; }
    if !p.cursor.eat(&TokenType::RightBracket) { p.emit(crate::error::unclosed('[', open, None, p.span())); }
    let span  = lo.merge(&p.span());
    let elems = p.arena.alloc_slice_copy(elems.as_slice());
    Some(p.alloc(Expr { kind: ExprKind::Array(elems), span }))
}

// ── If expression ─────────────────────────────────────────────────────────────

fn parse_if_expr<'ast, 'tok>(p: &mut Parser<'ast, 'tok>, lo: LSpan) -> Option<&'ast Expr<'ast>> {
    p.cursor.advance(); // `if`
    let prev_nsl  = p.enter_no_struct_lit();
    let condition = parse_expr(p);
    p.leave_no_struct_lit(prev_nsl);
    let condition = condition?;
    let then_body = crate::parsers::parse_stmt::parse_if_branch_body(p)?;
    let mut elif_branches: Vec<ElifBranch<'ast>> = Vec::with_capacity(2);
    while p.cursor.eat(&TokenType::Elif) {
        let blo  = p.span();
        let prev_nsl = p.enter_no_struct_lit();
        let cond = parse_expr(p);
        p.leave_no_struct_lit(prev_nsl);
        let cond = cond?;
        let body = crate::parsers::parse_stmt::parse_if_branch_body(p)?;
        let bspan = match &body {
            IfBranchBody::Block(b) => b.span,
            IfBranchBody::Expr(e)  => e.span,
        };
        elif_branches.push(ElifBranch { condition: cond, body, span: blo.merge(&bspan) });
    }
    let else_body = if p.cursor.eat(&TokenType::Else) {
        Some(crate::parsers::parse_stmt::parse_if_branch_body(p)?)
    } else { None };
    let span          = lo.merge(&p.span());
    let elif_branches = p.arena.alloc_slice_copy(elif_branches.as_slice());
    let node          = p.alloc(IfExpr { condition, then_body, elif_branches, else_body, span });
    Some(p.alloc(Expr { kind: ExprKind::If(node), span }))
}

// ── Match expression ──────────────────────────────────────────────────────────

fn parse_match_expr<'ast, 'tok>(p: &mut Parser<'ast, 'tok>, lo: LSpan) -> Option<&'ast Expr<'ast>> {
    let prev = p.enter(ParseContext::MatchArm);
    p.cursor.advance(); // `match`
    let prev_nsl  = p.enter_no_struct_lit();
    let scrutinee = parse_expr(p);
    p.leave_no_struct_lit(prev_nsl);
    let scrutinee = match scrutinee { Some(s) => s, None => { p.leave(prev); return None; } };
    let open = p.span();
    if let Err(e) = p.cursor.expect(&TokenType::LeftBrace) { p.emit(crate::error::from_cursor(e, ParseContext::MatchArm)); p.leave(prev); return None; }
    let mut arms: Vec<MatchArm<'ast>> = Vec::with_capacity(p.estimates.match_arms);
    while !p.cursor.is_at(&TokenType::RightBrace) && !p.cursor.is_eof() {
        let pos_before = p.cursor.position();
        if let Some(arm) = parse_match_arm(p) { arms.push(arm); } else { p.recover_to_stmt(); }
        p.eat_sep();
        p.guard_progress(pos_before);
    }
    if !p.cursor.eat(&TokenType::RightBrace) { p.emit(crate::error::unclosed('{', open, None, p.span())); }
    let span = lo.merge(&p.span());
    let arms = p.arena.alloc_slice_copy(arms.as_slice());
    let node = p.alloc(MatchExpr { scrutinee, arms, span });
    p.leave(prev);
    Some(p.alloc(Expr { kind: ExprKind::Match(node), span }))
}

fn parse_match_arm<'ast, 'tok>(p: &mut Parser<'ast, 'tok>) -> Option<MatchArm<'ast>> {
    let lo      = p.span();
    let pattern = crate::parsers::parse_pattern::parse_pattern(p)?;
    let guard   = if p.cursor.eat(&TokenType::Where) { Some(parse_expr(p)?) } else { None };
    // `then` is an accepted alternate spelling for `=>` here — purely
    // stylistic, since match arms already support brace-free single
    // expressions via `=>` (`Some(x) => x`); `then` doesn't change the
    // body-parsing rules below, just the separator token.
    if !p.cursor.eat(&TokenType::FatArrow) && !p.cursor.eat(&TokenType::Then) {
        let found = p.cursor.peek_token();
        p.emit(crate::error::unexpected(found, &["'=>'", "'then'"], ParseContext::MatchArm));
        return None;
    }
    let body = crate::parsers::parse_stmt::parse_match_arm_body(p)?;
    Some(MatchArm { pattern, guard, body, span: lo.merge(&p.span()) })
}

// ── Lambda ────────────────────────────────────────────────────────────────────

fn parse_lambda<'ast, 'tok>(p: &mut Parser<'ast, 'tok>, lo: LSpan) -> Option<&'ast Expr<'ast>> {
    p.cursor.advance(); // `fn`
    let open = p.span();
    if let Err(e) = p.cursor.expect(&TokenType::LeftParen) { p.emit(crate::error::from_cursor(e, ParseContext::FunctionParam)); return None; }
    let mut params: Vec<LambdaParam<'ast>> = Vec::with_capacity(p.estimates.fn_params);
    while !p.cursor.is_at(&TokenType::RightParen) && !p.cursor.is_eof() {
        let plo = p.span();
        let _   = p.cursor.eat(&TokenType::Mut);
        let (name, _) = p.expect_ident()?;
        let ty  = crate::parsers::parse_decl::parse_type_annotation_opt(p);
        params.push(LambdaParam { name, ty, span: plo.merge(&p.span()) }); p.eat_sep();
    }
    if !p.cursor.eat(&TokenType::RightParen) { p.emit(crate::error::unclosed('(', open, None, p.span())); }
    let body = if p.cursor.is_at(&TokenType::LeftBrace) {
        LambdaBody::Block(crate::parsers::parse_stmt::parse_block_inner(p)?)
    } else {
        LambdaBody::Expr(parse_expr(p)?)
    };
    let span   = lo.merge(&p.span());
    let params = p.arena.alloc_slice_copy(params.as_slice());
    let node   = p.alloc(Lambda { params, body, span });
    Some(p.alloc(Expr { kind: ExprKind::Lambda(node), span }))
}

// ── LINQ query ────────────────────────────────────────────────────────────────
// Removed: `from x in expr where ... select ...` was fully wired
// end-to-end (parser, name resolution, type inference, tier checking,
// interpreter) but eager, hardcoded to `Value::List` only, `groupby`
// was an unimplemented no-op, and it had zero fixture coverage — never
// actually run. Replaced by `Linqerizer<T>`, a real value/type with
// chainable, lazy query methods, rather than dedicated expression-level
// grammar. See docs/PARSER_RULES.md §6 for the reasoning.

// ── Format spec ───────────────────────────────────────────────────────────
//
// `[[fill]align] [sign] ['#'] [width] ['.' precision] ['?' | base]`, a
// deliberately separate, tiny grammar from the rest of the expression
// parser (see docs/PRINT_FORMAT_RULES.md §4). Runs on the same token
// stream `parse_expr` above already consumed part of, continuing from
// wherever the leftover `:` was.

fn parse_format_spec<'ast, 'tok>(p: &mut Parser<'ast, 'tok>) -> Option<FormatSpec> {
    // `[[fill]align]`: a fill character is only ever recognized when the
    // token right after it is an align marker; otherwise it's ambiguous
    // with everything else a spec can hold. Restricted to tokens whose own
    // lexeme is exactly one character and aren't `Ident`/a numeric literal
    // (so a fill char can never collide with `x`/`X`/`o`/`b` in the
    // trailing base position, or with an ordinary width digit).
    let mut fill: Option<char> = None;
    let mut align: Option<Align> = None;
    let next_is_align = matches!(p.cursor.peek_nth(1),
        TokenType::Less | TokenType::Greater | TokenType::Caret);
    if next_is_align && !matches!(p.cursor.peek(),
        TokenType::Ident(_) | TokenType::IntLit(_) | TokenType::FloatLit(_) | TokenType::DoubleLit(_))
    {
        let lexeme: &'tok str = p.cursor.peek_token().lexeme.as_str();
        let mut chars = lexeme.chars();
        if let (Some(c), None) = (chars.next(), chars.next()) {
            fill = Some(c);
            p.cursor.advance();
        }
    }
    match p.cursor.peek() {
        TokenType::Less    => { align = Some(Align::Left);   p.cursor.advance(); }
        TokenType::Greater => { align = Some(Align::Right);  p.cursor.advance(); }
        TokenType::Caret   => { align = Some(Align::Center); p.cursor.advance(); }
        _ => {}
    }

    // `[sign]`: `+` forces a sign on positive numbers too.
    let sign_plus = p.cursor.eat(&TokenType::Plus);

    // `['#']`: the alternate form. Only means something alongside a
    // trailing base (checked at TYPE-119, not here; this function only
    // parses, sema decides whether the combination makes sense).
    let alternate = p.cursor.eat(&TokenType::Hash);

    // `{value:10.2}` (width 10, precision 2, no space between them) is
    // genuinely ambiguous at the TOKEN level, not just visually: the
    // general tokenizer has no notion of "format spec" context, so
    // `10.2` with a digit immediately before the dot lexes as one
    // Float/DoubleLit token, same as it would anywhere else in the
    // language (`{value:.2}` alone is fine, a leading dot with no
    // digit before it never forms a numeric literal, confirmed via the
    // token stream in earlier testing). Recover width/precision from the
    // literal's own source text rather than its parsed f32/f64 value,
    // since a float's fractional part doesn't reliably round-trip back
    // to "how many digits were written" (`10.20` and `10.2` are the same
    // f64 but different precisions). The same source text also carries
    // whether the width half had a genuine zero-pad leading zero
    // (`010.2`) versus an ordinary width that happens to start past zero.
    if let TokenType::FloatLit(_) | TokenType::DoubleLit(_) = p.cursor.peek() {
        let lexeme = p.cursor.peek_token().lexeme.trim_end_matches(['f', 'F']);
        if let Some((w, prec)) = lexeme.split_once('.') {
            if let (Ok(w_val), Ok(prec)) = (w.parse::<u32>(), prec.parse::<u32>()) {
                let zero_pad = w.len() > 1 && w.starts_with('0');
                p.cursor.advance();
                let Some((debug, base)) = parse_format_trailer(p) else { return None; };
                if !p.cursor.is_eof() {
                    p.emit(crate::error::illegal_here(
                        "format spec", "unexpected trailing content",
                        p.cursor.current_span(),
                        Some("expected `[[fill]align][sign]['#']width[.precision][?|base]`, e.g. `{value:*>+#010x}`"),
                    ));
                    return None;
                }
                return Some(FormatSpec {
                    fill, align, sign_plus, alternate, zero_pad,
                    width: Some(w_val), precision: Some(prec), debug, base,
                });
            }
        }
    }

    let (zero_pad, width) = if let TokenType::IntLit(n) = p.cursor.peek() {
        let n = *n;
        if n < 0 {
            p.emit(crate::error::illegal_here(
                "format spec", "width cannot be negative", p.cursor.current_span(), None,
            ));
            return None;
        }
        let lexeme: &'tok str = p.cursor.peek_token().lexeme.as_str();
        let zero_pad = lexeme.len() > 1 && lexeme.starts_with('0');
        p.cursor.advance();
        (zero_pad, Some(n as u32))
    } else {
        (false, None)
    };

    let precision = if p.cursor.is_at(&TokenType::Dot) {
        p.cursor.advance();
        match p.cursor.peek() {
            TokenType::IntLit(n) if *n >= 0 => {
                let n = *n;
                p.cursor.advance();
                Some(n as u32)
            }
            _ => {
                p.emit(crate::error::illegal_here(
                    "format spec", "expected a non-negative integer after `.`",
                    p.cursor.current_span(), Some("e.g. `{value:.2}`"),
                ));
                return None;
            }
        }
    } else {
        None
    };

    let Some((debug, base)) = parse_format_trailer(p) else { return None; };

    if !p.cursor.is_eof() {
        p.emit(crate::error::illegal_here(
            "format spec", "unexpected trailing content",
            p.cursor.current_span(),
            Some("expected `[[fill]align][sign]['#']width[.precision][?|base]`, e.g. `{value:*>+#010x}`"),
        ));
        return None;
    }

    Some(FormatSpec { fill, align, sign_plus, alternate, zero_pad, width, precision, debug, base })
}

/// `['?' | base]`: trailing debug flag or numeric base, mutually
/// exclusive (an `Int`, the only type `base` accepts, never diverges
/// between `Display` and `debug_string` in the first place, so asking
/// for both at once has nothing to actually pick between). Returns
/// `None` (already emitted the error) only when both appear together.
fn parse_format_trailer<'ast, 'tok>(p: &mut Parser<'ast, 'tok>) -> Option<(bool, Option<NumericBase>)> {
    let debug = p.cursor.eat(&TokenType::Question);

    let base = if let TokenType::Ident(name) = p.cursor.peek() {
        let b = match name.as_str() {
            "x" => Some(NumericBase::Hex),
            "X" => Some(NumericBase::HexUpper),
            "o" => Some(NumericBase::Octal),
            "b" => Some(NumericBase::Binary),
            _ => None,
        };
        if b.is_some() { p.cursor.advance(); }
        b
    } else {
        None
    };

    if debug && base.is_some() {
        p.emit(crate::error::illegal_here(
            "format spec", "`?` and a numeric base can't both apply",
            p.cursor.current_span(),
            Some("pick one: `{value:?}` or `{value:x}` (or `X`/`o`/`b`)"),
        ));
        return None;
    }
    Some((debug, base))
}

// ── Interpolated string ───────────────────────────────────────────────────────

fn parse_interp<'ast, 'tok>(p: &mut Parser<'ast, 'tok>, parts: Vec<LexPart>, lo: LSpan) -> Option<&'ast Expr<'ast>> {
    let mut ast_parts: Vec<InterpolationPart<'ast>> = Vec::with_capacity(parts.len());
    for part in &parts {
        match part {
            LexPart::Text(t) => ast_parts.push(InterpolationPart::Text(p.intern(t))),
            LexPart::Expr(tokens) => {
                // The lexer already tokenized this hole's own source range
                // (see string_parser.rs::parse_interpolation_expr) — sub-parse
                // those tokens into a real expression right now, the same way
                // everything else in the file gets parsed. Shares `p.arena` so
                // the result is valid for 'ast; the sub-parser's own cursor
                // and error manager are local to just this one hole.
                let mut sub = Parser::new(p.arena, tokens, String::new());
                match parse_expr(&mut sub) {
                    Some(expr) => {
                        // A format spec (`{value:>10.2}`) is found by
                        // checking for a LEFTOVER `:` after `parse_expr`
                        // above has already consumed everything it
                        // recognizes as part of the expression — not by
                        // scanning for `:` in the lexer. (Ubel Stratum has
                        // no ternary operator to collide with here — `?`
                        // is the fallible-unwrap postfix, same as Rust's
                        // `?`, not a conditional; there's no `cond ? a :
                        // b` in this language.) The general reason this
                        // still has to happen at the parser level: only
                        // the parser actually knows where an expression
                        // ends — the lexer has no notion of expression
                        // structure at all.
                        let spec = if sub.cursor.is_at(&TokenType::Colon) {
                            sub.cursor.advance();
                            match parse_format_spec(&mut sub) {
                                Some(s) => Some(s),
                                None => {
                                    for err in sub.errors.take_parse_errors() { p.emit(err); }
                                    return None;
                                }
                            }
                        } else if !sub.cursor.is_eof() {
                            // Previously silently discarded — trailing
                            // tokens after a hole's expression (anything
                            // that isn't a format-spec `:`) just vanished
                            // with no error. Real, pre-existing gap, found
                            // while wiring format specs through this exact
                            // spot; fixed alongside since leaving it here
                            // once diagnosed would be an odd thing to walk
                            // past.
                            p.emit(crate::error::illegal_here(
                                "interpolation hole",
                                "unexpected trailing tokens after the expression",
                                sub.cursor.current_span(),
                                Some("did you mean to add a `:` format spec?"),
                            ));
                            return None;
                        } else {
                            None
                        };
                        ast_parts.push(InterpolationPart::Expr { expr, spec });
                    }
                    None => {
                        let sub_errors = sub.errors.take_parse_errors();
                        if sub_errors.is_empty() {
                            p.emit(crate::error::illegal_here(
                                "interpolation hole",
                                "expression could not be parsed",
                                lo,
                                None,
                            ));
                        } else {
                            for err in sub_errors {
                                p.emit(err);
                            }
                        }
                        return None;
                    }
                }
            }
        }
    }
    p.cursor.advance();
    let span  = lo.merge(&p.span());
    let parts = p.arena.alloc_slice_copy(ast_parts.as_slice());
    Some(p.alloc(Expr { kind: ExprKind::Lit(Literal::InterpolatedStr(parts)), span }))
}

// ── Postfix helpers ───────────────────────────────────────────────────────────

fn parse_dot<'ast, 'tok>(p: &mut Parser<'ast, 'tok>, target: &'ast Expr<'ast>, _op: LSpan, lo: LSpan) -> Option<&'ast Expr<'ast>> {
    let (field, fspan) = parse_field_name(p)?;
    if p.cursor.is_at(&TokenType::LeftParen) {
        let callee = p.alloc(Expr { kind: ExprKind::Field { target, field }, span: lo.merge(&fspan) });
        parse_call(p, callee, p.span(), lo)
    } else {
        Some(p.alloc(Expr { kind: ExprKind::Field { target, field }, span: lo.merge(&fspan) }))
    }
}

/// Like `p.expect_ident()`, but also accepts `where` specifically in
/// field/method-name position (`.where(...)`) — `where` is a real
/// reserved token (match-arm guards need it: `parse_match_arm`'s own
/// `p.cursor.eat(&TokenType::Where)` checks the raw token directly and
/// is completely unaffected by this), but `Linqerizer<T>.where(...)`
/// needs the same word usable as a method name too. Deliberately not
/// folded into `eat_ident()` itself — that's used far more broadly
/// (variable names, parameters, ...) where widening what counts as an
/// identifier is a bigger, less-audited change than this narrow spot
/// actually needs. Mirrors the equivalent exception already added to
/// the prefix dotted-path builder above.
fn parse_field_name<'ast, 'tok>(p: &mut Parser<'ast, 'tok>) -> Option<(&'ast str, LSpan)> {
    if let Some(id) = p.eat_ident() { return Some(id); }
    if matches!(p.cursor.peek(), TokenType::Where) {
        let span = p.cursor.current_span();
        p.cursor.advance();
        return Some((p.intern("where"), span));
    }
    p.expected(&["identifier after '.'"]);
    None
}

fn parse_call<'ast, 'tok>(p: &mut Parser<'ast, 'tok>, callee: &'ast Expr<'ast>, open_sp: LSpan, lo: LSpan) -> Option<&'ast Expr<'ast>> {
    // `(` already consumed by caller OR we need to consume it here
    let open = if p.cursor.is_at(&TokenType::LeftParen) { p.cursor.advance().span } else { open_sp };
    let prev_nsl = p.clear_no_struct_lit();
    let mut args: Vec<Arg<'ast>> = Vec::with_capacity(p.estimates.call_args);
    let mut ok = true;
    while !p.cursor.is_at(&TokenType::RightParen) && !p.cursor.is_eof() {
        match parse_arg(p) {
            Some(a) => { args.push(a); p.eat_sep(); }
            None => { ok = false; break; }
        }
    }
    p.leave_no_struct_lit(prev_nsl);
    if !ok { return None; }
    if !p.cursor.eat(&TokenType::RightParen) { p.emit(crate::error::unclosed('(', open, None, p.span())); }
    let span = lo.merge(&p.span());
    let args = p.arena.alloc_slice_copy(args.as_slice());
    Some(p.alloc(Expr { kind: ExprKind::Call { callee, args }, span }))
}

fn parse_arg<'ast, 'tok>(p: &mut Parser<'ast, 'tok>) -> Option<Arg<'ast>> {
    let lo = p.span();
    if let TokenType::Ident(name) = p.cursor.peek().clone() {
        if matches!(p.cursor.peek_nth(1), TokenType::Equal) {
            let name = p.intern(&name); p.cursor.advance(); p.cursor.advance();
            let value = parse_expr(p)?;
            return Some(Arg { kind: ArgKind::Named { name, value }, span: lo.merge(&value.span) });
        }
    }
    let e = parse_expr(p)?;
    Some(Arg { kind: ArgKind::Positional(e), span: lo.merge(&e.span) })
}

fn parse_index<'ast, 'tok>(p: &mut Parser<'ast, 'tok>, target: &'ast Expr<'ast>, open_sp: LSpan, lo: LSpan) -> Option<&'ast Expr<'ast>> {
    let prev_nsl = p.clear_no_struct_lit();
    let index = parse_expr(p);
    p.leave_no_struct_lit(prev_nsl);
    let index = index?;
    if !p.cursor.eat(&TokenType::RightBracket) { p.emit(crate::error::unclosed('[', open_sp, None, p.span())); }
    let span = lo.merge(&p.span());
    Some(p.alloc(Expr { kind: ExprKind::Index { target, index }, span }))
}

fn parse_opt_chain<'ast, 'tok>(p: &mut Parser<'ast, 'tok>, target: &'ast Expr<'ast>, _op: LSpan, lo: LSpan) -> Option<&'ast Expr<'ast>> {
    let (name, _) = p.expect_ident()?;
    let access = if p.cursor.is_at(&TokenType::LeftParen) {
        let open = p.cursor.advance().span;
        let mut args: Vec<Arg<'ast>> = Vec::with_capacity(p.estimates.call_args);
        while !p.cursor.is_at(&TokenType::RightParen) && !p.cursor.is_eof() { args.push(parse_arg(p)?); p.eat_sep(); }
        if !p.cursor.eat(&TokenType::RightParen) { p.emit(crate::error::unclosed('(', open, None, p.span())); }
        OptionalAccess::Method { name, args: p.arena.alloc_slice_copy(args.as_slice()) }
    } else {
        OptionalAccess::Field(name)
    };
    let span = lo.merge(&p.span());
    Some(p.alloc(Expr { kind: ExprKind::OptionalChain { target, access }, span }))
}

// ── OrElse RHS ────────────────────────────────────────────────────────────────

fn parse_or_else_rhs<'ast, 'tok>(p: &mut Parser<'ast, 'tok>) -> Option<OrElseFallback<'ast>> {
    match p.cursor.peek().clone() {
        TokenType::Return   => { p.cursor.advance(); let v = if p.can_start_expr() { Some(parse_expr(p)?) } else { None }; Some(OrElseFallback::Return(v)) }
        TokenType::Break    => { p.cursor.advance(); Some(OrElseFallback::Break) }
        TokenType::Continue => { p.cursor.advance(); Some(OrElseFallback::Continue) }
        _                   => { Some(OrElseFallback::Expr(parse_expr(p)?)) }
    }
}

// ── Binding power / operator tables ──────────────────────────────────────────

#[inline]
fn infix_bp(tt: &TokenType) -> Option<(u8, u8)> {
    match tt {
        // Postfix
        TokenType::Dot | TokenType::LeftParen | TokenType::LeftBracket |
        TokenType::Question | TokenType::QuestionDot => Some((29, 30)),
        // Assignment — right-assoc (r_bp < l_bp)
        TokenType::Equal | TokenType::PlusEqual | TokenType::MinusEqual |
        TokenType::StarEqual | TokenType::SlashEqual | TokenType::PercentEqual |
        TokenType::AmpEqual | TokenType::PipeEqual | TokenType::CaretEqual |
        TokenType::LeftShiftEqual | TokenType::RightShiftEqual => Some((2, 1)),
        TokenType::DotDot | TokenType::DotDotEqual => Some((3, 4)),
        TokenType::Or  | TokenType::PipePipe  => Some((5, 6)),
        TokenType::And | TokenType::AmpAmp    => Some((7, 8)),
        TokenType::EqualEqual | TokenType::BangEqual => Some((9, 10)),
        TokenType::Less | TokenType::Greater | TokenType::LessEqual | TokenType::GreaterEqual => Some((11, 12)),
        TokenType::PipeArrow => Some((13, 14)),
        TokenType::Pipe      => Some((15, 16)),
        TokenType::Caret     => Some((17, 18)),
        TokenType::Amp       => Some((19, 20)),
        TokenType::LeftShift | TokenType::RightShift => Some((21, 22)),
        TokenType::Plus | TokenType::Minus => Some((23, 24)),
        TokenType::Star | TokenType::Slash | TokenType::Percent => Some((25, 26)),
        TokenType::As => Some((27, 28)),
        _ => None,
    }
}

fn to_bin_op(tt: &TokenType) -> Option<BinOp> {
    match tt {
        TokenType::Plus          => Some(BinOp::Add),   TokenType::Minus     => Some(BinOp::Sub),
        TokenType::Star          => Some(BinOp::Mul),   TokenType::Slash     => Some(BinOp::Div),
        TokenType::Percent       => Some(BinOp::Rem),   TokenType::Amp       => Some(BinOp::BitAnd),
        TokenType::Pipe          => Some(BinOp::BitOr), TokenType::Caret     => Some(BinOp::BitXor),
        TokenType::LeftShift     => Some(BinOp::Shl),   TokenType::RightShift => Some(BinOp::Shr),
        TokenType::EqualEqual    => Some(BinOp::Eq),    TokenType::BangEqual => Some(BinOp::Ne),
        TokenType::Less          => Some(BinOp::Lt),    TokenType::LessEqual => Some(BinOp::Le),
        TokenType::Greater       => Some(BinOp::Gt),    TokenType::GreaterEqual => Some(BinOp::Ge),
        TokenType::And | TokenType::AmpAmp  => Some(BinOp::And),
        TokenType::Or  | TokenType::PipePipe => Some(BinOp::Or),
        TokenType::DotDot        => Some(BinOp::Range), TokenType::DotDotEqual => Some(BinOp::RangeIncl),
        _ => None,
    }
}

fn to_assign_op(tt: &TokenType) -> Option<AssignOp> {
    match tt {
        TokenType::Equal           => Some(AssignOp::Assign),
        TokenType::PlusEqual       => Some(AssignOp::AddAssign),
        TokenType::MinusEqual      => Some(AssignOp::SubAssign),
        TokenType::StarEqual       => Some(AssignOp::MulAssign),
        TokenType::SlashEqual      => Some(AssignOp::DivAssign),
        TokenType::PercentEqual    => Some(AssignOp::RemAssign),
        TokenType::AmpEqual        => Some(AssignOp::BitAndAssign),
        TokenType::PipeEqual       => Some(AssignOp::BitOrAssign),
        TokenType::CaretEqual      => Some(AssignOp::BitXorAssign),
        TokenType::LeftShiftEqual  => Some(AssignOp::ShlAssign),
        TokenType::RightShiftEqual => Some(AssignOp::ShrAssign),
        _ => None,
    }
            }
