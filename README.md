<p align="center">
  <img src="web/shared/ubel-logo.png" alt="Ubel Stratum" width="140">
</p>

<h1 align="center">Ubel Stratum</h1>

<p align="center"><strong>Multi-Tier Systems Language</strong></p>

[![License](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)
[![Status](https://img.shields.io/badge/status-early%20development-orange.svg)](#status)
[![Built With](https://img.shields.io/badge/built%20with-Rust-orange.svg)](https://www.rust-lang.org/)
[![Extension](https://img.shields.io/badge/source%20files-.ubl-blueviolet.svg)]()

> **"The right memory model for every function."**

Most languages make one memory bet for the whole program: garbage collection
everywhere, ownership everywhere, or manual management everywhere. Ubel
Stratum lets each **function** choose, and the compiler checks that the
choices never collide.

| Tier | Annotation | Memory | `await` | Use case |
|------|-----------|--------|---------|----------|
| HIGH | `@tier(high)` (default) | Garbage collected | yes | Business logic, I/O |
| MID  | `@tier(mid)` | Arena allocated | no | Parsers, hot paths |
| LOW  | `@tier(low)` | Ownership + borrow checker, no collector | no | Systems code, FFI, packets |

A function with no annotation is HIGH. You opt *down* for performance; you
never opt *up* for convenience.

---

## Status

Early development. What exists today:

| Piece | State |
|-------|-------|
| Front end: lexer, parser, name resolution, type inference, generics, traits, tier enforcement | **Working** |
| LOW-tier safety: borrow, move and lifetime checking | **Working**, intra-function, with documented scope limits ([`docs/OUTLIVES_RULES.md`](docs/OUTLIVES_RULES.md), [`docs/MEMORY_MODEL.md`](docs/MEMORY_MODEL.md) §9) |
| Tree-walking interpreter | **Working**; the reference for the language's behaviour |
| `ubel` command line (`check`, `run`, `build`) | **Working** ([`docs/ubel_stratum_cli.md`](docs/ubel_stratum_cli.md)) |
| `ubel-lsp` language server | **Stub**: publishes diagnostics only ([`docs/ubel_stratum_lsp.md`](docs/ubel_stratum_lsp.md)) |
| Browser playground and docs site | **Working** (`web/`, see [`docs/SITE_DOCS.md`](docs/SITE_DOCS.md)) |
| Native code through LLVM | **Preview**: `ubel build` compiles a growing subset of the language to an executable ([`docs/LLVM_BACKEND_STATUS.md`](docs/LLVM_BACKEND_STATUS.md)) |
| Standard library, package manager, releases | **Not started** |

**The end goal is native machine code via LLVM.** The interpreter is a
deliberate stepping stone: building it first proves out the language's
semantics before taking on code generation. The native backend (`ubel-llvm`,
in `crates/ubel_llvm`) emits LLVM IR for a subset (integers and booleans,
strings, structs, enums and `match`, closures, arenas and the collector) and
checks every program against the interpreter. `ubel build prog.ubl` runs it
end to end (clang 15+). Collections, floats, generics and traits are not
there yet.

No stable version has been released, so the website's docs are the
*development* docs and track `master`; the site is versioned and will show
release docs as soon as one is tagged.

---

## A look at the language

Names use `.` everywhere (no `::`), source files use the `.ubl` extension.

```ubl
@tier(mid)
fn sum_squares(n: int) int {
    let mut total = 0
    with arena(64KB) {          // MID tier: scratch memory freed at the block's end
        for i in 1..=n {
            total += i * i
        }
    }
    return total
}

struct Rectangle {
    width: int
    height: int

    pub fn new(w: int, h: int) Rectangle {
        return Rectangle { width = w, height = h }
    }

    pub fn area(self) int {
        return self.width * self.height
    }
}

enum Shape {
    Circle(int),
    Square(int),
    Nothing
}

fn describe(s: Shape) string {
    match s {
        Circle(r) => return $"circle, radius {r}",
        Square(side) => return $"square, side {side}",
        Nothing => return "no shape",
    }
}

fn main() {
    println($"sum of squares: {sum_squares(10)}")
    println($"area = {Rectangle.new(3, 4).area()}")
    println(describe(Shape.Circle(2)))
}
```

More, with every example compiled by CI against the real compiler: the
[tour](web/site/src/language-tour.md) and the [guide](web/site/src/guide/).

### Collections and queries

```ubl
fn main() {
    let scores = Dictionary.new()       // get/set, not insert
    scores.set("ada", 100)
    println(scores.get("ada"))

    let nums = List.new()
    for n in [1, 2, 3, 4, 5, 6] {
        nums.push(n)
    }
    let evens = nums.query().where(fn(x) x % 2 == 0).select(fn(x) x * 10).to_list()
    println(evens.len())
}
```

`.query()` gives a lazy `Linqerizer<T>` pipeline (HIGH tier only).

### Failure and absence

`T?` may be `null`; `T!` may fail; `?` passes a failure to the caller.

```ubl
fn parse_positive(n: int) int! {
    if n < 0 {
        fail "negative"
    }
    return n
}

fn double_it(n: int) int! {
    let v = parse_positive(n)?
    return v * 2
}
```

### Traits and generics

```ubl
trait Shape {
    fn area(self) int
}

struct Sq {
    s: int
}

impl Shape for Sq {
    fn area(self) int { return self.s * self.s }
}

fn total_area<T: Shape>(a: T, b: T) int {
    return a.area() + b.area()
}
```

### Errors have stable codes

```text
>>> error[TIER-005]: `@tier(low)` code cannot call `@tier(high)` function `helper`
  --> 7:12
```

Every diagnostic has a code that never changes meaning. All of them are
documented with a failing program, the exact output and the fix in
[`web/site/src/errors/`](web/site/src/errors/index.md), and the playground
links each code there.

---

## The tier model, in one page

The point of the tiers is the **boundary** between them. A MID function
allocates in an arena; when the arena is freed every pointer into it dies, so
HIGH code must never keep one. The compiler enforces this statically, with a
real call matrix and escape checking:

| Caller | Callee | Allowed? |
|--------|--------|----------|
| HIGH | MID | yes |
| HIGH | LOW | yes |
| MID | HIGH | no: an arena lifetime could escape |
| MID | LOW | yes |
| LOW | HIGH | no |
| LOW | MID | no |

Arena and pool references cannot leave their block, `with arena` is MID-only,
`await` is HIGH-only, and collections cannot be built inside LOW (LOW's memory
model is the ownership wrappers `Unique<T>`, `Shared<T>`, `SyncShared<T>` and
the borrow checker). The full model, the three patterns for crossing MID to
HIGH safely (callback, iterator, view), and what is and is not enforced yet:
[`web/site/src/tier-model.md`](web/site/src/tier-model.md) and
[`docs/MEMORY_MODEL.md`](docs/MEMORY_MODEL.md).

---

## Building

### Prerequisites

Current stable Rust works, and is what CI uses. The project is also kept
compatible with **Rust 1.75**, because it is developed on older hardware and
free CI runners. On 1.75:

1. the checked-in `Cargo.lock` is lockfile format 4, which Cargo 1.75 cannot
   read: delete it and run `cargo generate-lockfile`;
2. pin the dependencies whose newer releases need a newer compiler:

```bash
cargo update -p owo-colors  --precise 4.0.0
cargo update -p backtrace   --precise 0.3.69
cargo update -p proptest    --precise 1.4.0
cargo update -p tempfile    --precise 3.14.0
cargo update -p clap        --precise 4.4.18
cargo update -p rayon       --precise 1.10.0
cargo update -p rayon-core  --precise 1.12.1
cargo update -p half        --precise 2.4.1
```

LALRPOP is **not** required: `crates/parser` is an inactive reference
implementation (the live parser is the hand-written recursive-descent + Pratt
parser in `crates/rd_parser`, see [`docs/PARSER_RULES.md`](docs/PARSER_RULES.md)).

### Build and use

```bash
cargo build -p ubel_stratum_cli            # the `ubel` command
cargo build -p ubel_stratum_lsp            # the `ubel-lsp` language server

ubel check hello.ubl                       # lex, parse, type-check
ubel run   hello.ubl                       # check, then run `main`
ubel build hello.ubl -o hello              # native executable (subset; needs clang 15+)
ubel --json check hello.ubl                # machine-readable diagnostics
```

A bare `cargo build` builds only `core` and `rd_parser` (the workspace's
default members); the CLI, the LSP and the LLVM backend are workspace members
built explicitly.

```bash
cargo build --workspace --all-targets
cargo test  --workspace --lib --bins
cargo bench                                # core and rd_parser have benches/
```

### The native backend and the playground

The LLVM backend is a workspace member (`ubel build` calls it); the playground
is its own cargo workspace, with its own profile and target:

```bash
# native: needs clang 15+
ubel build prog.ubl -o prog
cargo build --release -p ubel_llvm        # also gives the `ubel-llvm` tool (emit, build, interp)

# playground (WebAssembly)
cargo build --manifest-path crates/wasm_playground/Cargo.toml \
  --target wasm32-unknown-unknown --release
```

---

## Testing

Three layers, each catching different bugs.

1. **Unit tests** in the source files (`#[cfg(test)]`), run with
   `cargo test --workspace --lib --bins`.
2. **The fixture sweep**, `tests/fixtures/*.ubl`: real programs driven through
   lex, parse, sema and the interpreter. `ok_*.ubl` must pass every stage;
   `err_*.ubl` must fail, at the stage its header comment names. Run it with
   `cargo run -p ubel_stratum_rd --example pipeline -- tests/fixtures`.
3. **The CI dashboards.** Every push renders the sweep as a Job Summary and on
   the site's results page, stage by stage, so an expected `err_*` failure is
   never confused with a regression.

The docs have their own test: every example in `web/site/src/` is compiled by
`scripts/check_doc_examples.py` (the *Docs Examples* workflow), so a
documented example cannot go stale.

---

## Documentation

- **The website**, built from [`web/`](web/): the guide, the tier model, the
  error reference, a browser playground and the CI results. Docs are
  versioned (`/docs/next/` for `master`, `/docs/vX.Y.Z/` per release tag,
  `/docs/latest/`). Readers can change the syntax-highlighting colours from
  the page. How it works: [`docs/SITE_DOCS.md`](docs/SITE_DOCS.md).
- **Engineering docs** in [`docs/`](docs/): the language reference
  ([`ubel_stratum.md`](docs/ubel_stratum.md), grammar in
  [`ubel.ebnf`](docs/ubel.ebnf)), memory model, traits, generics, enums,
  formatting, diagnostics rules, LLVM status, and the CLI and LSP notes.
- [`CONTRIBUTING.md`](CONTRIBUTING.md) for how to work on the project.

## Repository layout

| Path | What |
|------|------|
| `crates/core` | lexer, AST, semantic analysis, interpreter, diagnostics |
| `crates/rd_parser` | the live parser, plus `check_source` shared by CLI and LSP |
| `crates/cli`, `crates/lsp` | the `ubel` command and the `ubel-lsp` server |
| `crates/ubel_llvm` | the native-backend preview (`ubel build` calls it) |
| `crates/wasm_playground` | the browser build (own workspace) |
| `crates/parser` | inactive LALRPOP reference parser |
| `tests/fixtures` | the `ok_*` / `err_*` fixture programs |
| `web/` | landing page, docs site, playground, dashboard |
| `self-hosted/` | self-hosting work: a tokenizer written in Ubel |
| `examples/` | `arena_demo`, `basic`, `tier_demo` |

## License

MIT OR Apache-2.0
