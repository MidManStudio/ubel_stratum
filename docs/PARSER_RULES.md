# Ubel Stratum — Parser Implementation Rules

> **Canonical reference for `crates/rd_parser`.  
> Every contributor reads this before touching a parse file.**

---

## 1. Architecture Decision

### What We Use: Recursive Descent + Pratt + Targeted Memoization

No table-driven parser. No code generation. No `build.rs`. Here is why each
candidate was considered and dismissed:

| Approach | Why Not |
|---|---|
| **LALRPOP (LALR)** | Compile-time code-gen too heavy for target hardware; already in `crates/parser` for reference only |
| **GLR** | Handles ambiguous grammars — our grammar is designed to be unambiguous; adds irreducible runtime overhead we do not need |
| **Full Packrat / PEG** | Memoises *every* rule at *every* position; blows memory on large source files; we only need memoisation at three specific ambiguity points |
| **Action/Goto Tables** | Table-driven LR parsing is fast but impossible to debug, produces terrible error messages, and requires external generation — all three are blockers for this project |

### What We Use Instead

```
┌─────────────────────────────────────────────────────────────┐
│  Top-level / Declarations / Statements / Types              │
│  → Recursive Descent (RD)                                   │
│                                                             │
│  Expressions (all operator forms)                           │
│  → Pratt / TDOP (Top-Down Operator Precedence)              │
│                                                             │
│  Specific ambiguities only                                  │
│  → Targeted cursor-restore memoisation (≤3 rules)           │
└─────────────────────────────────────────────────────────────┘
```

This is the same architecture used by Rust (rustc), Go, TypeScript, and
Python's CPython — not by accident.

---

## 2. Directory Layout

```
crates/rd_parser/
├── Cargo.toml
└── src/
    ├── lib.rs              ← public API: parse() / parse_expr_str()
    ├── cursor.rs           ← Cursor<'tok>: peek/advance/expect/sync
    ├── error.rs            ← error constructor helpers → core ParseError
    ├── parser.rs           ← Parser<'ast,'tok> struct + shared helpers
    └── parsers/
        ├── mod.rs          ← declares all sub-modules
        ├── parse_attr.rs   ← @tier, @cfg, @core, @tag, custom attrs
        ├── parse_type.rs   ← type expressions, lifetimes, generics
        ├── parse_pattern.rs← destructure patterns, match arms
        ├── parse_expr.rs   ← Pratt expression parser (hot path)
        ├── parse_stmt.rs   ← statements + blocks
        ├── parse_decl.rs   ← fn / struct / enum / trait / impl / extend
        └── parse_program.rs← package, imports, top-level item list
```

Each `parsers/parse_*.rs` file adds `impl Parser<'ast, 'tok> { ... }` methods.
This is valid Rust — `impl` blocks for the same type may appear in any module
within the same crate.

---

## 3. Performance Rules — No Exceptions

### 3.1 Inlining

```rust
// HOT PATH — Cursor primitives: called O(n_tokens) per file.
// MUST be #[inline(always)].
#[inline(always)] pub fn peek(&self) -> &TokenType { ... }
#[inline(always)] pub fn advance(&mut self) -> &'tok Token { ... }
#[inline(always)] pub fn eat(&mut self, tt: &TokenType) -> bool { ... }
#[inline(always)] pub fn is_at(&self, tt: &TokenType) -> bool { ... }
#[inline(always)] pub fn is_eof(&self) -> bool { ... }
#[inline(always)] pub fn current_span(&self) -> Span { ... }

// HOT PATH — Parser shared helpers used inside expression loops.
// MUST be #[inline(always)].
#[inline(always)] pub(crate) fn peek(&self) -> &TokenType { ... }
#[inline(always)] pub(crate) fn span(&self) -> Span { ... }
#[inline(always)] pub(crate) fn intern(&self, s: &str) -> &'ast str { ... }
#[inline(always)] pub(crate) fn emit(&mut self, err: ParseError) { ... }
#[inline(always)] pub(crate) fn enter(&mut self, ctx: ParseContext) -> ParseContext { ... }

// WARM PATH — parse methods called inside loops (e.g., param lists, match arms).
// Use #[inline].
#[inline] pub(crate) fn parse_type_expr(&mut self) -> Option<TypeKind<'ast>> { ... }
#[inline] pub(crate) fn parse_pattern(&mut self) -> Option<Pattern<'ast>> { ... }

// COLD PATH — error formatting, recovery, sync. NEVER inline.
// Annotate with #[cold] to move off the hot icache page.
#[cold] pub(crate) fn recover_to_decl(&mut self) { ... }
#[cold] pub(crate) fn recover_to_stmt(&mut self) { ... }
```

**Rule:** If a function can be called more than once per token, it is hot.
When in doubt, profile before adding `#[inline(always)]` to non-cursor code
(over-inlining hurts icache).

### 3.2 Token-Type Dispatch — Always `match`, Never HashMap

`TokenType` does **not** implement `Hash` or `Eq`. You cannot use it as a
HashMap key or a `phf_map!` key without adding those derives — which we have
not done and do not plan to.

