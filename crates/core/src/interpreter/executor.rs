// src/interpreter/executor.rs
//! Who runs the workers: the interface between the thread pool and the program that hosts it
//! (docs/ASYNC_SYNC_DESIGN.md, AS3b).
//!
//! The runtime keeps the task queue and builds one interpreter per worker. It does not start
//! threads itself. An [`Executor`] is lent the work: it is handed a [`Lend`] that holds the worker
//! loops and the program's own `main`, and it decides which threads (or fibers) run them. The
//! default executor, [`DefaultExecutor`], gives every worker an operating-system thread. An engine
//! that owns its threads, pins them to cores or runs fibers brings an executor of its own.
//!
//! Every wait of the pool goes through the executor as well: [`Executor::park_until`] blocks the
//! caller until a condition holds, and [`Executor::wake`] tells the executor that something the
//! pool waits on has changed. A host that schedules fibers can park a fiber there instead of
//! blocking a thread.
//!
//! # What a host has to guarantee
//!
//! * `run` starts `Lend::workers()` workers, calls `Lend::run_main` exactly once on the calling
//!   thread, calls `Lend::stop` after it, and returns only when every worker has returned.
//! * A thread or fiber that runs a worker has at least [`WORKER_STACK_BYTES`] of stack. The
//!   runtime cannot check this.
//! * `park_until(ready)` returns only once `ready()` has returned true, and must not miss a
//!   `wake` that arrives between the last `ready()` call and the moment the caller is parked.
//! * `wake` is called after the state a waiter looks at has changed, and never while the caller
//!   holds a lock that `ready` takes.

use std::sync::{Condvar, Mutex};

use crate::stack::WORKER_STACK_BYTES;

/// What the program hands to an executor to run: the worker loops and the program's `main`.
pub struct Lend<'a> {
    workers:  usize,
    worker:   &'a (dyn Fn(usize) + Sync),
    main:     &'a mut dyn FnMut(),
    stop:     &'a (dyn Fn() + Sync),
    main_ran: bool,
}

impl<'a> Lend<'a> {
    pub(crate) fn new(
        workers: usize,
        worker:  &'a (dyn Fn(usize) + Sync),
        main:    &'a mut dyn FnMut(),
        stop:    &'a (dyn Fn() + Sync),
    ) -> Lend<'a> {
        Lend { workers, worker, main, stop, main_ran: false }
    }

    /// How many workers to start. The runtime has already settled the number (an explicit count,
    /// the executor's own preference or the default), so a host starts exactly this many.
    pub fn workers(&self) -> usize {
        self.workers
    }

    /// The loop of one worker. A host calls `worker()(i)` for each `i` in `0..workers()`, each on a
    /// thread or fiber of its own. The call returns after `stop` is called and the queue is empty.
    /// The returned reference does not borrow the `Lend`, so it can go to another thread.
    pub fn worker(&self) -> &'a (dyn Fn(usize) + Sync) {
        self.worker
    }

    /// The function that ends the workers, for a host that needs to call it from somewhere other
    /// than the thread that holds the `Lend`. Calling it more than once does no harm.
    pub fn stop_fn(&self) -> &'a (dyn Fn() + Sync) {
        self.stop
    }

    /// Tell the workers to end once the queue is empty. Calling it more than once does no harm.
    pub fn stop(&self) {
        (self.stop)()
    }

    /// Run the program's `main` on the calling thread, and return when it ends.
    ///
    /// # Panics
    ///
    /// When called more than once: `main` runs once.
    pub fn run_main(&mut self) {
        assert!(!self.main_ran, "Lend::run_main was called more than once");
        self.main_ran = true;
        (self.main)()
    }
}

/// The host of the worker pool.
pub trait Executor: Send + Sync {
    /// Start the workers, run `main` and, after it, stop the workers and wait for them to return.
    ///
    /// Blocking: the thread that calls `run` is the one that runs `main`. An `Err` means the
    /// workers could not be started; `main` has not run then, and the error text is shown as the
    /// reason the program did not start. A host that started some workers before it failed calls
    /// `Lend::stop` so that they end.
    fn run(&self, lend: Lend<'_>) -> Result<(), String>;

    /// Block the caller until `ready()` returns true. Called by a worker with no task to run, and
    /// by a task that waits for another one. `ready` is cheap, takes no executor lock and may be
    /// called many times.
    fn park_until(&self, ready: &dyn Fn() -> bool);

    /// Something a parked caller waits on has changed: a task was queued, a task ended, or the
    /// pool is shutting down. Parked callers look at their condition again.
    fn wake(&self);

    /// How many workers this executor wants when nothing else sets the number. `None` leaves it to
    /// the runtime's default. An explicit count (`--workers`, `UBEL_WORKERS`, the API) wins.
    fn preferred_workers(&self) -> Option<usize> {
        None
    }
}

/// Ends the workers when it is dropped, so that a panic in `main` does not leave them parked for
/// ever inside the thread scope that waits for them.
struct StopOnDrop<'a>(&'a (dyn Fn() + Sync));

