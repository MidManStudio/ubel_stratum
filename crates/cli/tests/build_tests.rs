// ============================================================================
// NOTICE: Full documentation, design decisions, and fix history for this file
// live in docs/ubel_stratum_cli.md, section "tests/build_tests.rs"
// ============================================================================
// crates/cli/tests/build_tests.rs
//! End-to-end tests of `ubel build`: spawn the real `ubel` binary against temp files.
//!
//! The tests that need a real clang skip themselves, with a message, when none is installed;
//! set `UBEL_REQUIRE_CC=1` to make that a failure instead. The ones that do not need a compiler
//! (a program with diagnostics, one the backend refuses, a compiler that is not there) always run.

use assert_cmd::Command;
use predicates::str::contains;
use std::fs;
use std::path::Path;
use tempfile::TempDir;

fn ubel() -> Command {
    Command::cargo_bin("ubel").unwrap()
}

const VALID: &str = "fn main() void {\n    println(\"hello from ubel\", 6 * 7)\n}\n";
const TYPE_ERROR: &str = "fn main() void { let x: int = \"a\" }\n";
const NOT_SUPPORTED_YET: &str = "struct Box<T> {\n    value: T\n}\n\nfn main() void {\n    println(\"hi\")\n}\n";

/// A temp directory holding `name` with the given contents.
fn project(name: &str, source: &str) -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join(name), source).unwrap();
    dir
}

fn have_cc() -> bool {
    match ubel_llvm::driver::find_compiler(None) {
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

fn run_exe(path: &Path) -> String {
    let out = std::process::Command::new(path).output().expect("cannot run the built program");
    assert!(out.status.success(), "the built program failed: {}", out.status);
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn build_makes_an_executable_that_runs() {
    if !have_cc() {
        return;
    }
    let dir = project("hello.ubl", VALID);
    ubel().current_dir(dir.path()).args(["build", "hello.ubl"]).assert().success().stdout(contains("built"));
    assert_eq!(run_exe(&dir.path().join("hello")), "hello from ubel 42\n");

    // -o, an optimisation level, and --verbose, which shows the compiler and the command
    ubel()
        .current_dir(dir.path())
        .args(["build", "hello.ubl", "-o", "other", "-O", "0", "--verbose"])
        .assert()
        .success()
        .stdout(contains("compiler"))
        .stdout(contains("-O0"));
    assert_eq!(run_exe(&dir.path().join("other")), "hello from ubel 42\n");
}

#[test]
fn build_keep_temps_keeps_the_ir_and_says_where() {
    if !have_cc() {
        return;
    }
    let dir = project("hello.ubl", VALID);
    let tmp = dir.path().join("tmp");
    fs::create_dir(&tmp).unwrap();
    ubel()
        .current_dir(dir.path())
        .env("TMPDIR", &tmp)
        .args(["build", "hello.ubl", "--keep-temps"])
        .assert()
        .success()
        .stdout(contains("kept"));
    let kept: Vec<_> = fs::read_dir(&tmp).unwrap().map(|e| e.unwrap().path()).collect();
    assert_eq!(kept.len(), 1, "exactly one kept directory expected, found {kept:?}");
    assert!(kept[0].join("program.ll").is_file());
    assert!(kept[0].join("ubel_rt.c").is_file());

    // without the flag nothing is left behind
    let before = fs::read_dir(&tmp).unwrap().count();
    ubel().current_dir(dir.path()).env("TMPDIR", &tmp).args(["build", "hello.ubl"]).assert().success();
    assert_eq!(fs::read_dir(&tmp).unwrap().count(), before, "a build must clean up after itself");
}

#[test]
fn build_quiet_prints_nothing_on_success() {
    if !have_cc() {
        return;
    }
    let dir = project("hello.ubl", VALID);
    ubel().current_dir(dir.path()).args(["build", "hello.ubl", "--quiet"]).assert().success().stdout("");
    assert_eq!(run_exe(&dir.path().join("hello")), "hello from ubel 42\n");
}

#[test]
fn build_json_reports_the_result() {
    if !have_cc() {
        return;
    }
    let dir = project("hello.ubl", VALID);
    ubel()
        .current_dir(dir.path())
        .args(["build", "hello.ubl", "--json"])
        .assert()
        .success()
        .stdout(contains("\"ok\": true"))
        .stdout(contains("\"stage\": \"build\""))
        .stdout(contains("\"output\": \"hello\""));
}

#[test]
fn build_reports_a_diagnostic_exactly_as_check_does() {
    let dir = project("bad.ubl", TYPE_ERROR);
    ubel().current_dir(dir.path()).args(["build", "bad.ubl"]).assert().code(1).stderr(contains("TYPE-101"));
    assert!(!dir.path().join("bad").exists(), "no executable may be written for a program with diagnostics");

    ubel()
        .current_dir(dir.path())
        .args(["build", "bad.ubl", "--json"])
        .assert()
        .code(1)
        .stdout(contains("\"stage\": \"sema\""))
        .stdout(contains("\"code\": \"TYPE-101\""));
}

#[test]
fn build_refuses_what_the_backend_cannot_do_yet() {
    let dir = project("generic.ubl", NOT_SUPPORTED_YET);
    ubel()
        .current_dir(dir.path())
        .args(["build", "generic.ubl"])
        .assert()
        .code(1)
        .stderr(contains("not supported yet: generic structs"))
        .stderr(contains("cannot build"));
    assert!(!dir.path().join("generic").exists());

    ubel()
        .current_dir(dir.path())
        .args(["build", "generic.ubl", "--json"])
        .assert()
        .code(1)
        .stdout(contains("\"ok\": false"))
        .stdout(contains("\"stage\": \"emit\""));
}

#[test]
fn build_without_a_usable_compiler_is_a_usage_error() {
    let dir = project("hello.ubl", VALID);
    ubel()
        .current_dir(dir.path())
        .args(["build", "hello.ubl", "--cc", "/nonexistent/clang"])
        .assert()
        .code(2)
        .stderr(contains("/nonexistent/clang"));
    assert!(!dir.path().join("hello").exists());
}

#[test]
fn build_refuses_to_overwrite_the_source() {
    let dir = project("prog", VALID);
    // `build prog` would name its output `prog` as well
    ubel().current_dir(dir.path()).args(["build", "prog"]).assert().code(2).stderr(contains("is the source file"));
    assert_eq!(fs::read_to_string(dir.path().join("prog")).unwrap(), VALID, "the source must be untouched");
    ubel().current_dir(dir.path()).args(["build", "prog", "-o", "./prog"]).assert().code(2);
    assert_eq!(fs::read_to_string(dir.path().join("prog")).unwrap(), VALID);
}

#[test]
fn build_passes_flags_after_the_double_dash_to_clang() {
    if !have_cc() {
        return;
    }
    let dir = project("hello.ubl", VALID);
    ubel()
        .current_dir(dir.path())
        .args(["build", "hello.ubl", "--", "-this-flag-does-not-exist"])
        .assert()
        .code(1)
        .stderr(contains("this-flag-does-not-exist"));
}

#[test]
fn build_rejects_a_bad_optimisation_level_and_a_missing_file() {
    let dir = project("hello.ubl", VALID);
    // clap's own usage error: status 2
    ubel().current_dir(dir.path()).args(["build", "hello.ubl", "-O", "9"]).assert().code(2);
    // the shared "file not found" path
    ubel().current_dir(dir.path()).args(["build", "missing.ubl"]).assert().code(2).stderr(contains("file not found"));
}
