// src/stack.rs
//! A large stack for the compiler and the interpreter.
//!
//! Every pass walks the tree recursively, and type inference alone needs
//! several KB of stack per level of expression nesting (about 15 KB in a debug
//! build). The parser bounds the depth (`PARSE-006`, `MAX_NESTING` in
//! `ubel_stratum_rd`), and the entry points run the whole pipeline on a thread
//! with a stack sized for that bound, because the default stack of a main
//! thread (8 MB) or of an ordinary spawned thread (2 MB) is not.
//!
//! The memory is reserved, not used: the stack is mapped lazily and only the
//! pages a program actually reaches are ever committed. A browser build has no
//! threads; its stack size is set at link time (`crates/wasm_playground/build.rs`).

/// The stack of the compiler thread. Sized for `MAX_NESTING` levels of nesting
/// through the largest debug-build frames of every pass, with a wide margin.
pub const COMPILER_STACK_BYTES: usize = 256 * 1024 * 1024;

/// Runs `f` on a thread with a [`COMPILER_STACK_BYTES`] stack and returns its
/// result. A panic in `f` is re-raised on the caller. If the thread cannot be
/// started, `f` runs on the current thread instead.
#[cfg(not(target_arch = "wasm32"))]
pub fn with_big_stack<R: Send, F: FnOnce() -> R + Send>(f: F) -> R {
    use std::sync::Mutex;
    let cell = Mutex::new(Some(f));
    let take = || (cell.lock().unwrap().take().expect("compiler task already taken"))();
    std::thread::scope(|scope| {
        let spawned = std::thread::Builder::new()
            .name("ubel-compiler".to_string())
            .stack_size(COMPILER_STACK_BYTES)
            .spawn_scoped(scope, take);
        match spawned {
            Ok(handle) => match handle.join() {
                Ok(result)   => result,
                Err(payload) => std::panic::resume_unwind(payload),
            },
            Err(_) => take(),
        }
    })
}

/// A browser build runs on the one thread it has.
#[cfg(target_arch = "wasm32")]
pub fn with_big_stack<R, F: FnOnce() -> R>(f: F) -> R {
    f()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn returns_the_result() {
        assert_eq!(with_big_stack(|| 6 * 7), 42);
    }

    #[test]
    fn borrows_from_the_caller() {
        let text = String::from("borrowed");
        assert_eq!(with_big_stack(|| text.len()), 8);
    }

    #[test]
    #[should_panic(expected = "boom")]
    fn a_panic_reaches_the_caller() {
        with_big_stack(|| -> () { panic!("boom") });
    }

    #[test]
    fn deep_recursion_fits() {
        fn depth(n: u32) -> u32 { if n == 0 { 0 } else { 1 + depth(n - 1) } }
        // 100_000 frames would overflow a 2 MB test thread many times over.
        assert_eq!(with_big_stack(|| depth(100_000)), 100_000);
    }
}
