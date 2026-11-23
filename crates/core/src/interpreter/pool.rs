//! The pieces of the thread pool that do not depend on the interpreter: a spawned task as plain
//! data, the slot its result is left in, and the queue the workers take tasks from
//! (docs/ASYNC_SYNC_DESIGN.md, AS3a and AS3b).
//!
//! A worker is a thread that owns an interpreter of its own. Nothing here holds a `Value`, an
//! `Rc` or a reference into the AST, so all of it can be shared between threads.
//!
//! Nothing here blocks on a condition variable of its own. Every wait (a worker with no task, a
//! thread that awaits a task) goes through the pool's `Executor`, and every change that a waiter
//! could be waiting for is announced to it with `Executor::wake`.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use crate::ast::common::Span;

use super::executor::Executor;
use super::send::Sent;
use super::value::FunctionId;

/// What a spawned task runs.
#[derive(Debug, Clone)]
pub enum SendFn {
    /// A function or method declared in the program.
    Declared(FunctionId),
    /// An `async { }` block. The block is found again by its place in the source, and the names
    /// it uses from the code around it travel with the task.
    Lambda { span: Span, captured: Vec<String> },
}

/// A task that can be handed to another thread: what to run and the values to run it with.
/// `values` holds the receiver (when `has_receiver`), then the `n_args` arguments, then one value
/// for each name in `Lambda::captured`, all copied together.
#[derive(Debug, Clone)]
pub struct SendTask {
    pub func:         SendFn,
    pub has_receiver: bool,
    pub n_args:       usize,
    pub values:       Sent,
}

/// How a task ended.
#[derive(Debug)]
pub enum Outcome {
    /// It returned a value.
    Done(Sent),
    /// It failed with a value (`fail`, or `?` on a failure).
    Failed(Sent),
    /// It panicked, or could not return what it computed.
    Panicked(String),
}

#[derive(Debug)]
enum SlotState {
    /// Waiting for a thread to take it.
    Queued(SendTask),
    Running,
    Done(Outcome),
    /// The outcome was handed to the one that awaited the task.
    Taken,
}

/// Where a spawned task waits to be run and where its outcome is left. Whoever awaits the task
/// either finds it still queued, and runs it there and then, or waits for the thread that took it.
/// The slot never blocks: the waiter parks through the pool's executor until `is_done`.
#[derive(Debug)]
pub struct TaskSlot {
    state: Mutex<SlotState>,
}

impl TaskSlot {
    pub fn new(task: SendTask) -> Arc<TaskSlot> {
        Arc::new(TaskSlot { state: Mutex::new(SlotState::Queued(task)) })
    }

    /// Take the task to run it, if no one has yet. Exactly one caller gets it.
    pub fn claim(&self) -> Option<SendTask> {
        let mut st = self.state.lock().unwrap();
        match std::mem::replace(&mut *st, SlotState::Running) {
            SlotState::Queued(task) => Some(task),
            other => {
                *st = other;
                None
            }
        }
    }

    /// Leave the outcome of a task that `claim` returned. The pool announces it to the executor
    /// (`PoolShared::finish`); on its own this does not wake anyone.
    pub fn finish(&self, outcome: Outcome) {
        *self.state.lock().unwrap() = SlotState::Done(outcome);
    }

    /// Has the task ended? True once its outcome was left, also after it was taken.
    pub fn is_done(&self) -> bool {
        matches!(&*self.state.lock().unwrap(), SlotState::Done(_) | SlotState::Taken)
    }

    /// Take the outcome of a task that has ended. Calling it before `is_done` is true, or a
    /// second time, is a panic outcome and changes nothing.
    pub fn take(&self) -> Outcome {
        let mut st = self.state.lock().unwrap();
        match std::mem::replace(&mut *st, SlotState::Taken) {
            SlotState::Done(outcome) => outcome,
            SlotState::Taken => {
                Outcome::Panicked("the outcome of this task was already taken".to_string())
            }
            other => {
                *st = other;
                Outcome::Panicked("this task has not ended yet".to_string())
            }
        }
    }
}

struct Queue {
    slots:    VecDeque<Arc<TaskSlot>>,
    shutdown: bool,
}

/// Run by a worker just before it starts a task. Only tests set one.
pub type TaskHook = Arc<dyn Fn() + Send + Sync>;

/// The queue the workers take from, the flag that tells them to stop, and the executor that every
/// wait and wake goes through.
pub struct PoolShared {
    exec:  Arc<dyn Executor>,
    queue: Mutex<Queue>,
    pub hook: Option<TaskHook>,
}

impl PoolShared {
    pub fn new(exec: Arc<dyn Executor>, hook: Option<TaskHook>) -> Arc<PoolShared> {
        Arc::new(PoolShared {
            exec,
            queue: Mutex::new(Queue { slots: VecDeque::new(), shutdown: false }),
            hook,
        })
    }

