// ============================================================================
// NOTICE: Full documentation, design decisions, and fix history for this file
// live in docs/ubel_stratum_cli.md, section "tests/cli_tests.rs"
// ============================================================================
// crates/cli/tests/cli_tests.rs
//! End-to-end tests: spawn the real `ubel` binary against temp files.

use assert_cmd::Command;
use predicates::str::contains;
use std::io::Write;
use tempfile::NamedTempFile;

fn ubl(source: &str) -> NamedTempFile {
    let mut f = tempfile::Builder::new().suffix(".ubl").tempfile().unwrap();
    f.write_all(source.as_bytes()).unwrap();
    f
}

fn ubel() -> Command {
    Command::cargo_bin("ubel").unwrap()
}

const VALID: &str = "fn main() void { println(\"hello from ubel\") }\n";
const TYPE_ERROR: &str = "fn main() void { let x: int = \"a\" }\n";

#[test]
fn check_accepts_a_valid_file() {
    let f = ubl(VALID);
    ubel().arg("check").arg(f.path()).assert().success().stdout(contains("is valid"));
}

#[test]
fn check_rejects_a_type_error_with_its_code() {
    let f = ubl(TYPE_ERROR);
    ubel().arg("check").arg(f.path()).assert().code(1).stderr(contains("TYPE-101"));
}

#[test]
fn check_json_reports_stage_and_code() {
    let f = ubl(TYPE_ERROR);
    ubel()
        .arg("check").arg(f.path()).arg("--json")
        .assert().code(1)
        .stdout(contains("\"stage\": \"sema\""))
        .stdout(contains("\"code\": \"TYPE-101\""));
}

#[test]
fn check_reports_a_parse_error() {
    let f = ubl("fn main( void {\n");
    ubel().arg("check").arg(f.path()).assert().code(1).stderr(contains("stopped at parse"));
}

#[test]
fn run_executes_main() {
    let f = ubl(VALID);
    ubel().arg("run").arg(f.path()).assert().success().stdout(contains("hello from ubel"));
}

#[test]
fn run_refuses_a_file_that_fails_the_check() {
    let f = ubl(TYPE_ERROR);
    ubel().arg("run").arg(f.path()).assert().code(1).stderr(contains("TYPE-101"));
}

#[test]
fn run_reports_a_runtime_error() {
    let f = ubl("fn main() void {\n let a = 1\n let b = 0\n println($\"{a / b}\")\n}\n");
    ubel().arg("run").arg(f.path()).assert().code(1).stderr(contains("runtime error"));
}

#[test]
fn a_missing_file_exits_two() {
    ubel().arg("check").arg("no_such_file.ubl").assert().code(2).stderr(contains("file not found"));
}

#[test]
fn debug_tokens_lists_tokens() {
    let f = ubl(VALID);
    ubel().arg("debug-tokens").arg(f.path()).assert().success().stdout(contains("[1:1]"));
}

#[test]
fn run_gives_an_unsuffixed_literal_the_type_its_context_picks() {
    // `250` is a u8 because of the annotation, so the sum wraps at 8 bits.
    // This only prints 4 when `run` hands the interpreter the literal
    // widths that sema resolved.
    let f = ubl("fn main() void {\n let a: u8 = 250\n let w = a + 10\n println(w)\n}\n");
    ubel().arg("run").arg(f.path()).assert().success().stdout(contains("4\n"));
}

#[test]
fn check_rejects_a_literal_that_does_not_fit_its_type() {
    let f = ubl("fn main() void {\n let bad: u8 = 300\n}\n");
    ubel().arg("check").arg(f.path()).assert().code(1).stderr(contains("TYPE-120"));
}

#[test]
fn run_shares_one_static_between_functions() {
    // The static is bumped inside `bump`, a different call from the one
    // that reads it. It only prints 2 when the interpreter keeps statics
    // in a store shared across calls.
    let f = ubl("static N: int = 0\nfn bump() void { N += 1 }\n\
                 fn main() void {\n bump()\n bump()\n println(N)\n}\n");
    ubel().arg("run").arg(f.path()).assert().success().stdout(contains("2\n"));
}