Even if you add `Hash`, for an enum dispatch a `match` statement compiles to
a jump table in release mode and is faster than any hash lookup for small
sets.

```rust
// CORRECT — compiler generates a perfect jump table
fn infix_binding_power(tt: &TokenType) -> Option<(u8, u8)> {
    match tt {
        TokenType::Or             => Some((5,  6)),
        TokenType::And            => Some((7,  8)),
        TokenType::EqualEqual
        | TokenType::BangEqual    => Some((9,  9)),   // non-associative
        TokenType::PipeArrow      => Some((11, 12)),  // |> pipe
        TokenType::Pipe           => Some((13, 14)),
        TokenType::Caret          => Some((15, 16)),
        TokenType::Amp            => Some((17, 18)),
        TokenType::LeftShift
        | TokenType::RightShift   => Some((19, 20)),
        TokenType::Plus
        | TokenType::Minus        => Some((21, 22)),
        TokenType::Star
        | TokenType::Slash
        | TokenType::Percent      => Some((23, 24)),
        // Postfix: . () [] ? .?  — handled as their own arms in Pratt loop
        _                         => None,
    }
}

// WRONG — won't compile, and would be slower even if it did
static TABLE: phf::Map<TokenType, (u8, u8)> = phf_map! { ... };
```

### 3.3 `phf::phf_map!` — Only for String Keys

`phf` is correct for maps where keys are `&'static str` (known at compile
time). Two approved uses in the parser:

```rust
// 1. Built-in attribute name → kind (in parse_attr.rs)
use phf::phf_map;

pub static BUILTIN_ATTRS: phf::Map<&'static str, BuiltinAttr> = phf_map! {
    "tier"   => BuiltinAttr::Tier,
    "cfg"    => BuiltinAttr::Cfg,
    "core"   => BuiltinAttr::Core,   // ECS archetype component
    "tag"    => BuiltinAttr::Tag,    // ECS bitmask component
    "doc"    => BuiltinAttr::Doc,
    "system" => BuiltinAttr::System, // ECS system marker
    "inline" => BuiltinAttr::Inline,
    "cold"   => BuiltinAttr::Cold,
};

// 2. cfg key → validator (in parse_attr.rs)
pub static CFG_KEYS: phf::Map<&'static str, CfgKeyKind> = phf_map! {
    "target"   => CfgKeyKind::Target,
    "platform" => CfgKeyKind::Platform,
    "build"    => CfgKeyKind::Build,
    "feature"  => CfgKeyKind::Feature,
    "render"   => CfgKeyKind::Render,
    "editor"   => CfgKeyKind::Editor,
};
```

Do **not** put function pointers or closures in phf maps — the lookup overhead
is not worth it when `match` on `TokenType` is already a jump table.

### 3.4 HashMap / HashSet → `FxHashMap` / `FxHashSet`

Any runtime hash map in the parser that does not need DoS resistance (i.e.,
keys are not user-controlled strings at map-build time) uses `FxHashMap` from
the `rustc-hash` crate. This is a non-cryptographic hash, typically 2–3x
faster than `SipHash`.

```rust
use rustc_hash::{FxHashMap, FxHashSet};

// Memoisation cache (keyed by packed position+rule integer — no DoS concern)
let mut memo: FxHashMap<u64, MemoResult> = FxHashMap::default();
```

Never use `std::collections::HashMap` inside any parse function.

### 3.5 Pre-Allocate All Parse-Time Collections

Before starting any list parse (parameters, arguments, struct members, generic
params, import names, match arms, block statements), allocate with an estimated
capacity. Reallocating mid-list is waste.

```rust
// Estimated capacities — calibrate against real .strat files over time
pub(crate) const CAP_FN_PARAMS:     usize = 4;
pub(crate) const CAP_CALL_ARGS:     usize = 4;
pub(crate) const CAP_STRUCT_FIELDS: usize = 8;
pub(crate) const CAP_BLOCK_STMTS:   usize = 16;
pub(crate) const CAP_MATCH_ARMS:    usize = 8;
pub(crate) const CAP_GENERIC_PARAMS:usize = 2;
pub(crate) const CAP_IMPORT_LIST:   usize = 4;
pub(crate) const CAP_ATTR_ARGS:     usize = 2;

// Usage pattern — always bump-backed vec, never std::vec::Vec for intermediates
let mut params: bumpalo::collections::Vec<FunctionParam<'ast>> =
    self.arena.vec_with_capacity(CAP_FN_PARAMS);
// ... push into params ...
let params: &'ast [FunctionParam<'ast>] = params.into_bump_slice();
```

For types that are `Copy`, use `Vec::with_capacity` and then
`arena.alloc_slice_copy`:

```rust
let mut bounds: Vec<&'ast str> = Vec::with_capacity(CAP_GENERIC_PARAMS);
// ... push ...
let bounds = self.arena.alloc_slice_copy(&bounds);
```

### 3.6 Regex — Cache or Eliminate

