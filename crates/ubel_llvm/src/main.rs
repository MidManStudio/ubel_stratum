//! `ubel-llvm`: emit LLVM IR from Ubel source, build a native executable, or run the
//! interpreter for comparison.
//!
//!   ubel-llvm emit   <file.ubl> [-o out.ll]    textual LLVM IR (to stdout without -o)
//!   ubel-llvm interp <file.ubl>                run in the interpreter, print its output
//!   ubel-llvm check  <file.ubl>                front end only
//!   ubel-llvm build  <file.ubl> [options]      native executable (needs clang 15 or newer)
//!
//! Options of `build`:
//!   -o <path>        the executable (default: the file's name without extension, in the
//!                    current directory)
//!   --cc <path>      the C compiler (default: $UBEL_CC, else clang-18 .. clang-15, clang)
//!   -O0 -O1 -O2 -O3 -Os   optimisation level (default -O2)
//!   --keep-temps     keep the generated IR and the runtime files, and print where they are
//!   -v               print the compiler that was used and the full command line
//!   -- <flags>       everything after this goes to clang (for example -m32)
//!
//! Exit status 0 on success, 1 on any error (messages go to stderr), 2 on a usage error.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use ubel_llvm::driver::{self, BuildError, BuildOptions, OptLevel};

fn usage() -> ExitCode {
    eprintln!("usage: ubel-llvm emit <file.ubl> [-o out.ll] | interp <file.ubl> | check <file.ubl>");
    eprintln!("       ubel-llvm build <file.ubl> [-o out] [--cc PATH] [-O0|-O1|-O2|-O3|-Os] [--keep-temps] [-v] [-- clang flags]");
    ExitCode::from(2)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 2 {
        return usage();
    }
    let (cmd, path) = (args[0].as_str(), args[1].as_str());
    let source = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: cannot read {path}: {e}");
            return ExitCode::from(1);
        }
    };

    match cmd {
        "check" => match ubel_llvm::check(&source) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => fail(path, e),
        },
        "interp" => {
            let run = ubel_llvm::interpret(&source);
            print!("{}", run.output);
            match run.error {
                None => ExitCode::SUCCESS,
                Some(e) => fail(path, e),
            }
        }
        "emit" => {
            let out_path = match args.iter().position(|a| a == "-o") {
                Some(i) => match args.get(i + 1) {
                    Some(p) => Some(p.clone()),
                    None => return usage(),
                },
                None => None,
            };
            match ubel_llvm::compile(&source) {
                Ok(ir) => match out_path {
                    Some(p) => match std::fs::write(&p, ir) {
                        Ok(()) => ExitCode::SUCCESS,
                        Err(e) => {
                            eprintln!("error: cannot write {p}: {e}");
                            ExitCode::from(1)
                        }
                    },
                    None => {
                        print!("{ir}");
                        ExitCode::SUCCESS
                    }
                },
                Err(e) => fail(path, e),
            }
        }
        "build" => build(path, &source, &args[2..]),
        _ => usage(),
    }
}

/// The value after an option, moving the cursor past it.
fn value<'a>(rest: &'a [String], i: &mut usize) -> Option<&'a str> {
    *i += 1;
    rest.get(*i).map(|s| s.as_str())
}

fn build(path: &str, source: &str, rest: &[String]) -> ExitCode {
    let mut output: Option<PathBuf> = None;
    let mut opts = BuildOptions::new("");
    let mut verbose = false;

    let mut i = 0;
    while i < rest.len() {
        match rest[i].as_str() {
            "-o" => match value(rest, &mut i) {
                Some(v) => output = Some(PathBuf::from(v)),
                None => return usage(),
            },
            "--cc" => match value(rest, &mut i) {
                Some(v) => opts.cc = Some(PathBuf::from(v)),
                None => return usage(),
            },
            "--keep-temps" => opts.keep_temps = true,
            "-v" | "--verbose" => verbose = true,
            "--" => {
                opts.extra_flags = rest[i + 1..].to_vec();
                break;
            }
            flag if flag.len() == 3 && flag.starts_with("-O") => match OptLevel::parse(&flag[2..]) {
                Some(level) => opts.opt = level,
                None => {
                    eprintln!("error: unknown optimisation level {flag} (use -O0, -O1, -O2, -O3 or -Os)");
                    return ExitCode::from(2);
                }
            },
            other => {
                eprintln!("error: unknown option {other}");
                return usage();
            }
        }
        i += 1;
    }

    let output = output.unwrap_or_else(|| driver::default_output(Path::new(path)));
    // An executable written over the source would destroy the program being built.
    if let (Ok(a), Ok(b)) = (std::fs::canonicalize(&output), std::fs::canonicalize(path)) {
        if a == b {
            eprintln!("error: the output {} is the source file; choose another name with -o", output.display());
            return ExitCode::from(1);
        }
    }
    opts.output = output;

    match driver::build(source, &opts) {
        Ok(built) => {
            if verbose {
                eprintln!("compiler: {} ({})", built.compiler.path.display(), built.compiler.version.line);
                eprintln!("command:  {}", built.command);
                if !built.warnings.trim().is_empty() {
                    eprintln!("compiler output:\n{}", built.warnings.trim_end());
                }
            }
            if let Some(dir) = &built.kept_dir {
                eprintln!("kept: {}", dir.display());
            }
            eprintln!("built {}", built.output.display());
            ExitCode::SUCCESS
        }
        Err(BuildError::Compile(e)) => fail(path, e),
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::from(1)
        }
    }
}

fn fail(path: &str, e: ubel_llvm::CompileError) -> ExitCode {
    eprintln!("{}", e.render(path));
    ExitCode::from(1)
}
