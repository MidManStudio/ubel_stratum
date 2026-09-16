// crates/rd_parser/src/parsers/parse_stmt.rs
//
// Corrected version — uses parse_pattern::{parse_binding_target, parse_destructure_pattern}
// for `let`, `for`, and `extract`. All AllocatorKind and Block types match statements.rs.

use ubel_stratum::{
    ast::{
        common::AssignOp,
        expressions::{ElifBranch, IfBranchBody, IfExpr, MatchArm, MatchArmBody},
        statements::{
            AllocatorKind, BindingTarget, Block, SizeExpr,
            Stmt, StmtKind, UsingBinding,
        },
    },
    error_management::errors::ParseContext,
    lexer::TokenType,
};

use crate::parser::Parser;
use crate::parsers::{parse_expr, parse_pattern};

impl<'ast, 'tok> Parser<'ast, 'tok> {
    pub(crate) fn parse_block(&mut self) -> Option<Block<'ast>> {
        parse_block_inner(self)
    }
}

pub(crate) fn parse_block_inner<'ast, 'tok>(p: &mut Parser<'ast, 'tok>) -> Option<Block<'ast>> {
    let prev = p.enter(ParseContext::Block);
    let lo   = p.span();

    let open_span = lo;
    if let Err(e) = p.cursor.expect(&TokenType::LeftBrace) {
        p.emit(crate::error::from_cursor(e, ParseContext::Block));
        p.leave(prev);
        return None;
    }

    let mut stmts: Vec<Stmt<'ast>> = Vec::with_capacity(p.estimates.block_stmts);

    while !p.cursor.is_at(&TokenType::RightBrace) && !p.cursor.is_eof() {
        while p.cursor.eat(&TokenType::Semicolon) {}
        if p.cursor.is_at(&TokenType::RightBrace) { break; }
        let pos_before = p.cursor.position();
        if let Some(stmt) = parse_stmt(p) {
            stmts.push(stmt);
        } else {
            p.recover_to_stmt();
        }
        p.guard_progress(pos_before);
    }

    let close = p.span();
    if !p.cursor.eat(&TokenType::RightBrace) {
        p.emit(crate::error::unclosed('{', open_span, None, close));
    }

    p.leave(prev);
    let span  = lo.merge(&close);
    let stmts = p.arena.alloc_slice_clone(&stmts);
    Some(Block { stmts, span })
}

// ── Dispatcher ────────────────────────────────────────────────────────────────

pub(crate) fn parse_stmt<'ast, 'tok>(p: &mut Parser<'ast, 'tok>) -> Option<Stmt<'ast>> {
    let lo   = p.span();
    let prev = p.enter(ParseContext::Statement);

    let kind = match p.cursor.peek().clone() {
        TokenType::Let      => parse_let_stmt(p)?,
        TokenType::Return   => parse_return_stmt(p)?,
        TokenType::Fail     => parse_fail_stmt(p)?,
        TokenType::If       => parse_if_stmt(p)?,
        TokenType::Match    => parse_match_stmt(p)?,
        TokenType::For      => parse_for_stmt(p)?,
        TokenType::While    => parse_while_stmt(p)?,
        TokenType::Loop     => parse_loop_stmt(p)?,
        TokenType::Break    => parse_break_stmt(p)?,
        TokenType::Continue => { p.cursor.advance(); StmtKind::Continue }
        TokenType::With     => parse_with_stmt(p)?,
        TokenType::Using    => parse_using_stmt(p)?,
        TokenType::Extract  => parse_extract_stmt(p)?,
        TokenType::Defer    => parse_defer_stmt(p)?,
        TokenType::Try      => parse_try_stmt(p)?,
        TokenType::Unsafe   => parse_unsafe_stmt(p)?,
        // Short decl: `name := expr`
        TokenType::Ident(_) if matches!(p.cursor.peek_nth(1), TokenType::ColonEqual) =>
            parse_short_decl(p)?,
        _ => {
            let expr = parse_expr::parse_expr(p)?;
            if let Some(op) = try_eat_assign_op(p) {
                let value = parse_expr::parse_expr(p)?;
                // Assignment as expression statement via ExprKind::Assign
                StmtKind::Expr(p.alloc(ubel_stratum::ast::expressions::Expr {
                    kind: ubel_stratum::ast::expressions::ExprKind::Assign {
                        op, target: expr, value,
                    },
                    span: lo.merge(&value.span),
                }))
            } else {
                StmtKind::Expr(expr)
            }
        }
    };

    p.eat_sep();
    p.leave(prev);
    Some(Stmt { kind, span: lo.merge(&p.span()) })
}

