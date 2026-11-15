# The `ubel` command line

`ubel` is the command-line front end for `.ubl` files. It has four
commands today. The layout is final; more commands are planned (below).

```text
ubel [OPTIONS] <COMMAND>

Commands:
  check         Lex, parse and type-check a .ubl file without running it
  run           Check a .ubl file, then run its `main`
  build         Check a .ubl file, then build a native executable
                (LLVM backend; needs clang 15 or newer)
  debug-tokens  [DEBUG] Print the token stream with positions

Options:
      --verbose   Print extra detail (token counts, timings)
      --quiet     Suppress success output
      --json      Machine-readable JSON on stdout
      --no-color  Disable ANSI colors
  -h, --help      Print help
  -V, --version   Print version
```

The options are global: they go before the command, as in
`ubel --json check file.ubl`.

## `ubel check <file>`

Lexes, parses and type-checks the file without running it, and prints every
diagnostic. This is the command to use in an editor save hook or in CI.

```text
$ ubel check hello.ubl
✓ hello.ubl is valid
```

When there are problems, each one is printed in the compiler's diagnostic
format and the summary says which stage stopped:

<!-- ubel-test: error NAME-002 -->
```ubel
fn main() {
    let x = 1
    let x = 2
}
```

```text
>>> error[NAME-002]: `x` is already defined in this scope
  --> 3:5
    |
  3 |     let x = 2
    |     ^^^^^^^^^
  ~> note: first defined here
    --> 2:5
      |
    2 |     let x = 1
      |     ^^^^^^^^^
  <~
<<<
✗ 1 diagnostic(s) in dup.ubl (stopped at sema)
```

The stage is `lex`, `parse`, `sema` or `clean`. Compilation stops at the
first stage that reports errors, so a file with a parse error shows no type
errors until the parse error is fixed.

### JSON output

`ubel --json check file.ubl` prints one object, which is what tools should
read instead of scraping the text:

```text
{
  "file": "tests/fixtures/err_move_after_use.ubl",
  "ok": false,
  "stage": "sema",
  "diagnostics": [
    {
      "code": "MOVE-001",
      "message": "use of `a` after it was already moved",
      "line": 10,
      "column": 13,
      "suggestion": "`a` is `Unique<T>` -- move-only, not `Copy`. Borrow it instead ..."
    }
  ],
  "elapsed_ms": 1.744417
}
```

`suggestion` is `null` when the compiler has nothing to add beyond the
message. The codes are stable; see [the error reference](../errors/index.md).

## `ubel run <file>`

Runs the same check, refuses to run a file that fails it, then runs `main`
through the interpreter. A failed check is reported exactly as `check`
reports it. Program output goes to standard output.

<!-- ubel-test: run -->
```ubel
fn main() {
    let words = ["tier", "stratum"]
    for w in words {
        println($"{w}: {w.len()} letters")
    }
}
```

```text
$ ubel run words.ubl
tier: 4 letters
stratum: 7 letters
```

## `ubel build <file>`

Checks the file exactly as `ubel check` does, then builds a **native
executable** through the [LLVM backend](./native-backend.md). It needs a
`clang` version 15 or newer. The backend compiles a *subset* of the language;
what it cannot translate yet is refused with a located `not supported yet`
message instead of being compiled wrongly.

```text
ubel build prog.ubl                 # writes ./prog
ubel build prog.ubl -o out/prog -O3
ubel build prog.ubl -- -m32         # everything after -- goes to clang
```

| Option | Meaning |
|--------|---------|
| `-o, --output <PATH>` | where to write the executable (default: the file name without its extension, in the current directory). It refuses to overwrite the source file. |
| `--cc <PATH>` | the C compiler (default: `$UBEL_CC`, then `clang-18` down to `clang-15`, then `clang`) |
| `-O, --opt-level <0\|1\|2\|3\|s>` | optimisation level passed to clang (default `2`) |
| `--keep-temps` | keep the generated IR and the runtime files, and print where they are |
| `-- <CLANG_ARGS>` | extra arguments for clang |

`--verbose` shows which compiler was used and the full command, and `--quiet`
prints nothing on success. With `--json` the object has `file`, `ok`, `stage`
(`build`, or `emit` when the backend refused the program), `output`,
`compiler`, `command`, `error` and `elapsed_ms`.

## `ubel debug-tokens <file>`

Prints the token stream with `line:column` positions. It is marked
`[DEBUG]` in the help and its format is **not stable**; it exists to
debug the lexer, not to build tooling on.

## Exit codes

| Code | Meaning |
|------|---------|
| `0` | Success |
| `1` | The file has diagnostics, the program failed at run time, or (for `build`) the backend refuses the program or clang fails on it |
| `2` | The command could not be carried out: a missing or unreadable file, a bad option, or (for `build`) no usable clang |

## Planned

In the order the sibling `mdix` CLI suggests: `debug-ast`, `debug-symbols`,
`fmt` (blocked on a formatter existing), `completions`, and `new` (a project
template). What `ubel build` can compile is described on
[Native backend](./native-backend.md).

## Notes for contributors

The whole command runs on a thread with a 256 MB stack, because every pass
walks the tree recursively and the default 8 MB of a main thread was
exhausted by a chain of about 500 operators in a debug build. The design
notes live in `docs/ubel_stratum_cli.md` in the repository.
