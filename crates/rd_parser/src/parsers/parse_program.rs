// crates/rd_parser/src/parsers/parse_program.rs
//
// Top-level program parser. Drives the item loop.
//
// Grammar:
//   Program    ::= PackageDecl? ImportList? ItemList?
//   PackageDecl::= "package" QualifiedIdent
//   Import     ::= "summon" ... | "from" ... "summon" ...
//   Item       ::= FunctionDecl | StructDecl | ...

use ubel_stratum::{
    ast::root::{Import, Item, Program},
    lexer::TokenType,
};

use crate::parser::{cap, Parser};

pub(crate) fn parse_program<'ast, 'tok>(p: &mut Parser<'ast, 'tok>) -> Program<'ast> {
    let lo = p.span();

    // ── Package declaration ───────────────────────────────────────────────────
    let package = if p.cursor.is_at(&TokenType::Package) {
        p.parse_package()
    } else { None };

    // ── Import list ───────────────────────────────────────────────────────────
    let mut imports: Vec<Import<'ast>> = Vec::with_capacity(cap::IMPORT_LIST);
    while matches!(p.cursor.peek(), TokenType::Summon | TokenType::From)
        && !p.cursor.is_eof()
    {
        // A `from` could be a LINQ expression at statement level later, but at
        // the top level before any items it's always an import.
        if let Some(imp) = p.parse_import() {
            imports.push(imp);
        } else {
            // Skip to next line-start heuristic
            p.cursor.skip_until_any(&[
                TokenType::Summon, TokenType::From,
                TokenType::Fn, TokenType::Struct, TokenType::Enum,
                TokenType::Trait, TokenType::Impl, TokenType::Extend,
                TokenType::Const, TokenType::Static, TokenType::TypeKw, TokenType::Pub,
                TokenType::At, TokenType::Edge, TokenType::Package,
                TokenType::Eof,
            ]);
        }
    }
    let imports = p.arena.alloc_slice_clone(&imports);

    // ── Item list ─────────────────────────────────────────────────────────────
    let mut items: Vec<Item<'ast>> = Vec::with_capacity(16);

    while !p.cursor.is_eof() {
        // Eat any stray separators between top-level items
        while p.eat_sep() {}

        if p.cursor.is_eof() { break; }

        p.parse_item_or_block(&mut items);
    }

    let items = p.arena.alloc_slice_clone(&items);
    let hi    = p.span();

    Program {
        package,
        imports,
        items,
        span: lo.merge(&hi),
    }
      }
