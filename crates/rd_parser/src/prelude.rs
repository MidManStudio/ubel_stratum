//! The prelude: declarations every program sees without writing them.
//!
//! The prelude is Ubel source (`PRELUDE_SRC`), lexed once, with every token
//! span shifted into the range reserved by `Span::is_prelude`, then parsed
//! with each program and put in front of the program's own items by
//! `inject`. Writing it in Ubel means the prelude traits go through exactly
//! the machinery a user trait does (signatures, default methods, bounds).
//!
//! Shadowing: a program that declares one of these names keeps its own, and
//! the prelude items that depend on the shadowed one are left out with it
//! (`DEPENDS_ON`), so a prelude signature never binds to a user's type.
//!
//! The trait names are the ones `@derive` already uses. `Hasher` is the
//! state `Hash.hash` writes into; `Hash.hash` itself is native. What a built-in or
//! derived type does for each method is native, see
//! `ubel_stratum::sema` (satisfaction) and the interpreter (the methods).

use std::sync::OnceLock;

use ubel_stratum::ast::arena::AstArena;
use ubel_stratum::ast::root::{Item, Program};
use ubel_stratum::lexer::token::{Span, PRELUDE_LINE_START, PRELUDE_SPAN_START};
use ubel_stratum::lexer::Token;

use crate::parser::Parser;

/// The prelude's source. Method names and signatures are documented in
/// docs/TRAITS_DESIGN.md, "What S2 settled".
pub const PRELUDE_SRC: &str = r#"enum Ordering {
    Less,
    Equal,
    Greater,
}

trait PartialEq {
    fn eq(self, other: Self) bool
    fn ne(self, other: Self) bool { return !self.eq(other) }
}

trait Eq {
}

trait PartialOrd {
    fn partial_cmp(self, other: Self) Ordering?
    fn lt(self, other: Self) bool {
        let c = self.partial_cmp(other)
        return c == Ordering.Less
    }
    fn le(self, other: Self) bool {
        let c = self.partial_cmp(other)
        return c == Ordering.Less or c == Ordering.Equal
    }
    fn gt(self, other: Self) bool {
        let c = self.partial_cmp(other)
        return c == Ordering.Greater
    }
    fn ge(self, other: Self) bool {
        let c = self.partial_cmp(other)
        return c == Ordering.Greater or c == Ordering.Equal
    }
}

trait Ord {
    fn cmp(self, other: Self) Ordering
}

trait Clone {
    fn clone(self) Self
}

struct Hasher {
    state: u64,
    fn new() Hasher { return Hasher { state = 1469598103934665603u64 } }
    fn finish(self) u64 { return self.state }
}

trait Hash {
    fn hash(self, state: Hasher) void
}
"#;

/// Prelude names and what each one needs declared before it can stand. A
/// program that declares a name on the left loses it and every name that
/// (transitively) depends on it.
const DEPENDS_ON: &[(&str, &[&str])] = &[
    ("Ordering",   &[]),
    ("PartialEq",  &[]),
    ("Eq",         &["PartialEq"]),
    ("PartialOrd", &["PartialEq", "Ordering"]),
    ("Ord",        &["PartialOrd", "Eq", "Ordering"]),
    ("Clone",      &[]),
    ("Hasher",     &[]),
    ("Hash",       &["Hasher", "Eq", "PartialEq"]),
];

/// The prelude's tokens with their spans shifted. Lexed once per process.
fn prelude_tokens() -> &'static [Token] {
    static TOKENS: OnceLock<Vec<Token>> = OnceLock::new();
    TOKENS.get_or_init(|| {
        let mut tokens = ubel_stratum::lexer::tokenize(PRELUDE_SRC)
            .unwrap_or_else(|_| panic!("the prelude must lex"));
        for t in tokens.iter_mut() {
            t.span = Span::new(
                t.span.start + PRELUDE_SPAN_START,
                t.span.end   + PRELUDE_SPAN_START,
                t.span.line  + PRELUDE_LINE_START,
                t.span.column,
            );
        }
        tokens
    })
}

