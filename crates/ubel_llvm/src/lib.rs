//! Stage 1B of the Ubel LLVM proof of concept: emit textual LLVM IR from Ubel source.
//!
//! The front end is the real one (lexer, recursive-descent parser, semantic analysis),
//! so a program that sema rejects never reaches the emitter. The emitter then handles a
//! deliberately small subset and refuses everything else with a located error rather than
//! guessing. See `docs/LLVM_POC.md` for the subset, the tier lowering rules, and how it
//! is checked against the interpreter.

mod emit;

pub use emit::CompileError;

use ubel_stratum::ast::arena::AstArena;
use ubel_stratum::error_management::{Diagnosable, Diagnostic};

fn fmt_diag(d: &Diagnostic) -> String {
    format!("{}:{}: {}", d.primary_span.line, d.primary_span.column, d.message)
}

/// Everything the front end produces, kept alive together.
struct FrontEnd<'a> {
    program: ubel_stratum::ast::Program<'a>,
    ctx: ubel_stratum::sema::SemaContext,
}

fn front_end<'a>(arena: &'a AstArena, source: &str) -> Result<FrontEnd<'a>, CompileError> {
    let tokens = match ubel_stratum::lexer::tokenize(source) {
        Ok(t) => t,
        Err(mut errs) => {
            let msgs: Vec<String> = errs
                .take_lexical_errors()
                .iter()
                .map(|e| fmt_diag(&e.to_diagnostic()))
                .collect();
            return Err(CompileError::stage("lex", msgs));
        }
    };

    let program = match ubel_stratum_rd::parse(arena, &tokens, source.to_string()) {
        Ok(p) => p,
        Err(mut errs) => {
            let msgs: Vec<String> = errs
                .take_parse_errors()
                .iter()
                .map(|e| fmt_diag(&e.to_diagnostic()))
                .collect();
            return Err(CompileError::stage("parse", msgs));
        }
    };

    let ctx = match ubel_stratum::sema::analyse(&program, arena, source.to_string()) {
        Ok(c) => c,
        Err(mut errs) => {
            let mut msgs: Vec<String> = Vec::new();
            msgs.extend(errs.take_name_errors().iter().map(|e| fmt_diag(&e.to_diagnostic())));
            msgs.extend(errs.take_type_errors().iter().map(|e| fmt_diag(&e.to_diagnostic())));
            msgs.extend(errs.take_tier_errors().iter().map(|e| fmt_diag(&e.to_diagnostic())));
            msgs.extend(errs.take_borrow_errors().iter().map(|e| fmt_diag(&e.to_diagnostic())));
            msgs.extend(errs.take_move_errors().iter().map(|e| fmt_diag(&e.to_diagnostic())));
            return Err(CompileError::stage("sema", msgs));
        }
    };

    Ok(FrontEnd { program, ctx })
}

/// Run the front end only. Ok means the program parses and passes semantic analysis.
pub fn check(source: &str) -> Result<(), CompileError> {
    let arena = AstArena::new();
    front_end(&arena, source).map(|_| ())
}

/// Compile Ubel source to textual LLVM IR.
pub fn compile(source: &str) -> Result<String, CompileError> {
    let arena = AstArena::new();
    let fe = front_end(&arena, source)?;
    emit::emit_program(&fe.program, &fe.ctx)
}

/// What the interpreter did: everything it printed, and the error if it failed.
/// Output is returned even on failure so a failing native build can be compared too.
pub struct Interpreted {
    pub output: String,
    pub error: Option<CompileError>,
}

/// Run the program in the tree-walking interpreter. This is the reference the native
/// build is compared against.
pub fn interpret(source: &str) -> Interpreted {
    let arena = AstArena::new();
    let fe = match front_end(&arena, source) {
        Ok(fe) => fe,
        Err(e) => return Interpreted { output: String::new(), error: Some(e) },
    };

    ubel_stratum::builtins::global::io::start_output_capture();
    let mut interp = ubel_stratum::interpreter::Interpreter::new(&arena);
    interp.set_int_literal_types(fe.ctx.int_literal_types.clone());
    interp.set_trait_call_sites(fe.ctx.trait_call_sites.clone());
    let result = interp.run_program(&fe.program);
    let output = ubel_stratum::builtins::global::io::take_captured_output();

    match result {
        Ok(()) => Interpreted { output, error: None },
        Err(e) => Interpreted {
            output,
            error: Some(CompileError::stage("interpret", vec![format!("{e}")])),
        },
    }
}
