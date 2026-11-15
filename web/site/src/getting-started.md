# Getting Started

There is no installer or binary release yet, so you build the tools from
source. This takes a few minutes. If you only want to try the language,
skip to [the playground](./tooling/playground.md): it runs in the browser
with nothing to install.

## 1. Get the source and a Rust toolchain

```text
git clone https://github.com/MidManStudio/ubel_stratum.git
cd ubel_stratum
```

The project builds with a current stable Rust, which is what CI uses. It is
also kept compatible with **Rust 1.75**, because it is developed on older
hardware. On 1.75 two extra steps are needed:

1. The checked-in `Cargo.lock` uses lockfile format 4, which Cargo 1.75
   cannot read. Delete it and let Cargo write a fresh one:
   `cargo generate-lockfile`.
2. A few dependencies have newer releases that need a newer compiler. Pin
   them to versions that still build on 1.75:

```text
cargo update -p owo-colors  --precise 4.0.0
cargo update -p backtrace   --precise 0.3.69
cargo update -p proptest    --precise 1.4.0
cargo update -p tempfile    --precise 3.14.0
cargo update -p clap        --precise 4.4.18
cargo update -p rayon       --precise 1.10.0
cargo update -p rayon-core  --precise 1.12.1
cargo update -p half        --precise 2.4.1
```

(`cargo update` reports an error for a package that is not in your
lockfile; skip that line.) With a recent stable toolchain none of this is
needed.

## 2. Build the `ubel` command

```text
cargo build -p ubel_stratum_cli
```

The binary is `target/debug/ubel`. Add `--release` for an optimised build
at `target/release/ubel`. The first build also compiles the compiler
crates and is the slow one.

## 3. Write and run a program

Save this as `hello.ubl`. Ubel source files use the `.ubl` extension.

<!-- ubel-test: run -->
```ubel
fn main() {
    println("Hello, Stratum!")
}
```

Check it without running (lex, parse and type-check only), then run it:

```text
$ ubel check hello.ubl
✓ hello.ubl is valid

$ ubel run hello.ubl
Hello, Stratum!
```

`ubel run` refuses to run a file that has diagnostics and prints them the
way `ubel check` does. See [The `ubel` command line](./tooling/cli.md) for the
flags, JSON output and exit codes.

## 4. Make an error on purpose

Errors are part of learning a language, so look at one early. This program
assigns a string to an `int` variable:

<!-- ubel-test: error TYPE-101 -->
```ubel
fn main() {
    let count: int = "three"
}
```

```text
>>> error[TYPE-101]: type mismatch: expected `int`, found `string`
  --> 2:5
    |
  2 |     let count: int = "three"
    |     ^^^^^^^^^^^^^^^^^^^^^^^^
<<<
```

Every diagnostic has this shape: a stable code (`TYPE-101`), a one-line
message, the exact source span, and sometimes notes and a `help:` line.
[The error reference](./errors/index.md) explains each code.

## 5. Where next

- Learn the language: start with [Values and variables](./guide/values.md).
- Understand what makes Ubel different: [The Tier Model](./tier-model.md).
- Hack on the compiler: the [repository](https://github.com/MidManStudio/ubel_stratum)
  has the engineering docs (`docs/`) and a fixture suite of over three
  hundred small `.ubl` programs in `tests/fixtures/`. A file named `ok_*.ubl`
  must pass every stage; an `err_*.ubl` must fail, and its header comment
  says where.