// ── let ───────────────────────────────────────────────────────────────────────

fn parse_let_stmt<'ast, 'tok>(p: &mut Parser<'ast, 'tok>) -> Option<StmtKind<'ast>> {
    p.cursor.advance(); // `let`
    let mutable = p.cursor.eat(&TokenType::Mut);
    // Full binding target: `x`, `(a, b)`, `{ name }`, `[first, ...rest]`
    let binding = parse_pattern::parse_binding_target(p)?;
    let ty      = crate::parsers::parse_decl::parse_type_annotation_opt(p);
    if let Err(e) = p.cursor.expect(&TokenType::Equal) {
        p.emit(crate::error::from_cursor(e, ParseContext::Statement));
        return None;
    }
    let value = parse_expr::parse_expr(p)?;
    Some(StmtKind::Let { mutable, binding, ty, value })
}

fn parse_short_decl<'ast, 'tok>(p: &mut Parser<'ast, 'tok>) -> Option<StmtKind<'ast>> {
    // `name := expr` — syntactic sugar for `let name = expr`
    let (name, _) = p.eat_ident()?;
    p.cursor.advance(); // `:=`
    let value = parse_expr::parse_expr(p)?;
    Some(StmtKind::Let {
        mutable: false,
        binding: BindingTarget::Ident(name),
        ty:      None,
        value,
    })
}

// ── return / fail ─────────────────────────────────────────────────────────────

fn parse_return_stmt<'ast, 'tok>(p: &mut Parser<'ast, 'tok>) -> Option<StmtKind<'ast>> {
    p.cursor.advance(); // `return`
    let value = if p.can_start_expr() { Some(parse_expr::parse_expr(p)?) } else { None };
    Some(StmtKind::Return(value))
}

fn parse_fail_stmt<'ast, 'tok>(p: &mut Parser<'ast, 'tok>) -> Option<StmtKind<'ast>> {
    p.cursor.advance(); // `fail`
    Some(StmtKind::Fail(parse_expr::parse_expr(p)?))
}

// ── if / elif / else ──────────────────────────────────────────────────────────

fn parse_if_stmt<'ast, 'tok>(p: &mut Parser<'ast, 'tok>) -> Option<StmtKind<'ast>> {
    let lo = p.span();
    p.cursor.advance(); // `if`

    let prev_nsl = p.enter_no_struct_lit();
    let condition = parse_expr::parse_expr(p);
    p.leave_no_struct_lit(prev_nsl);
    let condition = condition?;
    let then_body = parse_if_branch_body(p)?;

    let mut elif_branches: Vec<ElifBranch<'ast>> = Vec::with_capacity(2);
    while p.cursor.eat(&TokenType::Elif) {
        let elif_lo = p.span();
        let prev_nsl = p.enter_no_struct_lit();
        let cond    = parse_expr::parse_expr(p);
        p.leave_no_struct_lit(prev_nsl);
        let cond    = cond?;
        let body    = parse_if_branch_body(p)?;
        let bspan   = match &body {
            IfBranchBody::Block(b) => b.span,
            IfBranchBody::Expr(e)  => e.span,
        };
        let span    = elif_lo.merge(&bspan);
        elif_branches.push(ElifBranch { condition: cond, body, span });
    }

    let else_body = if p.cursor.eat(&TokenType::Else) {
        Some(parse_if_branch_body(p)?)
    } else { None };

    let span          = lo.merge(&p.span());
    let elif_branches = p.arena.alloc_slice_clone(&elif_branches);
    let node          = p.alloc(IfExpr { condition, then_body, elif_branches, else_body, span });
    Some(StmtKind::If(node))
}

