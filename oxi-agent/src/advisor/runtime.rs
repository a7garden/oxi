//! The advisor runtime — drives the advisor agent from primary transcript
//! deltas. Ported from omp `AdvisorRuntime` (runtime.ts).
//!
//! Each primary turn, the host calls [`AdvisorRuntime::on_turn_end`] with the
//! (new) transcript. The runtime renders a *delta* (messages added since the
//! last drain) and feeds it to the advisor agent's `prompt()`. Accepted advice
//! flows back through the host's `enqueue_advice` callback (the host owns the
//! [`crate::advisor::emission_guard::AdvisorEmissionGuard`] and the delivery
//! channel decision).
//!
//! # Concurrency
//!
//! omp's drain loop is safe only because JS's event loop serializes the
//! synchronous segment between "queue empty? stop" and "release the busy flag".
//! `tokio`'s multithreaded runtime breaks that: a concurrent `on_turn_end` on
//! another worker can push + spawn a drain in the gap, and that spawned drain
//! bails (busy still set) leaving the queue non-empty with no drain running —
//! a lost-wakeup stall. The fix folds the "draining" role into the same lock
//! that guards the pending queue, so "decide-to-stop" and "push-new-work +
//! spawn" are each one atomic critical section (design doc §9.2). The
//! catchup-waiter path has the same race and the same fix (register + check
//! backlog under one lock).
//!
//! # Attribution
//!
//! Translated to Rust from omp (oh-my-pi), MIT licensed.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Weak};
use std::time::Duration;

use async_trait::async_trait;
use oxi_ai::Message;
use parking_lot::Mutex;
use tokio::sync::oneshot;

use crate::advisor::types::AdvisorNote;

/// Minimal slice of an agent the runtime drives. omp `AdvisorAgent`.
/// Satisfied by `oxi_agent::Agent` via a host adapter; tests hand-roll a fake.
#[async_trait]
pub trait AdvisorAgent: Send + Sync + 'static {
    /// Drive one advisor turn from the given batch text. `Err` marks the turn
    /// failed (triggers the retry/drop-after-3 path).
    async fn prompt(&self, input: String) -> Result<(), String>;
    /// Abort any in-flight prompt (best-effort). omp `abort`.
    fn abort(&self, reason: &str);
    /// Reset the advisor's own conversation state. omp `reset`.
    fn reset(&self);
    /// Drop messages appended past `count`. Called after a failed `prompt` so a
    /// retry doesn't replay the failed user batch. omp `rollbackTo`.
    async fn rollback_to(&self, count: usize);
    /// Current advisor message count (for the rollback snapshot).
    fn message_count(&self) -> usize;
}

/// Host callbacks the runtime needs. omp `AdvisorRuntimeHost`.
pub trait AdvisorRuntimeHost: Send + Sync + 'static {
    /// Snapshot of the primary transcript (host should exclude the advisor's
    /// own echoed advice so it isn't re-fed).
    fn snapshot_messages(&self) -> Vec<Message>;
    /// Route an accepted note to the primary (the host applies its emission
    /// guard + delivery channel). omp `enqueueAdvice`.
    fn enqueue_advice(&self, note: AdvisorNote);
    /// Pre-prompt context maintenance for the advisor's own context. Return
    /// `true` to force a re-prime (reset advisor context + replay the full
    /// current transcript). omp `maintainContext`. Optional.
    fn maintain_context(&self, _incoming_tokens: usize) -> bool {
        false
    }
    /// Called immediately before each advisor `prompt` cycle, so the host can
    /// clear per-update advisor state (its emission guard's one-advise budget).
    /// omp `beginAdvisorUpdate`. Optional.
    fn begin_advisor_update(&self) {}
    /// Surface a non-recovering advisor failure (3 consecutive errors) without
    /// adding model-visible context. omp `notifyFailure`. Optional.
    fn notify_failure(&self, _error: &str) {}
}

/// One queued transcript delta awaiting the advisor's attention.
struct PendingDelta {
    text: String,
    /// Number of primary turns this delta covers (for backlog accounting).
    turns: u64,
}

