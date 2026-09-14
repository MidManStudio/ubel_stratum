# Ubel Stratum — Print & Format-Spec Rules

> Canonical reference for `print`/`println`/`log`, string interpolation,
> and the `{expr:spec}` format-spec syntax inside interpolation holes.
> First slice landed one session; the Debug-vs-Display split landed the
> next; the fill/sign/alternate-form/zero-pad/numeric-base leftovers
> landed the one after that. §5 covers what's still deliberately
> deferred, §6 for the first format-spec delivery's found-along-the-way
> fixes, §7 for the Debug-vs-Display delivery's, and §8 for open
> questions.

---

## 1. What exists

`print`/`println`/`log` are variadic global builtins
(`interpreter/builtins.rs`) that join their arguments with spaces and
write through `Value: Display`. String interpolation
(`$"...{expr}..."`, plus its verbatim form `$@"...{expr}..."`) embeds
expressions directly in a string literal — parsed once, at the same
time as everything else (`rd_parser::parsers::parse_expr::parse_interp`),
not re-parsed later by sema or the interpreter.

`Value: Display` already renders compound types close to Rust's `{:?}`
derive shape — `TypeName { field: value, ... }` for structs, recursing
correctly through nested collections — not just bare primitives.

`Value::debug_string()` is a second, separate formatter (not a `fmt::Debug`
trait impl, see §7) that the `?` flag in a format spec now genuinely
selects. It's real, not reserved, see §7 for exactly what it adds over
`Display` and, just as importantly, what it deliberately doesn't.

## 2. Format-spec syntax

```
{expr}                — unchanged, no spec
{expr:spec}            — spec = [align][width][.precision][?]
align      ::= "<" | ">" | "^"        (left / right / center)
width      ::= digit+
precision  ::= digit+
debug      ::= "?"
```

Examples: `{n:>10}` (right-align, width 10), `{pi:.2}` (2 decimal
places), `{pi:>10.2}` (both together), `{name:^20}` (centered), `{x:?}`
(debug flag, real, see §7).

### How the spec is found — parser-level, not lexer-level

A hole's raw token stream is fully tokenized once
(`string_parser.rs::parse_interpolation_expr`), then `parse_expr` parses
one real expression from it. Whatever the parser doesn't recognize as
part of that expression is leftover; if the leftover starts with `:`,
everything after it is parsed as a format spec
(`parse_expr.rs::parse_format_spec`) — a small, separate grammar, not
Ubel Stratum expression syntax. Any other leftover content is a real
parse error (`PARSE-004`), previously (see §6) it was silently
discarded instead.

