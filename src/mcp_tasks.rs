//! MCP Tasks (SEP-2663, the `io.modelcontextprotocol/tasks` extension): a slow
//! tool call becomes a task the client polls, instead of a request it has to
//! hold open past its own timeout.
//!
//! **The server decides, and decides late.** A call becomes a task only when
//! all four hold: the tool is one of [`TASK_TOOLS`], the client declared the
//! extension, the call is still running after [`TaskConfig::promote_after`],
//! and admission ([`TaskConfig::max_active`], [`TaskConfig::max_tracked`])
//! lets it in. Anything else runs exactly as a plain call always has, so a
//! client that never declared tasks sees no difference at all.
//!
//! **The token rule.** rmcp cancels a request's token when it sends that
//! request's response. A task's response is the `CreateTaskResult`, sent the
//! moment the call is promoted, so an operation running on the request's token
//! would cancel itself right there. Every promotable operation therefore runs
//! on a child of the server's own lifetime token, linked to the request's
//! token only while it is still a request.
//!
//! **Threads.** The operation runs on the one EventKit thread
//! (`mcp_thread.rs`), like every other call: EventKit stores are cached per
//! thread, so any other thread would build a store of its own (see
//! [`super::EventKitServer`]). Only the bookkeeping — promotion, the task's
//! status, the request's token — stays on rmcp's runtime. EventKit calls take
//! turns on that thread, so a promoted call holds it while it runs, which is
//! why [`TaskConfig::max_active`] is small.
//!
//! **Cancellation is cooperative.** `tasks/cancel` cancels the operation's
//! token; a batch stops between items ([`stop_requested`]), and a single
//! EventKit call runs to its end. A task whose tool saw the cancellation is
//! `cancelled`; one that finished first is `completed`. rmcp's TTL is the only
//! hard stop, and even it cancels the token first, so a synchronous batch (which
//! an abort cannot reach) stops between items. Closing the connection is not a
//! hard stop: an operation still running when the shutdown grace ends is
//! detached and stops at its next check (see [`Operation`]).
//!
//! **Progress.** After promotion a batch reports how far it got as the task's
//! status message, and `tasks/get` carries the numbers under
//! [`PROGRESS_META_KEY`].

