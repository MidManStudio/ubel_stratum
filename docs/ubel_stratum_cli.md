# ubel_stratum_cli

## Overview

`crates/cli` builds the `ubel` binary, the command-line front end for
Ubel Stratum. It is a stub in the sense that only four commands exist, but
the layout is the final one and the commands are real.

It is modeled on the `mdix` CLI in DixScript-Rust (`mdix-cli`), which uses
the same shape: a clap derive command tree in `main.rs`, one file per
subcommand under `commands/`, shared logic under `services/`, and terminal
and JSON presentation under `output/`.

**What was taken from `mdix-cli`:** `clap` 4 with derive, `colored` for
terminal output, `serde` and `serde_json` for `--json`, the global flags
`--verbose`, `--quiet`, `--json` and `--no-color`, a `GlobalOpts` struct
passed to every command, a `CliError` enum, `run(args, &global) -> i32`
returning the exit code, and `assert_cmd` plus `predicates` plus `tempfile`
for tests that spawn the real binary.

**What was left out for now:** `clap_complete` (shell completions), `toml`,
`anyhow`, `indicatif`, `dirs`, `rand`, `uuid`, `base64`. `mdix-cli` needs
them for conversion, encryption, keys and a config file; nothing in Ubel
needs them yet.

## Modules

### `main.rs`

The `Cli` struct (global flags) and the `Commands` enum. Adding a command
means one variant here, one file under `commands/`, and one match arm. The
whole command runs inside `ubel_stratum::stack::with_big_stack`, a thread with
a 256 MB stack: every pass walks the tree recursively, and the default stack of
a main thread is 8 MB, which a chain of about 500 operators exhausted in a
debug build (see `PARSER_RULES.md`, section 5.14).

### `commands/mod.rs`

`GlobalOpts`, `CliError`, `handle_error`, and the exit codes: `0` success,
`1` the file has diagnostics or the program failed at runtime, `2` the
command could not be carried out (missing or unreadable file).

### `commands/check.rs`

`ubel check <file>`. Lexes, parses and type-checks without running.
Diagnostics render through the compiler's own `render_all`, the same
renderer `examples/diagnose.rs` uses, so the text matches everywhere. With
`--json` the output is one object: `file`, `ok`, `stage` (`lex`, `parse`,
`sema`, `clean`), `diagnostics` (each with `code`, `message`, `line`,
`column`, `suggestion`), and `elapsed_ms`.

### `commands/run.rs`

`ubel run <file>`. Runs the same check, refuses to run a file that fails it,
then runs `main` through the interpreter. A failed check is reported exactly
as `check` reports it (`check::report` is shared).

### `commands/build.rs`

`ubel build <file> [-o PATH] [--cc PATH] [-O 0|1|2|3|s] [--keep-temps] [-- CLANG_ARGS]`. Runs the same check as `check`
and `run` (a failed check is reported by `check::report`, so the diagnostics and the `--json` object are the same),
then calls `ubel_llvm::driver::build`: it emits LLVM IR, writes it with the embedded C runtime to a private temporary
directory and makes one `clang` call (version 15 or newer; `--cc` or `$UBEL_CC` choose one). The default output is the
file name without its extension, in the current directory, and the command refuses to write over the source file. The
LLVM backend handles a subset of the language: what it cannot do yet is refused with a located `not supported yet`
message.

Exit codes follow the rest of the CLI: `0` built; `1` the program has diagnostics, the backend refuses it, or clang
fails on it; `2` the build could not be carried out (no usable clang, an output directory that does not exist, a bad
option). `--verbose` shows the compiler and the full command, `--quiet` prints nothing on success, `--keep-temps` keeps
the IR and the runtime files and prints where they are. With `--json` the object has `file`, `ok`, `stage` (`build`,
or `emit` when the backend refused the program), `output`, `compiler`, `command`, `error` and `elapsed_ms`. The backend
itself is described in `LLVM_POC.md` ("Building an executable"); for Android see `ANDROID_TERMUX.md`.

### `tests/build_tests.rs`

End-to-end tests of `ubel build`, in the style of `cli_tests.rs`. The ones that need a real clang skip themselves, with a
message, when none is installed, unless `UBEL_REQUIRE_CC=1` is set. The others always run: a program with diagnostics,
one the backend refuses, a compiler that is not there, the guard against writing over the source, and usage errors.

### `commands/debug_tokens.rs`

`ubel debug-tokens <file>`. Prints the token stream with `line:column`.
Marked `[DEBUG]` in its help; the format is not stable. Mirrors
`mdix debug-tokens`.

### `services/pipeline.rs`

`check_file` and `run_file`. Both go through
`ubel_stratum_rd::check_source`, the entry the language server also uses.
`run_file` then re-parses and runs sema again to obtain
`SemaContext::int_literal_types`, and passes it to
`Interpreter::set_int_literal_types` before `run_program`. Without it the
interpreter cannot know that `let x: u8 = 5` holds a `u8`.

### `services/file_io.rs`, `output/printer.rs`, `output/json_output.rs`

Reading a source file (mapping `NotFound` to its own error), colored
one-line output helpers, and pretty JSON on stdout.

**Planned, in the order `mdix-cli` suggests:** `debug-ast`, `debug-symbols`,
`fmt` (blocked on a formatter existing), `completions`, `new` (a project
template).

## CI and Workflows

`crates/cli` is a workspace member but not a default member, so a bare
`cargo build` or `cargo test` skips it and `cargo test --workspace`, which
`ci-check.yml` runs, includes it. Build it directly with
`cargo build -p ubel_stratum_cli`; the binary is `target/<profile>/ubel`.

`crates/ubel_llvm` (the LLVM backend, which `ubel build` calls) is a member too, so `cargo test --workspace` also runs its
tests: they build native programs when a clang 15 or newer is installed and skip themselves when none is. The workspace
release profile is LTO with one codegen unit; a build on a small machine can switch that off with
`CARGO_PROFILE_RELEASE_LTO=false CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16`.

`ubel-publish.yml` tests `ubel_stratum` and `ubel_stratum_rd` only and does
not touch this crate.

`mdix-cli` declares `rust-version = "1.85"`. This crate declares none: CI
uses stable, and the dependency versions are whatever the resolver picks.

## Fixes and Problems

### `services/pipeline.rs`

- `run_file` lexes and parses a second time after a clean check, because the
  AST borrows the arena that `check_source` owned and dropped. Harmless for a
  command-line tool. A `check_with_arena` variant on the parser crate is the
  fix if it ever matters. It now also runs sema a second time for the literal
  width table; the same `check_with_arena` variant would remove both repeats.

### Verification environment

- The crate was built and tested in a sandbox limited to rustc 1.75, where
  four transitive dependencies had to be pinned to older releases that do not
  require newer compilers (`assert_cmd` 2.0.16, plus the ones listed in
  `docs/ubel_stratum_lsp.md`). The pins live only in that sandbox's lockfile.
  CI uses stable and needs none of them.