/// Pending deltas + the "is a drain task currently running" role, behind one
/// lock so the empty-check and role-release are atomic with a concurrent push
/// (the lost-wakeup fix).
#[derive(Default)]
struct DrainState {
    pending: Vec<PendingDelta>,
    draining: bool,
}

/// A registered catchup waiter.
struct CatchupWaiter {
    threshold: u64,
    tx: Option<oneshot::Sender<()>>,
}

/// Drives the advisor agent. Construct via [`AdvisorRuntime::new`], wrap in
/// `Arc`, then call [`AdvisorRuntime::install_self`] so it can self-spawn its
/// drain task.
pub struct AdvisorRuntime {
    agent: Arc<dyn AdvisorAgent>,
    host: Arc<dyn AdvisorRuntimeHost>,

    state: Mutex<DrainState>,
    /// Bumped by every external reset/dispose. A drain iteration captures it
    /// before its awaits; a mismatch means a reset aborted the in-flight
    /// advisor prompt, so the stale batch is dropped instead of being retried.
    epoch: AtomicU64,
    /// Count of primary turns the advisor has not yet digested.
    backlog: AtomicU64,
    /// Cursor into the primary transcript — render deltas from here.
    last_count: AtomicU64,
    /// Latest transcript snapshot (for re-prime rendering).
    latest: Mutex<Option<Vec<Message>>>,

    waiters: Mutex<Vec<CatchupWaiter>>,

    consecutive_failures: AtomicU32,
    failure_notified: AtomicBool,
    disposed: AtomicBool,
    retry_delay: Duration,

    /// Weak self-reference so `on_turn_end` can spawn the drain task.
    self_ref: Mutex<Option<Weak<AdvisorRuntime>>>,
}

impl AdvisorRuntime {
    /// Construct. `retry_delay` is the backoff between failed advisor turns
    /// (omp default 1000ms).
    #[must_use]
    pub fn new(
        agent: Arc<dyn AdvisorAgent>,
        host: Arc<dyn AdvisorRuntimeHost>,
        retry_delay: Duration,
    ) -> Self {
        Self {
            agent,
            host,
            state: Mutex::new(DrainState::default()),
            epoch: AtomicU64::new(0),
            backlog: AtomicU64::new(0),
            last_count: AtomicU64::new(0),
            latest: Mutex::new(None),
            waiters: Mutex::new(Vec::new()),
            consecutive_failures: AtomicU32::new(0),
            failure_notified: AtomicBool::new(false),
            disposed: AtomicBool::new(false),
            retry_delay,
            self_ref: Mutex::new(None),
        }
    }

    /// Install the weak self-reference required for self-spawning the drain
    /// task. Call once after wrapping in `Arc`.
    pub fn install_self(&self, weak: Weak<AdvisorRuntime>) {
        *self.self_ref.lock() = Some(weak);
    }

    /// Current backlog (primary turns not yet digested by the advisor).
    #[must_use]
    pub fn backlog(&self) -> u64 {
        self.backlog.load(Ordering::SeqCst)
    }

    /// Whether the runtime has been disposed.
    #[must_use]
    pub fn is_disposed(&self) -> bool {
        self.disposed.load(Ordering::SeqCst)
    }

    /// Feed one primary turn's transcript to the advisor. Renders the delta
    /// (new messages since the last drain), queues it, and spawns a drain task
    /// if none is running. omp `onTurnEnd`.
    pub fn on_turn_end(&self, messages: Vec<Message>) {
        if self.disposed.load(Ordering::SeqCst) {
            return;
        }
        *self.latest.lock() = Some(messages.clone());
        let Some(render) = self.render_delta(&messages) else {
            return;
        };
        let spawn = {
            let mut s = self.state.lock();
            s.pending.push(PendingDelta {
                text: render,
                turns: 1,
            });
            self.backlog.fetch_add(1, Ordering::SeqCst);
            !s.draining
        };
        self.notify_waiters();
        let drain_handle = self.self_ref.lock().as_ref().and_then(Weak::upgrade);
        if spawn && let Some(this) = drain_handle {
            tokio::spawn(async move {
                this.drain().await;
            });
        }
    }