The lexer (`logos`) handles all regex. The parser should **never** construct a
`Regex` inside a function call.

If a situation genuinely requires regex in parse time (e.g., re-parsing string
interpolation fragments):

```rust
use once_cell::sync::Lazy;
use regex::Regex;

// Declared once, compiled once.
static INTERP_EXPR_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^\$\{([^}]*)\}").unwrap());
```

Direct alternatives to regex that are faster in parsing contexts:
- Char-by-char scanning with `cursor.advance()` — always preferred
- `memchr::memchr` for finding a single byte (e.g., end of a string segment)
- `memchr::memmem` for a byte subsequence

### 3.7 Lazy Statics

Use `once_cell::sync::Lazy<T>` (already transitively available) for any
heap-allocated static that cannot be `const`. Prefer `const` arrays/slices
for sync sets (they have zero overhead):

```rust
// GOOD — zero cost, lives in rodata
pub(crate) const DECL_SYNC: &[TokenType] = &[
    TokenType::Fn, TokenType::Struct, TokenType::Enum, TokenType::Eof, ...
];

// GOOD — heap-allocated static built once
static SOME_MAP: once_cell::sync::Lazy<FxHashMap<&'static str, u8>> =
    once_cell::sync::Lazy::new(|| { let mut m = FxHashMap::default(); ... m });

// BAD — re-allocated on every parse call
fn parse_something(&mut self) {
    let map: HashMap<_, _> = [("a", 1), ("b", 2)].into_iter().collect(); // NO
}
```

### 3.8 SIMD

SIMD opportunities in the **parser** (not the lexer, which logos handles):

| Use case | Tool | Where |
|---|---|---|
| Fast multi-token-type check (`is_any_of`) | Manual bitmask on discriminant | Pratt loop, sync sets |
| Finding end of byte sequence in source | `memchr::memchr` | String re-parsing |
| Batch identifier comparison | `std::simd` or manual u64 compare | Keyword disambiguation |

For `is_any_of` against a small static set, use a bitmask:

```rust
// Instead of: tt == A || tt == B || tt == C || tt == D
// Use: a const lookup against the enum discriminant

#[inline(always)]
fn is_assign_op(tt: &TokenType) -> bool {
    matches!(tt,
        TokenType::Equal
        | TokenType::PlusEqual  | TokenType::MinusEqual
        | TokenType::StarEqual  | TokenType::SlashEqual
        | TokenType::PercentEqual
        | TokenType::AmpEqual   | TokenType::PipeEqual
        | TokenType::CaretEqual
        | TokenType::LeftShiftEqual | TokenType::RightShiftEqual
    )
}
```

`matches!` on an enum expands to the same jump table as `match`. For sets
larger than 16, benchmark a `u64` bitset approach against a discriminant.

### 3.9 Arena — All AST Nodes

All nodes returned from parse functions are arena-allocated.

```rust
// CORRECT
fn parse_if_expr(&mut self) -> Option<&'ast Expr<'ast>> {
    let node = IfExpr { cond, then_branch, else_branch, span };
    Some(self.arena.alloc(node))  // lives as long as 'ast
}

// WRONG — leaks, breaks the ownership model
fn parse_if_expr(&mut self) -> Option<Box<Expr<'ast>>> { ... } // NO
```

Never `Box::new` inside a parse function. Never return owned `Vec<T>` from a
parse function; always intern to `&'ast [T]` before returning.

---

## 4. Expression Parsing — Pratt / TDOP

### 4.1 Concept

A Pratt parser assigns each operator a **binding power** (BP). The main loop
calls `parse_expr(min_bp)` recursively, consuming infix operators as long as
their left BP exceeds `min_bp`. This naturally handles precedence and
associativity without grammar explosion.

### 4.2 Binding Power Table for Ubel Stratum

All values are `u8`. Left-assoc: `left_bp = right_bp - 1`. Right-assoc:
`left_bp = right_bp`. Non-assoc (chaining forbidden): `left_bp == right_bp`
(caller must emit an error on a second consecutive comparison op).

```
┌──────────────────────────────────────────────────────────────┐
│ Operator group           │ Tokens              │ L   R       │
├──────────────────────────────────────────────────────────────┤
│ Assignment (right)       │ = += -= *= /= etc.  │  1   2      │
│ Range (left)             │ .. ..= ...          │  3   4      │
│ Logical Or               │ or  ||              │  5   6      │
│ Logical And              │ and &&              │  7   8      │
│ Equality (non-assoc)     │ == !=               │  9   9      │
│ Comparison (non-assoc)   │ < > <= >=           │ 11  11      │
│ Pipe operator            │ |>                  │ 13  14      │
│ Bitwise Or               │ |                   │ 15  16      │
│ Bitwise Xor              │ ^                   │ 17  18      │
│ Bitwise And              │ &                   │ 19  20      │
│ Shift                    │ << >>               │ 21  22      │
│ Additive                 │ + -                 │ 23  24      │
│ Multiplicative           │ * / %               │ 25  26      │
│ Unary prefix (right)     │ not ! - ~ await     │  —  27      │
│ Postfix / call           │ . () [] ? .?        │ 29   —      │
│ Generic open (speculative)│ <                  │ handled separately │
└──────────────────────────────────────────────────────────────┘
```

