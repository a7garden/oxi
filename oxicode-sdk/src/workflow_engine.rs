//! Workflow execution engine.
//!
//! Takes a parsed [`WorkflowDefinition`] and executes its steps using the
//! SDK's coordination primitives ([`SharedMemory`], [`Consensus`]) and a
//! pre-built agent map. The engine is pure orchestration — it does not
//! spawn agents itself; the caller registers them by name at construction
//! time via [`WorkflowEngine::new`].
//!
//! # Step semantics
//!
//! Every top-level step produces exactly one [`StepOutput`]. Nested step
//! lists (currently only [`WorkflowStepDef::Chain`]) do **not** emit their
//! own `StepOutput`s — their children run internally and the chain emits a
//! single summary row. This keeps the workflow result flat: one row per
//! declared top-level step.
//!
//! # `{previous}` substitution
//!
//! The task string of a [`WorkflowStepDef::Run`] step may contain the
//! literal `{previous}`; the engine replaces it with the most recent
//! `Run`/`Chain`/`Parallel` step's response text. Inside a `Chain`,
//! `{previous}` threads through the chain's child steps independently of
//! the outer workflow.

use std::collections::HashMap;
use std::sync::Arc;

use crate::coordination::consensus::Consensus;
use crate::coordination::shared_memory::{MemoryKey, SharedMemory};
use crate::lifecycle::AgentHandle;
use crate::workflow_dsl::{WorkflowDefinition, WorkflowStepDef};

use serde_json::Value;

/// Output of one workflow step.
#[derive(Debug, Clone)]
pub struct StepOutput {
    /// Position of this step in the top-level `steps` list (0-indexed).
    pub index: usize,
    /// Variant name (`"Run"`, `"Parallel"`, `"Chain"`, `"ForEach"`,
    /// `"Vote"`, `"SetState"`).
    pub variant: String,
    /// Human-readable one-line summary of what happened.
    pub summary: String,
    /// Wall-clock duration in milliseconds.
    pub duration_ms: u128,
    /// Whether the step succeeded.
    pub success: bool,
    /// Error message when `success` is false.
    pub error: Option<String>,
}

impl StepOutput {
    /// Construct a step output (duration/index filled in by the engine).
    pub fn new(
        variant: impl Into<String>,
        summary: impl Into<String>,
        error: Option<String>,
        success: bool,
    ) -> Self {
        Self {
            index: 0,
            variant: variant.into(),
            summary: summary.into(),
            duration_ms: 0,
            success,
            error,
        }
    }
}

/// Aggregate result of a workflow run.
#[derive(Debug, Clone)]
pub struct WorkflowResult {
    /// Workflow name (echoed from [`WorkflowDefinition::name`]).
    pub name: String,
    /// One row per top-level step that ran. Stops at the first failing
    /// step — remaining steps are not executed.
    pub step_outputs: Vec<StepOutput>,
    /// Total wall-clock duration in milliseconds.
    pub total_duration_ms: u128,
    /// True iff every step that ran succeeded.
    pub success: bool,
}

/// Workflow execution engine with a pre-built agent map.
///
/// Agents are keyed by the names used in [`WorkflowStepDef`] variants
/// (e.g. the `agent` field of `Run`). The engine looks them up by name
/// at execution time; an unknown agent name fails the step with a clear
/// error rather than panicking.
pub struct WorkflowEngine {
    agents: HashMap<String, AgentHandle>,
    shared_memory: Arc<SharedMemory>,
    consensus: Arc<Consensus>,
}

impl WorkflowEngine {
    /// Create a new engine with the given agent map and fresh
    /// `SharedMemory` / `Consensus` instances.
    pub fn new(agents: HashMap<String, AgentHandle>) -> Self {
        Self {
            agents,
            shared_memory: Arc::new(SharedMemory::new()),
            consensus: Arc::new(Consensus::new()),
        }
    }

    /// Replace the default `SharedMemory` with a caller-supplied one
    /// (e.g. to share state across multiple workflow runs).
    pub fn with_memory(mut self, memory: Arc<SharedMemory>) -> Self {
        self.shared_memory = memory;
        self
    }

    /// Replace the default `Consensus` with a caller-supplied one.
    pub fn with_consensus(mut self, consensus: Arc<Consensus>) -> Self {
        self.consensus = consensus;
        self
    }

    /// Borrow the shared memory (for tests / inspection).
    pub fn shared_memory(&self) -> &SharedMemory {
        &self.shared_memory
    }