fn item_name<'a>(item: &'a Item<'_>) -> Option<&'a str> {
    match item {
        Item::Function(f)  => Some(f.name),
        Item::Struct(s)    => Some(s.name),
        Item::Enum(e)      => Some(e.name),
        Item::Trait(t)     => Some(t.name),
        Item::Const(c)     => Some(c.name),
        Item::Static(s)    => Some(s.name),
        Item::TypeAlias(a) => Some(a.name),
        Item::Impl(_) | Item::Extend(_) => None,
    }
}

/// Which prelude names a program with these top-level names keeps.
fn kept_names(user_names: &[&str]) -> Vec<&'static str> {
    let mut lost: Vec<&str> = DEPENDS_ON.iter()
        .map(|(n, _)| *n)
        .filter(|n| user_names.contains(n))
        .collect();
    // Anything depending on a lost name is lost too (the table is ordered
    // so one pass reaches a fixpoint).
    for (name, deps) in DEPENDS_ON {
        if !lost.contains(name) && deps.iter().any(|d| lost.contains(d)) {
            lost.push(name);
        }
    }
    DEPENDS_ON.iter().map(|(n, _)| *n).filter(|n| !lost.contains(n)).collect()
}

/// Put the prelude's items in front of `program`'s own items.
pub fn inject<'ast>(arena: &'ast AstArena, program: &mut Program<'ast>) {
    let tokens = prelude_tokens();
    let parsed = Parser::new(arena, tokens, String::new())
        .parse_program()
        .unwrap_or_else(|_| panic!("the prelude must parse"));

    let user_names: Vec<&str> = program.items.iter().filter_map(item_name).collect();
    let keep = kept_names(&user_names);

    let mut items: Vec<Item<'ast>> = Vec::with_capacity(parsed.items.len() + program.items.len());
    for item in parsed.items.iter() {
        if item_name(item).is_some_and(|n| keep.contains(&n)) {
            items.push(item.clone());
        }
    }
    items.extend(program.items.iter().cloned());
    program.items = arena.alloc_slice_clone(&items);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_declared_keeps_everything() {
        assert_eq!(kept_names(&[]), vec!["Ordering", "PartialEq", "Eq", "PartialOrd", "Ord", "Clone", "Hasher", "Hash"]);
    }

    #[test]
    fn a_user_ordering_takes_the_ordering_traits_with_it() {
        assert_eq!(kept_names(&["Ordering"]), vec!["PartialEq", "Eq", "Clone", "Hasher", "Hash"]);
    }

    #[test]
    fn a_user_partial_eq_takes_every_trait_built_on_it() {
        assert_eq!(kept_names(&["PartialEq"]), vec!["Ordering", "Clone", "Hasher"]);
    }

    #[test]
    fn a_user_clone_loses_only_clone() {
        assert_eq!(kept_names(&["Clone"]), vec!["Ordering", "PartialEq", "Eq", "PartialOrd", "Ord", "Hasher", "Hash"]);
    }

    #[test]
    fn a_user_hasher_takes_hash_with_it() {
        assert_eq!(kept_names(&["Hasher"]), vec!["Ordering", "PartialEq", "Eq", "PartialOrd", "Ord", "Clone"]);
    }

    #[test]
    fn a_user_eq_takes_ord_and_hash_but_leaves_partial_eq() {
        assert_eq!(kept_names(&["Eq"]), vec!["Ordering", "PartialEq", "PartialOrd", "Clone", "Hasher"]);
    }

    #[test]
    fn every_prelude_item_is_declared_in_the_dependency_table() {
        let arena = AstArena::new();
        let parsed = Parser::new(&arena, prelude_tokens(), String::new()).parse_program().unwrap();
        let names: Vec<&str> = parsed.items.iter().filter_map(item_name).collect();
        let table: Vec<&str> = DEPENDS_ON.iter().map(|(n, _)| *n).collect();
        assert_eq!(names, table);
    }
}
