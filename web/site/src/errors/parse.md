# Parse errors (PARSE)

The parser checks that tokens form valid constructs. A `PARSE-` error stops compilation before type checking, so you see these first and any type errors only after they are fixed.

Every example below was compiled with the real `ubel` binary, and the output shown is exactly what it printed. Entries marked *no verified example yet* come from the compiler's diagnostics registry (`docs/DIAGNOSTICS_RULES.md`) but could not be reproduced from ordinary source when this page was written; the registry text is given as is.

## Parse errors

### PARSE-001

**UnexpectedToken** — `` unexpected `found` while parsing context, expected: ... ``

The parser met a token that cannot continue the construct it is in. The message names the construct (`statement`, `expression`, ...) and what it expected. When exactly one token would have been valid, a suggestion offers to replace the found token with it.

<!-- ubel-test: error PARSE-001 -->
```ubel
fn main() {
    let x = (1 + )
}
```

```text
>>> error[PARSE-001]: unexpected `)` while parsing statement, expected: expression
  --> 2:18
    |
  2 |     let x = (1 + )
    |                  ^
  ~> help: try replacing `)` with expression
  <~
<<<
```

**Fix:** Fix the token at the position shown. A missing operand (`1 + )`) is the most common cause.

### PARSE-002

**UnexpectedEof** — `unexpected end of file while parsing context, expected: ...`

The file ended in the middle of a construct. A truncated file usually also leaves a block open, which is why [PARSE-003](#parse-003) often appears next to it.

<!-- ubel-test: error PARSE-002,PARSE-003 -->
```ubel
fn main() {
    let x = 1 +
```

```text
>>> error[PARSE-002]: unexpected end of file while parsing statement, expected: expression
  --> 3:1
    |
  3 | 
    | ^
<<<
>>> error[PARSE-003]: unclosed `{` — reached end of file without closing
  --> 3:1
    |
  3 | 
    | ^
  ~> help: add a closing `}`
  <~
<<<
```

The compiler printed 2 diagnostics for this example; the others follow from the first.

**Fix:** Finish the unfinished construct and close any open `{`, `(` or `[`.

### PARSE-003

**UnclosedDelimiter** — `` unclosed `delim` — found `x` instead / reached end of file ``

An opening `(`, `[` or `{` has no matching closer. The message says what was found where the closer should have been, or that the file ended first.

<!-- ubel-test: error PARSE-003 -->
```ubel
fn main() {
    let x = (1 + 2
    println(x)
}
```

```text
>>> error[PARSE-003]: unclosed `(` — reached end of file without closing
  --> 3:5
    |
  3 |     println(x)
    |     ^^^^^^^
  ~> help: add a closing `)`
  <~
<<<
```

**Fix:** Add the matching closing delimiter.

### PARSE-004

**IllegalInContext** — `` `what` is not allowed here: reason ``

A construct is valid syntax but not in this place. Examples the compiler reports: a return type on a lambda; `@tier(...)` written on a `const` (a const is readable from every tier, so the annotation would do nothing) or `@tier(mid)` / `@tier(low)` on a `static`; and `async` on a method declared in a trait or in a trait impl. A `@tier` block around a const or static is not an annotation of its own and is not reported.

<!-- ubel-test: error PARSE-004 -->
```ubel
@tier(low)
static WRONG: int = 1

@tier(high)
static FINE: int = 2

fn main() void {
    println($"{FINE}")
}
```

```text
>>> error[PARSE-004]: ``@tier(low)` on a static` is not allowed here: a static lives in the HIGH tier and can only be used from `@tier(high)` code
  --> 1:1
    |
  1 | @tier(low)
    | ^^^^^^^^^^
  ~> help: remove the annotation or write `@tier(high)`
  <~
<<<
```

<!-- ubel-test: error PARSE-004 -->
```ubel
trait Job {
    async fn go(self) Task<int>
}

fn main() {
}
```

```text
>>> error[PARSE-004]: `async fn` is not allowed here: async methods in a trait are not supported yet
  --> 2:5
    |
  2 |     async fn go(self) Task<int>
    |     ^^^^^
  ~> help: write a plain method that returns a `Task<T>` from a free async function, or wait for async trait methods
  <~
<<<
```

**Fix:** Remove the construct, or move it where it is allowed. The reason in the message says which.

### PARSE-005

**Raw** — `(message passed through verbatim)`

An escape hatch for parser errors that fit none of the other shapes. The message is whatever the parser reported.

*No verified example yet.*

**Fix:** Read the message; there is no fixed suggestion.

### PARSE-006

**NestingTooDeep** — `expression or block nested more than 1000 levels deep`

An expression or block is nested more than 1000 levels deep. A chain such as `a + b + c + ...` counts one level per operator; parentheses, calls and blocks count one level each. No parse error after this one is reported (errors before it are kept), because the unclosed constructs of the over-deep expression make the rest of the file unreliable. Real programs do not reach this limit; it exists so a pathological file cannot overflow the compiler's stack.

*Not reproduced here: reproducing it needs a line of more than a thousand operators.*

**Fix:** Split the expression into several statements, or join pieces with a collection method.
