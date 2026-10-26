// crates/rd_parser/src/parsers/parse_attr.rs
//! Attribute parsing: `@name(args)` before any declaration.
//!
//! Every declaration parser calls `parse_attribute_list` first, collects
//! all `@` annotations, then proceeds to parse the declaration keyword.
//!
//! # Built-in attributes
//!
//! Recognised via a `phf_map!` keyed on the attribute name string.
//! Unknown names are accepted as custom attributes — they are stored in the
//! AST and passed through to downstream tools (ECS codegen, IDE, etc.).

use phf::phf_map;

use ubel_stratum::{
    ast::{
        arena::BumpVec,
        common::{AttrArg, AttrValue, Attribute, TierAnnotation},
    },
    error_management::errors::ParseContext,
    lexer::TokenType,
};

use crate::parser::{cap, Parser};

// ── Built-in attribute registry ───────────────────────────────────────────────

/// Compiler-recognised attribute names.
/// Unknown names → custom attribute, stored as-is, no validation here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltinAttr {
    /// `@tier(high | mid | low)` — memory tier annotation.
    Tier,
    /// `@cfg(...)` — conditional compilation.
    Cfg,
    /// `@core` — ECS: pack this component into a dense Archetype table.
    Core,
    /// `@tag` — ECS: store in a bitset/sparse-set (zero-size marker).
    Tag,
    /// `@system` — ECS: marks a function as an update system.
    System,
    /// `@doc("...")` — documentation comment attribute.
    Doc,
    /// `@inline` — hint to the codegen to inline this function.
    Inline,
    /// `@cold` — hint that this function is rarely called (error paths, etc.).
    Cold,
    /// `@derive(PartialEq)` — opt a `struct` into structural (field-wise)
    /// `==`, replacing the tier-consistent `Rc::ptr_eq` default. See
    /// docs/PRINT_FORMAT_RULES.md §6 for why `Debug`/`Display` are
    /// deliberately NOT part of this — both are automatic already, no
    /// opt-in attribute needed.
    Derive,
}

/// O(1) lookup: attribute name string → `BuiltinAttr`.
/// `phf_map!` keys are `&'static str` — valid because names are string literals.
static BUILTIN_ATTRS: phf::Map<&'static str, BuiltinAttr> = phf_map! {
    "tier"   => BuiltinAttr::Tier,
    "cfg"    => BuiltinAttr::Cfg,
    "core"   => BuiltinAttr::Core,
    "tag"    => BuiltinAttr::Tag,
    "system" => BuiltinAttr::System,
    "doc"    => BuiltinAttr::Doc,
    "inline" => BuiltinAttr::Inline,
    "cold"   => BuiltinAttr::Cold,
    "derive" => BuiltinAttr::Derive,
};

/// Valid keys inside `@cfg(key = "value")` or `@cfg(key)`.
#[derive(Debug, Clone, Copy)]
pub enum CfgKey {
    Target,    // "wasm" | "native" | "server"
    Platform,  // "windows" | "linux" | "macos" | "android" | "ios" | "switch" | "ps5" | "xbox"
    Build,     // "debug" | "release" | "profile"
    Feature,   // any named feature string
    Render,    // "vulkan" | "dx12" | "metal" | "wgpu"
    Editor,    // bare boolean flag
}

static CFG_KEYS: phf::Map<&'static str, CfgKey> = phf_map! {
    "target"   => CfgKey::Target,
    "platform" => CfgKey::Platform,
    "build"    => CfgKey::Build,
    "feature"  => CfgKey::Feature,
    "render"   => CfgKey::Render,
    "editor"   => CfgKey::Editor,
};

/// Composition operators inside `@cfg(...)`.
/// Only these three names are valid as nested `Ident(args)` forms.
static CFG_COMPOSE: phf::Map<&'static str, ()> = phf_map! {
    "not" => (),
    "any" => (),
    "all" => (),
};

// ── Public entry point ────────────────────────────────────────────────────────

