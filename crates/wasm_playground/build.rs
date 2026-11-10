// crates/wasm_playground/build.rs
//
// The browser has no threads, so the compiler cannot move itself onto a large
// stack the way the native entry points do (`ubel_stratum::stack`). Instead the
// stack of the whole module is sized at link time. wasm32-unknown-unknown
// defaults to 1 MiB, which a long expression chain exhausts: every pass walks
// the tree recursively. The stack sits at the start of linear memory, so an
// overflow runs off the bottom and traps as "index out of bounds" rather than
// as a stack overflow.
//
// Set here, not in a `.cargo/config.toml`, because cargo reads config files
// from the working directory, and the site workflow builds this crate with
// `--manifest-path` from the repository root.

/// 16 MiB. The parser caps nesting at `MAX_NESTING` levels (PARSE-006).
const STACK_BYTES: u32 = 16 * 1024 * 1024;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rustc-link-arg-cdylib=-zstack-size={}", STACK_BYTES);
}
