// ============================================================================
// NOTICE: Full documentation, design decisions, and fix history for this file
// live in docs/ubel_stratum_cli.md, section "commands/run.rs"
// ============================================================================
// crates/cli/src/commands/run.rs
//! `ubel run <file>`.

use std::path::PathBuf;

use clap::Args;

use crate::commands::{check, handle_error, GlobalOpts, EXIT_DIAGNOSTICS, EXIT_OK, EXIT_USAGE};
use crate::output::printer;
use crate::services::pipeline::{self, RunOutcome};

/// The environment variable that sets the worker thread count when `--workers` is not given.
pub const WORKERS_ENV: &str = "UBEL_WORKERS";

#[derive(Args)]
pub struct RunArgs {
    /// Path to the .ubl file
    pub file: PathBuf,

    /// Worker threads for tasks started with `Task.spawn` (default: logical cores minus one, at
    /// least one; `UBEL_WORKERS` sets it when this flag is absent; 0 runs with no workers)
    #[arg(long, value_name = "N")]
    pub workers: Option<usize>,
}

/// The worker count a run uses: the flag if there is one, else the value of `UBEL_WORKERS`, else
/// `None` (the runtime picks). The flag wins even when the variable is not a number. An unset or
/// blank variable is the same as no variable; a number may have spaces around it; zero is a count.
pub fn resolve_workers(flag: Option<usize>, env: Option<&str>) -> Result<Option<usize>, String> {
    if flag.is_some() {
        return Ok(flag);
    }
    let Some(raw) = env else { return Ok(None) };
    let value = raw.trim();
    if value.is_empty() {
        return Ok(None);
    }
    value.parse::<usize>().map(Some).map_err(|_| {
        format!("{WORKERS_ENV} must be a whole number of worker threads, found `{value}`")
    })
}

fn read_workers_env() -> Result<Option<String>, String> {
    match std::env::var(WORKERS_ENV) {
        Ok(v)                                   => Ok(Some(v)),
        Err(std::env::VarError::NotPresent)     => Ok(None),
        Err(std::env::VarError::NotUnicode(_))  => Err(format!("{WORKERS_ENV} is not valid text")),
    }
}

pub fn run(args: RunArgs, global: &GlobalOpts) -> i32 {
    let file = args.file.display().to_string();
    let workers = match read_workers_env().and_then(|env| resolve_workers(args.workers, env.as_deref())) {
        Ok(n)    => n,
        Err(msg) => {
            printer::error(&msg);
            return EXIT_USAGE;
        }
    };
    match pipeline::run_file_with(&args.file, workers) {
        Ok(RunOutcome::Finished)          => EXIT_OK,
        Ok(RunOutcome::Diagnostics(out))  => check::report(&file, &out, global),
        Ok(RunOutcome::RuntimeError(msg)) => {
            printer::error(&format!("runtime error: {msg}"));
            EXIT_DIAGNOSTICS
        }
        Err(e) => handle_error(&e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_set_leaves_the_count_to_the_runtime() {
        assert_eq!(resolve_workers(None, None), Ok(None));
    }

    #[test]
    fn the_flag_is_used_when_there_is_no_variable() {
        assert_eq!(resolve_workers(Some(3), None), Ok(Some(3)));
    }

    #[test]
    fn the_variable_is_used_when_there_is_no_flag() {
        assert_eq!(resolve_workers(None, Some("2")), Ok(Some(2)));
    }

    #[test]
    fn the_flag_wins_even_when_the_variable_is_not_a_number() {
        assert_eq!(resolve_workers(Some(1), Some("many")), Ok(Some(1)));
        assert_eq!(resolve_workers(Some(0), Some("4")), Ok(Some(0)));
    }

    #[test]
    fn an_empty_or_blank_variable_is_unset() {
        assert_eq!(resolve_workers(None, Some("")), Ok(None));
        assert_eq!(resolve_workers(None, Some("   ")), Ok(None));
    }

    #[test]
    fn the_value_is_trimmed_and_zero_is_a_count() {
        assert_eq!(resolve_workers(None, Some(" 5 ")), Ok(Some(5)));
        assert_eq!(resolve_workers(None, Some("0")), Ok(Some(0)));
    }

    #[test]
    fn a_value_that_is_not_a_number_names_the_variable_and_the_value() {
        for bad in ["many", "-1", "2.5", "4 threads"] {
            let err = resolve_workers(None, Some(bad)).unwrap_err();
            assert!(err.contains("UBEL_WORKERS"), "{err}");
            assert!(err.contains(bad.trim()), "{err}");
        }
    }
}
