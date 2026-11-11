//! Integration tests for the native build driver (`ubel_llvm::driver`) and `ubel-llvm build`.
//!
//! The tests that need a real clang are skipped, with a message, when none is installed. Set
//! `UBEL_REQUIRE_CC=1` (CI does) to make that a failure instead. The compiler search itself is
//! tested without a real clang in the unit tests of `src/driver.rs`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use ubel_llvm::driver::{self, BuildError, BuildOptions, OptLevel};

const BIN: &str = env!("CARGO_BIN_EXE_ubel-llvm");

/// True when a usable clang exists. Without one the caller returns early (skipped), unless
/// `UBEL_REQUIRE_CC` is set.
fn have_cc() -> bool {
    match driver::find_compiler(None) {
        Ok(_) => true,
        Err(e) => {
            if std::env::var_os("UBEL_REQUIRE_CC").is_some() {
                panic!("UBEL_REQUIRE_CC is set but no clang is usable: {e}");
            }
            eprintln!("skipped: no usable clang ({e})");
            false
        }
    }
}

/// A scratch directory that is removed again when the test ends.
struct Scratch(PathBuf);

impl Scratch {
    fn new(tag: &str) -> Scratch {
        let p = std::env::temp_dir().join(format!("ubel-test-{}-{tag}", std::process::id()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(p.join("tmp")).unwrap();
        Scratch(p)
    }
    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
    /// Options that build into this scratch directory and keep temporary files inside it.
    fn opts(&self, exe: &str) -> BuildOptions {
        let mut o = BuildOptions::new(self.path(exe));
        o.tmp_base = Some(self.path("tmp"));
        o
    }
    /// How many entries the build's temporary directory holds.
    fn leftovers(&self) -> usize {
        fs::read_dir(self.path("tmp")).unwrap().count()
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn crate_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn example(name: &str) -> String {
    let p = crate_dir().join("examples").join(format!("{name}.ubl"));
    fs::read_to_string(&p).unwrap_or_else(|e| panic!("cannot read {}: {e}", p.display()))
}

fn run(exe: &Path) -> std::process::Output {
    Command::new(exe).output().unwrap_or_else(|e| panic!("cannot run {}: {e}", exe.display()))
}

const HELLO: &str = "fn main() void {\n    println(\"hello\", 6 * 7)\n}\n";
const TYPE_ERROR: &str = "fn main() void {\n    let x: int = \"a\"\n    println(x)\n}\n";
const PARSE_ERROR: &str = "fn main( {\n";
const NOT_SUPPORTED_YET: &str = "struct Box<T> {\n    value: T\n}\n\nfn main() void {\n    println(\"hi\")\n}\n";

#[test]
fn examples_run_like_the_interpreter() {
    if !have_cc() {
        return;
    }
    let scratch = Scratch::new("examples");
    // A spread of what the backend does: arithmetic, structs, enums, strings, closures, and
    // the target program. Most are built at -O0 (quicker); two go through the default -O2.
    let mut cases: Vec<(String, String, OptLevel)> = vec![
        ("01_arith".into(), example("01_arith"), OptLevel::O0),
        ("10_struct_basic".into(), example("10_struct_basic"), OptLevel::O0),
        ("16_enum_gc".into(), example("16_enum_gc"), OptLevel::O0),
        ("19_strings".into(), example("19_strings"), OptLevel::O2),
        ("21_closures".into(), example("21_closures"), OptLevel::O0),
        ("30_string_build".into(), example("30_string_build"), OptLevel::O0),
        ("31_enum_unit".into(), example("31_enum_unit"), OptLevel::O2),
    ];
    let target = crate_dir().join("../../examples/arena_demo/ast_builder.ubl");
    if let Ok(src) = fs::read_to_string(&target) {
        cases.push(("ast_builder".into(), src, OptLevel::O2));
    }

    for (name, source, opt) in cases {
        let want = ubel_llvm::interpret(&source);
        assert!(want.error.is_none(), "{name}: the interpreter failed: {:?}", want.error);
        assert!(!want.output.is_empty(), "{name}: the interpreter printed nothing, the comparison would be empty");

        let mut o = scratch.opts(&name);
        o.opt = opt;
        let built = driver::build(&source, &o).unwrap_or_else(|e| panic!("{name}: build failed: {e}"));
        assert_eq!(built.output, scratch.path(&name));
        assert!(built.kept_dir.is_none());

        let out = run(&built.output);
        assert!(out.status.success(), "{name}: the native program failed: {}", out.status);
        assert_eq!(String::from_utf8_lossy(&out.stdout), want.output, "{name}: output differs from the interpreter");
    }
    assert_eq!(scratch.leftovers(), 0, "temporary directories were left behind");
}

#[test]
fn a_runtime_failure_is_not_a_build_failure() {
    if !have_cc() {
        return;
    }
    let scratch = Scratch::new("failing");
    let source = example("fail_div_zero");
    let want = ubel_llvm::interpret(&source);
    assert!(want.error.is_some(), "fail_div_zero is expected to fail in the interpreter");

    let built = driver::build(&source, &scratch.opts("fail_div_zero")).expect("it builds");
    let out = run(&built.output);
    assert_eq!(out.status.code(), Some(1), "the native program should exit with status 1");
    assert_eq!(String::from_utf8_lossy(&out.stdout), want.output, "output before the failure differs");
}

#[test]
fn front_end_and_emitter_errors_come_before_the_compiler_is_looked_for() {
    // The compiler named here does not exist. If it were looked up first, every one of these
    // would be reported as a missing compiler, which would blame the wrong thing.
    let scratch = Scratch::new("errors");
    let mut o = scratch.opts("never");
    o.cc = Some(PathBuf::from("/nonexistent/clang"));

    for (what, source, stage) in [
        ("parse error", PARSE_ERROR, "parse"),
        ("type error", TYPE_ERROR, "sema"),
        ("not supported yet", NOT_SUPPORTED_YET, "emit"),
    ] {
        match driver::build(source, &o) {
            Err(BuildError::Compile(e)) => {
                assert_eq!(e.stage, stage, "{what}");
                assert!(!e.messages.is_empty(), "{what}");
                // every message starts with line:col
                for m in &e.messages {
                    let (loc, _) = m.split_once(": ").unwrap_or_else(|| panic!("{what}: no location in {m:?}"));
                    let parts: Vec<&str> = loc.split(':').collect();
                    assert!(
                        parts.len() == 2 && parts.iter().all(|p| p.parse::<u32>().is_ok()),
                        "{what}: not a line:col location: {m:?}"
                    );
                }
            }
            other => panic!("{what}: expected a compile error, got {other:?}"),
        }
    }
    assert!(!scratch.path("never").exists(), "no executable may be written for a program that was refused");
    assert_eq!(scratch.leftovers(), 0);
}

#[test]
fn a_missing_compiler_is_a_clear_error() {
    let scratch = Scratch::new("nocc");
    let mut o = scratch.opts("hello");
    o.cc = Some(PathBuf::from("/nonexistent/clang"));
    match driver::build(HELLO, &o) {
        Err(e @ BuildError::ExplicitNotFound { .. }) => {
            let text = e.to_string();
            assert!(text.contains("/nonexistent/clang") && text.contains("--cc"), "{text}");
        }
        other => panic!("expected ExplicitNotFound, got {other:?}"),
    }
    assert!(!scratch.path("hello").exists());
    assert_eq!(scratch.leftovers(), 0);
}

#[test]
fn a_missing_output_directory_is_reported_before_clang_runs() {
    if !have_cc() {
        return;
    }
    let scratch = Scratch::new("outdir");
    let o = BuildOptions::new(scratch.path("no/such/dir/hello"));
    match driver::build(HELLO, &o) {
        Err(BuildError::OutputDirMissing(p)) => assert!(p.ends_with("no/such/dir"), "{p:?}"),
        other => panic!("expected OutputDirMissing, got {other:?}"),
    }
}

#[test]
fn clang_failures_carry_the_command_and_the_message() {
    if !have_cc() {
        return;
    }
    let scratch = Scratch::new("clangfail");

    // broken IR: clang runs and fails. The temporary directory is gone afterwards.
    match driver::build_ir("this is not llvm ir\n", &scratch.opts("broken")) {
        Err(BuildError::CompilerFailed { command, stderr, kept_dir, .. }) => {
            assert!(command.contains("program.ll") && command.contains("ubel_rt.c"), "{command}");
            assert!(!stderr.trim().is_empty(), "clang's message must be kept");
            assert!(kept_dir.is_none());
        }
        other => panic!("expected CompilerFailed, got {other:?}"),
    }
    assert_eq!(scratch.leftovers(), 0, "a failed build must clean up too");

    // with keep_temps the IR and the runtime files stay, and the error says where
    let mut o = scratch.opts("broken2");
    o.keep_temps = true;
    match driver::build_ir("this is not llvm ir\n", &o) {
        Err(BuildError::CompilerFailed { kept_dir: Some(dir), .. }) => {
            for f in ["program.ll", "ubel_rt.c", "ubel_rt.h", "ubel_unicode.h", "ubel_main.c"] {
                assert!(dir.join(f).is_file(), "{f} missing in {}", dir.display());
            }
            assert_eq!(fs::read_to_string(dir.join("program.ll")).unwrap(), "this is not llvm ir\n");
        }
        other => panic!("expected CompilerFailed with a kept directory, got {other:?}"),
    }
}

#[test]
fn extra_flags_reach_clang() {
    if !have_cc() {
        return;
    }
    let scratch = Scratch::new("flags");
    let mut o = scratch.opts("hello");
    o.extra_flags = vec!["-this-flag-does-not-exist".into()];
    match driver::build(HELLO, &o) {
        Err(BuildError::CompilerFailed { command, stderr, .. }) => {
            assert!(command.contains("-this-flag-does-not-exist"), "{command}");
            assert!(stderr.contains("this-flag-does-not-exist"), "{stderr}");
        }
        other => panic!("expected CompilerFailed, got {other:?}"),
    }
}

#[test]
fn temporary_files_are_kept_only_on_request_and_the_build_still_works() {
    if !have_cc() {
        return;
    }
    let scratch = Scratch::new("keep");
    let mut o = scratch.opts("hello");
    o.keep_temps = true;
    o.opt = OptLevel::O1;
    let built = driver::build(HELLO, &o).expect("builds");
    let dir = built.kept_dir.clone().expect("kept_dir is set with keep_temps");
    assert!(dir.starts_with(scratch.path("tmp")));
    assert!(dir.join("program.ll").is_file());
    // the directory holds generated code that is then compiled: nobody else may write into it
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700, "the temporary directory must be private");
    }
    assert_eq!(scratch.leftovers(), 1);
    assert!(built.command.contains("-O1"), "{}", built.command);
    assert!(built.command.contains("-Wno-override-module"), "{}", built.command);
    assert_eq!(String::from_utf8_lossy(&run(&built.output).stdout), "hello 42\n");
}

// ---- the command line -------------------------------------------------------------------

fn cli(args: &[&str], dir: &Path) -> std::process::Output {
    Command::new(BIN).args(args).current_dir(dir).output().expect("cannot run ubel-llvm")
}

#[test]
fn cli_builds_with_the_default_output_name_and_with_o() {
    if !have_cc() {
        return;
    }
    let scratch = Scratch::new("cli");
    fs::write(scratch.path("hello.ubl"), HELLO).unwrap();

    let out = cli(&["build", "hello.ubl"], &scratch.0);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(String::from_utf8_lossy(&out.stderr).contains("built hello"));
    assert_eq!(String::from_utf8_lossy(&run(&scratch.path("hello")).stdout), "hello 42\n");

    let out = cli(&["build", "hello.ubl", "-o", "other", "-O0", "-v"], &scratch.0);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("compiler:") && err.contains("command:") && err.contains("-O0"), "{err}");
    assert_eq!(String::from_utf8_lossy(&run(&scratch.path("other")).stdout), "hello 42\n");
}

#[test]
fn cli_reports_errors_with_the_right_exit_status() {
    let scratch = Scratch::new("clierr");
    fs::write(scratch.path("bad.ubl"), TYPE_ERROR).unwrap();
    fs::write(scratch.path("gen.ubl"), NOT_SUPPORTED_YET).unwrap();
    fs::write(scratch.path("hello.ubl"), HELLO).unwrap();

    // a program error: status 1, the located message on stderr, no output file
    let out = cli(&["build", "bad.ubl"], &scratch.0);
    assert_eq!(out.status.code(), Some(1));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("bad.ubl: sema error") && err.contains("2:5:"), "{err}");
    assert!(!scratch.path("bad").exists());

