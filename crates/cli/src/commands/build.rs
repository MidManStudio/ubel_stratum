// ============================================================================
// NOTICE: Full documentation, design decisions, and fix history for this file
// live in docs/ubel_stratum_cli.md, section "commands/build.rs"
// ============================================================================
// crates/cli/src/commands/build.rs
//! `ubel build <file>`: Ubel source to a native executable through the LLVM backend (`ubel_llvm`).
//!
//! The file is first checked exactly as `ubel check` checks it, so the diagnostics look the same.
//! Then `ubel_llvm::driver::build` emits LLVM IR, writes it and the embedded C runtime to a private
//! temporary directory, and one `clang` invocation (version 15 or newer) assembles and links the
//! executable. The backend handles a subset of the language; what it cannot do yet is refused with a
//! located "not supported yet" message and exit code 1.

use std::path::{Path, PathBuf};
use std::time::Instant;

use clap::Args;
use serde::Serialize;
use ubel_llvm::driver::{self, BuildError, BuildOptions, OptLevel};

use crate::commands::{check, handle_error, GlobalOpts, EXIT_DIAGNOSTICS, EXIT_OK, EXIT_USAGE};
use crate::output::{json_output, printer};
use crate::services::pipeline;

#[derive(Args)]
pub struct BuildArgs {
    /// Path to the .ubl file
    pub file: PathBuf,
    /// Where to write the executable (default: the file name without its extension, in the current directory)
    #[arg(short = 'o', long, value_name = "PATH")]
    pub output: Option<PathBuf>,
    /// The C compiler (default: $UBEL_CC, else clang-18 down to clang-15, else clang)
    #[arg(long, value_name = "PATH")]
    pub cc: Option<PathBuf>,
    /// Optimisation level passed to clang
    #[arg(short = 'O', long = "opt-level", value_name = "LEVEL", default_value = "2",
          value_parser = ["0", "1", "2", "3", "s"])]
    pub opt_level: String,
    /// Keep the generated IR and the runtime files, and print where they are
    #[arg(long)]
    pub keep_temps: bool,
    /// Extra arguments for clang, after `--` (for example `-- -m32`)
    #[arg(last = true, value_name = "CLANG_ARGS")]
    pub clang_args: Vec<String>,
}

/// The `--json` object of `ubel build`.
#[derive(Serialize)]
struct BuildJson {
    file:       String,
    ok:         bool,
    /// `build` when it worked or the compiler failed, `emit` when the backend refused the program.
    stage:      &'static str,
    output:     Option<String>,
    compiler:   Option<String>,
    command:    Option<String>,
    error:      Option<String>,
    elapsed_ms: f64,
}

pub fn run(args: BuildArgs, global: &GlobalOpts) -> i32 {
    let file = args.file.display().to_string();
    let started = Instant::now();

    // The same check `ubel check` and `ubel run` make, reported the same way.
    let outcome = match pipeline::check_file(&args.file) {
        Ok(o)  => o,
        Err(e) => return handle_error(&e),
    };
    if !outcome.report.is_clean() {
        return check::report(&file, &outcome, global);
    }

    let output = args.output.clone().unwrap_or_else(|| driver::default_output(&args.file));
    // An executable written over the source would destroy the program that is being built.
    if is_same_file(&output, &args.file) {
        return fail(
            global, &file, started, "build", EXIT_USAGE,
            format!("the output {} is the source file; choose another name with -o", output.display()),
        );
    }

    let mut opts = BuildOptions::new(&output);
    opts.cc          = args.cc;
    opts.opt         = OptLevel::parse(&args.opt_level).unwrap_or_default();
    opts.keep_temps  = args.keep_temps;
    opts.extra_flags = args.clang_args;

    match driver::build(&outcome.source, &opts) {
        Ok(built) => {
            if global.json {
                json_output::print_result(BuildJson {
                    file,
                    ok:         true,
                    stage:      "build",
                    output:     Some(built.output.display().to_string()),
                    compiler:   Some(built.compiler.path.display().to_string()),
                    command:    Some(built.command),
                    error:      None,
                    elapsed_ms: started.elapsed().as_secs_f64() * 1000.0,
                });
                return EXIT_OK;
            }
            if let Some(dir) = &built.kept_dir {
                printer::kv("kept", &dir.display().to_string());
            }
            if !global.quiet {
                printer::success(&format!("built {}", built.output.display()));
                if global.verbose {
                    printer::kv("compiler", &format!("{} ({})", built.compiler.path.display(), built.compiler.version.line));
                    printer::kv("command", &built.command);
                    if !built.warnings.trim().is_empty() {
                        eprintln!("compiler output:\n{}", built.warnings.trim_end());
                    }
                    printer::duration(started.elapsed());
                }
            }
            EXIT_OK
        }
        // The program uses something the backend does not do yet: a located message, like a diagnostic.
        Err(BuildError::Compile(e)) => {
            if !global.json {
                eprintln!("{}", e.render(&file));
            }
            fail(global, &file, started, "emit", EXIT_DIAGNOSTICS,
                 format!("the LLVM backend cannot build {file} yet (stopped at {})", e.stage))
        }
        // clang ran and failed: the build failed, with clang's message.
        Err(e @ BuildError::CompilerFailed { .. }) => {
            fail(global, &file, started, "build", EXIT_DIAGNOSTICS, e.to_string())
        }
        // No usable compiler, an output directory that is not there, an io error: not the program's fault.
        Err(e) => fail(global, &file, started, "build", EXIT_USAGE, e.to_string()),
    }
}

/// Report a failure in the requested format and return its exit code.
fn fail(global: &GlobalOpts, file: &str, started: Instant, stage: &'static str, code: i32, message: String) -> i32 {
    if global.json {
        json_output::print_result(BuildJson {
            file:       file.to_string(),
            ok:         false,
            stage,
            output:     None,
            compiler:   None,
            command:    None,
            error:      Some(message),
            elapsed_ms: started.elapsed().as_secs_f64() * 1000.0,
        });
    } else {
        printer::error(&message);
    }
    code
}

fn is_same_file(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
}
