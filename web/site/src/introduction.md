# Introduction

**Ubel Stratum** is a systems programming language built around one idea:
*the right memory model for every function.*

Most languages make a single memory bet for the whole program: garbage
collection everywhere, ownership everywhere, or manual management
everywhere. Ubel lets each **function** choose its own, and the compiler
checks that the choices never collide.

| Tier | Annotation | Memory | Typical use |
|------|-----------|--------|-------------|
| HIGH | `@tier(high)` (the default) | Garbage collected | Business logic, I/O, anything where convenience matters |
| MID  | `@tier(mid)` | Arena allocated | Parsers and hot paths that allocate a lot and free all at once |
| LOW  | `@tier(low)` | Ownership and borrow checking, no collector | Systems code, packet handling |

A function with no annotation is HIGH. You opt *down* for performance; you
never have to opt *up* for convenience. [The Tier Model](./tier-model.md)
explains how the three fit together and what the compiler enforces.

<!-- ubel-test: run -->
```ubel
// HIGH by default: no annotation needed
fn main() {
    println($"scratch total: {sum_squares(10)}")
}

// MID: allocates in an arena that is freed when the block ends
@tier(mid)
fn sum_squares(n: int) int {
    let mut total = 0
    with arena(64KB) {
        for i in 1..=n {
            total += i * i
        }
    }
    return total
}
```

## What exists today

Ubel Stratum is early and under active development. This is what you can
use right now:

- **The language front end**: lexer, parser, name resolution, type
  inference with generics and traits, tier enforcement, and a LOW-tier
  borrow, move and lifetime checker.
- **An interpreter** that runs every tier. It is the reference
  implementation of the language's behaviour.
- **The `ubel` command line** to [check, run and build](./tooling/cli.md) files, and
  **`ubel-lsp`**, a [language server](./tooling/lsp.md) that shows compiler
  diagnostics in your editor.
- **A browser [playground](./tooling/playground.md)** that runs the real
  compiler compiled to WebAssembly.
- **A native backend, in preview**: `ubel build` compiles a growing subset of
  the language to LLVM IR and then to an executable. See
  [Native backend](./tooling/native-backend.md) for exactly what works.

What does **not** exist yet: a standard library beyond the built-in
collections and functions, a package manager, and an installable release.
[Project Status](./project-status.md) tracks all of it, including the gaps.

## How this documentation is organised

- **[Getting Started](./getting-started.md)** builds the tools and runs your
  first program.
- **The Guide** teaches the language one topic at a time. Every code
  example is compiled by the project's CI against the real compiler, so an
  example that no longer works fails the build instead of going stale.
- **[The Tier Model](./tier-model.md)** is the part of Ubel that no other
  language has. Read it once the basics are comfortable.
- **[The error reference](./errors/index.md)** lists every diagnostic the
  compiler can print, with a failing example, the exact output and the fix.
  The playground links error codes straight to it.
- **[Versions and releases](./versions.md)** explains the version picker at
  the top of every page. No stable release has been published yet, so you are
  reading the development docs, which track the `master` branch.

> Code blocks can be recoloured: use the palette button in the top bar (or
> **Colours** in the playground) to change the syntax colours, pick a preset,
> or share a palette. It is remembered in your browser.