#[test]
fn check_rejects_a_static_used_from_outside_the_high_tier() {
    let f = ubl("static N: int = 0\n@tier(low)\nfn f() int { return N }\nfn main() void { println(f()) }\n");
    ubel().arg("check").arg(f.path()).assert().code(1).stderr(contains("TIER-015"));
}

#[test]
fn quiet_suppresses_success_output() {
    let f = ubl(VALID);
    ubel().arg("check").arg(f.path()).arg("--quiet").assert().success().stdout("");
}

// ── Worker threads: --workers and UBEL_WORKERS ───────────────────

const SPAWNS: &str = "async fn work(n: int) Task<int> {\n    return n * 2\n}\n\n\
                      fn main() void {\n    let t = Task.spawn(work(21))\n    println(t.join())\n}\n";

/// `ubel run FILE`, with `UBEL_WORKERS` removed unless a test sets it, so that the variable of the
/// machine the tests run on cannot change what they see.
fn run_spawns() -> (NamedTempFile, Command) {
    let f = ubl(SPAWNS);
    let mut cmd = ubel();
    cmd.env_remove("UBEL_WORKERS").arg("run").arg(f.path());
    (f, cmd)
}

#[test]
fn run_starts_workers_for_a_spawning_program_without_any_setting() {
    let (_f, mut cmd) = run_spawns();
    cmd.assert().success().stdout(contains("42\n"));
}

#[test]
fn run_takes_a_worker_count_from_the_flag() {
    let (_f, mut cmd) = run_spawns();
    cmd.args(["--workers", "3"]).assert().success().stdout(contains("42\n"));
}

#[test]
fn run_takes_a_worker_count_from_the_environment() {
    let (_f, mut cmd) = run_spawns();
    cmd.env("UBEL_WORKERS", "2").assert().success().stdout(contains("42\n"));
}

#[test]
fn zero_workers_leaves_a_spawning_program_with_nothing_to_run_its_tasks() {
    let (_f, mut cmd) = run_spawns();
    cmd.args(["--workers", "0"]).assert().code(1).stderr(contains("no worker threads"));
}

#[test]
fn the_flag_beats_the_environment_in_both_directions() {
    let (_f, mut cmd) = run_spawns();
    cmd.env("UBEL_WORKERS", "2").args(["--workers", "0"]).assert().code(1).stderr(contains("no worker threads"));

    let (_g, mut cmd) = run_spawns();
    cmd.env("UBEL_WORKERS", "0").args(["--workers", "2"]).assert().success().stdout(contains("42\n"));
}

#[test]
fn a_worker_count_that_is_not_a_number_is_a_usage_error() {
    let (_f, mut cmd) = run_spawns();
    cmd.env("UBEL_WORKERS", "many")
        .assert().code(2)
        .stderr(contains("UBEL_WORKERS"))
        .stderr(contains("many"));

    let (_g, mut cmd) = run_spawns();
    cmd.args(["--workers", "many"]).assert().code(2);
}

#[cfg(unix)]
#[test]
fn a_worker_count_that_is_not_text_is_a_usage_error() {
    use std::os::unix::ffi::OsStrExt;
    let (_f, mut cmd) = run_spawns();
    cmd.env("UBEL_WORKERS", std::ffi::OsStr::from_bytes(b"\xff\xfe"))
        .assert().code(2)
        .stderr(contains("UBEL_WORKERS"));
}

#[test]
fn a_program_that_never_spawns_ignores_a_worker_count_of_zero() {
    let f = ubl(VALID);
    ubel()
        .env_remove("UBEL_WORKERS")
        .arg("run").arg(f.path()).args(["--workers", "0"])
        .assert().success().stdout(contains("hello from ubel"));
}
