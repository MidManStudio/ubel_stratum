//! The pieces of the thread pool that do not depend on the interpreter: a spawned task as plain
//! data, the slot its result is left in, and the queue the workers take tasks from
//! (docs/ASYNC_SYNC_DESIGN.md, AS3a).
//!
//! A worker is a thread that owns an interpreter of its own. Nothing here holds a `Value`, an
//! `Rc` or a reference into the AST, so all of it can be shared between threads.

use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex};

use crate::ast::common::Span;

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
#[derive(Debug)]
pub struct TaskSlot {
    state: Mutex<SlotState>,
    done:  Condvar,
}

impl TaskSlot {
    pub fn new(task: SendTask) -> Arc<TaskSlot> {
        Arc::new(TaskSlot { state: Mutex::new(SlotState::Queued(task)), done: Condvar::new() })
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

    /// Leave the outcome of a task that `claim` returned.
    pub fn finish(&self, outcome: Outcome) {
        let mut st = self.state.lock().unwrap();
        *st = SlotState::Done(outcome);
        self.done.notify_all();
    }

    /// Wait for the thread that took the task, and take its outcome.
    pub fn wait(&self) -> Outcome {
        let mut st = self.state.lock().unwrap();
        loop {
            match std::mem::replace(&mut *st, SlotState::Taken) {
                SlotState::Done(outcome) => return outcome,
                SlotState::Taken => {
                    return Outcome::Panicked("the outcome of this task was already taken".to_string());
                }
                other => {
                    *st = other;
                    st = self.done.wait(st).unwrap();
                }
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

/// The queue the workers take from, and the flag that tells them to stop.
pub struct PoolShared {
    queue: Mutex<Queue>,
    work:  Condvar,
    pub hook: Option<TaskHook>,
}

impl PoolShared {
    pub fn new(hook: Option<TaskHook>) -> Arc<PoolShared> {
        Arc::new(PoolShared {
            queue: Mutex::new(Queue { slots: VecDeque::new(), shutdown: false }),
            work:  Condvar::new(),
            hook,
        })
    }

    pub fn submit(&self, slot: Arc<TaskSlot>) {
        let mut q = self.queue.lock().unwrap();
        q.slots.push_back(slot);
        self.work.notify_one();
    }

    /// The next task to run, waiting for one if there is none. `None` once the pool is shut down
    /// and the queue is empty.
    pub fn next(&self) -> Option<Arc<TaskSlot>> {
        let mut q = self.queue.lock().unwrap();
        loop {
            if let Some(slot) = q.slots.pop_front() {
                return Some(slot);
            }
            if q.shutdown {
                return None;
            }
            q = self.work.wait(q).unwrap();
        }
    }

    pub fn shut_down(&self) {
        let mut q = self.queue.lock().unwrap();
        q.shutdown = true;
        self.work.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::send::encode;

    fn task() -> SendTask {
        SendTask { func: SendFn::Declared(0), has_receiver: false, n_args: 0, values: encode(&[], 0).unwrap() }
    }

    fn done() -> Outcome {
        Outcome::Done(encode(&[], 0).unwrap())
    }

    #[test]
    fn a_task_is_claimed_once() {
        let slot = TaskSlot::new(task());
        assert!(slot.claim().is_some());
        assert!(slot.claim().is_none());
    }

    #[test]
    fn a_finished_task_is_not_claimed() {
        let slot = TaskSlot::new(task());
        slot.claim().unwrap();
        slot.finish(done());
        assert!(slot.claim().is_none());
    }

    #[test]
    fn wait_returns_the_outcome_that_was_left() {
        let slot = TaskSlot::new(task());
        slot.claim().unwrap();
        slot.finish(Outcome::Panicked("boom".to_string()));
        assert!(matches!(slot.wait(), Outcome::Panicked(m) if m == "boom"));
    }

    #[test]
    fn the_outcome_can_be_taken_only_once() {
        let slot = TaskSlot::new(task());
        slot.claim().unwrap();
        slot.finish(done());
        assert!(matches!(slot.wait(), Outcome::Done(_)));
        assert!(matches!(slot.wait(), Outcome::Panicked(m) if m.contains("already taken")));
    }

    #[test]
    fn wait_blocks_until_another_thread_finishes_the_task() {
        let slot = TaskSlot::new(task());
        let task = slot.claim().unwrap();
        let other = slot.clone();
        let handle = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(50));
            drop(task);
            other.finish(Outcome::Panicked("late".to_string()));
        });
        assert!(matches!(slot.wait(), Outcome::Panicked(m) if m == "late"));
        handle.join().unwrap();
    }

    #[test]
    fn wait_on_a_task_nobody_has_claimed_waits_for_whoever_does() {
        let slot = TaskSlot::new(task());
        let other = slot.clone();
        let handle = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(50));
            let _t = other.claim().unwrap();
            other.finish(done());
        });
        assert!(matches!(slot.wait(), Outcome::Done(_)));
        handle.join().unwrap();
    }

    #[test]
    fn the_queue_gives_tasks_in_the_order_they_were_submitted() {
        let pool = PoolShared::new(None);
        let (a, b) = (TaskSlot::new(task()), TaskSlot::new(task()));
        pool.submit(a.clone());
        pool.submit(b.clone());
        assert!(Arc::ptr_eq(&pool.next().unwrap(), &a));
        assert!(Arc::ptr_eq(&pool.next().unwrap(), &b));
    }

    #[test]
    fn shutting_down_ends_the_queue_but_not_before_it_is_empty() {
        let pool = PoolShared::new(None);
        let a = TaskSlot::new(task());
        pool.submit(a.clone());
        pool.shut_down();
        assert!(Arc::ptr_eq(&pool.next().unwrap(), &a));
        assert!(pool.next().is_none());
    }

    #[test]
    fn shutting_down_wakes_a_worker_that_is_waiting() {
        let pool = PoolShared::new(None);
        let worker = pool.clone();
        let handle = std::thread::spawn(move || worker.next().is_none());
        std::thread::sleep(std::time::Duration::from_millis(50));
        pool.shut_down();
        assert!(handle.join().unwrap());
    }

    #[test]
    fn a_waiting_worker_is_woken_by_a_submitted_task() {
        let pool = PoolShared::new(None);
        let worker = pool.clone();
        let handle = std::thread::spawn(move || worker.next().is_some());
        std::thread::sleep(std::time::Duration::from_millis(50));
        pool.submit(TaskSlot::new(task()));
        assert!(handle.join().unwrap());
    }
}
