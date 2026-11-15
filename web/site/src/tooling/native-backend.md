# Native backend (preview)

The long-term goal is native machine code through LLVM. Today the
interpreter is the reference implementation and the native backend is a
**preview that compiles a subset of the language**. This page says exactly
what works, because "LLVM backend" would otherwise suggest more than exists.

The numbers below come from `docs/LLVM_BACKEND_STATUS.md` in the
repository, which is the maintained record and is dated; check it for the
current state.

## What it is

- `crates/ubel_llvm` turns Ubel source into **textual LLVM IR**. It has no
  libLLVM dependency: it writes the IR, and `clang` assembles it.
- A small **C runtime** is linked in: a conservative mark-sweep garbage
  collector for HIGH-tier values, arenas for MID, strings with full
  Unicode support, and a collector stress mode for testing.
- Every supported program is checked to give **the same output as the
  interpreter**, at `-O0` and `-O2`, under the sanitizers, in 64-bit and
  32-bit builds.

## Using it

From source to an executable in one command, with a clang 15 or newer on
the `PATH`:

```text
ubel build prog.ubl -o prog
```

`ubel build` checks the file first, with the same diagnostics as
[`ubel check`](./cli.md), then calls the backend. Its options are on the
[command-line page](./cli.md#ubel-build-file).

The backend also has a binary of its own, `ubel-llvm`, which is a member of
the main cargo workspace and is the tool for looking at the IR:

```text
cargo build --release -p ubel_llvm

ubel-llvm emit   prog.ubl -o prog.ll     # textual LLVM IR
ubel-llvm interp prog.ubl                # run in the interpreter, for comparison
ubel-llvm check  prog.ubl                # front end only
ubel-llvm build  prog.ubl -o prog        # a native executable
```

`ubel-llvm build` needs **clang 15 or newer**. Options: `-o <path>`,
`--cc <path>` (otherwise `$UBEL_CC`, then `clang-18` down to `clang-15`, then
`clang`), `-O0 -O1 -O2 -O3 -Os` (default `-O2`), `--keep-temps`, `-v`, and
`-- <flags>` to pass the rest to clang (for example `-m32`). It exits `0` on
success, `1` on any error, `2` on a usage error. `ubel build` is the same
route behind the main command.

## What compiles today

| Area | Works |
|------|-------|
| Scalars and control flow | `int`, `bool`, functions, `if` (statement and expression), `while`, `loop`, `for` over an integer range, `break`/`continue`, recursion, interpolation in `println` |
| Memory tiers | HIGH (the collector), MID (`with arena`, cleanup on every exit, borrowed `&arena` parameters), LOW (no allocation) |
| Structs | plain structs in every tier, `edge struct` with lifetime parameters |
| Enums | payload variants, payload-free variants, `match` on enums and on strings |
| Strings | values, `+`, comparisons, interpolation, `len`, `is_empty`, `contains`, `starts_with`, `ends_with`, `to_upper`, `to_lower`, `trim`, `replace` with Rust's Unicode semantics |
| Closures | capturing and non-capturing, in every tier, under the closure-tier rule |

## What does not compile yet

The compiler refuses, with a message, anything it cannot translate. The
largest groups, in the order they block real programs:

- **Types**: `double` and `float`, `char`, tuples, generics, fallible types,
  `&mut`.
- **Collections**: `List`, `Dictionary`, `Queue`, `InlineList`, `Unique`,
  `Shared`, indexing, and `for` over anything but an integer range.
- **Declarations**: traits, `impl` and `extend`, struct methods, attributes
  such as `@derive`, `static` items, generic functions, `async`.
- **Calls**: other built-in functions (math, `len`, `push`, ...).
- **Patterns**: guards (`where`), nested patterns, struct-style variants.

So the native backend runs a *corner* of the language, and the interpreter
runs all of it. A real standard library and the rest of the type system are
the long road; a one-command executable for a toy program is the short one.
Progress is tracked in [Project Status](../project-status.md).