/// Parse an `if` / `elif` / `else` branch body: either a `{ block }` or
/// the single-line `then Expr` form (`then` is required for the
/// single-line form — see the doc comment on `IfBranchBody` for why a
/// keyword is structurally necessary here, unlike `Lambda`/match arms).
///
/// Shared between the statement-position (`parse_if_stmt`, this file)
/// and expression-position (`parse_if_expr`, parse_expr.rs) parsers —
/// same relationship `parse_match_arm_body` already has to its two
/// callers.
pub(crate) fn parse_if_branch_body<'ast, 'tok>(
    p: &mut Parser<'ast, 'tok>,
) -> Option<IfBranchBody<'ast>> {
    if p.cursor.is_at(&TokenType::LeftBrace) {
        Some(IfBranchBody::Block(parse_block_inner(p)?))
    } else if p.cursor.eat(&TokenType::Then) {
        if matches!(
            p.cursor.peek(),
            TokenType::Return | TokenType::Break | TokenType::Continue | TokenType::Fail
        ) {
            // Same carve-out as parse_match_arm_body: return/break/continue/fail
            // are statements, not expressions, in this language.
            let lo    = p.span();
            let stmt  = parse_stmt(p)?;
            let span  = lo.merge(&stmt.span);
            let stmts = p.arena.alloc_slice_copy(&[stmt]);
            Some(IfBranchBody::Block(Block { stmts, span }))
        } else {
            Some(IfBranchBody::Expr(parse_expr::parse_expr(p)?))
        }
    } else {
        let found = p.cursor.peek_token();
        p.emit(crate::error::unexpected(found, &["'{'", "'then'"], ParseContext::Statement));
        None
    }
}

// ── match arm body (shared with parse_expr.rs) ─────────────────────────────────

/// Parse a match-arm body.
///
/// `return` / `break` / `continue` / `fail` are statements, not
/// expressions, in this language (see `parse_stmt`'s dispatcher) — so a
/// bare `pattern => return value` arm cannot go through `parse_expr`.
/// Detect that case up front and parse it as a single statement wrapped
/// in an implicit one-statement block instead.
pub(crate) fn parse_match_arm_body<'ast, 'tok>(
    p: &mut Parser<'ast, 'tok>,
) -> Option<MatchArmBody<'ast>> {
    if p.cursor.is_at(&TokenType::LeftBrace) {
        Some(MatchArmBody::Block(parse_block_inner(p)?))
    } else if matches!(
        p.cursor.peek(),
        TokenType::Return | TokenType::Break | TokenType::Continue | TokenType::Fail
    ) {
        let lo   = p.span();
        let stmt = parse_stmt(p)?;
        let span = lo.merge(&stmt.span);
        let stmts = p.arena.alloc_slice_copy(&[stmt]);
        Some(MatchArmBody::Block(Block { stmts, span }))
    } else {
        Some(MatchArmBody::Expr(parse_expr::parse_expr(p)?))
    }
}

// ── match ─────────────────────────────────────────────────────────────────────