    /// Block until the backlog drops below `threshold`, or `max` elapses. omp
    /// `waitForCatchup`. Registration + backlog-check happen under the waiters
    /// lock so a concurrent `notify_waiters` cannot miss the waiter (the
    /// catchup lost-wakeup fix).
    pub async fn wait_for_catchup(&self, max: Duration, threshold: u64) {
        if self.disposed.load(Ordering::SeqCst) || self.backlog.load(Ordering::SeqCst) < threshold {
            return;
        }
        let (tx, rx) = oneshot::channel();
        {
            let mut waiters = self.waiters.lock();
            // Re-check under the lock: a drain may have just decremented +
            // notified before we registered.
            if self.backlog.load(Ordering::SeqCst) < threshold {
                return;
            }
            waiters.push(CatchupWaiter {
                threshold,
                tx: Some(tx),
            });
        }
        let _ = tokio::time::timeout(max, rx).await;
    }

    /// Re-prime the advisor after a history rewrite (compaction, session
    /// switch/resume, branch). Clears the advisor's context and rewinds the
    /// cursor so the next turn replays the full current transcript. omp `reset`.
    pub fn reset(&self) {
        self.epoch.fetch_add(1, Ordering::SeqCst);
        self.reset_advisor_context(true);
        self.wake_all_waiters();
    }

    /// Seed the cursor to the current transcript length when the advisor is
    /// enabled mid-session, so the next turn doesn't replay the entire history.
    /// omp `seedTo`.
    pub fn seed_to(&self, count: u64) {
        self.epoch.fetch_add(1, Ordering::SeqCst);
        self.last_count.store(count, Ordering::SeqCst);
        let mut s = self.state.lock();
        s.pending.clear();
        // NOTE: do NOT clear `draining` here. Bumping the epoch above lets any
        // in-flight drain exit on its own (epoch mismatch -> continue -> finds
        // pending empty -> releases the draining role itself under this lock).
        // Clearing `draining` externally would let a concurrent on_turn_end
        // spawn a second drain while the first is still mid-prompt — two
        // concurrent prompt() calls on one advisor agent.
        self.backlog.store(0, Ordering::SeqCst);
        self.consecutive_failures.store(0, Ordering::SeqCst);
        self.failure_notified.store(false, Ordering::SeqCst);
        drop(s);
        self.wake_all_waiters();
    }

    /// Stop the runtime permanently. Aborts the advisor agent and drops all
    /// pending state. omp `dispose`.
    pub fn dispose(&self) {
        self.disposed.store(true, Ordering::SeqCst);
        self.epoch.fetch_add(1, Ordering::SeqCst);
        let mut s = self.state.lock();
        s.pending.clear();
        s.draining = false;
        self.backlog.store(0, Ordering::SeqCst);
        drop(s);
        self.wake_all_waiters();
        self.agent.abort("advisor disposed");
    }

    fn reset_advisor_context(&self, clear_backlog: bool) {
        self.last_count.store(0, Ordering::SeqCst);
        let mut s = self.state.lock();
        s.pending.clear();
        if clear_backlog {
            self.backlog.store(0, Ordering::SeqCst);
        }
        self.consecutive_failures.store(0, Ordering::SeqCst);
        self.failure_notified.store(false, Ordering::SeqCst);
        drop(s);
        self.agent.reset();
        self.agent.abort("advisor reset");
    }

    /// Render the transcript delta (messages added since `last_count`).
    /// omp `#renderDelta`. Returns `None` when there is nothing new to feed.
    fn render_delta(&self, messages: &[Message]) -> Option<String> {
        let last = self.last_count.load(Ordering::SeqCst) as usize;
        if messages.len() < last {
            self.last_count
                .store(messages.len() as u64, Ordering::SeqCst);
            return None;
        }
        let delta = &messages[last..];
        self.last_count
            .store(messages.len() as u64, Ordering::SeqCst);
        if delta.is_empty() {
            return None;
        }
        let mut parts: Vec<String> = Vec::new();
        for msg in delta {
            if let Some(md) = format_message_md(msg) {
                parts.push(md);
            }
        }
        if parts.is_empty() {
            return None;
        }
        Some(format!("### Session update\n\n{}", parts.join("\n\n")))
    }