This has to happen at the parser level because only the parser actually
knows where an expression ends; the lexer has no notion of expression
structure. **Ubel Stratum has no ternary operator** — `?` is the
fallible-unwrap postfix (same as Rust's `?`), not a conditional — so
there's no `cond ? a : b`-style `:` for a format-spec `:` to collide
with. The leftover-token approach is still the right general mechanism
(it correctly separates "the expression" from "everything else" for any
reason, not just one specific collision), it just doesn't have that
particular ambiguity to resolve in this language.

### The `width.precision` token collision

`{value:10.2}` (width and precision back-to-back, no separator) is
genuinely ambiguous at the *token* level: the general tokenizer has no
notion of format-spec context, so a digit immediately before a `.`
lexes as one `Float`/`DoubleLit` token, the same as it would anywhere
else in the language (confirmed: `{value:.2}` alone is fine, since a
leading `.` with no digit before it never forms a numeric literal).
`parse_format_spec` recovers width/precision from the literal's own
source *text*, not its parsed `f32`/`f64` value — a float's fractional
part doesn't reliably round-trip back to "how many digits were
written" (`10.20` and `10.2` are the same `f64`, different precisions).

## 3. Type checking

`.precision` only applies to `Float`/`Double` (decimal places) or `Str`
(max length, truncating). Sign forcing (`+`) and zero-padding (`0`) only
apply to `Int`/`Float`/`Double`. A numeric base (`x`/`X`/`o`/`b`) only
applies to `Int`. The alternate form (`#`) requires a numeric base also
be present in the same spec. Anything outside these is
`TypeError::InvalidFormatSpec` (`TYPE-115`) for a wrong-type mismatch, or
`TypeError::AlternateFormatWithoutBase` (`TYPE-119`) for `#` with no
base (a spec-internal combination problem, not a type problem).
Width/align/fill/`?` apply to any type, since padding/truncation operate
on the rendered string regardless of what produced it. `?` and a
trailing base are mutually exclusive, checked in the parser rather than
here, since `Int` (the only type a base ever applies to) never diverges
between `Display` and `debug_string` in the first place, so the
combination is never grammatically valid to begin with.

## 4. Shipped this delivery

- **Custom fill character.** `[[fill]align]`, e.g. `{value:*>10}`. Only
  ever recognized when the token right after it is an align marker;
  restricted to tokens whose own lexeme is exactly one character and
  aren't `Ident`/a numeric literal, so a fill character can never
  collide with a numeric base in the trailing position.
- **Sign forcing (`+`).** Forces a `+` on positive `Int`/`Float`/
  `Double` values. A value that's already negative renders its own `-`
  and is untouched.
- **Alternate form (`#`) and zero-padding (`0`).** `#` prefixes a
  numeric-base rendering with `0x`/`0X`/`0o`/`0b`; `0` (distinguished
  from an ordinary width starting past zero by reading the width
  literal's own source text, the same technique the `width.precision`
  collision above already used) pads with `0` between the sign/prefix
  and the digits, not around the outside the way a custom fill
  character does, and ignores `align` entirely once set.
- **Numeric bases** (`x`/`X`/`o`/`b` for hex/hex-uppercase/octal/
  binary). `Int` only. A negative value renders as its 64-bit two's-
  complement bit pattern in the chosen base, matching Rust's own
  `{:x}` on a signed integer, not a `-` sign plus the magnitude's
  digits.

## 5. Still deliberately deferred (not in this slice)

- **A `format!`/positional-args function** separate from interpolation.
  Everything goes through `$"..."` holes; there's no `format(spec,
  arg1, arg2, ...)`-style call.


## 6. What this delivery fixed along the way (not originally in scope)

Two real, pre-existing gaps surfaced while landing format specs — both
fixed here, not deferred, since each was directly blocking verification
of the feature itself:

**Interpolation holes were never type-checked.** `infer_literal`'s
`InterpolatedStr`/`InterpolatedVerbatimStr` arm returned `Str`
immediately without ever calling `infer_expr` on any hole's expression.
A hole referencing an undefined name, or containing any other real type
error, sailed through sema completely undetected — `$"{undefined_var}"`
was accepted. Fixed as a direct consequence of needing each hole's type
anyway, to validate its format spec against it
(`err_interpolation_hole_undefined_name.ubl`).

**Name resolution never visited interpolation holes either — one layer
earlier, and the actual root cause of the above.** `resolve_expr`'s
`ExprKind::Lit(_) => {}` arm matched `InterpolatedStr` too, so nothing
inside a `{expr}` hole was ever registered in the `resolutions` map.
Invisible at runtime — the interpreter resolves identifiers by name
(`interp.lookup`, `eval/expr.rs`), completely independent of sema's
resolution map — which is exactly why nobody had noticed: interpolated
variable references always *worked*, they just were never *checked*.
Surfaced the moment `infer_literal` started calling `infer_expr` on
hole expressions and got `<unknown>` back for a perfectly ordinary
bound variable.

**A second interpolated string anywhere in a file broke the lexer —
a real, previously-documented, known gap** (see
`ok_collections_full.ubl`'s original header comment: *"Only ONE
interpolated string ... a second one anywhere later currently breaks
the lexer (known gap)"*). Root cause: `LogosLexer::handle_logos_token`
rebases its underlying `logos::Lexer` onto a subslice after every
interpolated/verbatim string and every block/doc comment
(`self.logos_lex = LogosToken::lexer(&self.input[pos..])`), which makes
`self.logos_lex.span()` relative to that rebase point, not to the
original source. The code was using that rebase-relative `span_range`
directly as if it were an absolute file offset — both for ordinary
token spans and, critically, as the starting position handed to the
*next* string/comment sub-parser. `self.position`, by contrast, is a
plain running byte counter (`update_position`) that's never reset by a
rebase, so it's the one value that was always trustworthy. Fixed by
computing every absolute position from `self.position` instead of
`span_range` directly. This was a real, general lexer bug — not
specific to format specs, and not specific to interpolation; it would
have affected multiple block/doc comments after a string too. Confirmed
fixed via `ok_multi_interpolation_and_format_spec.ubl` (several
interpolated strings, several holes each) and the full existing fixture
suite re-passing unchanged.

## 7. The Debug-vs-Display split (this delivery)

`Value::debug_string()` (`interpreter/value.rs`, right after `equals()`)
is a plain inherent method, not an `impl fmt::Debug for Value` — `Value`
already carries a `#[derive(Debug, Clone)]` at the Rust-implementation
level (used internally, e.g. `dbg!`/assertion failure output during
`ubel_stratum` development itself), and a second `fmt::Debug` impl for
the same type would conflict with it. Keeping them separate is also the
right call architecturally: the derived one is a Rust-maintainer
concern (raw enum-variant shape, `RefCell`/`Rc` wrappers visible), this
one is an Ubel-language concern (what `{x:?}` shows a person writing
`.ubl` source). `apply_format_spec` picks `debug_string()` over
`to_string()` based on `spec.debug`.