fn parse_match_stmt<'ast, 'tok>(p: &mut Parser<'ast, 'tok>) -> Option<StmtKind<'ast>> {
    p.cursor.advance(); // `match`
    let prev_nsl  = p.enter_no_struct_lit();
    let scrutinee = parse_expr::parse_expr(p);
    p.leave_no_struct_lit(prev_nsl);
    let scrutinee = scrutinee?;
    let open      = p.span();
    if let Err(e) = p.cursor.expect(&TokenType::LeftBrace) {
        p.emit(crate::error::from_cursor(e, ParseContext::MatchArm));
        return None;
    }

    let mut arms: Vec<MatchArm<'ast>> = Vec::with_capacity(p.estimates.match_arms);
    while !p.cursor.is_at(&TokenType::RightBrace) && !p.cursor.is_eof() {
        let pos_before = p.cursor.position();
        if let Some(arm) = parse_match_arm(p) { arms.push(arm); } else { p.recover_to_stmt(); }
        p.eat_sep();
        p.guard_progress(pos_before);
    }
    if !p.cursor.eat(&TokenType::RightBrace) {
        p.emit(crate::error::unclosed('{', open, None, p.span()));
    }
    Some(StmtKind::Match { scrutinee, arms: p.arena.alloc_slice_clone(&arms) })
}

fn parse_match_arm<'ast, 'tok>(p: &mut Parser<'ast, 'tok>) -> Option<MatchArm<'ast>> {
    let lo      = p.span();
    let pattern = parse_pattern::parse_pattern(p)?;
    let guard   = if p.cursor.eat(&TokenType::Where) {
        Some(parse_expr::parse_expr(p)?)
    } else { None };
    // `then` is an accepted alternate spelling for `=>` here — see the
    // matching comment in parse_expr.rs's copy of this function.
    if !p.cursor.eat(&TokenType::FatArrow) && !p.cursor.eat(&TokenType::Then) {
        let found = p.cursor.peek_token();
        p.emit(crate::error::unexpected(found, &["'=>'", "'then'"], ParseContext::MatchArm));
        return None;
    }
    let body = parse_match_arm_body(p)?;
    Some(MatchArm { pattern, guard, body, span: lo.merge(&p.span()) })
}

// ── for / while / loop / break ────────────────────────────────────────────────

fn parse_for_stmt<'ast, 'tok>(p: &mut Parser<'ast, 'tok>) -> Option<StmtKind<'ast>> {
    p.cursor.advance(); // `for`
    // Full binding: `for (a, b) in pairs` or `for x in list`
    let binding = parse_pattern::parse_binding_target(p)?;
    if let Err(e) = p.cursor.expect(&TokenType::In) {
        p.emit(crate::error::from_cursor(e, ParseContext::Statement));
        return None;
    }
    let prev_nsl = p.enter_no_struct_lit();
    let iter = parse_expr::parse_expr(p);
    p.leave_no_struct_lit(prev_nsl);
    let iter = iter?;
    let block = parse_block_inner(p)?;
    let body = p.alloc(block); // &'ast Block
    Some(StmtKind::For { binding, iter, body })
}

fn parse_while_stmt<'ast, 'tok>(p: &mut Parser<'ast, 'tok>) -> Option<StmtKind<'ast>> {
    p.cursor.advance(); // `while`
    let prev_nsl = p.enter_no_struct_lit();
    let condition = parse_expr::parse_expr(p);
    p.leave_no_struct_lit(prev_nsl);
    let condition = condition?;
    let block      = parse_block_inner(p)?;
    let body      = p.alloc(block);
    Some(StmtKind::While { condition, body })
}

fn parse_loop_stmt<'ast, 'tok>(p: &mut Parser<'ast, 'tok>) -> Option<StmtKind<'ast>> {
    p.cursor.advance(); // `loop`
    let block = parse_block_inner(p)?;
    Some(StmtKind::Loop(p.alloc(block)))
}

fn parse_break_stmt<'ast, 'tok>(p: &mut Parser<'ast, 'tok>) -> Option<StmtKind<'ast>> {
    p.cursor.advance(); // `break`
    let value = if p.can_start_expr() { Some(parse_expr::parse_expr(p)?) } else { None };
    Some(StmtKind::Break(value))
}

// ── with arena(...) ───────────────────────────────────────────────────────────