### 4.3 Pratt Loop Skeleton

```rust
// In parsers/parse_expr.rs

pub(crate) fn parse_expr(&mut self, min_bp: u8) -> Option<&'ast Expr<'ast>> {
    let lo = self.span();

    // ── Prefix ────────────────────────────────────────────────────
    let mut lhs = self.parse_prefix()?;

    // ── Infix loop ────────────────────────────────────────────────
    loop {
        let op_tok = self.cursor.peek_token().clone();  // cheap: Token is Clone

        let (l_bp, r_bp) = match infix_binding_power(&op_tok.kind) {
            Some(pair) => pair,
            None       => break,
        };

        if l_bp < min_bp { break; }

        self.cursor.advance();  // consume the operator

        // Postfix operators: no recursive call
        match op_tok.kind {
            TokenType::Dot       => { lhs = self.parse_field_access(lhs, op_tok.span)?; continue; }
            TokenType::LeftParen => { lhs = self.parse_call(lhs, op_tok.span)?; continue; }
            TokenType::LeftBracket => { lhs = self.parse_index(lhs, op_tok.span)?; continue; }
            TokenType::Question  => { lhs = self.parse_try_postfix(lhs, op_tok.span)?; continue; }
            TokenType::QuestionDot => { lhs = self.parse_null_safe_access(lhs, op_tok.span)?; continue; }
            _ => {}
        }

        // Infix: recurse for right-hand side
        let rhs = self.parse_expr(r_bp)?;
        let span = lo.merge(rhs_span);
        lhs = self.alloc(Expr { kind: ExprKind::BinOp { op: op_tok.kind, lhs, rhs }, span });
    }

    Some(lhs)
}

fn parse_prefix(&mut self) -> Option<&'ast Expr<'ast>> {
    let tok = self.cursor.peek_token().clone();
    match tok.kind {
        // Literals
        TokenType::IntLit(_) | TokenType::FloatLit(_) | ... => self.parse_literal(),

        // Identifiers — could be a variable, fn call start, or path
        TokenType::Ident(_) => self.parse_ident_or_path_expr(),

        // Grouping / tuple
        TokenType::LeftParen  => self.parse_paren_or_tuple(),

        // Blocks
        TokenType::LeftBrace  => self.parse_block_expr(),

        // Unary prefix
        TokenType::Minus | TokenType::Bang | TokenType::Not | TokenType::Tilde => {
            self.cursor.advance();
            let operand = self.parse_expr(27)?;
            Some(self.alloc(Expr { kind: ExprKind::UnaryOp { op: tok.kind, operand }, span: ... }))
        }

        // Await
        TokenType::Await => self.parse_await_expr(),

        // If expression
        TokenType::If => self.parse_if_expr(),

        // Match expression
        TokenType::Match => self.parse_match_expr(),

        // Try block
        TokenType::Try => self.parse_try_block(),

        // Unsafe block
        TokenType::Unsafe => self.parse_unsafe_block(),

        // Async block: `async { }` (not async fn — that's a declaration)
        TokenType::Async => self.parse_async_block(),

        _ => {
            self.expected(&["expression"]);
            None
        }
    }
}
```

---

## 5. Disambiguation Strategies

### 5.1 `<` — Less-Than vs Generic Open

This is the classic C++/Rust/Java ambiguity. Strategy: **optimistic speculative
parse with cursor restore**.

```
When Pratt encounters `<` as potential infix after an identifier/path:
  1. Save cursor position
  2. Try parse_generic_args()
     → scan for balanced < > (tracking nesting, skipping string literals)
  3. If balanced AND next token is ( { . :: → it's generic args; commit
  4. Otherwise → restore cursor, treat as less-than comparison (BP 11)
```

This is the one place targeted memoisation helps — cache the result of
`parse_generic_args` at a given cursor position so we don't re-scan if the
same generic expression appears in multiple positions:

```rust
// Key: cursor position (usize) — fits in u64, no hash collision risk
// Value: (Option<GenericArgs>, new_position)
type GenericMemo = FxHashMap<usize, (Option<GenericArgList<'ast>>, usize)>;
```

### 5.2 `from` — Single Meaning Now (Was: LINQ Query vs Import Statement)

`from` used to need position-based disambiguation: statement/top-level
position meant an import (`from mid.ecs summon`), expression position
used to mean a LINQ query start. LINQ is gone (§6), so `from` is back to
having exactly one meaning — it's only ever valid at statement/top-level
position, handled entirely by `parse_import_stmt` in `parse_stmt.rs`.
There's no expression-prefix arm for `TokenType::From` at all anymore;
`from` in expression position is simply a parse error (falls through to
`parse_prefix`'s `_ => { self.expected(&["expression"]); None }`
catch-all, same as any other token with no valid expression-starting
role).

