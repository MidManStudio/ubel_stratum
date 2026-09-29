// crates/rd_parser/src/parsers/parse_type.rs
//
// Type expression parser for Ubel Stratum.
//
// Grammar (from ubel.ebnf):
//   TypeExpr ::= base_type postfix*
//   base_type ::= PrimitiveType | CollectionType | "Task" | "fn" ... |
//                 "[" Integer "]" TypeExpr | "[]" TypeExpr |
//                 "(" TypeExpr "," ... ")" | "&" "mut"? LifetimeLabel? TypeExpr |
//                 Ident GenericArgs?
//   postfix   ::= "?" | "!"

use ubel_stratum::{
    ast::{
        common::{GenericParam, LifetimeConstraint, LifetimeParam},
        types::{FunctionType, Type, TypeKind},
    },
    error_management::errors::ParseContext,
    lexer::{Span, TokenType},
};

use crate::parser::{cap, Parser};

impl<'ast, 'tok> Parser<'ast, 'tok> {

    // ── Public entry point ────────────────────────────────────────────────────

    /// Parse a full type expression including postfix `?` and `!`.
    #[inline]
    pub(crate) fn parse_type_expr(&mut self) -> Option<&'ast Type<'ast>> {
        let prev = self.enter(ParseContext::TypeExpr);
        let result = self.parse_type_inner();
        self.leave(prev);
        result
    }

    // ── Core type parser ──────────────────────────────────────────────────────

    fn parse_type_inner(&mut self) -> Option<&'ast Type<'ast>> {
        let lo = self.span();
        let kind = self.parse_type_base(lo)?;

        // Postfix: `?` → Optional, `!` → Fallible
        // Both can stack: `T?!` is "optional fallible" (unusual but syntactically valid)
        let (kind, span) = self.parse_type_postfix(kind, lo);

        Some(self.alloc(Type { kind, span }))
    }

