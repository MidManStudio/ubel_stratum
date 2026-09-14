# Getting Started

There is no installable compiler or package manager yet (that is Phase 5).
Running Ubel Stratum today means building the interpreter from source and
driving it through the pipeline example, or trying source directly in the
[browser playground](/playground/) without installing anything.

## Prerequisites

Rust **1.75** specifically. The project targets free GitHub Actions
runners and older local hardware, so the toolchain is pinned rather than
tracking `stable`. A handful of dependencies need pinning to versions that
still support 1.75 after a fresh `cargo generate-lockfile`:

```bash
cargo update -p owo-colors  --precise 4.0.0
cargo update -p backtrace   --precise 0.3.69
cargo update -p proptest    --precise 1.4.0
cargo update -p tempfile    --precise 3.14.0
cargo update -p clap        --precise 4.4.18
cargo update -p rayon       --precise 1.10.0
cargo update -p rayon-core  --precise 1.12.1
cargo update -p half        --precise 2.4.1
cargo update -p textwrap    --precise 0.16.0
```

LALRPOP is not required. `crates/parser` is an inactive reference
implementation, not a default workspace member; it is only relevant when
working on that crate specifically.

## Build and test

```bash
cargo build --workspace --all-targets
cargo test  --workspace --lib --bins
cargo bench   # crates/core and crates/rd_parser each have benches/
```

## Running the pipeline

```bash
# Run the full pipeline against every fixture
cargo run -p ubel_stratum_rd --example pipeline -- tests/fixtures

# Against a single file
cargo run -p ubel_stratum_rd --example pipeline -- path/to/file.ubl
```

This is the same command the CI pipeline runs on every push; its output
backs the [CI Results](/results/) page.

## A first program

```ubel
fn main() {
    println("Hello, Stratum!")
}
```

Functions default to the HIGH tier, so this needs no `@tier` annotation.
Saved as `hello.ubl` and run through the pipeline above, it prints its
one line and exits. The [Language Tour](./language-tour.md) covers the
rest of the surface syntax; [The Tier Model](./tier-model.md) covers what
`@tier` actually changes about how a function compiles and runs.
