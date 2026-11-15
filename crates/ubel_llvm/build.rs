//! Build script of `ubel_llvm`: the Unicode tables of the native runtime.
//!
//! The interpreter implements `to_upper`, `to_lower` and `trim` with Rust's own `str` methods, so it follows the
//! Unicode version of the toolchain that builds it. The C runtime cannot call Rust, so it carries tables. They
//! used to be committed (`runtime/ubel_unicode.h`) and went out of date whenever a newer Rust was used. Now they
//! are generated here, on every build, from the standard library of the very toolchain that compiles this crate
//! (and, in the same cargo invocation, the interpreter), by `scripts/gen_unicode.rs`, included below as a
//! module. The driver embeds the result from `OUT_DIR` (`src/driver.rs`), so a program built with
//! `ubel build` or `ubel-llvm build` always agrees with `ubel run` on those methods, whatever Rust is installed.
//!
//! Cargo runs this again when this file, the generator, or the toolchain changes (a new compiler rebuilds the
//! script, and the run depends on the script). It takes about a second.
//!
//! If the generator fails (it asserts a few things about the standard library that a future Rust could change),
//! the build does not fail: it falls back to the committed `runtime/ubel_unicode.h` and says so with a cargo
//! warning. The unit test `unicode_tables_are_those_of_this_toolchain` in `src/driver.rs` fails in that case
//! unless the committed tables happen to be the right version, so it is not silent where it matters.
//!
//! The scripts that compile the C runtime directly (`run_equiv.sh`, `run_bench.sh`) still read the committed
//! header; that is what remains of it. Provisional, like the rest of the backend.

use std::env;
use std::fs;
use std::path::PathBuf;

#[path = "scripts/gen_unicode.rs"]
mod gen_unicode;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=scripts/gen_unicode.rs");

    let out = PathBuf::from(env::var_os("OUT_DIR").expect("cargo sets OUT_DIR for build scripts")).join("ubel_unicode.h");

    let generated = std::panic::catch_unwind(|| {
        let tables = gen_unicode::tables();
        gen_unicode::header(&tables).0
    });

    let text = match generated {
        Ok(text) => text,
        Err(payload) => {
            let why = payload
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "unknown panic".to_string());
            let why = why.replace(['\n', '\r'], " ");
            println!(
                "cargo:warning=ubel_llvm: could not generate the Unicode tables from this toolchain ({why}); using the \
                 committed runtime/ubel_unicode.h, which may be another Unicode version than the interpreter's, so \
                 to_upper, to_lower and trim may differ between `ubel run` and native programs"
            );
            let committed = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR"))
                .join("runtime")
                .join("ubel_unicode.h");
            println!("cargo:rerun-if-changed=runtime/ubel_unicode.h");
            fs::read_to_string(&committed).unwrap_or_else(|e| panic!("fallback {} unreadable: {e}", committed.display()))
        }
    };

    fs::write(&out, text).unwrap_or_else(|e| panic!("cannot write {}: {e}", out.display()));
}