impl Drop for StopOnDrop<'_> {
    fn drop(&mut self) {
        (self.0)()
    }
}

/// One operating-system thread per worker, started inside a thread scope that ends when `main`
/// does. Callers park on one condition variable.
pub struct DefaultExecutor {
    lock: Mutex<()>,
    cv:   Condvar,
}

impl DefaultExecutor {
    pub fn new() -> DefaultExecutor {
        DefaultExecutor { lock: Mutex::new(()), cv: Condvar::new() }
    }
}

impl Default for DefaultExecutor {
    fn default() -> Self {
        DefaultExecutor::new()
    }
}

impl Executor for DefaultExecutor {
    fn run(&self, mut lend: Lend<'_>) -> Result<(), String> {
        let worker = lend.worker();
        let stop = lend.stop_fn();
        std::thread::scope(|scope| -> Result<(), String> {
            let _stop_when_done = StopOnDrop(stop);
            for i in 0..lend.workers() {
                let started = std::thread::Builder::new()
                    .name(format!("ubel-worker-{i}"))
                    .stack_size(WORKER_STACK_BYTES)
                    .spawn_scoped(scope, move || worker(i));
                if let Err(e) = started {
                    return Err(format!("cannot start a worker thread: {e}"));
                }
            }
            lend.run_main();
            Ok(())
        })
    }

    fn park_until(&self, ready: &dyn Fn() -> bool) {
        let mut guard = self.lock.lock().unwrap();
        while !ready() {
            guard = self.cv.wait(guard).unwrap();
        }
    }

