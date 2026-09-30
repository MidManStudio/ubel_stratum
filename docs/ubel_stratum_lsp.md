# ubel_stratum_lsp

## Overview

`crates/lsp` builds the `ubel-lsp` binary and the `ubel_stratum_lsp`
library: a language server for `.ubl` files. It is a stub in that it does one
thing, publish the compiler's diagnostics as the document changes, but the
protocol plumbing, document store, position conversion and end-to-end test
are the real foundation the features will be added onto.

It is modeled on `mdix-lsp` in DixScript-Rust.

**What was taken from `mdix-lsp`:**
- The stack: `tower-lsp` 0.20 for the protocol, `tokio` as its runtime,
  `dashmap` for the shared document map, `tracing`, `tracing-subscriber`
  and `tracing-appender` for logging.
- A thin binary over a library: `src/main.rs` calls `setup_logging()` and
  `run()`, and `tower_lsp` is re-exported so a downstream crate names the
  same types.
- A `Backend` holding `Client` and `Arc<DashMap<Url, Document>>`, implementing
  `LanguageServer`.
- Analysis on `spawn_blocking`, guarded with `catch_unwind` so a compiler
  panic cannot take the server down, and dropped when a newer edit arrived
  while it ran (a version check before publishing).
- Logging to stderr only (stdout is the protocol channel), `RUST_LOG` for the
  level, and an env var (`UBEL_LSP_LOG`, `MDIX_LSP_LOG` there) to also write a
  file.
- A `features/` folder, one file per feature, and a `capabilities.rs`.
- An integration test that spawns the real binary and speaks JSON-RPC.

**What differs, and why:**
- `Document` holds only text and version. The `mdix-lsp` one caches tokens,
  AST and semantic result, but the Ubel AST borrows an `AstArena`, so it
  cannot simply sit beside the source. That gets designed with the first
  feature that needs the tree (hover, go to definition).
- Capabilities advertise only what is built. `mdix-lsp` advertises a long
  list; a client calls anything advertised, so here each capability is added
  in the same change as its handler.
- Diagnostics come from `ubel_stratum_rd::check_source`, shared with the
  command line, instead of a pipeline private to the server.
- No `extensions` module (`mdix-lsp` has one for dialects built on `.mdix`).
  There is no Ubel dialect story.

## Modules

### `lib.rs`, `main.rs`

`run()` serves the protocol over stdin and stdout. `setup_logging()` wires
tracing. `main` is `#[tokio::main]` plus those two calls.

### `server.rs`

`Backend`. `initialize` returns the capabilities and server info,
`did_open`, `did_change` and `did_close` maintain the document map and trigger
analysis, and `did_close` publishes an empty list to clear the file.
`analyze_and_publish` reads the text, checks it on a blocking thread, and
publishes only if the stored version still matches.

### `analyzer.rs`

`analyze(&str) -> Vec<Diagnostic>` over `check_source`, panic-guarded.

### `converters.rs`

Compiler diagnostics to LSP diagnostics. Compiler spans carry byte offsets
plus a 1-based line and column; LSP positions are 0-based and count UTF-16
code units. `position_at` walks the text, so a line containing non-ASCII
characters gets the right column. `range_for` falls back to the span's own
line and column when its offsets do not fit the text. The diagnostic code
(`TYPE-101`) becomes the LSP `code`, source is `ubel`, a suggestion becomes a
`help:` line after the message, and secondary spans become related
information.

### `capabilities.rs`

Full-document text sync, nothing else.

### `document.rs`, `features/mod.rs`

The document record, and a doc-only placeholder listing the first features
worth building (hover from resolved types, go to definition from the `DefId`
name resolution already records per identifier, semantic tokens from the
lexer's token kinds).

**Tests:** unit tests in `analyzer.rs`, `converters.rs`, `capabilities.rs`,
and `tests/lsp_stdio.rs`, which spawns the binary, drives it with
`Content-Length` framed JSON-RPC using only `std` and `serde_json`, and
asserts the diagnostics an editor would see (code, source, position, version,
and that a fix or a close clears them).

## CI and Workflows

`crates/lsp` is a workspace member but not a default member. A bare
`cargo build` or `cargo test` skips it, because it pulls in `tokio` and
`tower-lsp`; `cargo test --workspace`, which `ci-check.yml` runs, includes it.
Build it directly with `cargo build -p ubel_stratum_lsp`.

`mdix-lsp` has its own `lsp-ci.yml` that builds release binaries and runs a
latency-measuring integration harness. Nothing equivalent exists here yet.

`mdix-lsp` declares `rust-version = "1.85"`, which comes from `dixscript`
itself. This crate declares none.

## Fixes and Problems

### `tests/lsp_stdio.rs`

- The first version of the test spent 30 seconds per case waiting for the
  server process to exit after `exit`. `tower-lsp` 0.20 does not end its
  serve loop when the `exit` notification arrives; it ends when the next
  message arrives or stdin closes. A real client closes the pipe, so the test
  now closes stdin after `exit` and asserts a clean exit status. The run went
  from 120 s to 0.03 s. `mdix-lsp` behaves the same way and its harness never
  waits for exit.

### Verification environment

- Built and tested in a sandbox limited to rustc 1.75. Four transitive
  dependencies resolved to releases requiring a newer compiler or the unstable
  `edition2024` feature and were pinned down there: `time` 0.3.36 (through
  `tracing-appender`), `url` 2.5.0 (avoids the ICU crates), `tracing-appender`
  0.2.3 (later releases need `thiserror` 2), and `assert_cmd` 2.0.16. The pins
  live only in that sandbox's lockfile; CI uses stable and needs none of them.
  The committed `Cargo.lock` is a newer lockfile format than rustc 1.75 can
  read, which is why the sandbox regenerates its own.

### Known limitations

- Full-document sync only; every edit re-checks the whole file.
- Spans from a string-interpolation hole can be relative to the hole. The
  converter falls back to line and column for those, which is right to within
  a character but not a full range.
- Diagnostics are all reported as errors; the compiler has no warning level
  yet.