impl<'ast, 'tok> Parser<'ast, 'tok> {
    /// Parse zero or more `@attr` annotations that precede a declaration.
    ///
    /// Returns an arena-interned slice of `Attribute` nodes.
    /// Also returns the resolved `TierAnnotation` if a `@tier(...)` was found,
    /// so the declaration parser can set `self.tier` before parsing the body.
    ///
    /// Stops as soon as the current token is not `@`.
    pub(crate) fn parse_attribute_list(
        &mut self,
    ) -> (&'ast [Attribute<'ast>], Option<TierAnnotation>) {
        let mut attrs: BumpVec<Attribute<'ast>> =
            self.bump_vec_cap(cap::ATTR_ARGS);
        let mut resolved_tier: Option<TierAnnotation> = None;

        while self.cursor.is_at(&TokenType::At) {
            let prev_ctx = self.enter(ParseContext::AttributeDecl);

            if let Some(attr) = self.parse_single_attr() {
                // If this is @tier(...), extract and record the tier.
                if let Some(tier) = try_extract_tier(&attr) {
                    resolved_tier = Some(tier);
                }
                attrs.push(attr);
            } else {
                // Failed to parse the attribute — recover to the next `@` or
                // declaration keyword so we don't spiral.
                self.cursor.skip_until_any(&[
                    TokenType::At,
                    TokenType::Fn,
                    TokenType::Struct,
                    TokenType::Enum,
                    TokenType::Trait,
                    TokenType::Impl,
                    TokenType::Extend,
                    TokenType::Pub,
                    TokenType::Eof,
                ]);
            }

            self.leave(prev_ctx);
        }

        (attrs.into_bump_slice(), resolved_tier)
    }

    // ── Single attribute ──────────────────────────────────────────────────────

    /// Parse one `@name` or `@name(arg, arg, ...)` annotation.
    /// The leading `@` must be the current token.
    fn parse_single_attr(&mut self) -> Option<Attribute<'ast>> {
        let at_span = self.advance_span(); // consume `@`

        // Attribute name must be an identifier immediately after `@` — no space.
        let (name, name_span) = self.expect_ident()?;

        // Validate against built-in list (or pass through as custom).
        let builtin = BUILTIN_ATTRS.get(name).copied();

        // Args are optional: `@cold` has no parens, `@tier(low)` does.
        let args: &'ast [AttrArg<'ast>] = if self.cursor.is_at(&TokenType::LeftParen) {
            self.parse_attr_arg_list(name, builtin)?
        } else {
            // No argument list — check built-ins that REQUIRE args.
            if matches!(builtin, Some(BuiltinAttr::Tier)) {
                self.emit(crate::error::illegal_here(
                    "@tier",
                    "@tier requires an argument: @tier(high), @tier(mid), or @tier(low)",
                    name_span,
                    Some("add (high), (mid), or (low) after @tier"),
                ));
                return None;
            }
            &[]
        };

        let span = at_span.merge(&self.span());
        Some(Attribute { name, args, span })
    }

    // ── Argument list ─────────────────────────────────────────────────────────

    /// Parse `( arg, arg, ... )` for an attribute.
    /// The `(` must be the current token.
    fn parse_attr_arg_list(
        &mut self,
        _attr_name: &str,
        builtin:   Option<BuiltinAttr>,
    ) -> Option<&'ast [AttrArg<'ast>]> {
        let open_span = self.span();
        // consume `(`
        self.cursor.advance();

        // Empty arg list `@attr()` — unusual but valid.
        if self.cursor.eat(&TokenType::RightParen) {
            return Some(&[]);
        }

        let mut args: BumpVec<AttrArg<'ast>> = self.bump_vec_cap(cap::ATTR_ARGS);

        loop {
            // For @cfg we apply extra validation on each arg.
            let arg = if matches!(builtin, Some(BuiltinAttr::Cfg)) {
                self.parse_cfg_arg()?
            } else {
                self.parse_generic_attr_arg()?
            };

            args.push(arg);

            if self.cursor.eat(&TokenType::Comma) {
                // Trailing comma before `)` is allowed.
                if self.cursor.is_at(&TokenType::RightParen) {
                    break;
                }
            } else {
                break;
            }
        }

        // Expect closing `)`.
        if !self.cursor.eat(&TokenType::RightParen) {
            let at = self.span();
            self.emit(crate::error::unclosed('(', open_span, None, at));
            // Don't return None here — we have valid args, just a missing close.
            // The declaration parser will re-sync at the next keyword.
        }

        Some(args.into_bump_slice())
    }

    // ── Generic attribute argument (non-cfg) ──────────────────────────────────

    /// Parse one argument inside a non-`@cfg` attribute.
    ///
    /// ```text
    /// arg ::= Ident                         // bare: @deprecated
    ///       | StringLit                     // string: @doc("blah")
    ///       | IntLit                        // integer: @version(2)
    ///       | "true" | "false"              // boolean: @inline(true)
    ///       | Ident "=" AttrValue           // key=value: @min(value=0)
    ///       | Ident "(" AttrArg* ")"        // nested: (custom use)
    /// ```
    fn parse_generic_attr_arg(&mut self) -> Option<AttrArg<'ast>> {
        match self.cursor.peek().clone() {
            // String literal
            TokenType::StringLit(s) => {
                let s = self.intern(&s);
                self.cursor.advance();
                Some(AttrArg::Str(s))
            }
            // Integer literal
            TokenType::IntLit(n) => {
                self.cursor.advance();
                Some(AttrArg::Int(n))
            }
            // Boolean: `true` / `false`
            TokenType::True => { self.cursor.advance(); Some(AttrArg::Bool(true)) }
            TokenType::False => { self.cursor.advance(); Some(AttrArg::Bool(false)) }
            // Identifier: bare, key=value, or nested(...). Routed through
            // eat_ident() (rather than matching TokenType::Ident directly)
            // so keyword-shaped identifiers are accepted too — in
            // particular `high`/`mid`/`low` for `@tier(mid)`, which lex as
            // TokenType::High/Mid/Low, not TokenType::Ident. See
            // eat_ident()'s own doc comment and docs/MEMORY_MODEL.md.
            _ => {
                if let Some((name, _span)) = self.eat_ident() {
                    if self.cursor.eat(&TokenType::Equal) {
                        // key = value
                        let value = self.parse_attr_value()?;
                        Some(AttrArg::Named { key: name, value })
                    } else if self.cursor.is_at(&TokenType::LeftParen) {
                        // nested(args)
                        let inner = self.parse_attr_arg_list(name, None)?;
                        Some(AttrArg::Nested { name, args: inner })
                    } else {
                        Some(AttrArg::Ident(name))
                    }
                } else {
                    self.expected(&["attribute argument"]);
                    None
                }
            }
        }
    }

    // ── @cfg argument (with cfg-specific validation) ──────────────────────────

    /// Parse one argument specifically inside `@cfg(...)`.
    ///
    /// ```text
    /// cfg_arg ::= "not"  "(" cfg_arg ")"          // negation
    ///           | "any"  "(" cfg_arg ("," cfg_arg)* ")"  // OR
    ///           | "all"  "(" cfg_arg ("," cfg_arg)* ")"  // AND
    ///           | CfgKey "=" StringLit             // e.g. target = "wasm"
    ///           | CfgKey                           // bare flag: editor, debug
    /// ```
    fn parse_cfg_arg(&mut self) -> Option<AttrArg<'ast>> {
        let prev_ctx = self.enter(ParseContext::CfgAttribute);
        let result = self.parse_cfg_arg_inner();
        self.leave(prev_ctx);
        result
    }

    fn parse_cfg_arg_inner(&mut self) -> Option<AttrArg<'ast>> {
        let _name_tok = self.cursor.peek_token().clone();
        let (name, name_span) = self.eat_ident()?;

        // Composition operators: not / any / all
        if CFG_COMPOSE.contains_key(name) {
            // Must be followed by `(`.
            if !self.cursor.is_at(&TokenType::LeftParen) {
                self.emit(crate::error::illegal_here(
                    name,
                    "composition operator in @cfg must be followed by (args)",
                    name_span,
                    Some(&format!("use {}(...)", name)),
                ));
                return None;
            }
            let open_span = self.span();
            self.cursor.advance(); // consume `(`

            let mut inner: BumpVec<AttrArg<'ast>> = self.bump_vec_cap(4);
            loop {
                inner.push(self.parse_cfg_arg_inner()?);
                if self.cursor.eat(&TokenType::Comma) {
                    if self.cursor.is_at(&TokenType::RightParen) { break; }
                } else {
                    break;
                }
            }

            if !self.cursor.eat(&TokenType::RightParen) {
                let at = self.span();
                self.emit(crate::error::unclosed('(', open_span, None, at));
            }

            let args = inner.into_bump_slice();
            return Some(AttrArg::Nested { name, args });
        }

        // Validate cfg key names — warn on unknown keys but don't hard-fail
        // so custom feature flags still work: `@cfg(feature = "my_feature")`.
        if !CFG_KEYS.contains_key(name) {
            // Unknown key — accepted as a custom bare flag.
            // No error; just store it.
        }

        if self.cursor.eat(&TokenType::Equal) {
            // key = "value"
            let value = self.parse_attr_value()?;
            Some(AttrArg::Named { key: name, value })
        } else {
            // Bare flag: `@cfg(editor)`, `@cfg(debug)`
            Some(AttrArg::Ident(name))
        }
    }

    // ── AttrValue ─────────────────────────────────────────────────────────────

    /// Parse the right-hand side of a `key = <value>` attribute argument.
    fn parse_attr_value(&mut self) -> Option<AttrValue<'ast>> {
        match self.cursor.peek().clone() {
            TokenType::StringLit(s) => {
                let s = self.intern(&s);
                self.cursor.advance();
                Some(AttrValue::Str(s))
            }
            TokenType::IntLit(n) => {
                self.cursor.advance();
                Some(AttrValue::Int(n))
            }
            TokenType::True  => { self.cursor.advance(); Some(AttrValue::Bool(true)) }
            TokenType::False => { self.cursor.advance(); Some(AttrValue::Bool(false)) }
            TokenType::Ident(name) => {
                let name = name.clone();
                self.cursor.advance();
                Some(AttrValue::Ident(self.intern(&name)))
            }
            _ => {
                self.expected(&["string", "integer", "true", "false", "identifier"]);
                None
            }
        }
    }
}