    fn wake(&self) {
        // Taking the lock first means a caller that has just seen `ready()` false is already
        // inside `wait` by the time the notification is sent, so it cannot be missed.
        drop(self.lock.lock().unwrap());
        self.cv.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    #[test]
    fn a_lend_gives_back_the_pieces_it_was_made_from() {
        let seen = AtomicUsize::new(0);
        let stops = AtomicUsize::new(0);
        let worker = |i: usize| {
            seen.fetch_add(i + 1, Ordering::SeqCst);
        };
        let stop = || {
            stops.fetch_add(1, Ordering::SeqCst);
        };
        let mut main = || {};
        let lend = Lend::new(3, &worker, &mut main, &stop);
        assert_eq!(lend.workers(), 3);
        lend.worker()(4);
        assert_eq!(seen.load(Ordering::SeqCst), 5);
        lend.stop();
        (lend.stop_fn())();
        assert_eq!(stops.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn main_runs_when_the_host_asks_for_it() {
        let ran = AtomicUsize::new(0);
        let worker = |_: usize| {};
        let stop = || {};
        let mut main = || {
            ran.fetch_add(1, Ordering::SeqCst);
        };
        let mut lend = Lend::new(0, &worker, &mut main, &stop);
        assert_eq!(ran.load(Ordering::SeqCst), 0);
        lend.run_main();
        assert_eq!(ran.load(Ordering::SeqCst), 1);
    }

    #[test]
    #[should_panic(expected = "more than once")]
    fn main_cannot_run_twice() {
        let worker = |_: usize| {};
        let stop = || {};
        let mut main = || {};
        let mut lend = Lend::new(0, &worker, &mut main, &stop);
        lend.run_main();
        lend.run_main();
    }

    /// Run `DefaultExecutor` over workers that wait for the stop signal, as the real ones do.
    fn run_default(workers: usize, main_body: impl FnMut() + Send) -> (Vec<String>, usize, Result<(), String>) {
        let stopped = AtomicBool::new(false);
        let names = Mutex::new(Vec::<String>::new());
        let stops = AtomicUsize::new(0);
        let worker = |_: usize| {
            names.lock().unwrap().push(std::thread::current().name().unwrap_or("").to_string());
            while !stopped.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(2));
            }
        };
        let stop = || {
            stops.fetch_add(1, Ordering::SeqCst);
            stopped.store(true, Ordering::SeqCst);
        };
        let mut main = main_body;
        let result = DefaultExecutor::new().run(Lend::new(workers, &worker, &mut main, &stop));
        let mut names = names.into_inner().unwrap();
        names.sort();
        (names, stops.load(Ordering::SeqCst), result)
    }

    #[test]
    fn the_default_executor_gives_every_worker_a_named_thread() {
        let (names, _, result) = run_default(3, || {});
        assert!(result.is_ok());
        assert_eq!(names, vec!["ubel-worker-0", "ubel-worker-1", "ubel-worker-2"]);
    }

    #[test]
    fn the_default_executor_runs_main_on_the_calling_thread_and_then_stops() {
        let caller = std::thread::current().id();
        let main_thread = Mutex::new(None);
        let (_, stops, result) = run_default(2, || {
            *main_thread.lock().unwrap() = Some(std::thread::current().id());
        });
        assert!(result.is_ok());
        assert_eq!(*main_thread.lock().unwrap(), Some(caller));
        assert!(stops >= 1, "the workers were never told to stop");
    }

    #[test]
    fn the_default_executor_stops_the_workers_when_main_panics() {
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = run_default(2, || panic!("main failed"));
        }));
        // The scope returned instead of waiting for workers that nobody stopped.
        assert!(outcome.is_err());
    }

    #[test]
    fn park_until_returns_at_once_when_the_condition_already_holds() {
        DefaultExecutor::new().park_until(&|| true);
    }

    #[test]
    fn park_until_waits_for_the_condition_and_a_wake() {
        let exec = Arc::new(DefaultExecutor::new());
        let flag = Arc::new(AtomicBool::new(false));
        let returned = Arc::new(AtomicBool::new(false));
        let handle = {
            let (exec, flag, returned) = (exec.clone(), flag.clone(), returned.clone());
            std::thread::spawn(move || {
                exec.park_until(&|| flag.load(Ordering::SeqCst));
                returned.store(true, Ordering::SeqCst);
            })
        };
        std::thread::sleep(Duration::from_millis(50));
        assert!(!returned.load(Ordering::SeqCst));
        // A wake with the condition still false does not release the caller.
        exec.wake();
        std::thread::sleep(Duration::from_millis(50));
        assert!(!returned.load(Ordering::SeqCst));
        flag.store(true, Ordering::SeqCst);
        exec.wake();
        handle.join().unwrap();
        assert!(returned.load(Ordering::SeqCst));
    }

    #[test]
    fn a_wake_sent_while_a_caller_is_deciding_to_park_is_not_lost() {
        let exec = Arc::new(DefaultExecutor::new());
        let flag = Arc::new(AtomicBool::new(false));
        let deciding = Arc::new(AtomicBool::new(false));
        let (tx, rx) = std::sync::mpsc::channel();
        let parker = {
            let (exec, flag, deciding) = (exec.clone(), flag.clone(), deciding.clone());
            std::thread::spawn(move || {
                exec.park_until(&|| {
                    let seen = flag.load(Ordering::SeqCst);
                    if !seen {
                        // The window between looking at the condition and starting to wait.
                        deciding.store(true, Ordering::SeqCst);
                        std::thread::sleep(Duration::from_millis(100));
                    }
                    seen
                });
                tx.send(()).unwrap();
            })
        };
        while !deciding.load(Ordering::SeqCst) {
            std::thread::yield_now();
        }
        flag.store(true, Ordering::SeqCst);
        exec.wake();
        assert!(
            rx.recv_timeout(Duration::from_secs(5)).is_ok(),
            "the wake was lost: the parked caller never saw that the condition holds"
        );
        parker.join().unwrap();
    }

    #[test]
    fn one_wake_releases_every_parked_caller() {
        let exec = Arc::new(DefaultExecutor::new());
        let flag = Arc::new(AtomicBool::new(false));
        let handles: Vec<_> = (0..3)
            .map(|_| {
                let (exec, flag) = (exec.clone(), flag.clone());
                std::thread::spawn(move || exec.park_until(&|| flag.load(Ordering::SeqCst)))
            })
            .collect();
        std::thread::sleep(Duration::from_millis(50));
        flag.store(true, Ordering::SeqCst);
        exec.wake();
        for h in handles {
            h.join().unwrap();
        }
    }

    #[test]
    fn the_default_executor_has_no_preferred_worker_count() {
        assert_eq!(DefaultExecutor::new().preferred_workers(), None);
    }
}
