# Editor support: `ubel-lsp`

`ubel-lsp` is a language server for `.ubl` files. It speaks the Language
Server Protocol over standard input and output, so any editor with LSP
support can use it.

## What it does today

One thing: **it publishes the compiler's diagnostics as you type.** The
same errors `ubel check` prints appear as squiggles in the editor, with:

- the diagnostic code (for example `TYPE-101`) as the LSP `code`, so you can
  look it up in [the error reference](../errors/index.md);
- `ubel` as the source;
- the compiler's suggestion attached to the diagnostic.

Hover, go to definition and completion are **not built yet**. The server
advertises only the capabilities that exist, so an editor never offers a
feature that would do nothing.

## Build and run

```text
cargo build -p ubel_stratum_lsp
```

The binary is `target/debug/ubel-lsp`. Point your editor's LSP client at it
for files with the `.ubl` extension, with no arguments. The server logs to
standard error only (standard output is the protocol channel). Set
`RUST_LOG` to change the level, and `UBEL_LSP_LOG` to a file path to also
write a log file.

## Behaviour worth knowing

- Analysis runs on a background thread and is dropped if a newer edit
  arrives while it runs, so typing is never blocked and a stale result is
  never shown.
- A panic inside the compiler is caught; it cannot take the server down.
- Columns are converted to the protocol's UTF-16 positions, so a line with
  non-ASCII characters still underlines the right text.

The design notes are in `docs/ubel_stratum_lsp.md` in the repository. The
server is modelled on `mdix-lsp` from the DixScript-Rust project and shares
the compiler entry point with the command line, so the two always agree.
