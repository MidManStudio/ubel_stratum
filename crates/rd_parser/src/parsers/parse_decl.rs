// ============================================================================
// NOTICE: Full documentation, design decisions, and fix history for this file
// live in docs/ubel_stratum_rd.md, section "parsers/parse_decl.rs"
// ============================================================================
//
// crates/rd_parser/src/parsers/parse_decl.rs
//
// Declaration parser: fn, struct, enum, trait, impl, extend, const, type alias.
//
// EBNF summary (see docs/ubel.ebnf for full spec):
//   Item        ::= FunctionDecl | StructDecl | EnumDecl | TraitDecl |
//                   ImplBlock | ExtendDecl | ConstDecl | TypeAlias
//   FunctionDecl::= TierAttr? Attributes? "pub"? "async"? "fn" Ident
//                   LifetimeParams? GenericParams? "(" ParamList? ")" ReturnSpec? Block
//   StructDecl  ::= "pub"? "edge"? "struct" Ident LifetimeParams? GenericParams? "{" StructBody "}"
//   ... (see EBNF for full rules)

use ubel_stratum::{
    ast::{
        common::{Attribute, TierAnnotation, Visibility},
        declarations::{
            ConstDecl, EnumDecl, EnumVariant, EnumVariantPayload, ExtendDecl,
            FieldDecl, FunctionDecl, ImplBlock, MethodDecl, MethodSig,
            Param, ParamKind, PropertyDecl, ReturnType, StaticDecl, StructDecl,
            StructMember, TraitDecl, TraitItem, TypeAlias,
        },
        root::{Item, Import, ImportItems, ImportKind, PackageDecl},
    },
    error_management::errors::ParseContext,
    lexer::{Span, TokenType},
};

use crate::parser::{cap, Parser};
use crate::parsers::parse_attr::extract_tier_from_attrs;

impl<'ast, 'tok> Parser<'ast, 'tok> {

    // ── Visibility ────────────────────────────────────────────────────────────

    #[inline(always)]
    pub(crate) fn parse_visibility(&mut self) -> Visibility {
        if self.cursor.eat(&TokenType::Pub) { Visibility::Public }
        else { Visibility::Private }
    }

    // ── Top-level item dispatcher ─────────────────────────────────────────────

    /// Parse one top-level item OR a `@attr(...) { item item item }`
    /// attribute block, pushing whatever it produces into `out`.
    ///
    /// A block applies its attrs/tier to every item inside, wholesale — but
    /// only where an inner item didn't already write its own `@tier(...)`
    /// (checked specifically, not "any attrs present"), so an unrelated
    /// `@doc(...)` on one item doesn't silently opt it out of the block's
    /// tier. The block's generic attrs are always appended regardless. See
    /// docs/PARSER_RULES.md §5.7.
    pub(crate) fn parse_item_or_block(&mut self, out: &mut Vec<Item<'ast>>) {
        let (attrs, tier_opt) = if self.cursor.is_at(&TokenType::At) {
            self.parse_attribute_list()
        } else {
            (&[][..], None)
        };
        let tier = tier_opt.unwrap_or(TierAnnotation::High);

        // Block form — recurse so nested blocks work for free, then apply
        // this block's attrs/tier to whatever each recursive call produced.
        if !attrs.is_empty() && self.cursor.is_at(&TokenType::LeftBrace) {
            self.cursor.advance(); // consume `{`
            loop {
                while self.eat_sep() {}
                if self.cursor.is_at(&TokenType::RightBrace) || self.cursor.is_eof() { break; }
                let before = out.len();
                self.parse_item_or_block(out);
                let newly_added: Vec<Item<'ast>> = out.split_off(before);
                for item in newly_added {
                    out.push(self.apply_block_attrs(item, attrs, tier));
                }
                if before == out.len() {
                    // The recursive call emitted an error and produced
                    // nothing (already recovered internally) — avoid
                    // spinning forever on the same token.
                    break;
                }
            }
            if let Err(e) = self.cursor.expect(&TokenType::RightBrace) {
                self.emit(crate::error::from_cursor(e, ParseContext::TopLevel));
            }
            return;
        }

        let vis = self.parse_visibility();

        let prev = self.enter(ParseContext::TopLevel);
        let result = match self.cursor.peek().clone() {
            TokenType::Async | TokenType::Fn =>
                self.parse_fn_decl(attrs, tier, vis).map(Item::Function),
            TokenType::Edge | TokenType::Struct =>
                self.parse_struct_decl(attrs, vis).map(Item::Struct),
            TokenType::Enum  =>
                self.parse_enum_decl(attrs, vis).map(Item::Enum),
            TokenType::Trait =>
                self.parse_trait_decl(attrs, vis).map(Item::Trait),
            TokenType::Impl  =>
                self.parse_impl_block(attrs).map(Item::Impl),
            TokenType::Extend =>
                self.parse_extend_decl(attrs).map(Item::Extend),
            TokenType::Const => {
                self.reject_own_tier_on_const(attrs);
                self.parse_const_decl(attrs).map(Item::Const)
            }
            TokenType::Static => {
                self.require_high_tier_on_static(attrs, tier_opt);
                self.parse_static_decl(attrs, vis).map(Item::Static)
            }
            TokenType::TypeKw =>
                self.parse_type_alias(attrs).map(Item::TypeAlias),
            _ => {
                self.expected(&[
                    "'fn'", "'struct'", "'enum'", "'trait'",
                    "'impl'", "'extend'", "'const'", "'static'", "'type'",
                ]);
                self.recover_to_decl();
                None
            }
        };
        self.leave(prev);

        match result {
            Some(item) => out.push(item),
            None => {
                // The item parser already emitted an error and advanced past
                // garbage. If we're still stuck, force advance to avoid an
                // infinite loop — mirrors parse_program.rs's old recovery.
                if !self.cursor.is_eof() && !matches!(self.cursor.peek(),
                    TokenType::Fn | TokenType::Struct | TokenType::Enum |
                    TokenType::Trait | TokenType::Impl | TokenType::Extend |
                    TokenType::Const | TokenType::Static | TokenType::TypeKw |
                    TokenType::Pub |
                    TokenType::At | TokenType::Edge | TokenType::RightBrace |
                    TokenType::Eof
                ) {
                    self.cursor.advance();
                }
            }
        }
    }

