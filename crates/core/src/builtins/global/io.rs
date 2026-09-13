// crates/core/src/builtins/global/io.rs
//! Global output functions: `println`, `print`, `log`.

use std::cell::RefCell;
use crate::interpreter::value::{EvalResult, Value};

// ── Output capture ───────────────────────────────────────────────
//
// `println`/`print`/`log` write directly to the real process stdout/stderr
// by default, which is correct for the CLI and every existing fixture/test.
// Embedding contexts with no real stdout (the wasm playground) need that
// output redirected into an in-memory buffer instead. `BuiltinFn` is a bare
// `fn` pointer, not a closure (see `builtins::BuiltinFn`), so the redirect
// can't be captured per-call; a thread-local buffer is the least invasive
// option and is correct for both native use and wasm32 (single-threaded).
// Capture is off by default (`None`), so behaviour is unchanged unless a
// caller opts in.
thread_local! {
    static OUTPUT_CAPTURE: RefCell<Option<String>> = RefCell::new(None);
}

/// Starts capturing `println`/`print`/`log` output into an in-memory buffer
/// instead of the real stdout/stderr. Intended for embedding contexts
/// (the wasm playground) that have no real stdout to write to.
pub fn start_output_capture() {
    OUTPUT_CAPTURE.with(|c| *c.borrow_mut() = Some(String::new()));
}

/// Stops capturing and returns everything captured since the matching
/// `start_output_capture()` call. Returns an empty string if capture was
/// never started. `println` and `log` output are interleaved in arrival
/// order into the one buffer; they are not kept as separate streams.
pub fn take_captured_output() -> String {
    OUTPUT_CAPTURE.with(|c| c.borrow_mut().take().unwrap_or_default())
}

/// Appends `s` to the capture buffer if one is active. Returns `true` if
/// it was captured (caller must not also write to a real stream), `false`
/// if capture is off (caller falls back to its normal native stream).
fn try_capture(s: &str) -> bool {
    OUTPUT_CAPTURE.with(|c| {
        let mut slot = c.borrow_mut();
        match slot.as_mut() {
            Some(buf) => {
                buf.push_str(s);
                true
            }
            None => false,
        }
    })
}

pub fn println(args: &[Value]) -> EvalResult {
    let out: Vec<String> = args.iter().map(|v| v.to_string()).collect();
    let line = format!("{}\n", out.join(" "));
    if !try_capture(&line) {
        print!("{line}");
    }
    Ok(Value::Void)
}

pub fn print(args: &[Value]) -> EvalResult {
    let out: Vec<String> = args.iter().map(|v| v.to_string()).collect();
    let text = out.join(" ");
    if !try_capture(&text) {
        print!("{text}");
    }
    Ok(Value::Void)
}

pub fn log(args: &[Value]) -> EvalResult {
    let out: Vec<String> = args.iter().map(|v| v.to_string()).collect();
    let line = format!("[log] {}\n", out.join(" "));
    // Same capture buffer as println/print (interleaved, not a separate
    // stream — see take_captured_output); falls back to real stderr,
    // matching the original eprintln!-based behaviour, when not capturing.
    if !try_capture(&line) {
        eprint!("{line}");
    }
    Ok(Value::Void)
}

#[cfg(test)]
mod capture_tests {
    use super::*;

    // OUTPUT_CAPTURE is thread-local, but cargo runs tests on multiple
    // threads by default; a lock keeps these tests from interleaving
    // across threads and racing on capture state within one thread.
    static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn capture_off_by_default() {
        let _guard = TEST_LOCK.lock().unwrap();
        assert_eq!(take_captured_output(), "");
    }

    #[test]
    fn println_is_captured_when_active() {
        let _guard = TEST_LOCK.lock().unwrap();
        start_output_capture();
        println(&[Value::Str(std::rc::Rc::new("hi".to_string()))]).unwrap();
        assert_eq!(take_captured_output(), "hi\n");
    }

    #[test]
    fn print_and_log_interleave_into_one_buffer() {
        let _guard = TEST_LOCK.lock().unwrap();
        start_output_capture();
        print(&[Value::Str(std::rc::Rc::new("a".to_string()))]).unwrap();
        log(&[Value::Str(std::rc::Rc::new("b".to_string()))]).unwrap();
        assert_eq!(take_captured_output(), "a[log] b\n");
    }

    #[test]
    fn take_clears_the_buffer() {
        let _guard = TEST_LOCK.lock().unwrap();
        start_output_capture();
        println(&[Value::Str(std::rc::Rc::new("once".to_string()))]).unwrap();
        assert_eq!(take_captured_output(), "once\n");
        // capture was consumed by take(); a second take with no new
        // start_output_capture() call returns empty, not "once" again.
        assert_eq!(take_captured_output(), "");
    }
}
