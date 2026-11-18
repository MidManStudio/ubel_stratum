# Ubel Stratum — Naming Conventions

> **Canonical reference for identifier naming across `.ubl` source.**
> Every example below is checked against `others/ubel_stratum.ebnf` v1.1.0
> line by line, not written from memory of other languages — see §0.1
> for the specific mistakes that made that check necessary.

---

## 0. Scope and ground rules

This document sets **MUST** / **SHOULD** / **MAY** naming conventions for
Ubel Stratum source code. It does not repeat the grammar itself — see
`others/ubel_stratum.ebnf` and `docs/PARSER_RULES.md` for that — it only
governs what you *name* things, and it stays inside syntax the grammar
actually defines.

### 0.1 Why this note exists

A draft of this document previously used syntax that isn't Ubel Stratum
syntax at all. Recording the mistakes here so nobody reintroduces them:

- **`fn f(x: int) -> int { ... }` is not valid.** Ubel has no `->` in a
  function signature anywhere. The grammar is explicit:
  `FunctionDecl ::= ... "(" ParamList? ")" ReturnSpec? Block` — the
  return type sits directly after `)`, no arrow, no separator. Every
  real fixture in `tests/fixtures/` confirms this:
  `fn describe(d: Direction) string { ... }`,
  `fn identity<T>(x: T) T { ... }`. **The correct shape is
  `fn f(x: int) int { ... }`.**
- **`struct Dictionary<K, V> { ... }` is not valid.** `List`, `Dictionary`,
  `Set`, `Queue`, and `Stack` are `CollectionType` keyword tokens in the
  grammar (`CollectionType ::= "List" (...) | "Dictionary" (...) | ...`),
  not ordinary identifiers — you cannot declare a type with any of
  these five exact names. See §12.
- **`from std.text summon String as string_text` is not valid.** The
  `("as" Ident)?` alias clause only exists on the plain `summon
  QualifiedIdent as Ident` form. `from X summon [...]` has no alias
  clause in the grammar at all. See §6.

If you're ever unsure whether a snippet in this document (or anywhere
else) is real Ubel syntax, check it against the EBNF directly — don't
extrapolate from Rust, C#, or Go, even though Ubel borrows from all
three. The three items above are exactly the shape of mistake that
extrapolation produces.

### 0.2 Where these conventions come from

Not invented for this document — read directly off `others/ubel_stratum.ebnf`
and cross-checked against every `.ubl` fixture in `tests/fixtures/`:

| Convention | Confirmed by |
|---|---|
| Types (`struct`/`enum`/`trait`) → `PascalCase` | `Direction`, `Box<T>`, `Rectangle`, `Holder` across every fixture |
| Enum variants → `PascalCase` | `Direction { North, South, East, West }` in `ok_enum_variants.ubl` |
| Functions/methods → `snake_case` | `describe`, `identity`, `make_list`, `sum_first_n`, `parse_program` |
| Variables/params → `snake_case` | `outer`, `scratch`, `boxed`, `dir`, `desc` |
| Struct fields → `snake_case` | `width`, `height`, `data`, `value` |
| Constants → `SCREAMING_SNAKE_CASE` | Standard across the C-family languages Ubel draws from (Rust, C#, Go); no `const` example currently in `tests/fixtures/` — flagged as convention-by-extension, not yet fixture-confirmed. See §5. |
| Generic params → single uppercase letter by default | `Box<T>`, `identity<T>` |

---

## 1. Quick reference

| Kind | Convention | Example |
|---|---|---|
| `struct` / `enum` / `trait` / `type` alias | `PascalCase` | `Rectangle`, `Direction`, `Printable`, `UserId` |
| Enum variant | `PascalCase` | `North`, `Ok`, `NotFound` |
| Function / method name | `snake_case` | `parse_program`, `to_string` |
| Variable / parameter / binding | `snake_case` | `user_id`, `current_token` |
| Struct field | `snake_case` | `width`, `first_defined` |
| Constant (`const`) | `SCREAMING_SNAKE_CASE` | `MAX_RECURSION_DEPTH` |
| Package (`package` decl) | `snake_case`, dotted | `compiler.frontend` |
| Generic parameter | Single uppercase letter, or short `PascalCase` word for clarity | `T`, `K`, `V`, `Key`, `Value` |
| Lifetime | Short lowercase word, no `PascalCase`, no Rust `'a` | `[lifetime L]`, `[lifetime source]` |
| Trait bound | Same as the trait it names | `T: Comparable` |
| `@tier(...)` value | Lowercase, exactly `high` / `mid` / `low` | `@tier(mid)` |

---

## 2. Types — `struct`, `enum`, `trait`, `type` alias

### MUST

Use `PascalCase` for every type-level declaration:

```ubel
struct Rectangle {
    width: int
    height: int
}

enum Direction {
    North,
    South,
    East,
    West
}

trait Printable {
    fn to_string() string
}

type UserId = uint
```

### SHOULD

- Name a type after what it *is*, not how it's currently used —
  `Rectangle`, not `WidthHeightPair`.
- Give an arena-scoped or otherwise borrowed type a name that signals
  the distinction where it matters. There's no dedicated suffix
  enforced by the grammar (no `ArenaRef<T>` surface syntax exists —
  see `docs/MEMORY_MODEL.md` §5), so the naming has to carry that
  information: prefer `View` for a type that only makes sense borrowed,
  `Owned` for a persistent GC-managed counterpart when both exist side
  by side.

```ubel
struct TokenView {
    tokens: List<int>
}

struct OwnedAst {
    root: int
}
```

### MUST NOT

- **MUST NOT name a type `List`, `Dictionary`, `Set`, `Queue`, or
  `Stack`.** These are `CollectionType` keyword tokens (§12) — the
  parser doesn't treat them as ordinary identifiers, so a type
  declared with one of these names either fails to parse or shadows
  the builtin in a way that confuses every future reader.

---

## 3. Functions and methods

### MUST

Use `snake_case`. Return type sits directly after the parameter list —
**no `->`, ever** (see §0.1):

```ubel
fn parse_program(src: string) ParseResult {
    ...
}

struct Rectangle {
    width: int
    height: int

    pub fn area(self) int {
        return self.width * self.height
    }
}
```

### SHOULD

- Name a function after the action it performs. Constructors read as
  verbs: `new`, `from`, `parse`, `build`, `make`. Builtin collection
  constructors already follow a specific, settled shape worth matching
  for any user-defined type that offers the same kind of API: a
  zero-arg `Type.new()` plus chainable configuration
  (`.with_capacity()`, `.growable()`, etc.) rather than a constructor
  that takes every option as a positional argument.
- A `@tier(mid)`/`@tier(low)` function's name doesn't need to encode
  the tier — the annotation already says so, right above the
  signature, and repeating it in the name (`parse_program_mid`) is
  noise the annotation already provides.

```ubel
@tier(mid)
fn tokenize_input(src: string) TokenView {
    with arena(64KB) {
        ...
    }
}
```

### MUST NOT

- MUST NOT use `PascalCase` or `camelCase` for a function name.
- MUST NOT put `-` in a function name — `Ident ::= (Letter | "_")
  (Letter | Digit | "_")*` has no hyphen production at all, so this
  isn't just a style violation, it's a parse error.

---

## 4. Variables, parameters, and bindings

### MUST

Use `snake_case` for every `let`, function parameter, `for`-loop
binding, and destructured name:

```ubel
let user_id = 42
let mut current_token = next_token()

fn eval_expr(expr: int, env: int) int {
    return expr
}

for item in items {
    println(item)
}
```

### SHOULD

- Name a boolean so it reads like a yes/no question: `is_ready`,
  `has_value`, `can_escape` — not `ready_flag` or `value_status`.
- Give a `with arena(...)`-scoped local a name that doesn't imply it
  outlives the block it's declared in — see
  `docs/DIAGNOSTICS_RULES.md`'s worked escape-boundary examples for
  what happens when that assumption is wrong at the type level, not
  just the naming level.

### MAY

- Use a short name (`i`, `j`, `n`) in a small, obvious loop scope. Don't
  extend that allowance to anything that lives more than a few lines.

---

## 5. Constants

### MUST

Use `SCREAMING_SNAKE_CASE`:

```ubel
const MAX_RECURSION_DEPTH: uint = 1024
const DEFAULT_ARENA_SIZE: usize = 1048576
```

*(No `.ubl` fixture currently declares a `const` — this convention is
carried over from the languages Ubel draws from, not yet confirmed
against a real fixture. Worth adding an `ok_const_decl.ubl` fixture
alongside whatever code first needs one, so this row of §0.2's table
can move from "convention-by-extension" to "fixture-confirmed.")*

---

## 6. Packages and imports

### MUST

- Package names MUST use `snake_case`, dotted for nesting:

```ubel
package compiler.frontend
```

- An import alias MUST use the plain `summon X as Y` form — `from X
  summon [...]` has no alias clause in the grammar (§0.1):

```ubel
summon std.collections.List
summon std.net.http.Client as HttpClient
from std.collections summon [List, Dictionary]
```

- The alias itself follows the casing of whatever it's aliasing — a
  type alias is `PascalCase` (`HttpClient` above), a function or
  value alias would be `snake_case`.

### SHOULD

- Reach for an alias only to resolve a genuine name collision or to
  shorten a long qualified path used repeatedly — not by default on
  every import.

---

## 7. Lifetimes

### MUST

Use the grammar's own bracketed form — **not** Rust's `'a`:

```ubel
struct TokenView [lifetime L] {
    tokens: List<int>
}
```

Lifetime names MUST be plain lowercase identifiers (`LifetimeLabel ::=
Ident` — no special sigil, no capitalization requirement, but treat
them as their own short, lowercase namespace, distinct from type
names).

### SHOULD

- Use `L` when exactly one lifetime is in scope and its meaning is
  obvious.
- Use a descriptive lowercase name (`source`, `output`, `outer`) once
  more than one lifetime appears on the same declaration — `L`/`M`/`N`
  stops being readable fast. Avoid `arena` specifically even though it
  reads naturally here — it's a reserved allocator keyword token
  (§12), not a free identifier, so it can't actually be used as a
  lifetime name regardless of how well it fits semantically.

```ubel
struct MultiView [lifetime source, lifetime output] {
    input: List<int>
}
```

### MUST NOT

- MUST NOT write a lifetime as `'a`, `'source`, or any other
  tick-prefixed form. This isn't just a style preference — avoiding
  Rust's tick-prefixed lifetimes is a deliberate, documented design
  choice for this language (not something this document is
  speculating about). One plausible reason worth knowing even though
  it isn't confirmed anywhere in the project's own notes: Ubel's
  `CharLit` also starts with `'` (`CharLit ::= "'" (...) "'"`), so a
  tick-prefixed lifetime would force the lexer to disambiguate `'a`
  (lifetime) from the start of `'a'` (char literal) — the bracketed
  `[lifetime L]` form sidesteps that question entirely rather than
  solving it with lookahead.

---

## 8. Generic parameters

### MUST

Declare inside `<...>` as plain identifiers, optionally trait-bounded:

```ubel
struct Box<T> {
    value: T
}

fn identity<T>(x: T) T {
    return x
}

fn largest<T: Comparable>(items: List<T>) T {
    ...
}
```

### SHOULD

- Default to single uppercase letters for simple, obvious generics:
  `T` for "the type," `K`/`V` for a key/value pair, `R` for a distinct
  return type.
- Switch to a short `PascalCase` word once a signature has enough type
  parameters that single letters stop being self-explanatory, or once
  a specific parameter's role needs to be named to avoid confusion
  with another type parameter on the same declaration:

```ubel
struct Pair<Key, Value> {
    key: Key
    value: Value
}
```

- **Never name a generic parameter `List`, `Dictionary`, `Set`,
  `Queue`, or `Stack`** for the same reason a type can't be named one
  of those — they're keyword tokens, not identifiers, regardless of
  where in the grammar they'd appear.

---

## 9. Struct fields

### MUST

Use `snake_case`, matching every other binding-like name in the
language:

```ubel
struct Holder {
    data: List<int>
}

struct TokenView [lifetime L] {
    tokens: List<int>
    source: string
}
```

### SHOULD

- Name a field after the data it holds, not the mechanism that
  produced it.
- If a field is only ever populated inside a specific arena and
  reading it outside that arena's lifetime would be meaningless, that
  constraint currently lives in documentation and the type checker
  (`docs/DIAGNOSTICS_RULES.md` TYPE-204, `docs/MEMORY_MODEL.md` §6),
  not in the field's name — but a name like `scratch_data` over plain
  `data` can still be a useful, human-readable hint alongside the
  compiler's own enforcement.

---

## 10. Enum variants

### MUST

Use `PascalCase` — confirmed directly against `ok_enum_variants.ubl`:

```ubel
enum Direction {
    North,
    South,
    East,
    West
}
```

A variant carrying a payload or fields follows the same rule; only the
variant name is `PascalCase`, its own fields (if any) are `snake_case`
same as any other field:

```ubel
enum Shape {
    Circle(int),
    Rectangle { width: int, height: int }
}
```

### SHOULD

- Keep variant names short nouns or adjectives, consistent in
  register across one enum — don't mix `Ok` / `HasFailed` in the same
  enum; pick `Ok` / `Failed` or `Success` / `Failure`.

---

## 11. Traits

### MUST

`PascalCase` for the trait itself, `snake_case` for every method
signature inside it — same rule as everywhere else, traits don't get
an exception:

```ubel
trait Printable {
    fn to_string() string
}

trait Iterable<T> {
    fn next_item() T
}
```

### SHOULD

- Prefer a capability-shaped name (`Printable`, `Comparable`,
  `Iterable`) over a vague noun (`Helper`, `Util`).

---

## 12. Reserved names — must not shadow

These exact identifiers are grammar-level keyword tokens, not ordinary
identifiers, regardless of what kind of thing you're trying to name
(type, generic parameter, variable, or otherwise):

| Reserved | Why |
|---|---|
| `List`, `Dictionary`, `Set`, `Queue`, `Stack` | `CollectionType` keyword tokens — `CollectionType ::= "List" (...) \| "Dictionary" (...) \| "Set" (...) \| "Queue" (...) \| "Stack" (...)` |
| `Task` | `TaskType` keyword token, used for async return types |
| `self` | Reserved receiver-parameter keyword, never a plain binding name |
| `high`, `mid`, `low` (lowercase, inside `@tier(...)`) | `TierValue` — the *only* three legal values, and case-sensitive |
| `arena`, `pool`, `gc`, `heap` (lowercase, inside `with ...`) | `AllocatorExpr` keyword tokens |
| `edge` (as a struct modifier) | LOW-tier manual-heap-node marker — `StructDecl ::= "pub"? "edge"? "struct" ...` |
| `getter`, `setter` | Reserved for a not-yet-implemented property-accessor feature. **`get`/`set` themselves are deliberately *not* reserved** — they're ordinary identifiers, used as the read/write method names on collections (`Dictionary.get(key)`/`.set(key, value)`, `Pool.get(handle)`, `List.get(i)`/`.set(i, v)`). Renamed from `get`/`set` specifically so collections could have the names everyone expects; if you're about to reserve a bare word for a future feature, check whether something more common already wants it first — that's exactly how the previous version of this row went unused for an entire feature's lifetime. |
| `ref`, `deref` | Alternate spellings of `&`/`&mut` and `*`, in both type and expression position — same precedent as `and`/`&&`, `or`/`||`, `not`/`!`. Checked against both keyword tables before reserving (see the `getter`/`setter` row above), free at the time. `ref mut`/`&mut` is two tokens (`ref` + the already-reserved `mut`), not a single combined keyword. |

A capitalized form of one of the lowercase allocator/tier keywords
(`Arena`, `Pool`, `Gc`, `Heap`, `Tier`) is lexically a *different*,
unreserved identifier — Ubel identifiers are case-sensitive, so
`struct Arena { ... }` (capital A) doesn't collide with the `arena`
keyword used in `with arena(64KB) { ... }` (lowercase a). That's a
legal escape hatch, not a loophole to lean on by default: naming a
type `Arena` when the language already has a strong, unrelated meaning
for the lowercase word in the same file is still worth a second
thought, even though the parser won't stop you.

---

## 13. `@tier(...)` and `edge`

### MUST

- The tier value MUST be exactly `high`, `mid`, or `low`, lowercase —
  `@tier(HIGH)` and `@tier(High)` are both illegal, not just
  unconventional (`TierValue ::= "high" | "mid" | "low"`, no other
  casing in the production at all).
- `@tier(...)` MUST appear before `pub`/`async`/`fn` on its own line —
  `TierAttr? Attributes? "pub"? "async"? "fn"` is the fixed order:

```ubel
@tier(mid)
pub fn tokenize_input(src: string) TokenView {
    ...
}
```

- `edge` (for a LOW-tier manually-managed struct) MUST appear after
  `pub` and before `struct` — `"pub"? "edge"? "struct"`:

```ubel
pub edge struct Node {
    value: int
}
```

### SHOULD

- Leave a HIGH-tier function unannotated — `@tier(high)` is the
  default and writing it explicitly on every ordinary function is
  noise, not clarity. Reserve the explicit annotation for the rare
  case where contrasting it with nearby `@tier(mid)`/`@tier(low)` code
  genuinely helps a reader.

---

## 14. Worked example — good vs. bad

### Good

```ubel
package compiler.frontend

summon std.collections.List

const MAX_TOKENS: uint = 65536

struct TokenView [lifetime L] {
    tokens: List<int>
    source: string
}

@tier(mid)
fn tokenize_input(src: string) TokenView {
    with arena(64KB) {
        let tokens = [1, 2, 3]
        return TokenView { tokens = tokens, source = src }
    }
}

enum Direction {
    North,
    South,
    East,
    West
}

fn describe(d: Direction) string {
    match d {
        North => return "heading north",
        South => return "heading south",
        East  => return "heading east",
        West  => return "heading west",
    }
}
```

### Bad

```ubel
package CompilerFrontend                 // MUST be snake_case

const maxTokens: uint = 65536            // MUST be SCREAMING_SNAKE_CASE

struct token_view [lifetime L] {          // MUST be PascalCase
    Tokens: List<int>                     // MUST be snake_case
}

@tier(MID)                                // MUST be lowercase — illegal, not just unconventional
fn TokenizeInput(src: string) -> token_view {   // MUST be snake_case; -> is not valid Ubel syntax at all
    ...
}

enum direction {                          // MUST be PascalCase
    north, south, east, west              // MUST be PascalCase
}
```

---

## 15. Checklist

- [ ] Types, traits, type aliases, enum variants → `PascalCase`
- [ ] Functions, methods, variables, parameters, fields, packages → `snake_case`
- [ ] Constants → `SCREAMING_SNAKE_CASE`
- [ ] `@tier(...)` value is lowercase `high`/`mid`/`low`, placed before `pub`/`async`/`fn`
- [ ] `edge` (if used) placed after `pub`, before `struct`
- [ ] No `->` anywhere in a function/method signature — return type sits directly after `)`
- [ ] No `'a`-style lifetimes — `[lifetime L]` only
- [ ] No user type, generic parameter, or field named `List`, `Dictionary`, `Set`, `Queue`, `Stack`, or `Task`
- [ ] Import alias only on the plain `summon X as Y` form, never on `from X summon [...]`