    /// The executor this pool waits and wakes through.
    pub fn executor(&self) -> &Arc<dyn Executor> {
        &self.exec
    }

    pub fn submit(&self, slot: Arc<TaskSlot>) {
        self.queue.lock().unwrap().slots.push_back(slot);
        self.exec.wake();
    }

    /// Leave the outcome of a task in its slot, and wake whoever waits for it.
    pub fn finish(&self, slot: &TaskSlot, outcome: Outcome) {
        slot.finish(outcome);
        self.exec.wake();
    }

    /// The next task to run, parking until there is one. `None` once the pool is shut down and the
    /// queue is empty.
    pub fn next(&self) -> Option<Arc<TaskSlot>> {
        loop {
            {
                let mut q = self.queue.lock().unwrap();
                if let Some(slot) = q.slots.pop_front() {
                    return Some(slot);
                }
                if q.shutdown {
                    return None;
                }
            }
            self.exec.park_until(&|| {
                let q = self.queue.lock().unwrap();
                !q.slots.is_empty() || q.shutdown
            });
        }
    }

    pub fn shut_down(&self) {
        self.queue.lock().unwrap().shutdown = true;
        self.exec.wake();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::executor::{DefaultExecutor, Lend};
    use super::super::send::encode;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    /// The default executor that counts how often it is asked to park and to wake.
    struct Counting {
        inner: DefaultExecutor,
        parks: AtomicUsize,
        wakes: AtomicUsize,
    }

    impl Counting {
        fn new() -> Arc<Counting> {
            Arc::new(Counting { inner: DefaultExecutor::new(), parks: AtomicUsize::new(0), wakes: AtomicUsize::new(0) })
        }
        fn parks(&self) -> usize {
            self.parks.load(Ordering::SeqCst)
        }
        fn wakes(&self) -> usize {
            self.wakes.load(Ordering::SeqCst)
        }
    }

    impl Executor for Counting {
        fn run(&self, _lend: Lend<'_>) -> Result<(), String> {
            Err("not used in these tests".to_string())
        }
        fn park_until(&self, ready: &dyn Fn() -> bool) {
            self.parks.fetch_add(1, Ordering::SeqCst);
            self.inner.park_until(ready)
        }
        fn wake(&self) {
            self.wakes.fetch_add(1, Ordering::SeqCst);
            self.inner.wake()
        }
    }

    fn pool_with(exec: &Arc<Counting>) -> Arc<PoolShared> {
        let exec: Arc<dyn Executor> = exec.clone();
        PoolShared::new(exec, None)
    }

    fn task() -> SendTask {
        SendTask { func: SendFn::Declared(0), has_receiver: false, n_args: 0, values: encode(&[], 0).unwrap() }
    }

    fn done() -> Outcome {
        Outcome::Done(encode(&[], 0).unwrap())
    }

    // ── The slot ─────────────────────────────────────────────────

    #[test]
    fn a_task_is_claimed_once() {
        let slot = TaskSlot::new(task());
        assert!(slot.claim().is_some());
        assert!(slot.claim().is_none());
        // A refused claim changes nothing: the task is still running, not done.
        assert!(!slot.is_done());
    }

    #[test]
    fn a_finished_task_is_not_claimed() {
        let slot = TaskSlot::new(task());
        slot.claim().unwrap();
        slot.finish(done());
        assert!(slot.claim().is_none());
        // A refused claim changes nothing: the outcome is still there to be taken.
        assert!(slot.is_done());
        assert!(matches!(slot.take(), Outcome::Done(_)));
    }

    #[test]
    fn take_returns_the_outcome_that_was_left() {
        let slot = TaskSlot::new(task());
        slot.claim().unwrap();
        slot.finish(Outcome::Panicked("boom".to_string()));
        assert!(matches!(slot.take(), Outcome::Panicked(m) if m == "boom"));
    }

    #[test]
    fn the_outcome_can_be_taken_only_once() {
        let slot = TaskSlot::new(task());
        slot.claim().unwrap();
        slot.finish(done());
        assert!(matches!(slot.take(), Outcome::Done(_)));
        assert!(matches!(slot.take(), Outcome::Panicked(m) if m.contains("already taken")));
    }

    #[test]
    fn a_task_is_done_from_the_moment_its_outcome_is_left_and_stays_done_once_taken() {
        let slot = TaskSlot::new(task());
        assert!(!slot.is_done(), "queued");
        slot.claim().unwrap();
        assert!(!slot.is_done(), "running");
        slot.finish(done());
        assert!(slot.is_done(), "done");
        slot.take();
        assert!(slot.is_done(), "taken");
    }

    #[test]
    fn taking_the_outcome_of_a_task_that_has_not_ended_is_a_panic_that_changes_nothing() {
        let slot = TaskSlot::new(task());
        assert!(matches!(slot.take(), Outcome::Panicked(m) if m.contains("has not ended")));
        // Still queued: it can be claimed, and then finished and taken as usual.
        let claimed = slot.claim();
        assert!(claimed.is_some());
        assert!(matches!(slot.take(), Outcome::Panicked(m) if m.contains("has not ended")));
        slot.finish(done());
        assert!(matches!(slot.take(), Outcome::Done(_)));
    }

    // ── The queue ────────────────────────────────────────────────

    #[test]
    fn the_queue_gives_tasks_in_the_order_they_were_submitted() {
        let pool = pool_with(&Counting::new());
        let (a, b) = (TaskSlot::new(task()), TaskSlot::new(task()));
        pool.submit(a.clone());
        pool.submit(b.clone());
        assert!(Arc::ptr_eq(&pool.next().unwrap(), &a));
        assert!(Arc::ptr_eq(&pool.next().unwrap(), &b));
    }

    #[test]
    fn shutting_down_ends_the_queue_but_not_before_it_is_empty() {
        let pool = pool_with(&Counting::new());
        let a = TaskSlot::new(task());
        pool.submit(a.clone());
        pool.shut_down();
        assert!(Arc::ptr_eq(&pool.next().unwrap(), &a));
        assert!(pool.next().is_none());
    }

    #[test]
    fn shutting_down_wakes_a_worker_that_is_waiting() {
        let pool = pool_with(&Counting::new());
        let worker = pool.clone();
        let handle = std::thread::spawn(move || worker.next().is_none());
        std::thread::sleep(Duration::from_millis(50));
        pool.shut_down();
        assert!(handle.join().unwrap());
    }

    #[test]
    fn a_waiting_worker_is_woken_by_a_submitted_task() {
        let pool = pool_with(&Counting::new());
        let worker = pool.clone();
        let handle = std::thread::spawn(move || worker.next().is_some());
        std::thread::sleep(Duration::from_millis(50));
        pool.submit(TaskSlot::new(task()));
        assert!(handle.join().unwrap());
    }

    // ── The executor ─────────────────────────────────────────────

    #[test]
    fn the_pool_hands_out_the_executor_it_was_built_with() {
        let exec = Counting::new();
        let pool = pool_with(&exec);
        let as_dyn: Arc<dyn Executor> = exec.clone();
        assert!(Arc::ptr_eq(pool.executor(), &as_dyn));
    }

    #[test]
    fn a_worker_with_nothing_to_do_parks_through_the_executor() {
        let exec = Counting::new();
        let pool = pool_with(&exec);
        let worker = pool.clone();
        let handle = std::thread::spawn(move || worker.next().is_none());
        std::thread::sleep(Duration::from_millis(50));
        assert!(exec.parks() >= 1);
        pool.shut_down();
        handle.join().unwrap();
    }

    #[test]
    fn a_worker_that_finds_a_task_does_not_park() {
        let exec = Counting::new();
        let pool = pool_with(&exec);
        pool.submit(TaskSlot::new(task()));
        assert!(pool.next().is_some());
        assert_eq!(exec.parks(), 0);
    }

    #[test]
    fn submitting_a_task_wakes_the_executor() {
        let exec = Counting::new();
        let pool = pool_with(&exec);
        pool.submit(TaskSlot::new(task()));
        assert_eq!(exec.wakes(), 1);
    }

    #[test]
    fn shutting_down_wakes_the_executor() {
        let exec = Counting::new();
        let pool = pool_with(&exec);
        pool.shut_down();
        assert_eq!(exec.wakes(), 1);
    }

    #[test]
    fn finishing_a_task_leaves_the_outcome_and_wakes_the_executor() {
        let exec = Counting::new();
        let pool = pool_with(&exec);
        let slot = TaskSlot::new(task());
        slot.claim().unwrap();
        pool.finish(&slot, done());
        assert!(slot.is_done());
        assert_eq!(exec.wakes(), 1);
    }

    #[test]
    fn a_thread_that_awaits_a_task_parks_until_another_thread_finishes_it() {
        let exec = Counting::new();
        let pool = pool_with(&exec);
        let slot = TaskSlot::new(task());
        let claimed = slot.claim().unwrap();
        let (finisher, other) = (pool.clone(), slot.clone());
        let handle = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            drop(claimed);
            finisher.finish(&other, Outcome::Panicked("late".to_string()));
        });
        pool.executor().park_until(&|| slot.is_done());
        assert!(matches!(slot.take(), Outcome::Panicked(m) if m == "late"));
        handle.join().unwrap();
    }
}