It mirrors `Display`'s exact match structure, recursing through every
variant, and diverges in exactly two places — both chosen because
they're real information available *right now*, nothing fabricated for
the occasion:

1. **`Str`/`Char` are quoted and escaped** (Rust's own `{:?}` on
   `&str`/`char` does the same) — `hello` (Display) vs `"hello"`
   (Debug). Recurses correctly: a `Str` field nested inside a `Struct`,
   `List`, `Dict`, `Tuple`, or `Enum` payload gets quoted too, not just
   a bare top-level string.
2. **`Shared`/`SyncShared` show their live `Rc::strong_count`** — e.g.
   `Shared(refs=2, "hello")`. Genuinely useful systems-debugging info
   Display shouldn't clutter. One real subtlety, worth knowing before
   reading a count and being surprised by it: evaluating the interpolation
   hole itself clones the `Value` being formatted (`eval_interpolated`
   → `eval_expr` → `Interpreter::lookup` → `.cloned()`) before handing it
   to `apply_format_spec`, so the count you see always includes that
   transient print-time clone, not just the aliases the program itself
   holds — a `Shared` with 2 program-visible bindings will show
   `refs=3` while being printed. Confirmed empirically, not assumed
   (`ok_debug_display_split.ubl`'s own comment walks through it).

Everything else — `Null`, `Void`, `Bool`, `Int`, `Float`, `Double`,
`Function`, `Pool`, `Handle`, `Linqerizer`, `Unique` (recurses, but adds
no decoration of its own since single ownership has nothing to
disambiguate) — delegates straight to `Display`. Regression-guarded by
`test_debug_string_equals_display_when_nothing_new_to_show`.

**Explicitly considered and rejected: showing tier/arena info in
`Debug`.** This was the original motivating idea, and it doesn't work —
not "not yet," genuinely doesn't work, for two independent, already-
documented reasons. `MEMORY_MODEL.md` §2 records a deliberate decision
that tier lives in the *reference* (`GcRef`/`ArenaRef`/`OwnedRef`),
never in the type declaration — the same struct type is legitimately
HIGH tier in one place and MID in another depending on the call site,
so there's no single "this struct's tier" to look up even in principle.
And §8 records that the interpreter deliberately shares one
`Rust`-heap-backed `Value` representation across all three tiers today,
with zero per-instance runtime tier tag — real memory-layout divergence
is explicitly deferred to an LLVM-lowering phase that doesn't exist
yet, and building real per-instance tier tracking now, for a print
formatter, would be exactly the "wasted motion" that section warns
against. Recorded here so this doesn't get re-proposed without
re-deriving both reasons from scratch.

**No `@derive` attribute landed with this.** `Display` was already
unconditional for every struct/enum before this delivery (no opt-in
needed), and `Debug` is unconditional too now, for the same reason —
nothing here needs per-type codegen the way Rust's derive does, because
the interpreter already has full structural visibility into every
struct via one uniform `Value::Struct { fields: HashMap<...> }`. An
attribute that would make Debug/Display *harder* to get, with no
behavioral payoff, wasn't worth building. `@derive` still has a real,
different first use case waiting: flipping `Struct`'s `PartialEq` from
today's `Rc::ptr_eq` default to structural comparison on a specific
type — that's an actual behavior change worth gating behind explicit
opt-in, unlike this delivery.

**Found along the way, fixed, not deferred:** two doc comments in
`parse_attr.rs` (`parse_generic_attr_arg`, `parse_cfg_arg`) had
un-tagged ` ``` ` fences containing literal EBNF grammar text, which
rustdoc defaults to treating as compilable Rust and fails on. This was
silently breaking `cargo test --workspace`'s doctest run at the HEAD
this delivery started from — contradicts the prior handover's claim of
an independently-reverified clean baseline, so that verification step
evidently didn't include doctests. Fixed by tagging both fences
` ```text `.

## 8. Open questions for the next slice

- Fill character, sign, `#`, `0`-padding, and numeric bases — real
  Rust-parity would want all of these; §5 has the exact list.
- `@derive(PartialEq)` for structural equality on `Struct` — the
  attribute grammar already supports a bare comma-list of idents inside
  parens with zero parser changes (`@derive(Debug, Display)` parses
  today on the existing `AttrArgs ::= "(" AttrArg ("," AttrArg)* ")"`
  rule); what's not built is the sema flag it would set and the
  `equals()` branch that checks it. Genuinely deferred, not started.
- Whether nested string literals inside a `{}` hole should be
  supported — currently `$"{cond} {"literal"}"`-style holes containing
  their own `"..."` string will confuse the *outer* interpolated
  string's own boundary scan (a separate, real, still-open limitation,
  not touched by this delivery — the outer scanner doesn't track
  whether it's "inside a hole" when looking for the string's own
  closing `"`).