```rust
// In parse_stmt.rs — statement-level, the only place `from` is legal
TokenType::From => self.parse_import_stmt(),     // `from X summon [Y]`
```

### 5.3 `(` — Tuple vs Grouped Expression vs Closure

Two-token lookahead resolves this without backtracking:

```
(                  → start speculative check
  next is )        → empty tuple ()
  next is Ident :  → struct-like field, or closure param with type — speculate
  next is |        → closure: | params | body  (but | is a different token anyway)
  next is expr     → grouped expression, parse normally; if , found → tuple
```

No memoisation needed; `(expr,)` vs `(expr)` is resolved when we see the `,`.

### 5.4 `|` — Pattern Alternative vs Bitwise OR vs Pipe

- In **pattern position** (match arm left side): `|` is an alternative separator
- In **expression position**: `|` is bitwise OR (BP 15)
- `|>` is the pipe operator (BP 13) — the lexer already produces a distinct
  `PipeArrow` token, so no disambiguation needed

The parser knows which position it is in from context. Pattern parsing is
only called from `parse_match_arm` and `parse_let_pattern`, never from the
Pratt loop directly.

### 5.5 `then` — Single-Line if/elif/else Bodies (and an Alt Spelling for `=>`)

An `if`/`elif`/`else` branch body is either a full `{ block }` or the
single-line `then Expr` form:

```
IfBranchBody ::= "{" Stmt* "}" | "then" Expr
```

Unlike `Lambda` and match-arm bodies — both of which pick block-vs-expr
for free by checking whether `{` follows a hard delimiter (`)` for
`Lambda`, `=>` for match arms) — `if`'s condition has **no** hard
delimiter before its body: the condition is parsed by the general Pratt
expression parser, so `if a - 1 { ... }` would be genuinely ambiguous
(`- 1` reads as more condition) without an explicit marker. `then` is
that marker; it is not optional sugar the way the brace-free trick is
elsewhere.

`parse_if_branch_body` (in `parse_stmt.rs`) is the single shared
implementation, called from both `parse_if_stmt` (this file) and
`parse_if_expr` (`parse_expr.rs`) — same relationship
`parse_match_arm_body` already has to its two callers. It shares that
function's carve-out for `return`/`break`/`continue`/`fail` immediately
after `then` (those are statements, not expressions, in this language;
`if x < 0 then return null` parses as `IfBranchBody::Block` wrapping a
single statement, not as `IfBranchBody::Expr`).

Each branch of an `if`/`elif`/`else` chain is parsed independently — a
chain may freely mix `{ block }` and `then expr` branch-to-branch.

Separately, `then` is also accepted as an alternate spelling for `=>`
in match arms (`Some(x) then x`, identical to `Some(x) => x`). This is
purely stylistic — match arms already support brace-free single
expressions via `=>` alone — so it doesn't touch arm-body parsing at
all, only the separator token accepted in `parse_match_arm`.

---

### 5.6 `&`/`ref`, `&mut`/`ref mut`, `*`/`deref` — Dual-Spelled Reference Operators

Same precedent as `and`/`&&`, `or`/`||`, `not`/`!`: two distinct tokens,
unified at every site that matters into one AST node. Not two features —
`&x` and `ref x` produce the literally identical `ExprKind::Borrow`.

```
BorrowExpr ::= ("&" | "ref") "mut"? UnaryExpr
DerefExpr  ::= ("*" | "deref") UnaryExpr
```

Both live in `parse_prefix`'s prefix table at the same binding power as
`-`/`!`/`not`/`~` (28). `Amp` and `Star` keep their existing infix
meanings (bitwise-AND, multiply) — prefix and infix dispatch are
structurally disjoint positions in a Pratt parser, so adding a prefix
arm for a token that's already infix is the same zero-risk move as
unary `-` coexisting with binary `-`. No new ambiguity, no memoisation
needed.

The type-position grammar (`TypeKind::Reference`, `parse_type.rs`) got
the same treatment: `TokenType::Amp | TokenType::Ref` is now one match
arm sharing all the downstream construction code, so `&T`, `&mut T`,
`&L T` and `ref T`, `ref mut T`, `ref L T` all produce the identical
`TypeKind::Reference { mutable, lifetime, inner }` — this half already
existed before `ref` was added; only the alternate spelling is new.

**Real gotcha this surfaced, worth internalising for the next new
prefix operator:** adding a token to `parse_prefix`'s match is not
sufficient by itself. `Parser::can_start_expr` (`parser.rs`) is a
*separate* lookahead heuristic that statement-level constructs —
`return`, `fail`'s `or_else` fallback — use to decide whether a trailing
expression follows at all. It has its own token list, independent of
`parse_prefix`'s. Missing it here meant `return *x` silently parsed as
a bare `return` (void) followed by a dangling `*x` expression
statement — caught immediately by the first real fixture run, not by
inspection. Same failure shape, same lesson, as `where` colliding with
Linqerizer's `.query()`: check every place a new token needs to be
recognised, not just the obvious one.

---

### 5.7 `@attr(...) { item item item }` — Attribute Blocks

