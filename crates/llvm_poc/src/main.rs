//! `ubel-llvm`: emit LLVM IR from Ubel source, or run the interpreter for comparison.
//!
//!   ubel-llvm emit   <file.ubl> [-o out.ll]    textual LLVM IR (to stdout without -o)
//!   ubel-llvm interp <file.ubl>                run in the interpreter, print its output
//!   ubel-llvm check  <file.ubl>                front end only
//!
//! Exit status 0 on success, 1 on any error (messages go to stderr).

use std::process::ExitCode;

fn usage() -> ExitCode {
    eprintln!("usage: ubel-llvm emit <file.ubl> [-o out.ll] | interp <file.ubl> | check <file.ubl>");
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
        _ => usage(),
    }
}

fn fail(path: &str, e: ubel_llvm::CompileError) -> ExitCode {
    eprintln!("{}", e.render(path));
    ExitCode::from(1)
}