use std::{
    collections::{BTreeMap, VecDeque},
    future::Future,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use rmcp::{
    ErrorData as McpError,
    model::{
        CallToolResponse, CallToolResult, CreateTaskResult, GetTaskResult, InputResponses,
        MetaObject, Task,
    },
    task_manager::{TaskContext, TaskExit, TaskManager, TaskOptions},
};
use tokio::task::{JoinError, JoinHandle};
use tokio_util::sync::CancellationToken;

/// The tools a call to which may become a task: full-calendar and full-list
/// scans, the batches, and the two that wait on the user or the hardware (the
/// consent dialog, a location fix).
pub(super) const TASK_TOOLS: &[&str] = &[
    "list_reminders",
    "list_events",
    "search",
    "batch_delete",
    "batch_move",
    "batch_update",
    "request_access",
    "get_current_location",
];

/// The `_meta` key under which `tasks/get` reports a running task's latest
/// progress, as `{"progress": n, "total": m}`.
pub(super) const PROGRESS_META_KEY: &str = "io.agentmuse/progress";

#[derive(Debug, Clone, Copy)]
pub(super) struct TaskConfig {
    /// How long a call runs as a plain request before it may become a task.
    pub(super) promote_after: Duration,
    /// rmcp's hard stop: a task still running this long after it was created
    /// is aborted and marked `failed`, and a finished one is kept this long
    /// after it ends.
    pub(super) ttl: Duration,
    /// The polling interval the task suggests to the client.
    pub(super) poll_interval: Duration,
    /// Tasks running at once, each queued on the one EventKit thread. A call
    /// past this stays a request.
    pub(super) max_active: usize,
    /// Tasks held at once, running and finished. A call past this stays a
    /// request.
    pub(super) max_tracked: usize,
    /// How long running tasks get to stop when the connection closes, before
    /// whatever is left is detached (see [`Operation`]).
    pub(super) shutdown_grace: Duration,
}

impl TaskConfig {
    pub(super) const DEFAULT: Self = Self {
        promote_after: Duration::from_secs(1),
        ttl: Duration::from_secs(60 * 60),
        poll_interval: Duration::from_secs(1),
        max_active: 4,
        max_tracked: 256,
        shutdown_grace: Duration::from_secs(2),
    };
}

tokio::task_local! {
    /// The promotable call the current tool is running for, if any.
    static TOOL_RUN: Arc<ToolRun>;
}

/// One promotable call, as seen by the tool running it.
#[derive(Default)]
pub(super) struct ToolRun {
    /// Set once, when the call is promoted: from then on it is a task.
    task: OnceLock<PromotedRun>,
    /// Whether the operation saw a cancellation ([`stop_requested`]).
    cancel_observed: AtomicBool,
}

struct PromotedRun {
    context: TaskContext,
    progress: Arc<ProgressBoard>,
}

impl ToolRun {
    fn current() -> Option<Arc<Self>> {
        TOOL_RUN.try_with(Arc::clone).ok()
    }

    fn cancel_observed(&self) -> bool {
        self.cancel_observed.load(Ordering::Relaxed)
    }
}

/// Whether the call this tool runs for has been cancelled, recording that the
/// tool saw it. A batch checks this between items.
pub(super) fn stop_requested(ct: &CancellationToken) -> bool {
    let cancelled = ct.is_cancelled();
    if cancelled && let Some(run) = ToolRun::current() {
        run.cancel_observed.store(true, Ordering::Relaxed);
    }
    cancelled
}

/// Report how far a batch got, once its call is a task (a plain request has
/// no progress token to report on here).
pub(super) fn report_progress(done: usize, total: usize) {
    let Some(run) = ToolRun::current() else {
        return;
    };
    let Some(promoted) = run.task.get() else {
        return;
    };
    promoted
        .context
        .set_status_message(format!("{done} of {total}"));
    promoted.progress.record(
        promoted.context.task_id(),
        u64::try_from(done).unwrap_or(u64::MAX),
        u64::try_from(total).unwrap_or(u64::MAX),
    );
}

/// Set the task's status message, once the call is one.
pub(super) fn report_status(message: impl Into<String>) {
    if let Some(promoted) = ToolRun::current().as_deref().and_then(|run| run.task.get()) {
        promoted.context.set_status_message(message);
    }
}

/// The latest progress of each task, for `tasks/get`.
#[derive(Default)]
struct ProgressBoard(parking_lot::Mutex<BTreeMap<String, (u64, u64)>>);

impl ProgressBoard {
    fn record(&self, task_id: &str, completed: u64, total: u64) {
        self.0
            .lock()
            .insert(task_id.to_string(), (completed, total));
    }

    fn get(&self, task_id: &str) -> Option<(u64, u64)> {
        self.0.lock().get(task_id).copied()
    }

    fn forget(&self, task_id: &str) {
        self.0.lock().remove(task_id);
    }
}

/// The spawned operation behind a call, with the token it runs on and the
/// connection's lifetime.
///
/// Dropped before it ends, it is stopped, and how depends on why:
///
/// - The connection is shutting down (its lifetime is cancelled, so this
///   operation's token already is): DETACHED. It runs on to its next check
///   and ends there, the way any cancelled call ends; an abort at an
///   arbitrary await could stop a batch between an item's change and its
///   commit.
/// - Anything else (rmcp's TTL, the one hard stop): its token is cancelled
///   first, so a synchronous batch on the EventKit thread, which no abort can
///   reach, stops between items; then it is aborted.
struct Operation {
    handle: JoinHandle<Result<CallToolResponse, McpError>>,
    token: CancellationToken,
    lifetime: CancellationToken,
}

impl Operation {
    async fn join(&mut self) -> Result<Result<CallToolResponse, McpError>, JoinError> {
        (&mut self.handle).await
    }
}

impl Drop for Operation {
    fn drop(&mut self) {
        self.token.cancel();
        if !self.lifetime.is_cancelled() {
            self.handle.abort();
        }
    }
}

/// The tasks of one connection: the rmcp store, the lifetime every operation
/// token descends from, and the admission bounds.
pub(super) struct TaskRuntime {
    manager: TaskManager,
    lifetime: CancellationToken,
    config: TaskConfig,
    /// Tasks created here that rmcp may still hold, with when they were made.
    tracked: parking_lot::Mutex<VecDeque<(String, Instant)>>,
    progress: Arc<ProgressBoard>,
}

impl TaskRuntime {
    pub(super) fn new(config: TaskConfig) -> Self {
        Self {
            manager: TaskManager::new(),
            lifetime: CancellationToken::new(),
            config,
            tracked: parking_lot::Mutex::new(VecDeque::new()),
            progress: Arc::new(ProgressBoard::default()),
        }
    }

    /// A token for one operation: a child of this connection's lifetime, so
    /// it outlives the request that started it (see the module doc).
    pub(super) fn operation_token(&self) -> CancellationToken {
        self.lifetime.child_token()
    }

    /// Run one promotable call.
    ///
    /// `operation` runs on the EventKit thread (`mcp_thread.rs`), the one
    /// thread that holds the store, with this call's `ToolRun` in scope there:
    /// a tokio task-local does not follow work to another runtime, so it is
    /// set on the spawned operation itself, where a batch reads it to report
    /// progress and to record that it saw a cancellation.
    ///
    /// `operation` runs on `operation_ct` (from [`Self::operation_token`]).
    /// While the call is a request, a cancellation of `request_ct` (a
    /// `notifications/cancelled`, or the connection closing) is passed on to
    /// the operation, which then stops where a plain call stops. Once
    /// promoted, the request's token is left behind: rmcp cancels it as it
    /// sends the `CreateTaskResult`.
    pub(super) async fn run_call<F>(
        &self,
        tool: &str,
        request_ct: CancellationToken,
        operation_ct: CancellationToken,
        operation: F,
    ) -> Result<CallToolResponse, McpError>
    where
        F: Future<Output = Result<CallToolResponse, McpError>> + Send + 'static,
    {
        let run = Arc::new(ToolRun::default());
        let mut operation = Operation {
            handle: super::eventkit_thread::spawn(TOOL_RUN.scope(Arc::clone(&run), operation)),
            token: operation_ct.clone(),
            lifetime: self.lifetime.clone(),
        };
        let promote_at = tokio::time::sleep(self.config.promote_after);
        tokio::pin!(promote_at);
        let mut promotable = true;
        loop {
            tokio::select! {
                biased;
                joined = operation.join() => return settle_request(joined),
                () = request_ct.cancelled() => {
                    operation_ct.cancel();
                    return settle_request(operation.join().await);
                }
                () = &mut promote_at, if promotable => {
                    match self.promote(tool, &run, operation, operation_ct.clone()) {
                        Ok(task) => {
                            return Ok(CallToolResponse::Task(CreateTaskResult::new(task)));
                        }
                        // Not admitted: the call carries on as a request.
                        Err(not_admitted) => {
                            operation = not_admitted;
                            promotable = false;
                        }
                    }
                }
            }
        }
    }

    /// Turn a running call into a task, or hand the operation back when
    /// admission refuses it.
    fn promote(
        &self,
        tool: &str,
        run: &Arc<ToolRun>,
        operation: Operation,
        operation_ct: CancellationToken,
    ) -> Result<Task, Operation> {
        let mut tracked = self.tracked.lock();
        if !self.admits(&mut tracked) {
            tracing::debug!(
                target: "eventkit",
                tool,
                running = self.manager.running_task_count(),
                tracked = tracked.len(),
                "slow call stays a request: task admission is full"
            );
            return Err(operation);
        }
        let run = Arc::clone(run);
        let progress = Arc::clone(&self.progress);
        let options = TaskOptions::new()
            .with_ttl_ms(duration_ms(self.config.ttl))
            .with_poll_interval_ms(duration_ms(self.config.poll_interval))
            .with_status_message(format!("Running {tool}"));
        let task = self.manager.spawn(options, move |context| {
            // From here on the call is a task: progress becomes its status.
            let _ = run.task.set(PromotedRun {
                context: context.clone(),
                progress,
            });
            Box::pin(drive_task(operation, operation_ct, run, context))
        });
        tracked.push_back((task.task_id.clone(), Instant::now()));
        tracing::info!(
            target: "eventkit",
            tool,
            task = %task.task_id,
            "slow call became a task"
        );
        Ok(task)
    }

    /// Whether one more task fits, after forgetting the ones rmcp has evicted.
    fn admits(&self, tracked: &mut VecDeque<(String, Instant)>) -> bool {
        if self.lifetime.is_cancelled() {
            return false;
        }
        // rmcp keeps a finished task for one TTL after it ends, so a task
        // younger than one TTL cannot have been evicted yet.
        tracked.retain(|(task_id, created)| {
            let held =
                created.elapsed() < self.config.ttl || self.manager.get_task(task_id).is_ok();
            if !held {
                self.progress.forget(task_id);
            }
            held
        });
        self.manager.running_task_count() < self.config.max_active
            && tracked.len() < self.config.max_tracked
    }

    /// `tasks/get`, with the latest progress under [`PROGRESS_META_KEY`].
    pub(super) fn get(&self, task_id: &str) -> Result<GetTaskResult, McpError> {
        let mut result = GetTaskResult::new(self.manager.get_task(task_id)?);
        if let Some((progress, total)) = self.progress.get(task_id) {
            let mut meta = MetaObject::default();
            meta.0.insert(
                PROGRESS_META_KEY.to_string(),
                serde_json::json!({ "progress": progress, "total": total }),
            );
            result.meta = Some(meta);
        }
        Ok(result)
    }

    /// `tasks/update`. No tool here asks for input mid-task, so this only
    /// validates the task id; rmcp ignores responses nothing is waiting for.
    pub(super) fn update(&self, task_id: &str, responses: InputResponses) -> Result<(), McpError> {
        self.manager.update_task(task_id, responses)
    }

    /// `tasks/cancel`: records the request; [`drive_task`] passes it on.
    pub(super) fn cancel(&self, task_id: &str) -> Result<(), McpError> {
        self.manager.cancel_task(task_id)
    }

    /// The connection is gone, and its tasks with it: nothing can poll them
    /// any more. Ask every operation to stop, give them
    /// [`TaskConfig::shutdown_grace`] to do so, then drop the tasks. An
    /// operation still running is detached rather than aborted (see
    /// [`Operation`]): it stops at its next check.
    pub(super) async fn shutdown(&self) {
        self.lifetime.cancel();
        let deadline = tokio::time::Instant::now() + self.config.shutdown_grace;
        while self.manager.running_task_count() > 0 && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        self.manager.shutdown();
    }
}

/// The body of a task: wait for the operation, passing `tasks/cancel` on to it.
async fn drive_task(
    mut operation: Operation,
    operation_ct: CancellationToken,
    run: Arc<ToolRun>,
    context: TaskContext,
) -> Result<CallToolResult, TaskExit> {
    let joined = tokio::select! {
        biased;
        joined = operation.join() => joined,
        () = context.cancelled() => {
            operation_ct.cancel();
            operation.join().await
        }
    };
    settle_task(joined, &run, &context)
}

/// A task's terminal status. An operation that stopped because it was asked
/// to is `cancelled`. An `isError` result is otherwise `completed`: SEP-2663
/// reserves `failed` for a call that produced no result.
fn settle_task(
    joined: Result<Result<CallToolResponse, McpError>, JoinError>,
    run: &ToolRun,
    context: &TaskContext,
) -> Result<CallToolResult, TaskExit> {
    if context.is_cancel_requested() && run.cancel_observed() {
        return Err(TaskExit::Cancelled);
    }
    match joined {
        Ok(Ok(CallToolResponse::Complete(result))) => Ok(result),
        Ok(Ok(_)) => Err(TaskExit::Error(McpError::internal_error(
            "the tool answered with something other than a result, which a task cannot carry",
            None,
        ))),
        Ok(Err(error)) => Err(TaskExit::Error(error)),
        Err(join_error) => Err(TaskExit::Error(operation_lost(&join_error))),
    }
}

/// A call's answer while it is still a request.
fn settle_request(
    joined: Result<Result<CallToolResponse, McpError>, JoinError>,
) -> Result<CallToolResponse, McpError> {
    joined.unwrap_or_else(|join_error| Err(operation_lost(&join_error)))
}

fn operation_lost(join_error: &JoinError) -> McpError {
    if join_error.is_panic() {
        McpError::internal_error("the tool panicked", None)
    } else {
        McpError::internal_error("the tool was stopped before it finished", None)
    }
}

fn duration_ms(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmcp::model::{ContentBlock, TaskStatus};

    fn quick() -> TaskConfig {
        TaskConfig {
            promote_after: Duration::from_millis(20),
            ..TaskConfig::DEFAULT
        }
    }

    fn done(message: &str) -> Result<CallToolResponse, McpError> {
        Ok(CallToolResponse::Complete(CallToolResult::success(vec![
            ContentBlock::text(message.to_string()),
        ])))
    }

    async fn settled(runtime: &TaskRuntime, task_id: &str) -> GetTaskResult {
        for _ in 0..200 {
            let result = runtime.get(task_id).expect("task is held");
            if result.task.status().is_terminal() {
                return result;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("task {task_id} never settled");
    }

    fn task_id(response: Result<CallToolResponse, McpError>) -> String {
        match response {
            Ok(CallToolResponse::Task(created)) => created.task.task_id,
            other => panic!("expected a task, got {other:?}"),
        }
    }

    /// A call that finishes inside the budget is answered inline.
    #[tokio::test]
    async fn a_quick_call_stays_a_request() {
        let runtime = TaskRuntime::new(quick());
        let response = runtime
            .run_call(
                "list_events",
                CancellationToken::new(),
                runtime.operation_token(),
                async { done("quick") },
            )
            .await;
        assert!(matches!(response, Ok(CallToolResponse::Complete(_))));
        assert!(runtime.tracked.lock().is_empty());
    }

    /// What an operation saw of where it ran: the thread's name, whether its
    /// call's `ToolRun` was in scope, and whether that run was a task yet.
    type Seen = Arc<parking_lot::Mutex<Option<(Option<String>, bool, bool)>>>;

    fn look_around(seen: &Seen) {
        let run = ToolRun::current();
        *seen.lock() = Some((
            std::thread::current().name().map(str::to_string),
            run.is_some(),
            run.is_some_and(|run| run.task.get().is_some()),
        ));
    }

    /// A promotable call's tool runs on the one EventKit thread, where the
    /// store lives — not on rmcp's worker, which would build a store of its
    /// own — with its `ToolRun` in scope there, since a task-local does not
    /// follow work onto another runtime by itself.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_task_mode_call_runs_on_the_eventkit_thread() {
        let runtime = TaskRuntime::new(quick());
        let seen = Seen::default();
        let probe = Arc::clone(&seen);
        let response = runtime
            .run_call(
                "list_events",
                CancellationToken::new(),
                runtime.operation_token(),
                async move {
                    look_around(&probe);
                    done("quick")
                },
            )
            .await;
        assert!(matches!(response, Ok(CallToolResponse::Complete(_))));
        let (thread, in_scope, promoted) = seen.lock().take().expect("the operation ran");
        assert_eq!(thread.as_deref(), Some("eventkit"));
        assert!(
            in_scope,
            "the call's ToolRun is in scope on the EventKit thread"
        );
        assert!(!promoted);
    }

    /// Promotion does not move the work: a task keeps running on the EventKit
    /// thread, and its tool sees the promoted run, which is what progress
    /// and status reports go through.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_promoted_task_runs_on_the_eventkit_thread_and_sees_its_run() {
        let runtime = TaskRuntime::new(quick());
        let seen = Seen::default();
        let probe = Arc::clone(&seen);
        let response = runtime
            .run_call(
                "batch_update_reminders",
                CancellationToken::new(),
                runtime.operation_token(),
                async move {
                    tokio::time::sleep(Duration::from_millis(120)).await;
                    look_around(&probe);
                    done("slow")
                },
            )
            .await;
        let id = task_id(response);
        let finished = settled(&runtime, &id).await;
        assert_eq!(finished.task.status(), TaskStatus::Completed);
        let (thread, in_scope, promoted) = seen.lock().take().expect("the operation ran");
        assert_eq!(thread.as_deref(), Some("eventkit"));
        assert!(in_scope && promoted, "the tool sees its promoted run");
    }

    /// The token rule: rmcp cancels the request's token as it sends the
    /// `CreateTaskResult`, and the task carries on regardless.
    #[tokio::test]
    async fn a_promoted_task_outlives_its_request_token() {
        let runtime = TaskRuntime::new(quick());
        let request_ct = CancellationToken::new();
        let operation_ct = runtime.operation_token();
        let seen = operation_ct.clone();
        let response = runtime
            .run_call(
                "list_events",
                request_ct.clone(),
                operation_ct,
                async move {
                    tokio::time::sleep(Duration::from_millis(80)).await;
                    done(if seen.is_cancelled() {
                        "cancelled"
                    } else {
                        "finished"
                    })
                },
            )
            .await;
        let task_id = task_id(response);
        request_ct.cancel();
        let result = settled(&runtime, &task_id).await;
        assert_eq!(result.task.status(), TaskStatus::Completed, "{result:?}");
        let payload = serde_json::to_value(&result).unwrap();
        assert_eq!(payload["result"]["content"][0]["text"], "finished");
    }

    /// A batch stopped between items by `tasks/cancel` settles `cancelled`,
    /// and its status says how far it got.
    #[tokio::test]
    async fn a_batch_cancelled_between_items_settles_cancelled() {
        let runtime = TaskRuntime::new(quick());
        let operation_ct = runtime.operation_token();
        let watched = operation_ct.clone();
        let response = runtime
            .run_call(
                "batch_delete",
                CancellationToken::new(),
                operation_ct,
                async move {
                    let total = 1_000;
                    for done in 0..total {
                        if stop_requested(&watched) {
                            report_status(format!("Stopped after {done} of {total}"));
                            break;
                        }
                        report_progress(done + 1, total);
                        tokio::time::sleep(Duration::from_millis(5)).await;
                    }
                    done_batch()
                },
            )
            .await;
        let task_id = task_id(response);
        tokio::time::sleep(Duration::from_millis(60)).await;
        runtime.cancel(&task_id).unwrap();
        let result = settled(&runtime, &task_id).await;
        assert_eq!(result.task.status(), TaskStatus::Cancelled, "{result:?}");
        assert!(
            result
                .task
                .task
                .status_message
                .as_deref()
                .is_some_and(|message| message.starts_with("Stopped after")),
            "{result:?}"
        );
    }

    fn done_batch() -> Result<CallToolResponse, McpError> {
        done("batch")
    }

    /// A single EventKit call cannot stop part-way: a cancel that arrives
    /// while it runs leaves it `completed`.
    #[tokio::test]
    async fn a_call_that_cannot_stop_completes_despite_a_cancel() {
        let runtime = TaskRuntime::new(quick());
        let response = runtime
            .run_call(
                "request_access",
                CancellationToken::new(),
                runtime.operation_token(),
                async {
                    tokio::time::sleep(Duration::from_millis(80)).await;
                    done("granted")
                },
            )
            .await;
        let task_id = task_id(response);
        runtime.cancel(&task_id).unwrap();
        let result = settled(&runtime, &task_id).await;
        assert_eq!(result.task.status(), TaskStatus::Completed, "{result:?}");
    }

    /// A panic fails the task instead of stranding it `working` (the 0.6.x
    /// bug: a panicking tool held its slot until the process exited).
    #[tokio::test]
    async fn a_panicking_operation_fails_its_task() {
        let runtime = TaskRuntime::new(quick());
        let response = runtime
            .run_call(
                "search",
                CancellationToken::new(),
                runtime.operation_token(),
                async {
                    tokio::time::sleep(Duration::from_millis(60)).await;
                    panic!("boom");
                },
            )
            .await;
        let task_id = task_id(response);
        let result = settled(&runtime, &task_id).await;
        assert_eq!(result.task.status(), TaskStatus::Failed, "{result:?}");
        assert_eq!(runtime.manager.running_task_count(), 0, "no slot is held");
    }

    /// Admission is a ceiling: past it, a slow call finishes as a request.
    #[tokio::test]
    async fn a_call_past_admission_finishes_as_a_request() {
        let runtime = TaskRuntime::new(TaskConfig {
            max_active: 0,
            ..quick()
        });
        let response = runtime
            .run_call(
                "list_reminders",
                CancellationToken::new(),
                runtime.operation_token(),
                async {
                    tokio::time::sleep(Duration::from_millis(60)).await;
                    done("inline")
                },
            )
            .await;
        assert!(matches!(response, Ok(CallToolResponse::Complete(_))));
    }

    /// Closing the connection stops running tasks and drops them.
    #[tokio::test]
    async fn shutdown_stops_running_tasks() {
        let runtime = TaskRuntime::new(TaskConfig {
            shutdown_grace: Duration::from_millis(100),
            ..quick()
        });
        let operation_ct = runtime.operation_token();
        let watched = operation_ct.clone();
        let response = runtime
            .run_call(
                "batch_move",
                CancellationToken::new(),
                operation_ct,
                async move {
                    watched.cancelled().await;
                    done("stopped")
                },
            )
            .await;
        let task_id = task_id(response);
        runtime.shutdown().await;
        assert_eq!(runtime.manager.running_task_count(), 0);
        assert!(runtime.get(&task_id).is_err());
    }

    /// An operation still between two checks when the grace runs out is not
    /// cut there: it is detached, finishes the item it is on, and stops.
    #[tokio::test]
    async fn shutdown_lets_an_operation_reach_its_next_check() {
        let runtime = TaskRuntime::new(TaskConfig {
            shutdown_grace: Duration::from_millis(100),
            ..quick()
        });
        let item_done = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&item_done);
        let response = runtime
            .run_call(
                "batch_move",
                CancellationToken::new(),
                runtime.operation_token(),
                async move {
                    tokio::time::sleep(Duration::from_millis(400)).await;
                    flag.store(true, Ordering::SeqCst);
                    done("stopped after the item")
                },
            )
            .await;
        task_id(response);
        runtime.shutdown().await;
        assert!(!item_done.load(Ordering::SeqCst), "the grace ended first");
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert!(
            item_done.load(Ordering::SeqCst),
            "the item was finished, not cut"
        );
    }

    /// rmcp's TTL aborts a task that runs past it, and cancels the
    /// operation's token first: a synchronous batch, which the abort cannot
    /// reach, then stops between items.
    #[tokio::test]
    async fn the_ttl_cancels_the_operations_token() {
        let runtime = TaskRuntime::new(TaskConfig {
            ttl: Duration::from_millis(150),
            ..quick()
        });
        let operation_ct = runtime.operation_token();
        let token = operation_ct.clone();
        let response = runtime
            .run_call(
                "batch_delete",
                CancellationToken::new(),
                operation_ct,
                async {
                    tokio::time::sleep(Duration::from_secs(60)).await;
                    done("too late")
                },
            )
            .await;
        let task_id = task_id(response);
        // Polling is what runs rmcp's TTL sweep; the aborted driver, and the
        // operation with it, is dropped on the runtime's next turn after that.
        for _ in 0..100 {
            tokio::time::sleep(Duration::from_millis(20)).await;
            let _ = runtime.get(&task_id);
            if token.is_cancelled() {
                break;
            }
        }
        assert!(token.is_cancelled(), "the TTL cancelled the token");
    }
}
