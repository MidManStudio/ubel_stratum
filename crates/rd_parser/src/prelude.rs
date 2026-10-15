//! The prelude: declarations every program sees without writing them.
//!
//! The prelude is Ubel source (`PRELUDE_SRC`), lexed once, with every token
//! span shifted into the range reserved by `Span::is_prelude`, then parsed
//! with each program and put in front of the program's own items by
//! `inject`. Writing it in Ubel means the prelude traits go through exactly
//! the machinery a user trait does (signatures, default methods, bounds).
//!
//! Shadowing: a program that declares one of these names keeps its own, and
//! the prelude items that depend on the shadowed one are left out with it,
//! so a prelude signature never binds to a user's type. What a trait depends
//! on is what its declaration says: its supertraits (`trait Ord: PartialOrd
//! + Eq`) plus the types its signatures mention (`SIGNATURE_DEPS`).
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

trait Eq: PartialEq {
}

trait PartialOrd: PartialEq {
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

trait Ord: PartialOrd + Eq {
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

trait Hash: Eq {
    fn hash(self, state: Hasher) void
}
"#;

/// The prelude's names, in declaration order. A name comes after everything
/// it depends on, so one pass over this list in `kept_names` reaches a
/// fixpoint.
const NAMES: &[&str] = &[
    "Ordering", "PartialEq", "Eq", "PartialOrd", "Ord", "Clone", "Hasher", "Hash",
];

/// What a prelude item needs besides its supertraits (those are read from
/// the declaration): the types its signatures mention. A program that
/// declares a name loses it and every name that (transitively) depends on
/// it.
const SIGNATURE_DEPS: &[(&str, &[&str])] = &[
    ("PartialOrd", &["Ordering"]),
    ("Ord",        &["Ordering"]),
    ("Hash",       &["Hasher"]),
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

/// What the prelude item called `name` needs declared before it can stand:
/// the types its signatures mention (`SIGNATURE_DEPS`) and the supertraits
/// its declaration lists, read from `parsed` so the two cannot drift apart.
fn deps_of(name: &str, parsed: &[Item<'_>]) -> Vec<&'static str> {
    let mut deps: Vec<&'static str> = Vec::with_capacity(3);
    if let Some((_, sig)) = SIGNATURE_DEPS.iter().find(|(n, _)| *n == name) {
        deps.extend(sig.iter().copied());
    }
    for item in parsed {
        let Item::Trait(t) = item else { continue; };
        if t.name != name { continue; }
        for s in t.supertraits {
            if let Some(n) = NAMES.iter().copied().find(|n| *n == s.name) {
                deps.push(n);
            }
        }
    }
    deps
}

/// Which prelude names a program with these top-level names keeps.
fn kept_names(user_names: &[&str], parsed: &[Item<'_>]) -> Vec<&'static str> {
    let mut lost: Vec<&'static str> = NAMES.iter().copied()
        .filter(|n| user_names.contains(n))
        .collect();
    // Anything depending on a lost name is lost too (`NAMES` lists a name
    // after what it depends on, so one pass reaches a fixpoint).
    for name in NAMES {
        if !lost.contains(name) && deps_of(name, parsed).iter().any(|d| lost.contains(d)) {
            lost.push(name);
        }
    }
    NAMES.iter().copied().filter(|n| !lost.contains(n)).collect()
}

/// Put the prelude's items in front of `program`'s own items.
pub fn inject<'ast>(arena: &'ast AstArena, program: &mut Program<'ast>) {
    let tokens = prelude_tokens();
    let parsed = Parser::new(arena, tokens, String::new())
        .parse_program()
        .unwrap_or_else(|_| panic!("the prelude must parse"));

    let user_names: Vec<&str> = program.items.iter().filter_map(item_name).collect();
    let keep = kept_names(&user_names, parsed.items);

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

    /// `wasm32` has a 32-bit `usize`. A span range that does not fit it fails
    /// to compile there (it did once, with a range that started at 2^40), and
    /// that cannot be seen from a 64-bit build, so the range is checked here.
    #[test]
    fn the_prelude_span_range_fits_a_32_bit_usize() {
        let max = u32::MAX as u64;
        assert!(PRELUDE_SPAN_START as u64 <= max);
        assert!(PRELUDE_LINE_START as u64 <= max);
        for t in prelude_tokens() {
            assert!(t.span.start as u64 <= max && t.span.end as u64 <= max, "span {:?}", t.span);
            assert!(t.span.line as u64 <= max, "span {:?}", t.span);
        }
    }

    /// `kept_names` for the prelude as parsed.
    fn kept(user_names: &[&str]) -> Vec<&'static str> {
        let arena = AstArena::new();
        let parsed = Parser::new(&arena, prelude_tokens(), String::new()).parse_program().unwrap();
        kept_names(user_names, parsed.items)
    }

    #[test]
    fn nothing_declared_keeps_everything() {
        assert_eq!(kept(&[]), vec!["Ordering", "PartialEq", "Eq", "PartialOrd", "Ord", "Clone", "Hasher", "Hash"]);
    }

    #[test]
    fn a_user_ordering_takes_the_ordering_traits_with_it() {
        assert_eq!(kept(&["Ordering"]), vec!["PartialEq", "Eq", "Clone", "Hasher", "Hash"]);
    }

    #[test]
    fn a_user_partial_eq_takes_every_trait_built_on_it() {
        assert_eq!(kept(&["PartialEq"]), vec!["Ordering", "Clone", "Hasher"]);
    }

    #[test]
    fn a_user_clone_loses_only_clone() {
        assert_eq!(kept(&["Clone"]), vec!["Ordering", "PartialEq", "Eq", "PartialOrd", "Ord", "Hasher", "Hash"]);
    }

    #[test]
    fn a_user_hasher_takes_hash_with_it() {
        assert_eq!(kept(&["Hasher"]), vec!["Ordering", "PartialEq", "Eq", "PartialOrd", "Ord", "Clone"]);
    }

    #[test]
    fn a_user_eq_takes_ord_and_hash_but_leaves_partial_eq() {
        assert_eq!(kept(&["Eq"]), vec!["Ordering", "PartialEq", "PartialOrd", "Clone", "Hasher"]);
    }

    #[test]
    fn every_prelude_item_is_declared_in_the_dependency_table() {
        let arena = AstArena::new();
        let parsed = Parser::new(&arena, prelude_tokens(), String::new()).parse_program().unwrap();
        let names: Vec<&str> = parsed.items.iter().filter_map(item_name).collect();
        assert_eq!(names, NAMES);
    }

    #[test]
    fn dependencies_come_from_the_declarations_and_the_signature_table() {
        let arena = AstArena::new();
        let parsed = Parser::new(&arena, prelude_tokens(), String::new()).parse_program().unwrap();
        let deps = |n: &str| deps_of(n, parsed.items);
        assert_eq!(deps("Ordering"),   Vec::<&str>::new());
        assert_eq!(deps("PartialEq"),  Vec::<&str>::new());
        assert_eq!(deps("Eq"),         vec!["PartialEq"]);
        assert_eq!(deps("PartialOrd"), vec!["Ordering", "PartialEq"]);
        assert_eq!(deps("Ord"),        vec!["Ordering", "PartialOrd", "Eq"]);
        assert_eq!(deps("Clone"),      Vec::<&str>::new());
        assert_eq!(deps("Hasher"),     Vec::<&str>::new());
        assert_eq!(deps("Hash"),       vec!["Hasher", "Eq"]);
    }

    #[test]
    fn every_dependency_is_declared_before_the_item_that_needs_it() {
        let arena = AstArena::new();
        let parsed = Parser::new(&arena, prelude_tokens(), String::new()).parse_program().unwrap();
        for (i, name) in NAMES.iter().enumerate() {
            for dep in deps_of(name, parsed.items) {
                let at = NAMES.iter().position(|n| *n == dep).unwrap();
                assert!(at < i, "{name} depends on {dep}, which is declared after it");
            }
        }
    }

    #[test]
    fn a_prelude_supertrait_is_always_a_prelude_name() {
        let arena = AstArena::new();
        let parsed = Parser::new(&arena, prelude_tokens(), String::new()).parse_program().unwrap();
        for item in parsed.items {
            let Item::Trait(t) = item else { continue; };
            for s in t.supertraits {
                assert!(NAMES.contains(&s.name), "{} names {}, which is not a prelude name", t.name, s.name);
            }
        }
    }
}