Not a new ambiguity to resolve — the `{` after a parsed attribute list
disambiguates cleanly with a single peek, no backtracking or
memoisation needed. Documented here anyway because it's the mirror
image of 5.6: another place where the language accepts a second,
higher-level spelling of something that already existed, this time to
avoid repeating the same attribute on every item in a group.

```
Item     ::= AttributeList Visibility? (FunctionDecl | StructDecl | ...)
           | AttributeList "{" Item* "}"
```

`parse_item_or_block` (`parse_decl.rs`) replaces the old `parse_item`.
Same attrs are parsed either way; if what follows is `{` instead of a
declaration keyword, every item inside gets the block's attrs/tier
applied by `apply_block_attrs` once each inner item has been parsed
normally (recursively — nested blocks fall out of that recursion for
free, no special-cased nesting logic anywhere).

**The merge rule is deliberately coarse-grained per attribute kind, not
per item:**
- `tier`: applied only if the item didn't write its own `@tier(...)` —
  checked by name specifically (`has_own_tier`), not "does this item
  have any attributes at all." A lone `@doc(...)` on one item inside a
  `@tier(low) { }` block must not silently opt it out of the tier — see
  `err_attr_block_unrelated_attr_keeps_tier.ubl`.
- The generic `attributes` list: always appended, regardless of tier
  overrides.

Only `FunctionDecl.tier` and `ImplBlock.tier` exist to override — every
other item kind (`struct`/`enum`/`trait`/`extend`/`const`/`type`) only
has `attributes` to merge, tier is meaningless for them. `ImplBlock`
already had exactly this "one tier for a whole group of methods"
concept before this — its `tier: Option<TierAnnotation>` field is the
direct precedent this generalises from.

Collecting a block's items uses `Vec::split_off`, not in-place mutation
— `parse_item_or_block` doesn't know in advance how many items a
recursive call will produce (could be zero on a recovered error, one
for a plain item, or many for a nested block), so it takes everything
newly pushed, transforms it, and pushes it back rather than trying to
index into a still-growing `Vec` mid-loop.

---

### 5.8 `{` after a bare identifier — Struct Literal vs Block

Found via exploratory testing (nested `while` loops doing a diagonal
grid scan), root-caused and fixed directly against the parser source,
not guessed at:

```
if x == y {
    hit_count = hit_count + 1
}
```

misparsed as `y { hit_count = hit_count + 1 }` — a struct literal named
`y` with one field `hit_count` — because `is_struct_open`'s 2-token
lookahead (`parse_expr.rs`) treats any `{ Ident Equal` opener as a
struct literal, and a block whose first statement happens to be a plain
assignment produces exactly that same 2-token shape. This is not a
lookahead-tuning problem: `{ Ident Equal` is genuinely ambiguous between
`Foo { field = value, ... }` and `Foo` followed by a block starting with
`field = value`; no fixed amount of extra lookahead resolves it in
general, since both parses are well-formed C-family-familiar syntax.

