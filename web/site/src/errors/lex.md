# Lexical errors (LEX)

The lexer turns source text into tokens. A `LEX-` error means the text could not be split into valid tokens, so nothing after it can be parsed reliably.

Every example below was compiled with the real `ubel` binary, and the output shown is exactly what it printed. Entries marked *no verified example yet* come from the compiler's diagnostics registry (`docs/DIAGNOSTICS_RULES.md`) but could not be reproduced from ordinary source when this page was written; the registry text is given as is.

## Lexical errors

### LEX-001

**UnexpectedChar** — `Unexpected character 'c'`

The lexer met a character that cannot begin any token. An unterminated plain string is also reported this way today: the opening quote is the unexpected character, because no closing quote follows.

<!-- ubel-test: error LEX-001 -->
```ubel
fn main() {
    let x = 5 ` 3
}
```

```text
>>> error[LEX-001]: Unexpected character '`'
  --> 2:15
    |
  2 |     let x = 5 ` 3
    |               ^
  ~> help: Remove this character or check for typos
  <~
<<<
```

<!-- ubel-test: error LEX-001 -->
```ubel
fn main() {
    let s = "never closed
    println(s)
}
```

```text
>>> error[LEX-001]: Unexpected character '"'
  --> 2:13
    |
  2 |     let s = "never closed
    |             ^^^^^^^^^^^^^
  ~> help: Remove this character or check for typos
  <~
<<<
```

**Fix:** Remove or replace the character. For a string, add the closing `"`.

### LEX-002

**UnterminatedString** — `Unterminated {kind} string literal`

A string literal reached the end of the file without its closing quote. The kind in the message is `Normal`, `Interpolated`, `Verbatim` or `InterpolatedVerbatim`. Today an unterminated plain `"...` string is reported as [LEX-001](#lex-001) instead, so this code is not what you will see for the simple case.

*No verified example yet.*

**Fix:** Add the missing closing quote.

### LEX-003

**UnterminatedBlockComment** — `Unterminated block comment (nesting level: N)`

A `/* ... */` comment was opened and never closed. Block comments nest, so the level in the message is how many are still open at the end of the file.

<!-- ubel-test: error LEX-003 -->
```ubel
fn main() {
    let x = 1
    /* this comment never ends
}
```

```text
>>> error[LEX-003]: Unterminated block comment (nesting level: 1)
  --> 3:5
    |
  3 |     /* this comment never ends
    |     ^^^^^^^^^^^^^^^^^^^^^^^^^^
  ~> help: Add closing */
  <~
<<<
```

**Fix:** Add the missing `*/`.

### LEX-004

**InvalidNumber** — `Invalid number literal 'text': reason`

A numeric literal is malformed, for example a stray extra `..`, or a `0x` / `0b` prefix with no digits after it.

*No verified example yet.*

**Fix:** Fix the literal. For a prefix, add the digits that belong to it.

### LEX-005

**InvalidEscape** — `Invalid escape sequence 'seq'`

A backslash escape inside a string is not one the language defines. A plain string accepts `\"`, `\\`, `\n`, `\r` and `\t`; an interpolated string also accepts `\{` and `\}`. Today a bad escape such as `"\q"` surfaces as [LEX-001](#lex-001) diagnostics rather than this code.

*No verified example yet.*

**Fix:** Use one of the valid escapes, or write a verbatim string (`@"..."`) where backslashes are ordinary characters.

### LEX-006

**InvalidInterpolation** — `(message from the interpolation parser)`

A hole in an interpolated string `$"...{expr}..."` is malformed, most often a `{` that is never closed. The message and the suggestion come straight from the interpolation parser, so they name the specific problem.

<!-- ubel-test: error LEX-001,LEX-006 -->
```ubel
fn main() void {
    println($"this hole never closes: {missing_close")
}
```

```text
>>> error[LEX-006]: Unclosed interpolation expression
  --> 2:40
    |
  2 |     println($"this hole never closes: {missing_close")
    |                                        ^
  ~> help: Add closing }
  <~
<<<
>>> error[LEX-001]: Unexpected character '"'
  --> 2:51
    |
  2 |     println($"this hole never closes: {missing_close")
    |                                                   ^^^^
  ~> help: Remove this character or check for typos
  <~
<<<
```

The compiler printed 2 diagnostics for this example; the others follow from the first.

**Fix:** Close the hole with `}`. To print a literal brace, write `\{` (and `\}`).

### LEX-007

**InvalidCharLiteral** — `Invalid character literal 'content': reason`

A character literal must contain exactly one character (or one escape). Today `'ab'` is reported as [LEX-001](#lex-001) at the first quote.

*No verified example yet.*

**Fix:** Use a string (`"ab"`) for more than one character.
