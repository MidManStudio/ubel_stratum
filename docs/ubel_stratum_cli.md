# ubel_stratum_cli

## Overview

`crates/cli` builds the `ubel` binary, the command-line front end for
Ubel Stratum. It is a stub in the sense that only three commands exist, but
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
means one variant here, one file under `commands/`, and one match arm.

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

### `commands/debug_tokens.rs`

`ubel debug-tokens <file>`. Prints the token stream with `line:column`.
Marked `[DEBUG]` in its help; the format is not stable. Mirrors
`mdix debug-tokens`.

### `services/pipeline.rs`

`check_file` and `run_file`. Both go through
`ubel_stratum_rd::check_source`, the entry the language server also uses.

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

`ubel-publish.yml` tests `ubel_stratum` and `ubel_stratum_rd` only and does
not touch this crate.

`mdix-cli` declares `rust-version = "1.85"`. This crate declares none: CI
uses stable, and the dependency versions are whatever the resolver picks.

## Fixes and Problems

### `services/pipeline.rs`

- `run_file` lexes and parses a second time after a clean check, because the
  AST borrows the arena that `check_source` owned and dropped. Harmless for a
  command-line tool. A `check_with_arena` variant on the parser crate is the
  fix if it ever matters.

### Verification environment

- The crate was built and tested in a sandbox limited to rustc 1.75, where
  four transitive dependencies had to be pinned to older releases that do not
  require newer compilers (`assert_cmd` 2.0.16, plus the ones listed in
  `docs/ubel_stratum_lsp.md`). The pins live only in that sandbox's lockfile.
  CI uses stable and needs none of them.
