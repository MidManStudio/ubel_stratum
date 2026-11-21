//! `Task.spawn`, `join`, and the worker threads (docs/ASYNC_SYNC_DESIGN.md, AS3a).
//!
//! A program that mentions `Task.spawn` runs inside a thread scope that holds the workers. Each
//! worker owns an interpreter built from the same program, so every function has the same id in
//! all of them. What crosses between interpreters is copied (`interpreter::send`): the receiver,
//! the arguments and, for an `async { }` block, the names the block uses from the code around it.
//!
//! A task is awaited at most once, and `await` and `join` are the same thing for one: whoever
//! awaits a task that no worker has taken yet runs it there and then, so a worker that waits on a
//! task it spawned can never be waiting for a thread that is waiting for it.

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use crate::ast::common::{Span, TierAnnotation};
use crate::ast::expressions::{Expr, ExprKind, Lambda, LambdaBody};
use crate::ast::literals::{IntSuffix, InterpolationPart, Literal};
use crate::ast::root::{Item, Program};
use crate::ast::visitor::{walk_expr, AstVisitor};
use crate::builtins::global::io::{attach_capture_sink, detach_capture_sink, share_capture_sink};
use crate::interpreter::env::Environment;
use crate::interpreter::pool::{Outcome, PoolShared, SendFn, SendTask, TaskHook, TaskSlot};
use crate::interpreter::send::{self, Sent};
use crate::interpreter::value::{EvalResult, FunctionId, Signal, TaskState, Value};

use super::{FunctionBody, FunctionDef, Interpreter};

/// The stack a worker gets, the same order of size the interpreter needs on the main thread.
const WORKER_STACK_BYTES: usize = 128 * 1024 * 1024;

/// What a worker needs to build the interpreter it runs tasks in.
struct WorkerSetup {
    int_literal_types: HashMap<Span, IntSuffix>,
    trait_call_sites:  HashMap<Span, String>,
    /// The values of the program's constants and statics when the pool started. A static is a copy
    /// in each worker from here on: a write to it is seen only by the task that made it.
    consts:  Vec<(String, Sent)>,
    statics: Vec<(String, Sent)>,
}

/// The number of workers when the program does not say: one fewer than the logical cores (the main
/// thread is the other one), and never fewer than one.
pub fn default_worker_count() -> usize {
    std::thread::available_parallelism().map(|n| n.get()).unwrap_or(2).saturating_sub(1).max(1)
}

// ── Finding things in the AST ────────────────────────────────────

/// Does the program call `Task.spawn` anywhere? Only then does it need workers.
struct SpawnFinder {
    found: bool,
}

impl<'ast> AstVisitor<'ast> for SpawnFinder {
    fn visit_expr(&mut self, e: &'ast Expr<'ast>) {
        if let ExprKind::Field { target, field } = &e.kind {
            if *field == "spawn" && matches!(target.kind, ExprKind::Ident("Task")) {
                self.found = true;
            }
        }
        walk_expr(self, e);
    }
}

/// Every lambda of the program, by its place in the source.
struct LambdaIndex<'ast> {
    map: HashMap<Span, &'ast Lambda<'ast>>,
}

impl<'ast> AstVisitor<'ast> for LambdaIndex<'ast> {
    fn visit_expr(&mut self, e: &'ast Expr<'ast>) {
        if let ExprKind::Lambda(l) = &e.kind {
            self.map.insert(l.span, l);
        }
        walk_expr(self, e);
    }
}

/// Every name a lambda mentions. More than it needs (a name it declares itself is in here too),
/// which only means a value is copied that is then not used.
struct NameCollector {
    names: BTreeSet<String>,
}