    /// Applies a block's attrs/tier to one item parsed inside it. Only
    /// `Function` and `Impl` carry a `tier` field at all — everything else
    /// just gets the generic attrs appended. `has_own_tier` gates the tier
    /// override specifically (not "attrs.is_empty()"), so e.g. a lone
    /// `@doc(...)` on one function doesn't silently pull it out of the
    /// block's tier.
    fn apply_block_attrs(
        &self, item: Item<'ast>, block_attrs: &'ast [Attribute<'ast>], block_tier: TierAnnotation,
    ) -> Item<'ast> {
        fn has_own_tier(attrs: &[Attribute]) -> bool {
            attrs.iter().any(|a| a.name == "tier")
        }

        let merge_attrs = |own: &'ast [Attribute<'ast>]| -> &'ast [Attribute<'ast>] {
            if block_attrs.is_empty() { return own; }
            if own.is_empty() { return block_attrs; }
            let mut merged: Vec<Attribute<'ast>> = Vec::with_capacity(own.len() + block_attrs.len());
            merged.extend_from_slice(own);
            merged.extend_from_slice(block_attrs);
            self.arena.alloc_slice_clone(&merged)
        };

        match item {
            Item::Function(mut f) => {
                if !has_own_tier(f.attributes) { f.tier = block_tier; }
                f.attributes = merge_attrs(f.attributes);
                Item::Function(f)
            }
            Item::Struct(mut s) => { s.attributes = merge_attrs(s.attributes); Item::Struct(s) }
            Item::Enum(mut e)   => { e.attributes = merge_attrs(e.attributes); Item::Enum(e) }
            Item::Trait(mut t)  => { t.attributes = merge_attrs(t.attributes); Item::Trait(t) }
            Item::Impl(mut i) => {
                if i.tier.is_none() && !has_own_tier(i.attributes) { i.tier = Some(block_tier); }
                i.attributes = merge_attrs(i.attributes);
                Item::Impl(i)
            }
            Item::Extend(mut e)    => { e.attributes = merge_attrs(e.attributes); Item::Extend(e) }
            Item::Const(mut c)     => { c.attributes = merge_attrs(c.attributes); Item::Const(c) }
            // A block's `@tier` is deliberately not applied to a static
            // (always HIGH) or a const (tier-agnostic), only its generic
            // attributes, so `@tier(low) { const N = 1  fn f() {} }` stays
            // legal. See `parse_item_or_block` for the direct-annotation check.
            Item::Static(mut s)    => { s.attributes = merge_attrs(s.attributes); Item::Static(s) }
            Item::TypeAlias(mut t) => { t.attributes = merge_attrs(t.attributes); Item::TypeAlias(t) }
        }
    }

    // ── Function declaration ──────────────────────────────────────────────────

    pub(crate) fn parse_fn_decl(
        &mut self,
        attrs: &'ast [Attribute<'ast>],
        tier:  TierAnnotation,
        vis:   Visibility,
    ) -> Option<FunctionDecl<'ast>> {
        let prev = self.enter(ParseContext::FunctionDecl);
        let lo   = self.span();

        let is_async = self.cursor.eat(&TokenType::Async);
        if let Err(e) = self.cursor.expect(&TokenType::Fn) {
            self.emit(crate::error::from_cursor(e, ParseContext::FunctionDecl));
            self.leave(prev);
            return None;
        }

        let (name, _) = self.expect_ident()?;

        // [lifetime L, ...] then <T: Bound, ...>
        let lifetime_params = self.parse_lifetime_params();
        let generic_params  = self.parse_generic_params();

        // Parameter list
        let open_span = self.span();
        if let Err(e) = self.cursor.expect(&TokenType::LeftParen) {
            self.emit(crate::error::from_cursor(e, ParseContext::FunctionParam));
            self.leave(prev);
            return None;
        }
        let params = self.parse_param_list();
        if !self.cursor.eat(&TokenType::RightParen) {
            let at = self.span();
            self.emit(crate::error::unclosed('(', open_span, None, at));
        }

        // Return type (optional)
        let return_type = self.parse_return_spec();

        // Body
        let body = self.parse_block()?;

        let span = lo.merge(&body.span);

        // Warn if async is used outside HIGH tier
        if is_async && tier != TierAnnotation::High {
            self.emit(crate::error::illegal_here(
                "async fn",
                "async functions are only permitted in @tier(high) context",
                lo,
                Some("remove @tier(mid) / @tier(low), or remove 'async'"),
            ));
        }

        let prev_tier = self.enter_tier(tier);
        self.leave_tier(prev_tier);
        self.leave(prev);

        Some(FunctionDecl {
            attributes: attrs,
            tier,
            visibility: vis,
            is_async,
            name,
            lifetime_params,
            generic_params,
            params,
            return_type,
            body,
            span,
        })
    }

    // ── Parameter list ────────────────────────────────────────────────────────

    fn parse_param_list(&mut self) -> &'ast [Param<'ast>] {
        let mut params: Vec<Param<'ast>> = Vec::with_capacity(cap::FN_PARAMS);

        while !self.cursor.is_at(&TokenType::RightParen) && !self.cursor.is_eof() {
            let prev = self.enter(ParseContext::FunctionParam);
            if let Some(p) = self.parse_param() {
                params.push(p);
            } else {
                // Skip to next param or close
                while !self.cursor.is_at(&TokenType::Comma)
                    && !self.cursor.is_at(&TokenType::RightParen)
                    && !self.cursor.is_eof()
                {
                    self.cursor.advance();
                }
            }
            self.leave(prev);
            self.eat_sep(); // optional comma or semicolon
        }

        self.arena.alloc_slice_clone(&params)
    }

    fn parse_param(&mut self) -> Option<Param<'ast>> {
        let lo = self.span();
        match self.cursor.peek().clone() {
            // `self`, `mut self`, `&self`, `&mut self`
            TokenType::SelfKw => {
                self.cursor.advance();
                Some(Param { kind: ParamKind::SelfVal, span: lo })
            }
            TokenType::Amp => {
                self.cursor.advance();
                let mutable = self.cursor.eat(&TokenType::Mut);
                if let Err(e) = self.cursor.expect(&TokenType::SelfKw) {
                    self.emit(crate::error::from_cursor(e, ParseContext::FunctionParam));
                    return None;
                }
                let kind = if mutable { ParamKind::SelfRefMut } else { ParamKind::SelfRef };
                Some(Param { kind, span: lo })
            }
            TokenType::Mut => {
                self.cursor.advance();
                if self.cursor.eat(&TokenType::SelfKw) {
                    Some(Param { kind: ParamKind::SelfMut, span: lo })
                } else {
                    // `mut name: Type`
                    let (name, _) = self.expect_ident()?;
                    let ty = self.parse_type_annotation();
                    let default = if self.cursor.eat(&TokenType::Equal) {
                        self.parse_expr_or_none()
                    } else { None };
                    Some(Param {
                        kind: ParamKind::Named { mutable: true, name, ty, default },
                        span: lo,
                    })
                }
            }
            TokenType::Underscore => {
                self.cursor.advance();
                let ty = self.parse_type_annotation();
                Some(Param { kind: ParamKind::Discard { ty }, span: lo })
            }
            TokenType::Ident(_) => {
                let (name, _) = self.eat_ident().unwrap();
                let ty = self.parse_type_annotation();
                let default = if self.cursor.eat(&TokenType::Equal) {
                    self.parse_expr_or_none()
                } else { None };
                Some(Param {
                    kind: ParamKind::Named { mutable: false, name, ty, default },
                    span: lo,
                })
            }
            _ => {
                self.expected(&["parameter name", "'self'", "'&self'", "'mut'"]);
                None
            }
        }
    }

    // ── Return type spec ──────────────────────────────────────────────────────

    fn parse_return_spec(&mut self) -> Option<ReturnType<'ast>> {
        let prev = self.enter(ParseContext::ReturnType);
        let result = if self.is_type_start() {
            let ty = self.parse_type_expr()?;
            let is_fallible = self.cursor.eat(&TokenType::Bang);
            Some(ReturnType { ty, is_fallible })
        } else { None };
        self.leave(prev);
        result
    }

    // ── Type annotation helper ────────────────────────────────────────────────

    pub(crate) fn parse_type_annotation(
        &mut self,
    ) -> Option<&'ast ubel_stratum::ast::types::Type<'ast>> {
        if self.cursor.eat(&TokenType::Colon) {
            self.parse_type_expr()
        } else {
            None
        }
    }

    // ── Struct declaration ────────────────────────────────────────────────────

    pub(crate) fn parse_struct_decl(
        &mut self,
        attrs: &'ast [Attribute<'ast>],
        vis:   Visibility,
    ) -> Option<StructDecl<'ast>> {
        let prev = self.enter(ParseContext::StructDecl);
        let lo   = self.span();

        let is_edge = self.cursor.eat(&TokenType::Edge);
        if let Err(e) = self.cursor.expect(&TokenType::Struct) {
            self.emit(crate::error::from_cursor(e, ParseContext::StructDecl));
            self.leave(prev);
            return None;
        }

        let (name, _) = self.expect_ident()?;

        // Lifetimes before generics: `[lifetime parse]`
        let lifetime_params = self.parse_lifetime_params();
        let generic_params  = self.parse_generic_params();

        // Struct body
        let open_span = self.span();
        if let Err(e) = self.cursor.expect(&TokenType::LeftBrace) {
            self.emit(crate::error::from_cursor(e, ParseContext::StructDecl));
            self.leave(prev);
            return None;
        }

        let members = self.parse_struct_body();

        let close_span = self.span();
        if !self.cursor.eat(&TokenType::RightBrace) {
            self.emit(crate::error::unclosed('{', open_span, None, close_span));
        }

        let span = lo.merge(&close_span);
        self.leave(prev);

        Some(StructDecl {
            attributes: attrs,
            visibility: vis,
            is_edge,
            name,
            lifetime_params,
            generic_params,
            members,
            span,
        })
    }

    fn parse_struct_body(&mut self) -> &'ast [StructMember<'ast>] {
        let mut members: Vec<StructMember<'ast>> = Vec::with_capacity(cap::STRUCT_FIELDS);

        while !self.cursor.is_at(&TokenType::RightBrace) && !self.cursor.is_eof() {
            let pos_before = self.cursor.position();
            if let Some(m) = self.parse_struct_member() {
                members.push(m);
            } else {
                self.recover_to_struct_member();
            }
            self.eat_sep(); // optional separator between members
            self.guard_progress(pos_before);
        }

        self.arena.alloc_slice_clone(&members)
    }

    fn parse_struct_member(&mut self) -> Option<StructMember<'ast>> {
        // Attributes (only valid on methods)
        let (attrs, _) = if self.cursor.is_at(&TokenType::At) {
            self.parse_attribute_list()
        } else {
            (&[][..], None)
        };

        let tier = extract_tier_from_attrs(attrs);
        let vis  = self.parse_visibility();

        match self.cursor.peek().clone() {
            TokenType::Async | TokenType::Fn => {
                self.parse_method_decl_inner(attrs, tier, vis)
                    .map(StructMember::Method)
            }
            TokenType::Ident(_) => {
                if !attrs.is_empty() {
                    self.emit(crate::error::illegal_here(
                        "attribute",
                        "attributes are not valid on struct fields; only on methods",
                        attrs[0].span,
                        Some("move the attribute to a method declaration"),
                    ));
                }
                self.parse_field_or_property(vis)
            }
            _ => {
                self.expected(&["field name", "'fn'", "'async fn'"]);
                None
            }
        }
    }

    fn parse_field_or_property(&mut self, vis: Visibility) -> Option<StructMember<'ast>> {
        let lo = self.span();
        let (name, _) = self.expect_ident()?;

        // Type annotation: `name: Type`
        let ty = self.parse_type_annotation()?;

        // Is this a property? `name: Type { get { } set { } }`
        if self.cursor.is_at(&TokenType::LeftBrace) {
            let prop = self.parse_property_body(vis, name, ty, lo)?;
            return Some(StructMember::Property(prop));
        }

        let span = lo.merge(&ty.span);
        Some(StructMember::Field(FieldDecl { visibility: vis, name, ty, span }))
    }

    fn parse_property_body(
        &mut self,
        vis:  Visibility,
        name: &'ast str,
        ty:   &'ast ubel_stratum::ast::types::Type<'ast>,
        lo:   Span,
    ) -> Option<PropertyDecl<'ast>> {
        let open_span = self.span();
        self.cursor.advance(); // consume `{`

        // `get { ... }`
        let (kw, _) = self.eat_ident()?;
        if kw != "get" {
            self.emit(crate::error::raw("expected 'get' accessor", self.span()));
            return None;
        }
        let getter = self.parse_block()?;

        // Optional `set { ... }`
        let setter = if let TokenType::Ident(ref s) = self.cursor.peek().clone() {
            if s == "set" {
                self.cursor.advance();
                Some(self.parse_block()?)
            } else { None }
        } else { None };

        let close_span = self.span();
        if !self.cursor.eat(&TokenType::RightBrace) {
            self.emit(crate::error::unclosed('{', open_span, None, close_span));
        }

        Some(PropertyDecl {
            visibility: vis,
            name,
            ty,
            getter,
            setter,
            span: lo.merge(&close_span),
        })
    }

    #[cold]
    fn recover_to_struct_member(&mut self) {
        while !self.cursor.is_eof() {
            match self.cursor.peek() {
                TokenType::RightBrace | TokenType::Eof => break,
                TokenType::Pub | TokenType::Fn | TokenType::Async | TokenType::At => break,
                TokenType::Ident(_) => break,
                _ => { self.cursor.advance(); }
            }
        }
    }

    // ── Method (shared between struct, impl, extend, trait) ───────────────────

    pub(crate) fn parse_method_decl_inner(
        &mut self,
        attrs: &'ast [Attribute<'ast>],
        tier:  TierAnnotation,
        vis:   Visibility,
    ) -> Option<MethodDecl<'ast>> {
        let lo       = self.span();
        let is_async = self.cursor.eat(&TokenType::Async);
        if let Err(e) = self.cursor.expect(&TokenType::Fn) {
            self.emit(crate::error::from_cursor(e, ParseContext::FunctionDecl));
            return None;
        }
        let (name, _) = self.expect_ident()?;
        let generic_params = self.parse_generic_params();

        let open_span = self.span();
        if let Err(e) = self.cursor.expect(&TokenType::LeftParen) {
            self.emit(crate::error::from_cursor(e, ParseContext::FunctionParam));
            return None;
        }
        let params = self.parse_param_list();
        if !self.cursor.eat(&TokenType::RightParen) {
            self.emit(crate::error::unclosed('(', open_span, None, self.span()));
        }

        let return_type = self.parse_return_spec();
        let body        = self.parse_block()?;
        let span        = lo.merge(&body.span);

        Some(MethodDecl { attributes: attrs, tier, visibility: vis, is_async,
                          name, generic_params, params, return_type, body, span })
    }

    // ── Enum declaration ──────────────────────────────────────────────────────

    pub(crate) fn parse_enum_decl(
        &mut self,
        attrs: &'ast [Attribute<'ast>],
        vis:   Visibility,
    ) -> Option<EnumDecl<'ast>> {
        let prev = self.enter(ParseContext::EnumDecl);
        let lo   = self.span();

        self.cursor.advance(); // consume `enum`
        let (name, _) = self.expect_ident()?;
        let generic_params = self.parse_generic_params();

        let open_span = self.span();
        if let Err(e) = self.cursor.expect(&TokenType::LeftBrace) {
            self.emit(crate::error::from_cursor(e, ParseContext::EnumDecl));
            self.leave(prev);
            return None;
        }

        let mut variants: Vec<EnumVariant<'ast>> = Vec::with_capacity(cap::ENUM_VARIANTS);
        while !self.cursor.is_at(&TokenType::RightBrace) && !self.cursor.is_eof() {
            let pos_before = self.cursor.position();
            if let Some(v) = self.parse_enum_variant() {
                variants.push(v);
            } else {
                self.recover_to_struct_member();
            }
            self.eat_sep();
            self.guard_progress(pos_before);
        }

        let close_span = self.span();
        if !self.cursor.eat(&TokenType::RightBrace) {
            self.emit(crate::error::unclosed('{', open_span, None, close_span));
        }

        let span = lo.merge(&close_span);
        self.leave(prev);

        Some(EnumDecl {
            attributes: attrs,
            visibility: vis,
            name,
            generic_params,
            variants: self.arena.alloc_slice_clone(&variants),
            span,
        })
    }

    fn parse_enum_variant(&mut self) -> Option<EnumVariant<'ast>> {
        let lo = self.span();
        let (name, _) = self.expect_ident()?;

        let payload = match self.cursor.peek().clone() {
            // Tuple variant: `Ok(T, U)`
            TokenType::LeftParen => {
                self.cursor.advance();
                let mut types = Vec::with_capacity(2);
                while !self.cursor.is_at(&TokenType::RightParen) && !self.cursor.is_eof() {
                    let pos_before = self.cursor.position();
                    if let Some(t) = self.parse_type_expr() { types.push(t); }
                    self.eat_sep();
                    self.guard_progress(pos_before);
                }
                self.cursor.eat(&TokenType::RightParen);
                let tys: Vec<&'ast ubel_stratum::ast::types::Type<'ast>> = types;
                let slice = self.arena.alloc_slice_clone(&tys);
                EnumVariantPayload::Tuple(slice)
            }
            // Struct variant: `Err { code: int }`
            TokenType::LeftBrace => {
                self.cursor.advance();
                let mut fields: Vec<FieldDecl<'ast>> = Vec::with_capacity(4);
                while !self.cursor.is_at(&TokenType::RightBrace) && !self.cursor.is_eof() {
                    let pos_before = self.cursor.position();
                    if let Some(f) = self.parse_enum_field() { fields.push(f); }
                    self.eat_sep();
                    self.guard_progress(pos_before);
                }
                self.cursor.eat(&TokenType::RightBrace);
                EnumVariantPayload::Struct(self.arena.alloc_slice_clone(&fields))
            }
            // Discriminant: `Active = 1`
            TokenType::Equal => {
                self.cursor.advance();
                if let Some(e) = self.parse_expr_or_none() {
                    EnumVariantPayload::Discriminant(e)
                } else {
                    EnumVariantPayload::None
                }
            }
            _ => EnumVariantPayload::None,
        };

        Some(EnumVariant { name, payload, span: lo.merge(&self.span()) })
    }

    fn parse_enum_field(&mut self) -> Option<FieldDecl<'ast>> {
        let lo  = self.span();
        let vis = self.parse_visibility();
        let (name, _) = self.expect_ident()?;
        let ty = self.parse_type_annotation()?;
        Some(FieldDecl { visibility: vis, name, ty, span: lo.merge(&ty.span) })
    }

    // ── Trait declaration ─────────────────────────────────────────────────────

    pub(crate) fn parse_trait_decl(
        &mut self,
        attrs: &'ast [Attribute<'ast>],
        vis:   Visibility,
    ) -> Option<TraitDecl<'ast>> {
        let prev = self.enter(ParseContext::TraitDecl);
        let lo   = self.span();

        self.cursor.advance(); // consume `trait`
        let (name, _) = self.expect_ident()?;
        let generic_params = self.parse_generic_params();

        // Body is optional per EBNF: `("{ TraitItem* "}")?`
        let items: &'ast [TraitItem<'ast>] = if self.cursor.is_at(&TokenType::LeftBrace) {
            let open_span = self.span();
            self.cursor.advance();
            let mut items: Vec<TraitItem<'ast>> = Vec::with_capacity(cap::TRAIT_ITEMS);

            while !self.cursor.is_at(&TokenType::RightBrace) && !self.cursor.is_eof() {
                let pos_before = self.cursor.position();
                if let Some(item) = self.parse_trait_item() { items.push(item); } else { self.recover_to_decl(); }
                self.eat_sep();
                self.guard_progress(pos_before);
            }

            if !self.cursor.eat(&TokenType::RightBrace) {
                self.emit(crate::error::unclosed('{', open_span, None, self.span()));
            }
            self.arena.alloc_slice_clone(&items)
        } else { &[] };

        let span = lo.merge(&self.span());
        self.leave(prev);

        Some(TraitDecl { attributes: attrs, visibility: vis, name, generic_params, items, span })
    }

    fn parse_trait_item(&mut self) -> Option<TraitItem<'ast>> {
        let (attrs, _) = if self.cursor.is_at(&TokenType::At) {
            self.parse_attribute_list()
        } else {
            (&[][..], None)
        };

        // Associated type: `type Output`
        if self.cursor.eat(&TokenType::TypeKw) {
            let (name, span) = self.expect_ident()?;
            return Some(TraitItem::AssociatedType { name, span });
        }

        // Method signature or default method
        let is_async = self.cursor.eat(&TokenType::Async);
        if let Err(e) = self.cursor.expect(&TokenType::Fn) {
            self.emit(crate::error::from_cursor(e, ParseContext::TraitDecl));
            return None;
        }
        let (name, _) = self.expect_ident()?;
        let generic_params = self.parse_generic_params();

        let open_span = self.span();
        if let Err(e) = self.cursor.expect(&TokenType::LeftParen) {
            self.emit(crate::error::from_cursor(e, ParseContext::FunctionParam));
            return None;
        }
        let params = self.parse_param_list();
        if !self.cursor.eat(&TokenType::RightParen) {
            self.emit(crate::error::unclosed('(', open_span, None, self.span()));
        }
        let return_type = self.parse_return_spec();

        // Has body → default method
        if self.cursor.is_at(&TokenType::LeftBrace) {
            let body = self.parse_block()?;
            let span = body.span;
            return Some(TraitItem::DefaultMethod(MethodDecl {
                attributes: attrs,
                tier: TierAnnotation::High,
                visibility: Visibility::Public,
                is_async,
                name, generic_params, params, return_type, body, span,
            }));
        }

        // No body → required method signature
        Some(TraitItem::MethodSig(MethodSig {
            name, generic_params, params, return_type,
            span: self.span(),
        }))
    }

    // ── Impl block ────────────────────────────────────────────────────────────

    pub(crate) fn parse_impl_block(
        &mut self,
        attrs: &'ast [Attribute<'ast>],
    ) -> Option<ImplBlock<'ast>> {
        let prev = self.enter(ParseContext::ImplBlock);
        let lo   = self.span();

        self.cursor.advance(); // consume `impl`

        // Block-level tier: `@tier(low) impl Foo { ... }` — all methods inherit
        let tier = extract_tier_from_attrs(attrs);

        // Check for trait impl: `impl TraitName for Type` vs `impl Type`
        // Heuristic: if we see `Ident` followed by `for`, it's a trait impl.
        let trait_path: Option<&'ast [&'ast str]> = {
            let pos = self.cursor.position();
            // Speculatively collect a path
            let mut segs: Vec<&'ast str> = Vec::with_capacity(cap::PATH_SEGS);

            while let Some((seg, _)) = self.eat_ident() {
                segs.push(seg);
                if !self.cursor.eat(&TokenType::Dot) { break; }
            }
            // Also consume generic args if any (trait can be generic: impl Foo<T> for Bar)
            let _ = self.try_parse_generic_args();

            if self.cursor.eat(&TokenType::For) {
                // Confirmed trait impl
                let path = self.arena.alloc_slice_clone(&segs);
                Some(path)
            } else {
                // Wasn't a trait impl — restore and re-parse as the target type
                self.cursor.restore(pos);
                None
            }
        };

        // Parse the target type
        let target_type = self.parse_type_expr()?;

        let open_span = self.span();
        if let Err(e) = self.cursor.expect(&TokenType::LeftBrace) {
            self.emit(crate::error::from_cursor(e, ParseContext::ImplBlock));
            self.leave(prev);
            return None;
        }

        let mut methods: Vec<MethodDecl<'ast>> = Vec::with_capacity(cap::IMPL_ITEMS);
        while !self.cursor.is_at(&TokenType::RightBrace) && !self.cursor.is_eof() {
            let pos_before = self.cursor.position();
            let (m_attrs, _) = if self.cursor.is_at(&TokenType::At) {
                self.parse_attribute_list()
            } else {
                (&[][..], None)
            };
            let m_tier = if m_attrs.is_empty() { tier }
                         else { extract_tier_from_attrs(m_attrs) };
            let m_vis = self.parse_visibility();
            if let Some(m) = self.parse_method_decl_inner(m_attrs, m_tier, m_vis) {
                methods.push(m);
            } else {
                self.recover_to_decl();
            }
            self.eat_sep();
            self.guard_progress(pos_before);
        }

        let close_span = self.span();
        if !self.cursor.eat(&TokenType::RightBrace) {
            self.emit(crate::error::unclosed('{', open_span, None, close_span));
        }

        let span = lo.merge(&close_span);
        self.leave(prev);

        Some(ImplBlock {
            attributes: attrs,
            tier: if tier == TierAnnotation::High { None } else { Some(tier) },
            trait_path,
            target_type,
            methods: self.arena.alloc_slice_clone(&methods),
            span,
        })
    }

    // ── Extend declaration ────────────────────────────────────────────────────

    pub(crate) fn parse_extend_decl(
        &mut self,
        attrs: &'ast [Attribute<'ast>],
    ) -> Option<ExtendDecl<'ast>> {
        let prev = self.enter(ParseContext::ExtendDecl);
        let lo   = self.span();

        self.cursor.advance(); // consume `extend`
        let target_type = self.parse_type_expr()?;

        // Body is optional per EBNF: `("{" ExtendItem* "}")?`
        let methods: &'ast [MethodDecl<'ast>] = if self.cursor.is_at(&TokenType::LeftBrace) {
            let open_span = self.span();
            self.cursor.advance();
            let mut ms: Vec<MethodDecl<'ast>> = Vec::with_capacity(cap::IMPL_ITEMS);

            while !self.cursor.is_at(&TokenType::RightBrace) && !self.cursor.is_eof() {
                let pos_before = self.cursor.position();
                let (m_attrs, _) = if self.cursor.is_at(&TokenType::At) {
                    self.parse_attribute_list()
                } else {
                    (&[][..], None)
                };
                let m_tier = extract_tier_from_attrs(m_attrs);
                let m_vis  = self.parse_visibility();
                if let Some(m) = self.parse_method_decl_inner(m_attrs, m_tier, m_vis) {
                    ms.push(m);
                } else {
                    self.recover_to_decl();
                }
                self.eat_sep();
                self.guard_progress(pos_before);
            }

            if !self.cursor.eat(&TokenType::RightBrace) {
                self.emit(crate::error::unclosed('{', open_span, None, self.span()));
            }
            self.arena.alloc_slice_clone(&ms)
        } else { &[] };

        let span = lo.merge(&self.span());
        self.leave(prev);
        Some(ExtendDecl { attributes: attrs, target_type, methods, span })
    }

    // ── Const declaration ─────────────────────────────────────────────────────

    pub(crate) fn parse_const_decl(
        &mut self,
        attrs: &'ast [Attribute<'ast>],
    ) -> Option<ConstDecl<'ast>> {
        let prev = self.enter(ParseContext::ConstDecl);
        let lo   = self.span();

        self.cursor.advance(); // consume `const`
        let (name, _) = self.expect_ident()?;
        let ty = self.parse_type_annotation();

        if let Err(e) = self.cursor.expect(&TokenType::Equal) {
            self.emit(crate::error::from_cursor(e, ParseContext::ConstDecl));
            self.leave(prev);
            return None;
        }

        let value = self.parse_expr_or_none()?;
        let span  = lo.merge(&value.span);
        self.eat_sep();
        self.leave(prev);

        Some(ConstDecl { attributes: attrs, name, ty, value, span })
    }

    // ── Static declaration ────────────────────────────────────────────────────

    /// `[pub] static NAME: Type = expr`. The type annotation is required:
    /// every function that assigns to the static would otherwise take part
    /// in deciding its type, and which body is inferred first must never
    /// change the answer. Missing `:` is an ordinary "expected ':'" error.
    pub(crate) fn parse_static_decl(
        &mut self,
        attrs: &'ast [Attribute<'ast>],
        vis:   Visibility,
    ) -> Option<StaticDecl<'ast>> {
        let prev = self.enter(ParseContext::StaticDecl);
        let lo   = self.span();

        // Every failure below has already emitted its diagnostic. Skip to
        // the next item boundary before returning, otherwise the leftover
        // tokens of this declaration (`= 5` after a missing `:`) would be
        // parsed as a top-level item and reported a second time.
        self.cursor.advance(); // consume `static`
        let Some((name, _)) = self.expect_ident() else {
            self.recover_to_decl();
            self.leave(prev);
            return None;
        };

        if let Err(e) = self.cursor.expect(&TokenType::Colon) {
            self.emit(crate::error::from_cursor(e, ParseContext::StaticDecl));
            self.recover_to_decl();
            self.leave(prev);
            return None;
        }
        let Some(ty) = self.parse_type_expr() else {
            self.recover_to_decl();
            self.leave(prev);
            return None;
        };

        if let Err(e) = self.cursor.expect(&TokenType::Equal) {
            self.emit(crate::error::from_cursor(e, ParseContext::StaticDecl));
            self.recover_to_decl();
            self.leave(prev);
            return None;
        }

        let Some(value) = self.parse_expr_or_none() else {
            self.recover_to_decl();
            self.leave(prev);
            return None;
        };
        let span = lo.merge(&value.span);
        self.eat_sep();
        self.leave(prev);

        Some(StaticDecl { attributes: attrs, visibility: vis, name, ty, value, span })
    }

    /// The span of the item's own `@tier(...)` attribute, if it wrote one.
    fn own_tier_attr_span(attrs: &'ast [Attribute<'ast>]) -> Option<Span> {
        attrs.iter().find(|a| a.name == "tier").map(|a| a.span)
    }

    /// A `const` is tier-agnostic (every tier may read it), so an explicit
    /// `@tier(...)` on the item itself would be silently ignored. Report it
    /// instead. A surrounding `@tier(...) { }` block is fine and is not an
    /// own attribute: `apply_block_attrs` never gives a const a tier.
    fn reject_own_tier_on_const(&mut self, attrs: &'ast [Attribute<'ast>]) {
        if let Some(span) = Self::own_tier_attr_span(attrs) {
            self.emit(ubel_stratum::error_management::errors::ParseError::IllegalInContext {
                what:       "a tier annotation on a const".to_string(),
                reason:     "a const can be read from every tier, so the annotation would have no effect".to_string(),
                span,
                suggestion: Some("remove the `@tier(...)` annotation".to_string()),
            });
        }
    }

    /// A `static` lives in the HIGH tier, so `@tier(high)` is accepted as a
    /// redundant spelling and `@tier(mid)` / `@tier(low)` are errors.
    fn require_high_tier_on_static(
        &mut self,
        attrs:    &'ast [Attribute<'ast>],
        tier_opt: Option<TierAnnotation>,
    ) {
        let Some(t) = tier_opt else { return; };
        if t == TierAnnotation::High { return; }
        let span = Self::own_tier_attr_span(attrs).unwrap_or_else(|| self.span());
        self.emit(ubel_stratum::error_management::errors::ParseError::IllegalInContext {
            what:       format!(
                "`@tier({})` on a static",
                if t == TierAnnotation::Mid { "mid" } else { "low" },
            ),
            reason:     "a static lives in the HIGH tier and can only be used from `@tier(high)` code".to_string(),
            span,
            suggestion: Some("remove the annotation or write `@tier(high)`".to_string()),
        });
    }

    // ── Type alias ────────────────────────────────────────────────────────────

    pub(crate) fn parse_type_alias(
        &mut self,
        attrs: &'ast [Attribute<'ast>],
    ) -> Option<TypeAlias<'ast>> {
        let prev = self.enter(ParseContext::TypeAliasDecl);
        let lo   = self.span();

        self.cursor.advance(); // consume `type`
        let (name, _) = self.expect_ident()?;
        let generic_params = self.parse_generic_params();

        if let Err(e) = self.cursor.expect(&TokenType::Equal) {
            self.emit(crate::error::from_cursor(e, ParseContext::TypeAliasDecl));
            self.leave(prev);
            return None;
        }

        let ty   = self.parse_type_expr()?;
        let span = lo.merge(&ty.span);
        self.eat_sep();
        self.leave(prev);

        Some(TypeAlias { attributes: attrs, name, generic_params, ty, span })
    }

    // ── Package + import (used by parse_program.rs) ───────────────────────────

    pub(crate) fn parse_package(&mut self) -> Option<PackageDecl<'ast>> {
        let lo = self.span();
        self.cursor.advance(); // consume `package`
        let (path, span) = self.parse_qualified_path()?;
        self.eat_sep();
        Some(PackageDecl { path, span: lo.merge(&span) })
    }

    pub(crate) fn parse_import(&mut self) -> Option<Import<'ast>> {
        let prev = self.enter(ParseContext::ImportDecl);
        let lo   = self.span();

        let kind = match self.cursor.peek().clone() {
            // `summon path.to.Thing`
            TokenType::Summon => {
                self.cursor.advance();
                let (path, _) = self.parse_qualified_path()?;
                let alias = if self.cursor.eat(&TokenType::As) {
                    self.eat_ident().map(|(n, _)| n)
                } else { None };
                ImportKind::Summon { path, alias }
            }
            // `from path.to summon [A, B]` or `from path.to summon A`
            TokenType::From => {
                self.cursor.advance();
                let (module_path, _) = self.parse_qualified_path()?;
                if let Err(e) = self.cursor.expect(&TokenType::Summon) {
                    self.emit(crate::error::from_cursor(e, ParseContext::ImportDecl));
                    self.leave(prev);
                    return None;
                }
                let items = if self.cursor.eat(&TokenType::LeftBracket) {
                    let mut names: Vec<&'ast str> = Vec::with_capacity(cap::IMPORT_LIST);
                    while !self.cursor.is_at(&TokenType::RightBracket) && !self.cursor.is_eof() {
                        let pos_before = self.cursor.position();
                        if let Some((n, _)) = self.eat_ident() { names.push(n); }
                        self.eat_sep();
                        self.guard_progress(pos_before);
                    }
                    self.cursor.eat(&TokenType::RightBracket);
                    ImportItems::List(self.arena.alloc_slice_clone(&names))
                } else {
                    let (name, _) = self.expect_ident()?;
                    ImportItems::Single(name)
                };
                ImportKind::FromSummon { module_path, items }
            }
            _ => {
                self.expected(&["'summon'", "'from'"]);
                self.leave(prev);
                return None;
            }
        };

        let span = lo.merge(&self.span());
        self.eat_sep();
        self.leave(prev);
        Some(Import { kind, span })
    }

    // ── Qualified path helper ─────────────────────────────────────────────────

    pub(crate) fn parse_qualified_path(&mut self) -> Option<(&'ast [&'ast str], Span)> {
        let lo = self.span();
        let mut segs: Vec<&'ast str> = Vec::with_capacity(cap::PATH_SEGS);
        let (first, _) = self.expect_ident()?;
        segs.push(first);
        while self.cursor.eat(&TokenType::Dot) {
            if let Some((seg, _)) = self.eat_ident() {
                segs.push(seg);
            } else {
                self.expected(&["identifier after '.'"]);
                break;
            }
        }
        let hi   = self.span();
        let path = self.arena.alloc_slice_clone(&segs);
        Some((path, lo.merge(&hi)))
    }

    // ── Expression stub (implemented in parse_expr.rs) ────────────────────────

    /// Thin wrapper — calls into parse_expr.rs.
    /// Returns None and emits an error if the expression fails.
    pub(crate) fn parse_expr_or_none(
        &mut self,
    ) -> Option<&'ast ubel_stratum::ast::expressions::Expr<'ast>> {
        crate::parsers::parse_expr::parse_expr(self)
    }
}

/// Free function version — callable from parse_stmt.rs and parse_expr.rs.
pub(crate) fn parse_type_annotation_opt<'ast, 'tok>(
    p: &mut Parser<'ast, 'tok>,
) -> Option<&'ast ubel_stratum::ast::types::Type<'ast>> {
    if p.cursor.eat(&TokenType::Colon) { p.parse_type_expr() } else { None }
            }