Same restriction Rust reaches for in the same spot. `Parser` carries a
`no_struct_lit: bool` (`parser.rs`), off by default. `enter_no_struct_lit`
turns it on for exactly one expression — the condition/iterable/scrutinee
immediately before an `if`/`elif`/`while`/`for`/`match` body, at both the
statement-position parsers (`parse_stmt.rs`) and the expression-position
ones (`parse_expr.rs`'s `parse_if_expr`/`parse_match_expr`) — and
`leave_no_struct_lit` restores whatever it was before. The struct-literal
postfix check in the Pratt loop (`parse_expr.rs`) just adds `&&
!p.no_struct_lit` to its existing conditions; nothing about
`is_struct_prefix`/`is_struct_open` themselves changed.

**The restriction has to be turned back off inside real brackets, not
just left on for the rest of the expression.** A first pass that set
`no_struct_lit` for the whole condition and only cleared it after
parsing broke the escape hatch — `if (Foo { x = 1 }).ready { ... }`,
parenthesizing a genuine struct literal being exactly how a person
opts back in when they do mean one there — because the flag is one
plain field on `Parser`, not scoped per nesting level, so it stayed on
for everything nested inside the condition, parens included. Caught by
actually running the fixture that exercises the escape hatch, not by
inspection: it failed to parse with an unclosed-`(` error. Fixed with a
second helper, `clear_no_struct_lit`, called at every point the parser
itself consumes an opening bracket with a mandatory matching close
before returning — `(...)` (both grouped and tuple), call arguments,
`[...]` index, and array-literal elements — since once inside a real
bracket pair a following `{` can only mean a struct literal; there is no
competing "this opens the block" reading left to be ambiguous with.
`parse_dict`/`parse_anon_object` (both themselves `{`-delimited, but
triggered as a *primary* expression rather than this postfix case) were
not touched — a dict or anon-object literal used directly as an
unparenthesized condition, itself containing a nested struct literal,
is unlikely enough to not chase down in the same pass; noted here rather
than silently left unconsidered.

**Tests:** `tests/fixtures/ok_condition_struct_lit_ambiguity_isolated.ubl`
(the four positions in isolation) and
`tests/fixtures/ok_condition_struct_lit_ambiguity_combined.ubl` (the
actual diagonal-grid scan that surfaced it, plus the parenthesized
escape hatch). No `err_` pair: the fix removes a miscompile, it doesn't
add a new intentional error path — attempting the ambiguous form now
either parses correctly (the common case) or, unparenthesized and
genuinely intended as a struct literal in a `bool`-typed condition,
still fails type-checking the same way it always would have once
parsed, same as any other type mismatch.

---

## 6. LINQ Query Parsing — Removed

There used to be a dedicated LINQ sub-parser here (`from x in expr where
... select ...`, called from the Pratt prefix arm same as `parse_if_expr`
still is). It's gone — removed outright, not deprecated — as of the
session that decided `Linqerizer<T>` (see `docs/DATASTRUCTURES.md`) would
be the actual query mechanism going forward.

**Why removal, not "keep both":** the old implementation was fully wired
end-to-end — real parser, name resolution, type inference, tier
enforcement (`TierError::LinqInWrongTier`, `TIER-007`, now retired per
`docs/DIAGNOSTICS_RULES.md` §4's "never reassign a retired code" rule),
and a real interpreter routine (`eval_linq`). But it was eager (not
lazy), hardcoded to `Value::List` only (`Dictionary`/`Queue`/`Stack`/
`Pool` sources would have panicked), `group_by` was a literal `// TODO`
no-op, and — the deciding factor — it had **zero fixture coverage**. It
had never actually been run. Keeping it running alongside `Linqerizer<T>`
would mean two different implementations of "iterate, filter, project"
that could silently drift apart from each other over time, which this
codebase has explicitly avoided elsewhere (see `list_methods.rs`'s own
header comment on why `len`/`push`/`pop`/`contains` were consolidated
into one real implementation instead of two hand-written copies).
`Linqerizer<T>` covers everything the old grammar did and more, as an
ordinary value/type with chainable methods rather than special
expression-level grammar — no dedicated sub-parser, no contextual-keyword
handling, no grammar-level ambiguity with plain method calls.

**What's still true of the surrounding grammar:** `from` remains a real
token — it's still how `@IMPORTS`-equivalent `from X summon Y` import
statements start (§5.2's disambiguation notes there are unaffected,
since that was always position-based: `from` at statement/top-level was
never the LINQ path to begin with). `where` remains a real reserved
token too, used for match-arm guards (`pattern where expr => ...`,
§8's `parse_match_arm`) — LINQ's `where` clause reused that same token,
so removing LINQ cost it nothing. `order_by`/`group_by`/`select` were
never real lexer keywords in the first place — always plain `Ident`
tokens the old LINQ sub-parser lexeme-matched — so they needed no
cleanup beyond deleting the `LINQ_KEYWORDS` phf map and
`is_linq_keyword` helper that did that matching (`rd_parser/src/
keywords.rs`).

---

## 7. Targeted Memoisation

Only three rules are memoised. No full packrat.

```rust
/// The three rules worth caching.
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum MemoRule {
    GenericArgs    = 0,   // < T, U > after ident
    TypeExpr       = 1,   // full type expression (used in param lists)
    ClosureParams  = 2,   // | a: T, b: T | before ->
}

/// Key: (start_position << 2) | rule_discriminant
/// All three rules fit in a u64 key with no collision for any realistic file.
pub(crate) struct MemoCache<'ast> {
    map: FxHashMap<u64, MemoEntry<'ast>>,
}

pub(crate) enum MemoEntry<'ast> {
    // Parse succeeded, result ends at this cursor position
    Hit { result_end_pos: usize, node: MemoNode<'ast> },
    // Parse failed at start_position for this rule — don't retry
    Miss,
}
```

The memo cache lives on the `Parser` struct and is allocated once at parse
start. Clear it when entering a new top-level item if memory pressure is a
concern; in practice it is small.

---

## 8. @Attribute Parsing Rules

All `@` attributes are parsed before the declaration they annotate. The
attribute parser (`parse_attr.rs`) runs first and returns a
`Vec<Attribute<'ast>>` which the declaration parsers consume.

```
AttributeList ::= Attribute*
Attribute      ::= "@" Ident AttrArgs?
AttrArgs       ::= "(" AttrArg ("," AttrArg)* ")"
AttrArg        ::= Ident                            (* bare flag: @cfg(debug) *)
               | Ident "=" StringLit               (* key=value: @cfg(target="wasm") *)
               | Ident "(" AttrArg ("," AttrArg)* ")" (* nested: @cfg(not(debug)) *)
```

An attribute list can apply to one following item, or to a whole
`{ }`-delimited group of them — see §5.7 for the block form and its
merge rules.

### Built-in Attribute Dispatch

Use `BUILTIN_ATTRS: phf::Map<&str, BuiltinAttr>` to identify compiler-known
attributes. If name is not in the map → custom/user attribute, stored as-is
for downstream tools (ECS codegen, IDE plugins, etc.).

### `@cfg` Composition

`@cfg` supports boolean composition through nesting:

```ubel
@cfg(debug)                              // bare flag
@cfg(target = "wasm")                   // key=value
@cfg(not(debug))                        // negation
@cfg(any(target = "wasm", target = "native"))  // OR
@cfg(all(build = "debug", platform = "windows")) // AND
```

The `AttrArg::Nested` variant handles `not(...)`, `any(...)`, `all(...)`.
Validation of cfg key names (`target`, `platform`, etc.) happens against
`CFG_KEYS: phf::Map<&str, CfgKeyKind>`.

### ECS Attributes

`@core` and `@tag` on `struct` declarations map to `BuiltinAttr::Core` and
`BuiltinAttr::Tag`. The parser stores them as regular `Attribute` nodes — it
does **not** validate that `@core struct` has no fields marked private or that
`@tag struct` is zero-sized. That is the ECS codegen pass's job.

---

## 9. Error Recovery

Recovery uses panic-mode synchronisation. When a hard error occurs:

1. Emit the error via `self.emit(...)` — **always emit before recovering**
2. Call the appropriate sync function
3. Return `None` from the current parse method

```rust
// Declaration-level sync — find the next item boundary
pub(crate) fn recover_to_decl(&mut self) {
    self.cursor.skip_until_any(Self::DECL_SYNC);
}

// Statement-level sync — find the next ; or }
pub(crate) fn recover_to_stmt(&mut self) {
    self.cursor.skip_until_any(Self::STMT_SYNC);
    self.cursor.eat(&TokenType::Semicolon); // consume the sync token
}
```

Rules:
- **Never silently skip tokens** without emitting a diagnostic first
- **Never panic** in a parse function — return `None`, let the caller decide
  whether to recover or propagate
- After recovery the parser must be in a state where it can continue to produce
  useful diagnostics for the rest of the file — do not stop at first error

---

## 10. Visitor Pattern — Sema Only

The question of "do we need a visitor?" — yes, but not in the parser.

The parser produces the AST and stops. It does **not** walk the AST for
type checking, tier enforcement, or name resolution. Those are semantic passes.

```
Parser  →  AST
               ↓  NameResolutionVisitor   (sema/name_resolution.rs)
               ↓  TypeInferenceVisitor    (sema/type_infer.rs)
               ↓  TierCheckVisitor        (sema/tier_check.rs)
```

A `Visitor` trait on the AST enables each sema pass to be independent:

```rust
// Proposed — goes in crates/core/src/ast/visitor.rs (future)
pub trait AstVisitor<'ast> {
    fn visit_program(&mut self, p: &'ast Program<'ast>)          { walk_program(self, p) }
    fn visit_function_decl(&mut self, f: &'ast FunctionDecl<'ast>) { walk_fn(self, f) }
    fn visit_expr(&mut self, e: &'ast Expr<'ast>)               { walk_expr(self, e) }
    fn visit_stmt(&mut self, s: &'ast Stmt<'ast>)               { walk_stmt(self, s) }
    // ... one method per node type, default impl calls the walker
}
```

The parser does not implement or call any visitor. Adding one to the parser
would couple parsing and semantics, which we specifically avoid.

---

## 11. What Belongs Where

| Question | Answer |
|---|---|
| Does `await` appear outside HIGH tier? | **Parser**: warn; **TierChecker**: hard error |
| Is this a valid type expression? | **Parser**: syntactic validity only; **TypeInfer**: structural validity |
| Is this attribute recognised? | **Parser**: known vs unknown via phf; **ECS codegen**: ECS-specific validation |
| Are these lifetime constraints satisfiable? | **TierChecker / Borrow pass**: not the parser |
| Is this `edge struct` used in the right tier? | **TierChecker**: not the parser |

The parser's job: turn a flat token stream into a tree. No more.

---

## 12. Pre-Submit Checklist

Before merging any change to `crates/rd_parser`:

- [ ] All `Cursor` primitives have `#[inline(always)]`
- [ ] All hot inner loop helpers in `parser.rs` have `#[inline(always)]`
- [ ] No `std::collections::HashMap` anywhere — only `FxHashMap`
- [ ] No `Box::new()` in any parse return path — only `self.arena.alloc(...)`
- [ ] All list-building code starts with `Vec::with_capacity(CAP_*)` or `arena.vec_with_capacity(...)`
- [ ] No regex constructed inside a function — `once_cell::sync::Lazy` only
- [ ] Any new static string dispatch map uses `phf_map!`
- [ ] Error-path functions annotated `#[cold]`
- [ ] Recovery always emits a diagnostic before skipping
- [ ] Memoisation only for the three approved rules (GenericArgs, TypeExpr, ClosureParams)
- [ ] No sema logic in any parse function
- [ ] New token types: update `infix_binding_power` / `prefix` match in `parse_expr.rs`
- [ ] New prefix-position token: also update `Parser::can_start_expr` (`parser.rs`) — it's a separate lookahead list from `parse_prefix`'s, and `return`/`fail`'s fallback both depend on it (see §5.6's gotcha)