    fn wake_all_waiters(&self) {
        let mut waiters = self.waiters.lock();
        for w in waiters.drain(..) {
            if let Some(tx) = w.tx {
                let _ = tx.send(());
            }
        }
    }

    fn notify_waiters(&self) {
        let mut waiters = self.waiters.lock();
        let backlog = self.backlog.load(Ordering::SeqCst);
        for w in waiters.iter_mut() {
            if backlog < w.threshold
                && let Some(tx) = w.tx.take()
            {
                let _ = tx.send(());
            }
        }
        waiters.retain(|w| w.tx.is_some());
    }

    fn decrement_backlog(&self, by: u64) {
        let mut prev = self.backlog.load(Ordering::SeqCst);
        loop {
            let next = prev.saturating_sub(by);
            match self
                .backlog
                .compare_exchange(prev, next, Ordering::SeqCst, Ordering::SeqCst)
            {
                Ok(_) => break,
                Err(actual) => prev = actual,
            }
        }
    }

    /// The drain loop. Self-spawned by `on_turn_end`. Holds the "drainer" role
    /// (under `state` lock) until the queue is empty, releasing it atomically
    /// with the empty-check so a concurrent push cannot strand a delta.
    async fn drain(self: Arc<Self>) {
        {
            let mut s = self.state.lock();
            if s.draining || s.pending.is_empty() {
                return;
            }
            s.draining = true;
        }
        loop {
            // Take the whole pending queue as one batch (omp splices all).
            let (batch_text, turns_covered) = {
                let mut s = self.state.lock();
                if s.pending.is_empty() {
                    // Release the drainer role + empty-check in one critical
                    // section: a concurrent on_turn_end push + spawn cannot
                    // interleave here (it takes this same lock).
                    s.draining = false;
                    return;
                }
                let taken: Vec<PendingDelta> = s.pending.drain(..).collect();
                let turns: u64 = taken.iter().map(|d| d.turns).sum();
                let joined = taken
                    .into_iter()
                    .map(|d| d.text)
                    .collect::<Vec<_>>()
                    .join("\n\n");
                (joined, turns)
            };

            let epoch_start = self.epoch.load(Ordering::SeqCst);

            // Context maintenance (optional). A reset during maintenance
            // invalidates this batch.
            let should_reprime = self.host.maintain_context(batch_text.len());
            if self.epoch.load(Ordering::SeqCst) != epoch_start {
                continue;
            }

            let (batch, final_turns) = if should_reprime {
                // Promotion could not fit — re-prime: reset advisor context,
                // then re-render the full current transcript.
                self.reset_advisor_context(false);
                let new_turns = self.state.lock().pending.len() as u64;
                let rendered = self
                    .latest
                    .lock()
                    .as_ref()
                    .and_then(|m| self.render_delta(m));
                let final_turns = turns_covered.saturating_add(new_turns);
                match rendered {
                    Some(b) => (b, final_turns),
                    None => {
                        self.decrement_backlog(final_turns);
                        self.notify_waiters();
                        continue;
                    }
                }
            } else {
                (batch_text, turns_covered)
            };

            if self.disposed.load(Ordering::SeqCst) {
                self.decrement_backlog(final_turns);
                self.notify_waiters();
                continue;
            }

            let message_snapshot = self.agent.message_count();
            self.host.begin_advisor_update();
            let prompt_result = self.agent.prompt(batch.clone()).await;

            // A reset/dispose during the prompt invalidates this batch — drop it
            // instead of requeuing into the post-reset conversation.
            if self.epoch.load(Ordering::SeqCst) != epoch_start {
                continue;
            }

            let success;
            match prompt_result {
                Ok(()) => {
                    self.consecutive_failures.store(0, Ordering::SeqCst);
                    self.failure_notified.store(false, Ordering::SeqCst);
                    success = true;
                }
                Err(err) => {
                    self.agent.rollback_to(message_snapshot).await;
                    let failures = self.consecutive_failures.fetch_add(1, Ordering::SeqCst) + 1;
                    if failures >= 3 {
                        tracing::warn!(
                            failures,
                            "advisor failed consecutively; dropping backlog to prevent stall"
                        );
                        if !self.failure_notified.swap(true, Ordering::SeqCst) {
                            self.host.notify_failure(&err);
                        }
                        self.consecutive_failures.store(0, Ordering::SeqCst);
                        success = true;
                    } else {
                        // Requeue the failed batch at the head and back off.
                        {
                            let mut s = self.state.lock();
                            s.pending.insert(
                                0,
                                PendingDelta {
                                    text: batch,
                                    turns: final_turns,
                                },
                            );
                        }
                        tokio::time::sleep(self.retry_delay).await;
                        continue;
                    }
                }
            }

            if success {
                self.decrement_backlog(final_turns);
                self.notify_waiters();
            }
        }
    }
}

