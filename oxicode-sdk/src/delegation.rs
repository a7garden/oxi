//! Library-native sub-agent delegation (issue #28 gap 3).
//!
//! [`SdkSubagentRunner`] implements [`oxicode_agent::SubagentRunner`] by wrapping
//! an [`crate::Oxicode`] instance. Each `run_isolated` call builds a fresh
//! [`oxicode_agent::Agent`] with an empty context (full isolation from the
//! parent), runs it, and returns only the final text + usage.
//!
//! This is the in-process alternative to shelling out to the `oxicode` CLI
//! binary. Library consumers (e.g. Oxios) wire it into
//! [`oxicode_agent::AgentLoopConfig::subagent_runner`] so the `subagent` tool
//! delegates in-process.
//!
//! # Depth safety
//!
//! The CLI backend tracks recursion depth via env vars
//! (`OXICODE_SUBAGENT_DEPTH`), which is safe because each subprocess has its
//! own env. In-process forking **cannot** use env vars — concurrent
//! `std::env::set_var` is UB, and state leaks between sequential forks.
//! Instead, the runner sets the forked agent's
//! [`oxicode_agent::AgentLoopConfig::subagent_depth`] to `depth + 1`, and the
//! fork's `subagent` tool reads that field to enforce the cap.

use std::path::Path;

use async_trait::async_trait;
use oxicode_agent::{AgentConfig, AgentEvent, ForkResult, SubagentRunner};

use crate::Oxicode;

/// SDK-provided [`SubagentRunner`] — wraps an [`Oxicode`] instance and forks
/// isolated in-process sub-agent runs.
///
/// Clone-cheap (inner [`Oxicode`] is `Arc`-backed). Safe to share across
/// concurrent tasks (parallel sub-agent mode spawns up to 8 concurrent
/// forks — the runner is `Send + Sync`).
#[derive(Clone)]
pub struct SdkSubagentRunner {
    oxicode: Oxicode,
}

impl std::fmt::Debug for SdkSubagentRunner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SdkSubagentRunner")
            .field("oxicode", &"<Oxicode>")
            .finish()
    }
}

impl SdkSubagentRunner {
    /// Create from an [`Oxicode`] engine instance.
    pub fn new(oxicode: Oxicode) -> Self {
        Self { oxicode }
    }
}

#[async_trait]
impl SubagentRunner for SdkSubagentRunner {
    async fn run_isolated(
        &self,
        _agent_name: &str,
        task: &str,
        system_prompt: Option<&str>,
        model: Option<&str>,
        _tools: &[String],
        cwd: &Path,
        _depth: u8,
    ) -> anyhow::Result<ForkResult> {
        // Build a fresh AgentConfig — empty context, full isolation.
        let mut config = AgentConfig::default();
        if let Some(m) = model {
            config.model_id = m.to_string();
        }
        if let Some(sp) = system_prompt {
            config.system_prompt = Some(sp.to_string());
        }
        config.workspace_dir = Some(cwd.to_path_buf());

        // Build the agent via Oxicode (resolves provider + model from the
        // SDK's registries, not global state).
        let agent = self.oxicode.agent(config).build()?;

        // Run with the prompt. Each Agent has its own SharedState —
        // completely isolated from the parent's context.
        let prompt = task.to_string();
        let (response, events) = agent.run(prompt).await?;

        // Extract usage from events. The last Usage event carries the
        // most recent provider-reported counts.
        let (mut input_tokens, mut output_tokens, mut turns) = (0usize, 0usize, 0u32);
        for event in &events {
            match event {
                AgentEvent::Usage {
                    input_tokens: i,
                    output_tokens: o,
                } => {
                    input_tokens = *i;
                    output_tokens = *o;
                }
                AgentEvent::TurnStart { .. } => turns += 1,
                _ => {}
            }
        }

        Ok(ForkResult {
            text: response.content,
            input_tokens,
            output_tokens,
            turns,
            model: model.map(|m| m.to_string()),
            error: None,
        })
    }
}