fn parse_with_stmt<'ast, 'tok>(p: &mut Parser<'ast, 'tok>) -> Option<StmtKind<'ast>> {
    let prev = p.enter(ParseContext::ArenaBlock);
    p.cursor.advance(); // `with`

    let allocator = parse_allocator_kind(p)?;
    let block      = parse_block_inner(p)?;
    let body      = p.alloc(block);

    p.leave(prev);
    Some(StmtKind::With { allocator, body })
}

fn parse_allocator_kind<'ast, 'tok>(
    p: &mut Parser<'ast, 'tok>,
) -> Option<AllocatorKind<'ast>> {
    let (kw, span) = p.eat_ident()?;
    match kw {
        "arena" => {
            let open_span = p.span();
            if let Err(e) = p.cursor.expect(&TokenType::LeftParen) {
                p.emit(crate::error::from_cursor(e, ParseContext::ArenaBlock));
                return None;
            }
            let size = parse_size_expr(p)?;
            if !p.cursor.eat(&TokenType::RightParen) {
                p.emit(crate::error::unclosed('(', open_span, None, p.span()));
            }
            Some(AllocatorKind::Arena(size))
        }
        "pool" => {
            // `pool<Type>(count)` — consume optional `<Type>`
            if p.cursor.eat(&TokenType::Less) {
                let ty = p.parse_type_expr();
                p.cursor.eat(&TokenType::Greater);
                let open_span = p.span();
                if let Err(e) = p.cursor.expect(&TokenType::LeftParen) {
                    p.emit(crate::error::from_cursor(e, ParseContext::ArenaBlock));
                    return None;
                }
                let count = parse_expr::parse_expr(p)?;
                if !p.cursor.eat(&TokenType::RightParen) {
                    p.emit(crate::error::unclosed('(', open_span, None, p.span()));
                }
                let ty = ty.unwrap_or_else(|| p.alloc(ubel_stratum::ast::types::Type {
                    kind: ubel_stratum::ast::types::TypeKind::Infer,
                    span,
                }));
                Some(AllocatorKind::Pool { ty, count })
            } else {
                p.emit(crate::error::raw(
                    "pool allocator requires a type argument: pool<T>(count)",
                    span,
                ));
                None
            }
        }
        "gc"   => Some(AllocatorKind::Gc),
        "heap" => Some(AllocatorKind::Heap),
        _ => {
            p.emit(crate::error::raw(
                format!("unknown allocator '{}'; expected arena, pool, gc, or heap", kw),
                span,
            ));
            None
        }
    }
}

fn parse_size_expr<'ast, 'tok>(p: &mut Parser<'ast, 'tok>) -> Option<SizeExpr<'ast>> {
    // Check for `Integer SizeUnit` (e.g. `256 KB`) before falling back to expr
    if let TokenType::IntLit(n) = p.cursor.peek().clone() {
        let saved = p.cursor.position();
        p.cursor.advance();
        if let TokenType::Ident(unit_str) = p.cursor.peek().clone() {
            if let Some(unit) = crate::keywords::parse_size_unit(&unit_str) {
                p.cursor.advance();
                return Some(SizeExpr::WithUnit { value: n as u64, unit });
            }
        }
        // Not a unit string — restore and parse as expression
        p.cursor.restore(saved);
    }
    Some(SizeExpr::Expr(parse_expr::parse_expr(p)?))
}

// ── using ─────────────────────────────────────────────────────────────────────

fn parse_using_stmt<'ast, 'tok>(p: &mut Parser<'ast, 'tok>) -> Option<StmtKind<'ast>> {
    p.cursor.advance(); // `using`
    let mut bindings: Vec<UsingBinding<'ast>> = Vec::with_capacity(2);

    loop {
        let blo = p.span();
        if let Err(e) = p.cursor.expect(&TokenType::Let) {
            p.emit(crate::error::from_cursor(e, ParseContext::Statement));
            break;
        }
        let mutable = p.cursor.eat(&TokenType::Mut);
        let (name, _) = p.expect_ident()?;
        if let Err(e) = p.cursor.expect(&TokenType::Equal) {
            p.emit(crate::error::from_cursor(e, ParseContext::Statement));
            break;
        }
        let value = parse_expr::parse_expr(p)?;
        let span  = blo.merge(&value.span);
        bindings.push(UsingBinding { mutable, name, value, span });
        if !p.cursor.eat(&TokenType::Comma) { break; }
    }

    let block     = parse_block_inner(p)?;
    let body     = p.alloc(block);
    let bindings = p.arena.alloc_slice_clone(&bindings);
    Some(StmtKind::Using { bindings, body })
}