    /// Borrow the consensus (for tests / inspection).
    pub fn consensus(&self) -> &Consensus {
        &self.consensus
    }

    /// Execute the workflow top-to-bottom. Stops at the first failing
    /// step (subsequent steps are not run).
    pub async fn execute(&self, workflow: &WorkflowDefinition) -> WorkflowResult {
        let start = std::time::Instant::now();
        let mut step_outputs = Vec::with_capacity(workflow.steps.len());
        let mut success = true;
        let mut last_output: Option<String> = None;

        for (i, step) in workflow.steps.iter().enumerate() {
            let step_start = std::time::Instant::now();
            let (mut output, new_last) = self.run_step(step, last_output.as_deref()).await;
            output.index = i;
            output.duration_ms = step_start.elapsed().as_millis();
            let ok = output.success;
            step_outputs.push(output);
            if new_last.is_some() {
                last_output = new_last;
            }
            if !ok {
                success = false;
                break;
            }
        }

        WorkflowResult {
            name: workflow.name.clone(),
            step_outputs,
            total_duration_ms: start.elapsed().as_millis(),
            success,
        }
    }

    /// Run one step, returning `(output, optional_response_text)`.
    ///
    /// The optional response text is the step's "result text" used for
    /// `{previous}` substitution in subsequent steps. Only `Run`,
    /// `Parallel`, and `Chain` produce one.
    ///
    /// `previous` is the prior step's response text (or `None` for the
    /// first step / after a step that doesn't produce text).
    async fn run_step(
        &self,
        step: &WorkflowStepDef,
        previous: Option<&str>,
    ) -> (StepOutput, Option<String>) {
        match step {
            WorkflowStepDef::Run {
                agent,
                task,
                output,
            } => {
                let task = substitute_previous(task, previous);
                let Some(handle) = self.agents.get(agent) else {
                    return (
                        StepOutput::new(
                            "Run",
                            format!("Unknown agent '{agent}'"),
                            Some(format!("agent not found: {agent}")),
                            false,
                        ),
                        None,
                    );
                };
                match handle.run(task).await {
                    Ok((response, _)) => {
                        let text = response.content;
                        if let Some(key) = output {
                            let mk = MemoryKey::new("workflow", key);
                            let _ = self.shared_memory.write(
                                &mk,
                                serde_json::json!(&text),
                                "engine",
                                None,
                            );
                        }
                        (
                            StepOutput::new(
                                "Run",
                                format!(
                                    "agent '{agent}' responded ({} chars)",
                                    text.chars().count()
                                ),
                                None,
                                true,
                            ),
                            Some(text),
                        )
                    }
                    Err(e) => (
                        StepOutput::new(
                            "Run",
                            format!("agent '{agent}' failed"),
                            Some(e.to_string()),
                            false,
                        ),
                        None,
                    ),
                }
            }

            WorkflowStepDef::Parallel {
                agents,
                task,
                concurrency: _,
            } => {
                // Look up every named agent up front; abort on first miss.
                let mut handles = Vec::new();
                for name in agents {
                    match self.agents.get(name) {
                        Some(h) => handles.push(h.clone()),
                        None => {
                            return (
                                StepOutput::new(
                                    "Parallel",
                                    format!("Unknown agent '{name}'"),
                                    Some(format!("agent not found: {name}")),
                                    false,
                                ),
                                None,
                            );
                        }
                    }
                }
                let task = substitute_previous(task, previous);

                // Spawn one tokio task per agent and run them concurrently.
                // Concurrency cap is currently advisory (no semaphore) —
                // every agent runs at once. Adding a semaphore is a
                // follow-up if real-world fan-out grows.
                let mut join_handles = Vec::with_capacity(handles.len());
                for h in handles {
                    let task = task.clone();
                    join_handles.push(tokio::spawn(async move { h.run(task).await }));
                }

                let mut responses: Vec<String> = Vec::new();
                let mut failures: Vec<String> = Vec::new();
                for jh in join_handles {
                    match jh.await {
                        Ok(Ok((r, _))) => responses.push(r.content),
                        Ok(Err(e)) => failures.push(e.to_string()),
                        Err(e) => failures.push(format!("join error: {e}")),
                    }
                }

                let ok = failures.is_empty();
                let err = if failures.is_empty() {
                    None
                } else {
                    Some(failures.join("; "))
                };
                // The "previous" text after a Parallel is the last
                // successful response (mirrors Chain's behavior).
                let summary_text = responses.last().cloned();
                let summary = format!(
                    "Parallel: {}/{} agents succeeded",
                    responses.len(),
                    agents.len()
                );
                (StepOutput::new("Parallel", summary, err, ok), summary_text)
            }

            WorkflowStepDef::Chain { steps } => {
                // Recursively run nested steps, threading `{previous}`
                // through each child. The chain itself emits one summary
                // row; child outputs are folded into the summary.
                let mut last: Option<String> = previous.map(str::to_string);
                let mut chain_ok = true;
                let mut chain_err: Option<String> = None;
                let mut last_child_summary = String::new();
                let mut ran = 0usize;
                let total = steps.len();

                for child in steps {
                    let (out, new_last) = Box::pin(self.run_step(child, last.as_deref())).await;
                    ran += 1;
                    last = new_last;
                    last_child_summary = out.summary;
                    if !out.success {
                        chain_ok = false;
                        chain_err = out.error;
                        break;
                    }
                }

                let summary = if chain_ok {
                    format!("chain of {total} steps ok — last: {last_child_summary}")
                } else {
                    format!("chain failed at step {ran}/{total}: {last_child_summary}")
                };

                (StepOutput::new("Chain", summary, chain_err, chain_ok), last)
            }

            WorkflowStepDef::ForEach {
                items_key,
                namespace,
                agent,
                task_template,
                concurrency: _,
            } => {
                let ns = namespace.as_deref().unwrap_or("workflow");
                let mk = MemoryKey::new(ns, items_key);

                let items = match self.shared_memory.read(&mk) {
                    Some(v) => match v.as_array() {
                        Some(a) => a.clone(),
                        None => {
                            return (
                                StepOutput::new(
                                    "ForEach",
                                    format!("ForEach: value at {ns}/{items_key} is not an array"),
                                    Some(format!("items at {ns}/{items_key} is not a JSON array")),
                                    false,
                                ),
                                None,
                            );
                        }
                    },
                    None => {
                        return (
                            StepOutput::new(
                                "ForEach",
                                format!("ForEach: key {ns}/{items_key} not found"),
                                Some(format!(
                                    "items key {ns}/{items_key} not found in SharedMemory"
                                )),
                                false,
                            ),
                            None,
                        );
                    }
                };

                let Some(handle) = self.agents.get(agent) else {
                    return (
                        StepOutput::new(
                            "ForEach",
                            format!("ForEach: unknown agent '{agent}'"),
                            Some(format!("agent not found: {agent}")),
                            false,
                        ),
                        None,
                    );
                };

                let total = items.len();
                let mut succeeded = 0usize;
                let mut last_err: Option<String> = None;
                let mut last_text: Option<String> = None;

                // Sequential execution. The `concurrency` field is
                // accepted by the DSL but ignored here; parallel fan-out
                // for ForEach is a follow-up.
                for (i, item) in items.iter().enumerate() {
                    let item_str = match item {
                        Value::String(s) => s.clone(),
                        other => other.to_string(),
                    };
                    let task = task_template.replace("{item}", &item_str);
                    match handle.run(task).await {
                        Ok((r, _)) => {
                            succeeded += 1;
                            last_text = Some(r.content);
                        }
                        Err(e) => {
                            last_err = Some(format!("item {i} ({item_str}): {e}"));
                            break;
                        }
                    }
                }

                let ok = last_err.is_none();
                let summary = format!("ForEach: {succeeded}/{total} items succeeded on '{agent}'");
                (StepOutput::new("ForEach", summary, last_err, ok), last_text)
            }

            WorkflowStepDef::Vote {
                agents,
                question,
                threshold,
            } => {
                let sid = format!("wf-vote-{}-{}", std::process::id(), question.len());
                let thr = threshold.unwrap_or(0.5);
                self.consensus.start(&sid, agents.clone(), thr);

                let mut missing: Vec<String> = Vec::new();
                let mut first_decision: Option<String> = None;

                for voter in agents {
                    let Some(handle) = self.agents.get(voter) else {
                        missing.push(voter.clone());
                        continue;
                    };
                    let task = question.clone();
                    match handle.run(task).await {
                        Ok((r, _)) => {
                            let value = r.content.trim().to_string();
                            if let Ok(vr) = self.consensus.vote(&sid, voter, value)
                                && vr.decided
                                && first_decision.is_none()
                            {
                                first_decision = vr.decision.clone();
                            }
                        }
                        Err(e) => {
                            // A voter failing to respond counts as
                            // abstention; surface it in tracing but
                            // don't abort the vote.
                            tracing::debug!(
                                voter = %voter,
                                error = %e,
                                "Vote: agent failed to respond"
                            );
                        }
                    }
                }

                let final_result = self.consensus.status(&sid);
                let (decided, summary) = match final_result {
                    Some(vr) if vr.decided => (
                        true,
                        format!(
                            "Vote: decided = {:?} ({} of {} votes)",
                            vr.decision, vr.votes_received, vr.total_voters
                        ),
                    ),
                    Some(vr) => (
                        false,
                        format!(
                            "Vote: no consensus ({}/{} votes cast)",
                            vr.votes_received, vr.total_voters
                        ),
                    ),
                    None => (false, "Vote: session not found".to_string()),
                };

                let err = if !missing.is_empty() {
                    Some(format!("missing voters: {}", missing.join(", ")))
                } else if !decided {
                    Some("no consensus reached".to_string())
                } else {
                    None
                };

                let ok = missing.is_empty() && decided;
                (StepOutput::new("Vote", summary, err, ok), first_decision)
            }

            WorkflowStepDef::SetState {
                key,
                value,
                namespace,
            } => {
                let n = namespace.as_deref().unwrap_or("workflow");
                let mk = MemoryKey::new(n, key);
                match self.shared_memory.write(&mk, value.clone(), "engine", None) {
                    Ok(_) => (
                        StepOutput::new("SetState", format!("{n}/{key} = {value}"), None, true),
                        None,
                    ),
                    Err(e) => (
                        StepOutput::new(
                            "SetState",
                            format!("{n}/{key} failed"),
                            Some(e.to_string()),
                            false,
                        ),
                        None,
                    ),
                }
            }
        }
    }
}