    let out = cli(&["build", "gen.ubl"], &scratch.0);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("not supported yet: generic structs"));

    // a compiler that is not there: status 1, and the message names it
    let out = cli(&["build", "hello.ubl", "--cc", "/nonexistent/clang"], &scratch.0);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("/nonexistent/clang"));

    // usage errors: status 2
    for args in [
        vec!["build", "hello.ubl", "-o"],
        vec!["build", "hello.ubl", "--cc"],
        vec!["build", "hello.ubl", "--nonsense"],
        vec!["build", "hello.ubl", "-O9"],
    ] {
        let out = cli(&args, &scratch.0);
        assert_eq!(out.status.code(), Some(2), "{args:?}: {}", String::from_utf8_lossy(&out.stderr));
    }

    // an unreadable source file: status 1
    let out = cli(&["build", "missing.ubl"], &scratch.0);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("cannot read missing.ubl"));
}

#[test]
fn cli_refuses_to_overwrite_the_source() {
    let scratch = Scratch::new("clisrc");
    fs::write(scratch.path("prog"), HELLO).unwrap();
    // `build prog` would name its output `prog` as well
    let out = cli(&["build", "prog"], &scratch.0);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("is the source file"));
    assert_eq!(fs::read_to_string(scratch.path("prog")).unwrap(), HELLO, "the source must be untouched");

    // and with -o spelled differently
    let out = cli(&["build", "prog", "-o", "./prog"], &scratch.0);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(fs::read_to_string(scratch.path("prog")).unwrap(), HELLO);
}

#[test]
fn cli_passes_flags_after_the_double_dash_to_clang() {
    if !have_cc() {
        return;
    }
    let scratch = Scratch::new("cliflags");
    fs::write(scratch.path("hello.ubl"), HELLO).unwrap();
    let out = cli(&["build", "hello.ubl", "--", "-this-flag-does-not-exist"], &scratch.0);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("this-flag-does-not-exist"));
}
