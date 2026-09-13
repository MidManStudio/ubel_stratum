// crates/wasm_playground/src/lib.rs
//! wasm bindings for the Ubel Stratum browser playground.
//!
//! Wraps the same tokenize -> parse -> sema -> interpret pipeline as
//! `ubel_stratum_rd`'s `examples/diagnose.rs`, returning one JSON string
//! instead of printing to a terminal — there is no real stdout in a
//! browser. `run_pipeline` is the only export the playground page calls.
//!
//! Program output (`println`/`print`/`log`) is captured via
//! `ubel_stratum::builtins::global::io::{start_output_capture,
//! take_captured_output}` rather than the real process stdout, which
//! wasm32-unknown-unknown has no working implementation of.

use serde::Serialize;
use ubel_stratum::ast::arena::AstArena;
use ubel_stratum::error_management::{Diagnosable, Diagnostic};
use wasm_bindgen::prelude::*;

/// One diagnostic, flattened to plain JSON-friendly fields. Mirrors
/// `error_management::Diagnostic` — see that type for field meanings.
#[derive(Serialize)]
struct DiagnosticDto {
    code:       String,
    message:    String,
    line:       usize,
    column:     usize,
    start:      usize,
    end:        usize,
    label:      Option<String>,
    suggestion: Option<String>,
}

impl From<&Diagnostic> for DiagnosticDto {
    fn from(d: &Diagnostic) -> Self {
        DiagnosticDto {
            code:       d.code.to_string(),
            message:    d.message.clone(),
            line:       d.primary_span.line,
            column:     d.primary_span.column,
            start:      d.primary_span.start,
            end:        d.primary_span.end,
            label:      d.primary_label.clone(),
            suggestion: d.suggestion.clone(),
        }
    }
}

/// The stage the pipeline stopped at. Later stages are only reached once
/// every earlier one succeeds — see `run_pipeline`.
#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum Stage {
    Lex,
    Parse,
    Sema,
    Interpret,
}

/// Full result of one `run_pipeline` call. Serialized to JSON for the
/// playground page to parse and render — see `web/playground`.
#[derive(Serialize)]
struct PipelineResult {
    /// Furthest stage the pipeline reached.
    stage:          Stage,
    /// Whether that furthest stage itself succeeded. `false` means
    /// `diagnostics` explains why; `true` at stage `Interpret` means the
    /// program actually ran to completion (or was skipped — see
    /// `ran_main`, not this field, for that distinction).
    ok:             bool,
    token_count:    Option<usize>,
    item_count:     Option<usize>,
    /// `false` when sema succeeded but no `fn main(` was found, so the
    /// interpreter was never invoked — same heuristic `diagnose.rs` uses.
    ran_main:       bool,
    diagnostics:    Vec<DiagnosticDto>,
    /// Rustc-style plain-text rendering of `diagnostics` against the
    /// source, ready to drop into a `<pre>` — the same renderer the CLI
    /// tools use, so playground output matches `mdix`/`stratc` output.
    diagnostics_text: String,
    /// Captured `println`/`print`/`log` output from the interpreted
    /// program, if it ran. `None` if the interpreter was never reached.
    program_output: Option<String>,
    /// Set when `run_program` returns `Err` — a language-level runtime
    /// error (panic/failed assertion), not a compiler bug.
    runtime_error:  Option<String>,
}