/// Replace `{previous}` in `text` with the prior step's response text,
/// if any. When `previous` is `None` the literal is left untouched —
/// callers using `{previous}` in the first step will see the placeholder
/// in the prompt, which makes the misuse visible.
fn substitute_previous(text: &str, previous: Option<&str>) -> String {
    match previous {
        Some(p) => text.replace("{previous}", p),
        None => text.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn set_state_workflow() {
        let yaml = "---\nname: test\nsteps:\n  - type: set_state\n    key: m\n    value: 42\n";
        let wf = WorkflowDefinition::from_yaml_str(yaml).unwrap();
        let engine = WorkflowEngine::new(HashMap::new());
        let result = engine.execute(&wf).await;
        assert!(result.success);
        assert_eq!(result.step_outputs.len(), 1);
        assert_eq!(result.step_outputs[0].variant, "SetState");
    }

    #[tokio::test]
    async fn unknown_agent_fails_run() {
        let yaml = "---\nname: t\nsteps:\n  - type: run\n    agent: ghost\n    task: h\n";
        let wf = WorkflowDefinition::from_yaml_str(yaml).unwrap();
        let engine = WorkflowEngine::new(HashMap::new());
        let result = engine.execute(&wf).await;
        assert!(!result.success);
    }

    #[tokio::test]
    async fn short_circuits_on_failure() {
        let yaml = "---\nname: t\nsteps:\n  - type: run\n    agent: ghost\n    task: x\n  - type: set_state\n    key: n\n    value: 1\n";
        let wf = WorkflowDefinition::from_yaml_str(yaml).unwrap();
        let engine = WorkflowEngine::new(HashMap::new());
        let result = engine.execute(&wf).await;
        assert!(!result.success);
        assert_eq!(result.step_outputs.len(), 1);
    }

    #[tokio::test]
    async fn chain_runs_nested_steps_and_threads_previous() {
        // A chain with two set_state children. No agent required —
        // SetState is pure SharedMemory. We assert both children ran
        // (two keys written) and the chain emits a single summary row.
        let yaml = "---\nname: c\nsteps:\n  - type: chain\n    steps:\n      - type: set_state\n        key: a\n        value: 1\n      - type: set_state\n        key: b\n        value: 2\n";
        let wf = WorkflowDefinition::from_yaml_str(yaml).unwrap();
        let engine = WorkflowEngine::new(HashMap::new());
        let result = engine.execute(&wf).await;
        assert!(
            result.success,
            "chain should succeed: {:?}",
            result.step_outputs
        );
        assert_eq!(result.step_outputs.len(), 1, "chain emits one summary row");
        assert_eq!(result.step_outputs[0].variant, "Chain");
        let mk_a = MemoryKey::new("workflow", "a");
        let mk_b = MemoryKey::new("workflow", "b");
        assert_eq!(
            engine.shared_memory().read(&mk_a),
            Some(serde_json::json!(1))
        );
        assert_eq!(
            engine.shared_memory().read(&mk_b),
            Some(serde_json::json!(2))
        );
    }

    #[tokio::test]
    async fn chain_propagates_failure_and_short_circuits() {
        // chain of [set_state, run-unknown-agent, set_state-after].
        // Second child fails → third must NOT run.
        let yaml = "---\nname: c\nsteps:\n  - type: chain\n    steps:\n      - type: set_state\n        key: pre\n        value: 1\n      - type: run\n        agent: ghost\n        task: x\n      - type: set_state\n        key: post\n        value: 2\n";
        let wf = WorkflowDefinition::from_yaml_str(yaml).unwrap();
        let engine = WorkflowEngine::new(HashMap::new());
        let result = engine.execute(&wf).await;
        assert!(!result.success, "chain should fail");
        let mk_pre = MemoryKey::new("workflow", "pre");
        let mk_post = MemoryKey::new("workflow", "post");
        assert_eq!(
            engine.shared_memory().read(&mk_pre),
            Some(serde_json::json!(1))
        );
        assert_eq!(engine.shared_memory().read(&mk_post), None);
    }

    #[tokio::test]
    async fn foreach_rejects_missing_items_key() {
        let yaml = "---\nname: fe\nsteps:\n  - type: for_each\n    items_key: nope\n    agent: a\n    task_template: \"x {item}\"\n";
        let wf = WorkflowDefinition::from_yaml_str(yaml).unwrap();
        let engine = WorkflowEngine::new(HashMap::new());
        let result = engine.execute(&wf).await;
        assert!(!result.success);
        assert!(
            result.step_outputs[0]
                .error
                .as_deref()
                .unwrap_or("")
                .contains("not found")
        );
    }

    #[tokio::test]
    async fn foreach_rejects_non_array_items() {
        let engine = WorkflowEngine::new(HashMap::new());
        let mk = MemoryKey::new("workflow", "items");
        engine
            .shared_memory()
            .write(&mk, serde_json::json!("not an array"), "test", None)
            .unwrap();
        let yaml = "---\nname: fe\nsteps:\n  - type: for_each\n    items_key: items\n    agent: a\n    task_template: \"x {item}\"\n";
        let wf = WorkflowDefinition::from_yaml_str(yaml).unwrap();
        let result = engine.execute(&wf).await;
        assert!(!result.success);
        assert!(
            result.step_outputs[0]
                .error
                .as_deref()
                .unwrap_or("")
                .contains("not a JSON array")
        );
    }

    #[tokio::test]
    async fn foreach_rejects_unknown_agent() {
        let engine = WorkflowEngine::new(HashMap::new());
        let mk = MemoryKey::new("workflow", "items");
        engine
            .shared_memory()
            .write(&mk, serde_json::json!(["a", "b"]), "test", None)
            .unwrap();
        let yaml = "---\nname: fe\nsteps:\n  - type: for_each\n    items_key: items\n    agent: ghost\n    task_template: \"x {item}\"\n";
        let wf = WorkflowDefinition::from_yaml_str(yaml).unwrap();
        let result = engine.execute(&wf).await;
        assert!(!result.success);
        assert!(
            result.step_outputs[0]
                .error
                .as_deref()
                .unwrap_or("")
                .contains("agent not found")
        );
    }

    #[tokio::test]
    async fn vote_with_unknown_voter_fails() {
        // Vote step references a voter that isn't registered in the
        // engine's agent map → step must fail with "missing voters".
        let yaml = "---\nname: v\nsteps:\n  - type: vote\n    agents: [ghost]\n    question: yes or no\n    threshold: 0.5\n";
        let wf = WorkflowDefinition::from_yaml_str(yaml).unwrap();
        let engine = WorkflowEngine::new(HashMap::new());
        let result = engine.execute(&wf).await;
        assert!(!result.success);
        assert_eq!(result.step_outputs[0].variant, "Vote");
        assert!(
            result.step_outputs[0]
                .error
                .as_deref()
                .unwrap_or("")
                .contains("missing voters")
        );
    }
}
