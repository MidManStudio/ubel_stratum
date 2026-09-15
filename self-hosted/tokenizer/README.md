# Ubel Stratum Self-Hosted Tokenizer (`tokenizer.ubl`)

The foundational lexical analyzer for the self-hosted Ubel Stratum compiler pipeline. It converts raw byte streams (ASCII integer buffers) into typed lexical token streams.

## API Specification

### `tokenize(raw_ascii: List<int>) -> List<Token>`
Accepts a list of integer ASCII codes and produces a token vector ended by an `Eof` token variant.

## Token Grammar & Lexeme Mapping

| Lexeme / Sequence | `TokenKind` | Output Payload (`str_val` / `num_val`) |
| :--- | :--- | :--- |
| `{` / `}` | `LBrace` / `RBrace` | `"{"` or `"}"` |
| `[` / `]` | `LBracket` / `RBracket` | `"["` or `"]"` |
| `:` / `,` | `Colon` / `Comma` | `":"` or `","` |
| `"..."` | `StringLit` | Extracted sequence string |
| Numeric (`123`, `-45`) | `NumberLit` | Double-precision numeric value |
| Non-matching | `Invalid` | Diagnostic error string |

## Lexer Compiler Invariants & Syntax Rules

* **Explicit Primitive Casting:** Integer ASCII conversions to numeric floats require explicit `as double` casting to pass type validation.
* **Struct Literal Disambiguation:** When allocating tokens inside conditional branches or array pushes, always bind the struct instantiation to a local variable (`let t = Token { ... }`) to prevent parser ambiguity between blocks and struct initializers.
* **Condition Expression Boundaries:** Conditional logic containing identifiers must use parenthesis wrappers or explicit comparison operators `(condition == true)` to prevent the parser from mistaking condition identifiers for struct names.
