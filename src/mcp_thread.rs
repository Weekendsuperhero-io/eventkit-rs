//! The one thread the MCP server touches EventKit on.
//!
//! Every tool call and every prompt is handed to this thread ([`run`]). The
//! store cache (`StoreCache` in `imp.rs`) keeps one `EKEventStore` per
//! thread, so the server holds exactly one store however many connections,
//! sessions or worker threads its host has.
//!
//! It used to hold a store for every tokio worker that had ever served a
//! tool, and two of them (reminders and events), so twenty on a ten-core Mac.
//! Each one is a connection to the calendar daemon, and every authorization
//! change resets all of them. On 2026-10-14 and 2026-10-15 Agent Muse's beta
//! was refused with `EKCADErrorDomain 1021`: *"This process has too many
//! EKEventStore instances. Use fewer event stores."*
//!
//! The thread runs a current-thread tokio runtime. The MCP service loop is
//! NOT on it: rmcp reads the transport and answers pings on the host's
//! runtime, so an EventKit call that waits on a person (the consent dialog,
//! up to two minutes) does not make the host judge the server dead. Tool
//! calls wait for one another instead, which one store needs anyway.

use std::future::Future;
use std::sync::OnceLock;

use tokio::runtime::Handle;
use tokio::task::{AbortHandle, JoinError, JoinHandle};

/// The runtime on the EventKit thread. The thread starts on first use and
/// lives as long as the process.
fn handle() -> &'static Handle {
    static HANDLE: OnceLock<Handle> = OnceLock::new();
    HANDLE.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .name("eventkit".to_string())
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("build the EventKit thread's runtime");
                tx.send(runtime.handle().clone())
                    .expect("the first caller waits for this handle");
                runtime.block_on(std::future::pending::<()>());
            })
            .expect("spawn the EventKit thread");
        rx.recv().expect("the EventKit thread sends its handle")
    })
}

/// Aborts the task it guards when dropped. The caller's future goes away
/// when its request is cancelled, and the work it was waiting for then
/// stops at its next `.await`.
struct AbortOnDrop(AbortHandle);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Run `work` on the EventKit thread and wait for its output.
///
/// A panic in `work` comes back as `Err`, and the thread goes on serving the
/// next call.
pub(super) async fn run<F>(work: F) -> Result<F::Output, JoinError>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    let task = spawn(work);
    let _abort = AbortOnDrop(task.abort_handle());
    task.await
}

/// Start `work` on the EventKit thread and hand back its handle, for a caller
/// that keeps the work past a single `.await`: a call that may become a task
/// (`mcp_tasks.rs`). Dropping the handle does NOT stop the work, so that
/// caller owns the abort.
pub(super) fn spawn<F>(work: F) -> JoinHandle<F::Output>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    handle().spawn(work)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    /// However many callers, on however many threads, the work runs on one
    /// thread: the one named `eventkit`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn every_call_runs_on_the_one_eventkit_thread() {
        let mut calls = tokio::task::JoinSet::new();
        for _ in 0..32 {
            calls.spawn(run(async {
                let me = std::thread::current();
                (me.id(), me.name().map(str::to_string))
            }));
        }
        let mut seen = Vec::new();
        while let Some(call) = calls.join_next().await {
            seen.push(call.expect("the caller task").expect("the work"));
        }
        let first = seen[0].clone();
        assert!(seen.iter().all(|s| *s == first), "{seen:?}");
        assert_eq!(first.1.as_deref(), Some("eventkit"));
        assert_ne!(first.0, std::thread::current().id());
    }

    /// A panicking call is an `Err` for its caller and nothing else: the
    /// thread, and with it the cached store, stays up for the next call.
    #[tokio::test]
    async fn a_panic_is_an_error_and_the_thread_keeps_serving() {
        let panicked = run(async { panic!("a tool panicked") }).await;
        assert!(panicked.is_err_and(|e| e.is_panic()));
        assert_eq!(run(async { 7 }).await.expect("the next call"), 7);
    }

    /// Dropping the caller, as rmcp does with a cancelled request, aborts
    /// the work at its next `.await`. The work is seen to start first, so a
    /// pass cannot come from work that never ran.
    #[tokio::test]
    async fn dropping_the_caller_aborts_the_work() {
        let started = Arc::new(AtomicBool::new(false));
        let finished = Arc::new(AtomicBool::new(false));
        let (began, ended) = (Arc::clone(&started), Arc::clone(&finished));
        let caller = tokio::spawn(run(async move {
            began.store(true, Ordering::SeqCst);
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            ended.store(true, Ordering::SeqCst);
        }));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !started.load(Ordering::SeqCst) {
            assert!(
                std::time::Instant::now() < deadline,
                "the work never started"
            );
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        caller.abort();
        tokio::time::sleep(std::time::Duration::from_millis(400)).await;
        assert!(
            !finished.load(Ordering::SeqCst),
            "the work ran on after its caller went away"
        );
    }
}
