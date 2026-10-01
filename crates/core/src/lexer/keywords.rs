// src/lexer/keywords.rs

use phf::phf_map;
use crate::lexer::TokenType;

pub static KEYWORDS: phf::Map<&'static str, TokenType> = phf_map! {
    // ── Core keywords ────────────────────────────────────────────
    "fn"         => TokenType::Fn,
    "let"        => TokenType::Let,
    "mut"        => TokenType::Mut,
    "const"      => TokenType::Const,
    "static"     => TokenType::Static,
    "if"         => TokenType::If,
    "elif"       => TokenType::Elif,
    "else"       => TokenType::Else,
    "match"      => TokenType::Match,
    "where"      => TokenType::Where,
    "then"       => TokenType::Then,
    "for"        => TokenType::For,
    "in"         => TokenType::In,
    "while"      => TokenType::While,
    "loop"       => TokenType::Loop,
    "break"      => TokenType::Break,
    "continue"   => TokenType::Continue,
    "return"     => TokenType::Return,
    "summon"     => TokenType::Summon,
    "from"       => TokenType::From,
    "as"         => TokenType::As,
    "package"    => TokenType::Package,
    "async"      => TokenType::Async,
    "await"      => TokenType::Await,
    "Task"       => TokenType::Task,
    "try"        => TokenType::Try,
    "catch"      => TokenType::Catch,
    "fail"       => TokenType::Fail,
    "struct"     => TokenType::Struct,
    "enum"       => TokenType::Enum,
    "trait"      => TokenType::Trait,
    "impl"       => TokenType::Impl,
    "pub"        => TokenType::Pub,
    "edge"       => TokenType::Edge,
    "unsafe"     => TokenType::Unsafe,
    "with"       => TokenType::With,
    "defer"      => TokenType::Defer,
    "and"        => TokenType::And,
    "or"         => TokenType::Or,
    "not"        => TokenType::Not,
    "true"       => TokenType::True,
    "false"      => TokenType::False,
    "null"       => TokenType::Null,
    "self"       => TokenType::SelfKw,
    "getter"     => TokenType::Getter,
    "setter"     => TokenType::Setter,
    "ref"        => TokenType::Ref,
    "deref"      => TokenType::Deref,

    // ── Declaration / statement keywords ─────────────────────────
    "extend"     => TokenType::Extend,
    "type"       => TokenType::TypeKw,
    "extract"    => TokenType::Extract,
    "using"      => TokenType::Using,
    "lifetime"   => TokenType::Lifetime,

    // ── Tier system ──────────────────────────────────────────────
    "tier"       => TokenType::Tier,
    "high"       => TokenType::High,
    "mid"        => TokenType::Mid,
    "low"        => TokenType::Low,

    // ── Allocator keywords ───────────────────────────────────────
    "arena"      => TokenType::Arena,
    "pool"       => TokenType::Pool,
    "gc"         => TokenType::Gc,
    "heap"       => TokenType::Heap,

    // ── Built-in collection type names ───────────────────────────
    "List"       => TokenType::KwList,
    "Dictionary" => TokenType::KwDictionary,
    "Set"        => TokenType::KwSet,
    "Queue"      => TokenType::KwQueue,
    "Stack"      => TokenType::KwStack,
    "InlineList" => TokenType::KwInlineList,
};

#[inline]
pub fn get_keyword(word: &str) -> Option<TokenType> {
    KEYWORDS.get(word).cloned()
}