impl<'ast> AstVisitor<'ast> for NameCollector {
    fn visit_expr(&mut self, e: &'ast Expr<'ast>) {
        match &e.kind {
            ExprKind::Ident(name) => {
                self.names.insert(name.to_string());
            }
            ExprKind::SelfExpr => {
                self.names.insert("self".to_string());
            }
            ExprKind::Lit(Literal::InterpolatedStr(parts))
            | ExprKind::Lit(Literal::InterpolatedVerbatimStr(parts)) => {
                for part in parts.iter() {
                    if let InterpolationPart::Expr { expr, .. } = part {
                        self.visit_expr(expr);
                    }
                }
            }
            _ => {}
        }
        walk_expr(self, e);
    }
}

fn names_used_by<'ast>(lambda: &'ast Lambda<'ast>) -> BTreeSet<String> {
    let mut c = NameCollector { names: BTreeSet::new() };
    match &lambda.body {
        LambdaBody::Block(b) => c.visit_block(b),
        LambdaBody::Expr(e)  => c.visit_expr(e),
    }
    c.names
}

// ── The interpreter side ─────────────────────────────────────────

impl<'ast> Interpreter<'ast> {
    /// How many worker threads a program that spawns gets. Without this, `default_worker_count`.
    pub fn set_worker_count(&mut self, n: usize) {
        self.worker_count = Some(n);
    }

    /// A function each worker calls just before it starts a task. For tests of what runs at the
    /// same time.
    pub fn set_task_start_hook(&mut self, hook: TaskHook) {
        self.task_hook = Some(hook);
    }

    fn lambda_ast(&mut self, span: Span) -> Result<&'ast Lambda<'ast>, String> {
        if self.lambda_table.is_none() {
            let program = self.program.ok_or_else(|| "no program is loaded".to_string())?;
            let mut index = LambdaIndex { map: HashMap::new() };
            index.visit_program(program);
            self.lambda_table = Some(index.map);
        }
        self.lambda_table.as_ref().and_then(|t| t.get(&span)).copied()
            .ok_or_else(|| "an async block of the task is not in the program".to_string())
    }

    /// `Task.spawn(task)`: hand a cold task to the workers. The task is the same value afterwards,
    /// now running, and is awaited or joined like any other.
    pub(crate) fn spawn_task(&mut self, task: Value) -> EvalResult {
        let Value::Task(cell) = &task else {
            return Err(Signal::Panic(format!("`Task.spawn` needs a task, found {}", task.type_name())));
        };
        let Some(pool) = self.pool.clone() else {
            return Err(Signal::Panic(
                "`Task.spawn` is not available here: this run has no worker threads".to_string()));
        };
        let (func, receiver, args) = match &*cell.borrow() {
            TaskState::Cold { func, receiver, args } => (*func, receiver.clone(), args.clone()),
            _ => return Err(Signal::Panic(
                "a task can be spawned only once, and only before it is awaited".to_string())),
        };
        let sendable = self.encode_task(func, receiver, args).map_err(Signal::Panic)?;
        let slot = TaskSlot::new(sendable);
        *cell.borrow_mut() = TaskState::Spawned(slot.clone());
        pool.submit(slot);
        Ok(task)
    }

    /// Wait for a spawned task, running it here if no worker has taken it yet.
    pub(crate) fn join_slot(&mut self, slot: Arc<TaskSlot>) -> EvalResult {
        // A worker that finds the task still queued runs it itself, so that it never waits for a
        // thread that is waiting for it. The thread that started the pool only waits: a task run
        // there would see and change the statics of the program instead of a worker's copy.
        let claimed = if self.is_worker { slot.claim() } else { None };
        let outcome = match claimed {
            Some(task) => self.run_send_task(task),
            None       => slot.wait(),
        };
        match outcome {
            Outcome::Done(sent)    => Ok(sent.decode().pop().unwrap_or(Value::Void)),
            Outcome::Failed(sent)  => Err(Signal::Fail(sent.decode().pop().unwrap_or(Value::Void))),
            Outcome::Panicked(msg) => Err(Signal::Panic(msg)),
        }
    }