    fn parse_type_base(&mut self, lo: Span) -> Option<TypeKind<'ast>> {
        match self.cursor.peek().clone() {

            // ── Reference: `&`/`ref`, `&mut`/`ref mut`, `&L T`/`ref L T` ─────
            // `&` and `ref` are dual spellings of the same operator — same
            // precedent as `and`/`&&`, `or`/`||` (docs/PARSER_RULES.md §5.6).
            TokenType::Amp | TokenType::Ref => {
                self.cursor.advance();
                let mutable = self.cursor.eat(&TokenType::Mut);

                // Disambiguate: &Ident ...
                // If next is Ident AND what follows that Ident can start a type
                // → first Ident is a lifetime label
                let lifetime: Option<&'ast str> = self.try_eat_lifetime_label();

                let inner = self.parse_type_inner()?;
                Some(TypeKind::Reference { mutable, lifetime, inner })
            }

            // ── Array or Slice: `[N]T` or `[]T` ──────────────────────────────
            TokenType::LeftBracket => {
                self.cursor.advance(); // consume `[`
                if self.cursor.eat(&TokenType::RightBracket) {
                    // `[]T` — slice type
                    let inner = self.parse_type_inner()?;
                    Some(TypeKind::Slice(inner))
                } else if let TokenType::IntLit(n) = self.cursor.peek().clone() {
                    // `[N]T` — fixed array type
                    let len = n as u64;
                    self.cursor.advance();
                    if let Err(e) = self.cursor.expect(&TokenType::RightBracket) {
                        self.emit(crate::error::from_cursor(e, ParseContext::TypeExpr));
                        return None;
                    }
                    let elem = self.parse_type_inner()?;
                    Some(TypeKind::Array { len, elem })
                } else {
                    self.expected(&["integer size", "']' for slice type"]);
                    None
                }
            }

            // ── Tuple: `(T, U, ...)` ──────────────────────────────────────────
            TokenType::LeftParen => {
                self.cursor.advance();
                let mut elems: Vec<&'ast Type<'ast>> = Vec::with_capacity(cap::GENERIC_ARGS);
                let first = self.parse_type_inner()?;
                elems.push(first);

                // A tuple MUST have at least two elements — `(T,)` is invalid,
                // `(T)` is a grouped type not a tuple.
                let mut has_comma = self.cursor.eat(&TokenType::Comma);
                while has_comma && !self.cursor.is_at(&TokenType::RightParen) && !self.cursor.is_eof() {
                    elems.push(self.parse_type_inner()?);
                    has_comma = self.cursor.eat(&TokenType::Comma);
                }

                if let Err(e) = self.cursor.expect(&TokenType::RightParen) {
                    self.emit(crate::error::from_cursor(e, ParseContext::TypeExpr));
                    return None;
                }

                if elems.len() < 2 {
                    // Single element in parens → grouped type, return inner
                    return Some(elems[0].kind.clone()); // return the inner kind
                }

                let elems = self.arena.alloc_slice_clone(&elems);
                Some(TypeKind::Tuple(elems))
            }

            // ── Function type: `fn(A, B) ReturnType` ─────────────────────────
            TokenType::Fn => {
                self.cursor.advance();
                if let Err(e) = self.cursor.expect(&TokenType::LeftParen) {
                    self.emit(crate::error::from_cursor(e, ParseContext::TypeExpr));
                    return None;
                }
                let params = self.parse_type_list_until(TokenType::RightParen);
                self.cursor.eat(&TokenType::RightParen);

                // Optional return type
                let (return_type, is_fallible) = if self.is_type_start() {
                    let rt = self.parse_type_inner()?;
                    let fallible = self.cursor.eat(&TokenType::Bang);
                    (Some(rt), fallible)
                } else {
                    (None, false)
                };

                Some(TypeKind::Function(FunctionType {
                    params: self.arena.alloc_slice_clone(&params),
                    return_type,
                    is_fallible,
                }))
            }

            // ── Task<T>  ──────────────────────────────────────────────────────
            TokenType::Task => {
                self.cursor.advance();
                let inner = if self.cursor.eat(&TokenType::Less) {
                    let t = self.parse_type_inner()?;
                    if let Err(e) = self.cursor.expect_generic_close() {
                        self.emit(crate::error::from_cursor(e, ParseContext::TypeExpr));
                        return None;
                    }
                    Some(t)
                } else {
                    None
                };
                Some(TypeKind::Task(inner))
            }

            // ── Built-in collection types: `List<T>`, `Dictionary<K,V>`,
            // `Set<T>`, `Queue<T>`, `Stack<T>` ─────────────────────────────
            // These lex as dedicated keyword tokens (KwList etc.), not
            // TokenType::Ident, so they never reach the Ident arm below —
            // that arm's call to try_collection_type() only ever fires for
            // list/dict/etc. written some other way they can't actually be
            // lexed. Without this arm, `List<T>` cannot be used as a type
            // annotation at all (only as an expression-position
            // constructor, `List.new()`) — see docs/MEMORY_MODEL.md.
            TokenType::KwList | TokenType::KwDictionary | TokenType::KwSet
            | TokenType::KwQueue | TokenType::KwStack | TokenType::KwInlineList => {
                let name = self.cursor.peek().to_string();
                self.cursor.advance();
                self.try_collection_type(&name)
            }

            // ── Named / Primitive / Collection / Generic ──────────────────────
            TokenType::Ident(name) => {
                let name = name.clone();
                self.cursor.advance();

                // Check primitive types first (match on string)
                if let Some(prim) = Self::try_primitive(&name) {
                    return Some(prim);
                }

                // Check built-in collection types
                if let Some(coll) = self.try_collection_type(&name) {
                    return Some(coll);
                }

                // User-defined named type — possibly generic: `Foo<T, U>`
                // Parse a dotted path first: `std.io.File`
                let name = self.intern(&name);
                let mut segs: Vec<&'ast str> = Vec::with_capacity(cap::PATH_SEGS);
                segs.push(name);

                while self.cursor.eat(&TokenType::Dot) {
                    if let Some((seg, _)) = self.eat_ident() {
                        segs.push(seg);
                    } else {
                        self.expected(&["identifier after '.'"]);
                        break;
                    }
                }

                // Try to parse generic args `<T, U>` — speculative
                let args = self.try_parse_generic_args();
                let path = self.arena.alloc_slice_clone(&segs);
                Some(TypeKind::Named { path, args })
            }

            _ => {
                self.expected(&[
                    "type", "'&'", "'[]'", "'fn'", "'Task'", "'('", "'['"
                ]);
                None
            }
        }
    }

    /// Apply postfix type modifiers: `?` (Optional) then `!` (Fallible).
    fn parse_type_postfix(
        &mut self,
        mut kind: TypeKind<'ast>,
        lo: Span,
    ) -> (TypeKind<'ast>, Span) {
        let mut hi = lo;

        loop {
            if self.cursor.eat(&TokenType::Question) {
                let inner = self.alloc(Type { kind, span: hi });
                kind = TypeKind::Optional(inner);
                hi = self.span();
            } else if self.cursor.eat(&TokenType::Bang) {
                let inner = self.alloc(Type { kind, span: hi });
                kind = TypeKind::Fallible(inner);
                hi = self.span();
            } else {
                break;
            }
        }

        (kind, lo.merge(&hi))
    }

    // ── Primitive type lookup ─────────────────────────────────────────────────

    fn try_primitive(name: &str) -> Option<TypeKind<'static>> {
        // match on &str — compiles to efficient comparison chain in release
        let k = match name {
            "int"    => TypeKind::Int,     "uint"   => TypeKind::Uint,
            "long"   => TypeKind::Long,    "ulong"  => TypeKind::Ulong,
            "short"  => TypeKind::Short,   "ushort" => TypeKind::Ushort,
            "byte"   => TypeKind::Byte,    "ubyte"  => TypeKind::Ubyte,
            "float"  => TypeKind::Float,   "double" => TypeKind::Double,
            "bool"   => TypeKind::Bool,    "char"   => TypeKind::Char,
            "string" => TypeKind::Str,     "void"   => TypeKind::Void,
            "i8"     => TypeKind::I8,      "i16"    => TypeKind::I16,
            "i32"    => TypeKind::I32,     "i64"    => TypeKind::I64,
            "u8"     => TypeKind::U8,      "u16"    => TypeKind::U16,
            "u32"    => TypeKind::U32,     "u64"    => TypeKind::U64,
            "f32"    => TypeKind::F32,     "f64"    => TypeKind::F64,
            "isize"  => TypeKind::Isize,   "usize"  => TypeKind::Usize,
            _ => return None,
        };
        Some(k)
    }

    // ── Collection type lookup ────────────────────────────────────────────────

    fn try_collection_type(&mut self, name: &str) -> Option<TypeKind<'ast>> {
        match name {
            "List" => {
                let inner = self.try_single_generic_arg();
                Some(TypeKind::List(inner))
            }
            "Dictionary" => {
                let kv = self.try_kv_generic_args();
                Some(TypeKind::Dictionary(kv))
            }
            "Set" => {
                let inner = self.try_single_generic_arg();
                Some(TypeKind::Set(inner))
            }
            "Queue" => {
                let inner = self.try_single_generic_arg();
                Some(TypeKind::Queue(inner))
            }
            "Stack" => {
                let inner = self.try_single_generic_arg();
                Some(TypeKind::Stack(inner))
            }
            "InlineList" => {
                let inner = self.try_single_generic_arg();
                Some(TypeKind::InlineList(inner))
            }
            _ => None,
        }
    }

    /// Parse `<T>` for collections — returns `None` if no `<` follows.
    fn try_single_generic_arg(&mut self) -> Option<&'ast Type<'ast>> {
        if !self.cursor.is_at(&TokenType::Less) { return None; }
        self.cursor.advance();
        let t = self.parse_type_inner()?;
        if let Err(e) = self.cursor.expect_generic_close() {
            self.emit(crate::error::from_cursor(e, ParseContext::TypeExpr));
        }
        Some(t)
    }

    /// Parse `<K, V>` for Dictionary — returns `None` if no `<` follows.
    fn try_kv_generic_args(&mut self) -> Option<(&'ast Type<'ast>, &'ast Type<'ast>)> {
        if !self.cursor.is_at(&TokenType::Less) { return None; }
        self.cursor.advance();
        let k = self.parse_type_inner()?;
        self.cursor.eat(&TokenType::Comma); // comma between K and V is required here
        let v = self.parse_type_inner()?;
        if let Err(e) = self.cursor.expect_generic_close() {
            self.emit(crate::error::from_cursor(e, ParseContext::TypeExpr));
        }
        Some((k, v))
    }

    // ── Generic arg parsing (speculative, memoised) ───────────────────────────

    /// Try to parse `<T, U, ...>` after a type name.
    /// Speculative: restores cursor if it looks like a comparison not a generic.
    /// Uses the MemoRule::GenericArgs cache.
    pub(crate) fn try_parse_generic_args(&mut self) -> &'ast [&'ast Type<'ast>] {
        if !self.cursor.is_at(&TokenType::Less) {
            return &[];
        }

        let start_pos = self.cursor.position();

        // Check cache first
        if let Some(entry) = self.memo_get(start_pos, crate::parser::MemoRule::GenericArgs) {
            match entry {
                crate::parser::MemoEntry::Hit { end_pos } => {
                    // Already succeeded — but we can't return the node from cache
                    // without re-parsing (we don't cache the actual args, just success).
                    // Fall through to re-parse; the fast path is skipping on Miss.
                    let _ = end_pos;
                }
                crate::parser::MemoEntry::Miss => return &[],
            }
        }

        // Speculative: save cursor, try to parse, restore on failure
        let saved = self.cursor.position();
        self.cursor.advance(); // consume `<`

        let mut args: Vec<&'ast Type<'ast>> = Vec::with_capacity(cap::GENERIC_ARGS);
        let mut ok = true;

        loop {
            if self.cursor.at_generic_close() || self.cursor.is_eof() { break; }
            if let Some(t) = self.parse_type_inner() {
                args.push(t);
            } else {
                ok = false; break;
            }
            self.eat_sep();
            if self.cursor.at_generic_close() { break; }
        }

        // Confirm close `>`. NOTE: earlier revisions of this function also
        // speculatively backed out if the token right after `>` was `=`/`==`
        // (worried about misparsing a chained comparison `a < b >= c`) — but
        // both real call sites (`parse_type_base`'s `Named` arm, and the
        // `impl Foo<T> for Bar` trait-impl check in `parse_decl.rs`) are
        // *type*-grammar positions, not expression positions; there's no
        // comparison-expression interpretation possible there at all, so
        // that guard only ever misfired — e.g. `let x: Option<int> = ...`
        // (`=` right after `>`, the single most ordinary case for a
        // generic type annotation) would wrongly restore and leave a bare
        // `<` for the caller to choke on. Removed; nothing currently calls
        // this from expression/Pratt context, where a real ambiguity could
        // exist (see PARSER_RULES.md §5.1 — that disambiguation isn't
        // wired through this function).
        if ok && self.cursor.eat_generic_close() {
            // consumed one `>` (half of a `>>` when nested)
            self.memo_set(start_pos, crate::parser::MemoRule::GenericArgs,
                crate::parser::MemoEntry::Hit { end_pos: self.cursor.position() });
            self.arena.alloc_slice_clone(&args)
        } else {
            self.cursor.restore(saved);
            self.memo_set(start_pos, crate::parser::MemoRule::GenericArgs,
                crate::parser::MemoEntry::Miss);
            &[]
        }
    }

    // ── Generic DECLARATION params: `<T: Bound, U>` ──────────────────────────

    /// Parse `<T: Bound, U>` in a declaration context (fn, struct, enum, trait).
    /// Returns an empty slice if no `<` present.
    pub(crate) fn parse_generic_params(&mut self) -> &'ast [GenericParam<'ast>] {
        if !self.cursor.eat(&TokenType::Less) { return &[]; }

        let mut params: Vec<GenericParam<'ast>> = Vec::with_capacity(cap::GENERIC_PARAMS);

        while !self.cursor.is_at(&TokenType::Greater) && !self.cursor.is_eof() {
            let (name, span) = match self.eat_ident() {
                Some(p) => p,
                None => { self.expected(&["type parameter name"]); break; }
            };

            // Optional trait bound: `T: Trait + Other`
            let bounds: &'ast [&'ast str] = if self.cursor.eat(&TokenType::Colon) {
                let mut bs: Vec<&'ast str> = Vec::with_capacity(2);
                loop {
                    if let Some((b, _)) = self.eat_ident() {
                        bs.push(b);
                    } else {
                        self.expected(&["trait bound name"]);
                        break;
                    }
                    if !self.cursor.eat(&TokenType::Plus) { break; }
                }
                self.arena.alloc_slice_clone(&bs)
            } else {
                &[]
            };

            params.push(GenericParam { name, bounds, span });
            self.eat_sep();
        }

        if let Err(e) = self.cursor.expect(&TokenType::Greater) {
            self.emit(crate::error::from_cursor(e, ParseContext::TypeExpr));
        }

        self.arena.alloc_slice_clone(&params)
    }

    // ── Lifetime params: `[lifetime L where L outlives M]` ───────────────────

    /// Parse `[lifetime L, lifetime M where M outlives L]` — returns empty if no `[`.
    pub(crate) fn parse_lifetime_params(&mut self) -> &'ast [LifetimeParam<'ast>] {
        if !self.cursor.eat(&TokenType::LeftBracket) { return &[]; }

        let mut params: Vec<LifetimeParam<'ast>> = Vec::with_capacity(2);

        while !self.cursor.is_at(&TokenType::RightBracket) && !self.cursor.is_eof() {
            // Expect `lifetime`  keyword
            if !self.cursor.eat(&TokenType::Lifetime) {
                self.expected(&["'lifetime'"]);
                break;
            }

            let (name, span) = match self.eat_ident() {
                Some(p) => p,
                None => { self.expected(&["lifetime name"]); break; }
            };

            // Optional `where L outlives M`
            let constraint: Option<LifetimeConstraint<'ast>> =
                if self.cursor.eat(&TokenType::Where) {
                    let (longer, lspan) = self.eat_ident()
                        .unwrap_or_else(|| { self.expected(&["lifetime name"]); ("_", span) });
                    // Expect `outlives` — it's a contextual keyword (Ident token)
                    if let Some((kw, _)) = self.eat_ident() {
                        if kw != "outlives" {
                            self.emit(crate::error::raw(
                                "expected 'outlives' in lifetime constraint",
                                self.span(),
                            ));
                        }
                    } else {
                        self.expected(&["'outlives'"]);
                    }
                    let (shorter, sspan) = self.eat_ident()
                        .unwrap_or_else(|| { self.expected(&["lifetime name"]); ("_", span) });

                    Some(LifetimeConstraint {
                        longer,
                        shorter,
                        span: lspan.merge(&sspan),
                    })
                } else {
                    None
                };

            params.push(LifetimeParam { name, constraint, span });
            self.eat_sep();
        }

        if let Err(e) = self.cursor.expect(&TokenType::RightBracket) {
            self.emit(crate::error::from_cursor(e, ParseContext::TypeExpr));
        }

        self.arena.alloc_slice_clone(&params)
    }

    // ── Helpers ───────────────────────────────────────────────────────────────

    /// Try to eat a lifetime label before a type in a reference.
    /// Returns the label name if the lookahead confirms it's a lifetime,
    /// not the type name itself.
    fn try_eat_lifetime_label(&mut self) -> Option<&'ast str> {
        // Heuristic: if current is Ident AND next token can start a type,
        // the current Ident is a lifetime label.
        if let TokenType::Ident(name) = self.cursor.peek() {
            let name = name.clone();
            let next = self.cursor.peek_nth(1);
            if self.token_can_start_type(next) {
                self.cursor.advance();
                return Some(self.intern(&name));
            }
        }
        None
    }

    /// Returns true if this token can start a type expression.
    fn token_can_start_type(&self, tt: &TokenType) -> bool {
        matches!(tt,
            TokenType::Ident(_)
            | TokenType::Amp
            | TokenType::Ref
            | TokenType::LeftBracket
            | TokenType::LeftParen
            | TokenType::Fn
            | TokenType::Task
            // Built-in collection types — see the matching arm in
            // parse_type_base for why these need listing separately from
            // TokenType::Ident.
            | TokenType::KwList
            | TokenType::KwDictionary
            | TokenType::KwSet
            | TokenType::KwQueue
            | TokenType::KwStack
            | TokenType::KwInlineList
        )
    }

    /// Returns true if the CURRENT token can start a type expression.
    pub(crate) fn is_type_start(&self) -> bool {
        self.token_can_start_type(self.cursor.peek())
    }

    /// Parse a comma/sep separated list of type expressions until `until` token.
    fn parse_type_list_until(&mut self, until: TokenType) -> Vec<&'ast Type<'ast>> {
        let mut types: Vec<&'ast Type<'ast>> = Vec::with_capacity(cap::FN_PARAMS);
        while !self.cursor.is_at(&until) && !self.cursor.is_eof() {
            if let Some(t) = self.parse_type_inner() {
                types.push(t);
            } else {
                break;
            }
            self.eat_sep();
        }
        types
    }
}

/// Free function wrapper — used by parse_stmt.rs to avoid `impl Parser` conflicts.
pub(crate) fn parse_type_annotation_opt_inner<'ast, 'tok>(
    p: &mut Parser<'ast, 'tok>,
) -> Option<&'ast ubel_stratum::ast::types::Type<'ast>> {
    if p.cursor.eat(&ubel_stratum::lexer::TokenType::LeftBracket) {
        // This was called speculatively for pool<T> generic arg — not a real type annotation
        // Restore and return None
        p.cursor.restore(p.cursor.position().saturating_sub(1));
        None
    } else if p.cursor.eat(&ubel_stratum::lexer::TokenType::Less) {
        // pool<T>: consume the generic arg and `>`
        let ty = p.parse_type_expr();
        p.cursor.eat(&ubel_stratum::lexer::TokenType::Greater);
        ty
    } else {
        None
    }
                }