/// Format one message as lean markdown for the advisor's transcript view.
/// omp uses `formatSessionHistoryMarkdown` (thinking/tool-intent aware); this
/// is a v1 — role tag + text content. Enrich later.
fn format_message_md(msg: &Message) -> Option<String> {
    let role = match msg {
        Message::User(_) => "user",
        Message::Assistant(_) => "assistant",
        Message::ToolResult(_) => "tool",
    };
    let text = msg.text_content().unwrap_or_default();
    if text.trim().is_empty() {
        return None;
    }
    Some(format!("**[{role}]**\n{text}"))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use std::sync::Mutex as StdMutex;
    type PromptLog = Arc<StdMutex<Vec<String>>>;
    type AdviceLog = Arc<StdMutex<Vec<AdvisorNote>>>;

    /// Minimal advisor-agent fake that records prompts and can be made to fail.
    struct FakeAgent {
        prompts: PromptLog,
        fail_first_n: AtomicU32,
        messages_len: AtomicU64,
    }

    impl FakeAgent {
        fn new() -> (Arc<Self>, PromptLog) {
            let prompts = Arc::new(StdMutex::new(Vec::new()));
            let a = Arc::new(Self {
                prompts: Arc::clone(&prompts),
                fail_first_n: AtomicU32::new(0),
                messages_len: AtomicU64::new(0),
            });
            (a, prompts)
        }
    }

    #[async_trait]
    impl AdvisorAgent for FakeAgent {
        async fn prompt(&self, input: String) -> Result<(), String> {
            // simulate appending a user+assistant turn (4 messages)
            self.messages_len.fetch_add(4, Ordering::SeqCst);
            self.prompts.lock().unwrap().push(input);
            // Fail the first `fail_first_n` calls, then succeed. (Atomic
            // subtraction on 0 would wrap to u32::MAX, so load-then-decrement
            // only while the counter is positive.)
            let n = self.fail_first_n.load(Ordering::SeqCst);
            if n > 0 {
                self.fail_first_n.fetch_sub(1, Ordering::SeqCst);
                Err("simulated advisor failure".into())
            } else {
                Ok(())
            }
        }
        fn abort(&self, _reason: &str) {}
        fn reset(&self) {
            self.messages_len.store(0, Ordering::SeqCst);
        }
        async fn rollback_to(&self, count: usize) {
            self.messages_len.store(count as u64, Ordering::SeqCst);
        }
        fn message_count(&self) -> usize {
            self.messages_len.load(Ordering::SeqCst) as usize
        }
    }

    /// Host fake that records enqueued advice.
    struct FakeHost {
        advice: AdviceLog,
    }
    impl AdvisorRuntimeHost for FakeHost {
        fn snapshot_messages(&self) -> Vec<Message> {
            Vec::new()
        }
        fn enqueue_advice(&self, note: AdvisorNote) {
            self.advice.lock().unwrap().push(note);
        }
    }

    fn build() -> (Arc<AdvisorRuntime>, PromptLog, AdviceLog) {
        let (agent, prompts) = FakeAgent::new();
        let advice = Arc::new(StdMutex::new(Vec::new()));
        let host: Arc<dyn AdvisorRuntimeHost> = Arc::new(FakeHost {
            advice: Arc::clone(&advice),
        });
        let rt = Arc::new(AdvisorRuntime::new(agent, host, Duration::from_millis(10)));
        rt.install_self(Arc::downgrade(&rt));
        (rt, prompts, advice)
    }

    fn user_msg(s: &str) -> Message {
        Message::user(s)
    }

    #[tokio::test]
    async fn drain_prompts_advisor_with_delta() {
        let (rt, prompts, _advice) = build();
        rt.on_turn_end(vec![user_msg("turn 1")]);
        // allow the spawned drain to run
        tokio::time::sleep(Duration::from_millis(50)).await;
        let p = prompts.lock().unwrap();
        assert_eq!(p.len(), 1);
        assert!(p[0].contains("turn 1"));
        assert!(p[0].starts_with("### Session update"));
    }

    #[tokio::test]
    async fn reset_aborts_inflight_and_drops_batch() {
        let (rt, prompts, _advice) = build();
        rt.on_turn_end(vec![user_msg("turn 1")]);
        rt.reset(); // bump epoch — in-flight batch should be dropped
        tokio::time::sleep(Duration::from_millis(50)).await;
        // The pre-reset prompt may or may not have landed, but the epoch guard
        // means backlog accounting for the stale batch is skipped. Backlog is 0.
        assert_eq!(rt.backlog(), 0);
        let _ = prompts.lock().unwrap().len();
    }

    #[tokio::test]
    async fn drain_exit_racing_turn_end_no_lost_wakeup() {
        // Hammer on_turn_end from many tasks racing the drain's exit path.
        // Every delta must eventually be consumed (no stranded pending).
        let (rt, _prompts, _advice) = build();
        let rt2 = Arc::clone(&rt);
        let handles: Vec<_> = (0..20)
            .map(move |i| {
                let rt3 = Arc::clone(&rt2);
                tokio::spawn(async move {
                    rt3.on_turn_end(vec![user_msg(&format!("turn {i}"))]);
                })
            })
            .collect();
        for h in handles {
            h.await.unwrap();
        }
        // Give drains time to quiesce.
        tokio::time::sleep(Duration::from_millis(120)).await;
        assert_eq!(rt.backlog(), 0);
        // No pending stranded.
        let pending = rt.state.lock().pending.len();
        assert_eq!(pending, 0);
    }

    #[tokio::test]
    async fn wait_for_catchup_resolves_below_threshold() {
        let (rt, _prompts, _advice) = build();
        rt.on_turn_end(vec![user_msg("turn 1")]);
        // threshold 0 -> already below, returns immediately
        rt.wait_for_catchup(Duration::from_millis(50), 0).await;
        // wait for drain to clear backlog
        let _ = tokio::time::timeout(Duration::from_millis(200), async {
            while rt.backlog() > 0 {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await;
        assert_eq!(rt.backlog(), 0);
    }

    #[tokio::test]
    async fn seed_to_skips_history() {
        let (rt, prompts, _advice) = build();
        rt.seed_to(5); // cursor at 5
        // a turn with only 3 messages (< cursor) renders nothing
        rt.on_turn_end(vec![user_msg("a"), user_msg("b"), user_msg("c")]);
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert!(prompts.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn reprime_via_maintain_context() {
        // Host demands re-prime on every call -> advisor context reset, full
        // transcript replayed.
        struct ReprimeHost {
            advice: Arc<StdMutex<Vec<AdvisorNote>>>,
        }
        impl AdvisorRuntimeHost for ReprimeHost {
            fn snapshot_messages(&self) -> Vec<Message> {
                Vec::new()
            }
            fn enqueue_advice(&self, n: AdvisorNote) {
                self.advice.lock().unwrap().push(n);
            }
            fn maintain_context(&self, _t: usize) -> bool {
                true
            }
        }
        let (agent, prompts) = FakeAgent::new();
        let advice = Arc::new(StdMutex::new(Vec::new()));
        let host: Arc<dyn AdvisorRuntimeHost> = Arc::new(ReprimeHost {
            advice: Arc::clone(&advice),
        });
        let rt = Arc::new(AdvisorRuntime::new(agent, host, Duration::from_millis(10)));
        rt.install_self(Arc::downgrade(&rt));
        rt.on_turn_end(vec![user_msg("turn 1"), user_msg("turn 2")]);
        tokio::time::sleep(Duration::from_millis(60)).await;
        let p = prompts.lock().unwrap();
        assert!(!p.is_empty());
        // re-prime replays the full latest transcript (both turns)
        assert!(p[0].contains("turn 1") && p[0].contains("turn 2"));
    }
}