    fn encode_task(
        &mut self,
        func:     FunctionId,
        receiver: Option<Value>,
        args:     Vec<Value>,
    ) -> Result<SendTask, String> {
        let has_receiver = receiver.is_some();
        let n_args = args.len();
        let mut values: Vec<Value> = Vec::with_capacity(n_args + 1);
        values.extend(receiver);
        values.extend(args);

        let sfn = if func < self.declared_functions {
            SendFn::Declared(func)
        } else {
            let span = *self.lambda_origin.get(&func)
                .ok_or_else(|| "a closure cannot be sent to another task".to_string())?;
            let lambda = self.lambda_ast(span)?;
            let mut captured = Vec::new();
            for name in names_used_by(lambda) {
                if let Some(v) = self.functions[func].closure.local_get(&name) {
                    values.push(v.clone());
                    captured.push(name);
                }
            }
            SendFn::Lambda { span, captured }
        };
        let sent = send::encode(&values, self.declared_functions)?;
        Ok(SendTask { func: sfn, has_receiver, n_args, values: sent })
    }

    /// Run a task that was copied out of some interpreter, in this one.
    pub(crate) fn run_send_task(&mut self, task: SendTask) -> Outcome {
        let SendTask { func, has_receiver, n_args, values } = task;
        let mut vals = values.decode();
        let captured = vals.split_off(has_receiver as usize + n_args);
        let args = vals.split_off(has_receiver as usize);
        let receiver = if has_receiver { vals.pop() } else { None };

        let (id, temporary) = match func {
            SendFn::Declared(id) => (id, false),
            SendFn::Lambda { span, captured: names } => match self.register_task_lambda(span, &names, captured) {
                Ok(id)   => (id, true),
                Err(msg) => return Outcome::Panicked(msg),
            },
        };
        let result = match receiver {
            Some(r) => self.run_method(id, r, &args),
            None    => self.run_function(id, &args),
        };
        // The entry made for an async block is not needed again, unless something made a function
        // after it (a lambda the block created could still be reachable).
        if temporary && self.functions.len() == id + 1 {
            self.functions.pop();
        }
        match result {
            Ok(v) | Err(Signal::Return(v)) => match send::encode(&[v], self.declared_functions) {
                Ok(sent) => Outcome::Done(sent),
                Err(msg) => Outcome::Panicked(format!("a task could not return its result: {msg}")),
            },
            Err(Signal::Fail(v)) => match send::encode(&[v], self.declared_functions) {
                Ok(sent) => Outcome::Failed(sent),
                Err(msg) => Outcome::Panicked(format!("a task could not return its failure: {msg}")),
            },
            Err(Signal::Panic(msg))  => Outcome::Panicked(msg),
            Err(Signal::Break(_))    => Outcome::Panicked("break outside loop".to_string()),
            Err(Signal::Continue)    => Outcome::Panicked("continue outside loop".to_string()),
        }
    }

    /// A function entry for an `async { }` block that was sent here, closed over the module scope
    /// of this interpreter and the values that came with it.
    fn register_task_lambda(
        &mut self,
        span:     Span,
        names:    &[String],
        captured: Vec<Value>,
    ) -> Result<FunctionId, String> {
        let lambda = self.lambda_ast(span)?;
        let mut env: Environment = self.module_scope.clone();
        env.push();
        for (name, value) in names.iter().zip(captured) {
            env.define(name, value);
        }
        let params: Vec<String> = lambda.params.iter().map(|p| p.name.to_string()).collect();
        let body = match &lambda.body {
            LambdaBody::Block(b) => FunctionBody::Ast { block: *b },
            LambdaBody::Expr(e)  => FunctionBody::ExprBody { expr: e },
        };
        Ok(self.alloc_function(FunctionDef {
            name: None,
            params,
            body,
            closure: env,
            tier: TierAnnotation::High,
            is_async: false,
        }))
    }

    // ── The pool ─────────────────────────────────────────────────

    /// Does the program need workers?
    pub(super) fn program_spawns(program: &Program<'_>) -> bool {
        let mut finder = SpawnFinder { found: false };
        finder.visit_program(program);
        finder.found
    }