// ── extract ───────────────────────────────────────────────────────────────────

fn parse_extract_stmt<'ast, 'tok>(p: &mut Parser<'ast, 'tok>) -> Option<StmtKind<'ast>> {
    p.cursor.advance(); // `extract`
    // Full destructure pattern on the left side
    let pattern = parse_pattern::parse_destructure_pattern(p)?;
    if let Err(e) = p.cursor.expect(&TokenType::Equal) {
        p.emit(crate::error::from_cursor(e, ParseContext::Statement));
        return None;
    }
    let value = parse_expr::parse_expr(p)?;
    Some(StmtKind::Extract { pattern, value })
}

// ── defer / try / unsafe ──────────────────────────────────────────────────────

fn parse_defer_stmt<'ast, 'tok>(p: &mut Parser<'ast, 'tok>) -> Option<StmtKind<'ast>> {
    p.cursor.advance(); // `defer`
    Some(StmtKind::Defer(parse_expr::parse_expr(p)?))
}

fn parse_try_stmt<'ast, 'tok>(p: &mut Parser<'ast, 'tok>) -> Option<StmtKind<'ast>> {
    p.cursor.advance(); // `try`
    let block = parse_block_inner(p)?;
    let body = p.alloc(block);

    let (catch_binding, catch_body) = if p.cursor.eat(&TokenType::Catch) {
        let open = p.span();
        if let Err(e) = p.cursor.expect(&TokenType::LeftParen) {
            p.emit(crate::error::from_cursor(e, ParseContext::Statement));
            return Some(StmtKind::Try { body, catch_binding: None, catch_body: None });
        }
        let (name, _) = p.expect_ident()?;
        if !p.cursor.eat(&TokenType::RightParen) {
            p.emit(crate::error::unclosed('(', open, None, p.span()));
        }
        let block = parse_block_inner(p)?;
        (Some(name), Some(p.alloc(block)))
    } else {
        (None, None)
    };

    Some(StmtKind::Try { body, catch_binding, catch_body })
}

fn parse_unsafe_stmt<'ast, 'tok>(p: &mut Parser<'ast, 'tok>) -> Option<StmtKind<'ast>> {
    p.cursor.advance(); // `unsafe`
    let block = parse_block_inner(p)?;
    Some(StmtKind::Unsafe(p.alloc(block)))
}

// ── Assign op helper ──────────────────────────────────────────────────────────

fn try_eat_assign_op(p: &mut Parser<'_, '_>) -> Option<AssignOp> {
    let op = match p.cursor.peek() {
        TokenType::Equal           => AssignOp::Assign,
        TokenType::PlusEqual       => AssignOp::AddAssign,
        TokenType::MinusEqual      => AssignOp::SubAssign,
        TokenType::StarEqual       => AssignOp::MulAssign,
        TokenType::SlashEqual      => AssignOp::DivAssign,
        TokenType::PercentEqual    => AssignOp::RemAssign,
        TokenType::AmpEqual        => AssignOp::BitAndAssign,
        TokenType::PipeEqual       => AssignOp::BitOrAssign,
        TokenType::CaretEqual      => AssignOp::BitXorAssign,
        TokenType::LeftShiftEqual  => AssignOp::ShlAssign,
        TokenType::RightShiftEqual => AssignOp::ShrAssign,
        _                          => return None,
    };
    p.cursor.advance();
    Some(op)
            }