// ── Tier extraction helper ────────────────────────────────────────────────────

/// Pull the resolved `TierAnnotation` out of a `@tier(...)` attribute.
/// Returns `None` if the attribute is not `@tier` or has invalid args.
fn try_extract_tier(attr: &Attribute<'_>) -> Option<TierAnnotation> {
    if attr.name != "tier" { return None; }

    // @tier must have exactly one bare-identifier argument.
    let arg = attr.args.first()?;
    match arg {
        AttrArg::Ident(tier_name) => match *tier_name {
            "high" => Some(TierAnnotation::High),
            "mid"  => Some(TierAnnotation::Mid),
            "low"  => Some(TierAnnotation::Low),
            _      => None, // invalid tier name — tier_check will catch it
        },
        _ => None,
    }
}

// ── Public re-export helper for declaration parsers ───────────────────────────

/// Whether the attribute list writes a `@tier(...)` at all, valid or not.
/// This, and not "the list is empty", decides whether an item keeps the tier
/// of the block, struct or impl around it: an unrelated attribute such as
/// `@inline` or `@doc(...)` must never change which tier a method has.
pub(crate) fn has_own_tier(attrs: &[Attribute<'_>]) -> bool {
    attrs.iter().any(|a| a.name == "tier")
}

/// Convenience: given an attribute list already parsed, find the tier.
/// Called by parse_decl.rs so it doesn't re-implement the extraction.
pub(crate) fn extract_tier_from_attrs(attrs: &[Attribute<'_>]) -> TierAnnotation {
    attrs
        .iter()
        .find_map(|a| try_extract_tier(a))
        .unwrap_or(TierAnnotation::High)
    }