/// Runs one `.ubl` source string through tokenize -> parse -> sema ->
/// interpret and returns a JSON-encoded `PipelineResult` (see above).
///
/// Never panics on malformed input by design — every stage's error path
/// returns diagnostics instead of unwrapping. A genuine Rust panic (a
/// compiler bug, not a language error) still propagates as a wasm trap;
/// the playground page wraps this call in try/catch and shows a distinct
/// "internal error" message for that case, since it means something in
/// the pipeline itself broke, not the user's program.
#[wasm_bindgen]
pub fn run_pipeline(source: &str) -> String {
    let source = source.to_string();

    // ── Stage 1: lex ────────────────────────────────────────────────
    let tokens = match ubel_stratum::lexer::tokenize(&source) {
        Ok(t) => t,
        Err(mut errs) => {
            let diags: Vec<Diagnostic> = errs
                .take_lexical_errors()
                .iter()
                .map(|e| e.to_diagnostic())
                .collect();
            return finish(Stage::Lex, false, None, None, false, &diags, &source, None, None);
        }
    };
    let token_count = tokens.len();

    // ── Stage 2: parse ────────────────────────────────────────────────
    let arena = AstArena::new();
    let program = match ubel_stratum_rd::parse(&arena, &tokens, source.clone()) {
        Ok(p) => p,
        Err(mut errs) => {
            let diags: Vec<Diagnostic> = errs
                .take_parse_errors()
                .iter()
                .map(|e| e.to_diagnostic())
                .collect();
            return finish(
                Stage::Parse, false, Some(token_count), None, false, &diags, &source, None, None,
            );
        }
    };
    let item_count = program.items.len();

    // ── Stage 3: sema ───────────────────────────────────────────────
    let sema_result = ubel_stratum::sema::analyse(&program, &arena, source.clone());
    if let Err(mut errs) = sema_result {
        let mut diags: Vec<Diagnostic> = errs
            .take_name_errors()
            .iter()
            .map(|e| e.to_diagnostic())
            .collect();
        diags.extend(errs.take_type_errors().iter().map(|e| e.to_diagnostic()));
        diags.extend(errs.take_tier_errors().iter().map(|e| e.to_diagnostic()));
        diags.extend(errs.take_borrow_errors().iter().map(|e| e.to_diagnostic()));
        diags.extend(errs.take_move_errors().iter().map(|e| e.to_diagnostic()));
        return finish(
            Stage::Sema,
            false,
            Some(token_count),
            Some(item_count),
            false,
            &diags,
            &source,
            None,
            None,
        );
    }

    // ── Stage 4: interpret ────────────────────────────────────────────
    // Same "does it declare a main function" heuristic as diagnose.rs —
    // a file that only declares types/functions with no entry point is a
    // valid, successfully-checked program, just not one that runs.
    if !source.contains("fn main(") {
        return finish(
            Stage::Interpret,
            true,
            Some(token_count),
            Some(item_count),
            false,
            &[],
            &source,
            None,
            None,
        );
    }

    ubel_stratum::builtins::global::io::start_output_capture();
    let mut interp = ubel_stratum::interpreter::Interpreter::new(&arena);
    let run_result = interp.run_program(&program);
    let output = ubel_stratum::builtins::global::io::take_captured_output();

    let (ok, runtime_error) = match run_result {
        Ok(()) => (true, None),
        Err(e) => (false, Some(e)),
    };

    finish(
        Stage::Interpret,
        ok,
        Some(token_count),
        Some(item_count),
        true,
        &[],
        &source,
        Some(output),
        runtime_error,
    )
}

#[allow(clippy::too_many_arguments)]
fn finish(
    stage: Stage,
    ok: bool,
    token_count: Option<usize>,
    item_count: Option<usize>,
    ran_main: bool,
    diags: &[Diagnostic],
    source: &str,
    program_output: Option<String>,
    runtime_error: Option<String>,
) -> String {
    let diagnostics_text = ubel_stratum::error_management::render_all(diags, source);
    let result = PipelineResult {
        stage,
        ok,
        token_count,
        item_count,
        ran_main,
        diagnostics: diags.iter().map(DiagnosticDto::from).collect(),
        diagnostics_text,
        program_output,
        runtime_error,
    };
    serde_json::to_string(&result).unwrap_or_else(|e| {
        format!(r#"{{"stage":"lex","ok":false,"ran_main":false,"diagnostics":[],"diagnostics_text":"internal error: failed to serialize pipeline result: {e}","program_output":null,"runtime_error":null}}"#)
    })
}

/// Installs the `console_error_panic_hook` so a Rust panic shows up as a
/// readable message in the browser console instead of an opaque
/// "unreachable executed" wasm trap. Call once, on page load, before the
/// first `run_pipeline` call.
#[wasm_bindgen]
pub fn init_panic_hook() {
    console_error_panic_hook::set_once();
}
