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
fn quiet_suppresses_success_output() {
    let f = ubl(VALID);
    ubel().arg("check").arg(f.path()).arg("--quiet").assert().success().stdout("");
}