    /// Run `main` with a pool of workers beside it. The workers live inside a thread scope that
    /// ends when `main` does, so they can read the program without any `unsafe`.
    pub(super) fn run_main_with_pool(&mut self, program: &'ast Program<'ast>) -> Result<(), String> {
        let workers = self.worker_count.unwrap_or_else(default_worker_count);
        if workers == 0 || cfg!(target_arch = "wasm32") {
            return self.run_main();
        }
        let setup = self.worker_setup(program);
        let shared = PoolShared::new(self.task_hook.clone());
        let sink = share_capture_sink();

        let result = std::thread::scope(|scope| -> Result<(), String> {
            for i in 0..workers {
                let worker_shared = shared.clone();
                let sink = sink.clone();
                let setup = &setup;
                let started = std::thread::Builder::new()
                    .name(format!("ubel-worker-{i}"))
                    .stack_size(WORKER_STACK_BYTES)
                    .spawn_scoped(scope, move || {
                        attach_capture_sink(sink);
                        let mut interp = Interpreter::<'ast>::new_detached();
                        interp.prepare_worker(program, setup, worker_shared.clone());
                        interp.worker_loop(&worker_shared);
                    });
                if let Err(e) = started {
                    shared.shut_down();
                    return Err(format!("cannot start a worker thread: {e}"));
                }
            }
            self.pool = Some(shared.clone());
            let result = self.run_main();
            self.pool = None;
            shared.shut_down();
            result
        });
        detach_capture_sink();
        result
    }

    fn worker_setup(&self, program: &Program<'_>) -> WorkerSetup {
        let mut consts = Vec::new();
        let mut statics = Vec::new();
        for item in program.items.iter().copied() {
            match item {
                Item::Const(c) => {
                    if let Some(v) = self.env.get(c.name) {
                        if let Ok(sent) = send::encode(&[v.clone()], self.declared_functions) {
                            consts.push((c.name.to_string(), sent));
                        }
                    }
                }
                Item::Static(s) => {
                    if let Some(v) = self.statics.get(s.name) {
                        if let Ok(sent) = send::encode(&[v.clone()], self.declared_functions) {
                            statics.push((s.name.to_string(), sent));
                        }
                    }
                }
                _ => {}
            }
        }
        WorkerSetup {
            int_literal_types: self.int_literal_types.clone(),
            trait_call_sites:  self.trait_call_sites.clone(),
            consts,
            statics,
        }
    }

    /// Build the state of a worker: everything the program declares, registered in the same order
    /// as in the interpreter that started the pool, and the constants and statics as they were.
    fn prepare_worker(&mut self, program: &'ast Program<'ast>, setup: &WorkerSetup, shared: Arc<PoolShared>) {
        self.set_int_literal_types(setup.int_literal_types.clone());
        self.set_trait_call_sites(setup.trait_call_sites.clone());
        let top_level_fns = self.register_items(program);
        self.declared_functions = self.functions.len();
        for (name, sent) in &setup.consts {
            if let Some(v) = sent.clone().decode().pop() {
                self.env.define(name, v);
            }
        }
        for (name, sent) in &setup.statics {
            if let Some(v) = sent.clone().decode().pop() {
                self.statics.insert(name.clone(), v);
            }
        }
        let module_scope = self.env.snapshot();
        for id in top_level_fns.iter().copied() {
            self.functions[id].closure = module_scope.clone();
        }
        self.module_scope = module_scope;
        self.program = Some(program);
        self.pool = Some(shared);
        self.is_worker = true;
    }

    fn worker_loop(&mut self, shared: &Arc<PoolShared>) {
        while let Some(slot) = shared.next() {
            if let Some(task) = slot.claim() {
                if let Some(hook) = &shared.hook {
                    hook();
                }
                let outcome = self.run_send_task(task);
                slot.finish(outcome);
            }
        }
    }
}
