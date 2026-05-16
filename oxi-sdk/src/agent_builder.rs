//! AgentBuilder — Fluent API for creating agents

use std::path::PathBuf;
use std::sync::Arc;

use oxi_ai::{Api, Model, Provider};
use oxi_agent::{
    Agent, AgentLoop, AgentLoopConfig, AgentConfig,
    ToolRegistry, AgentTool, BeforeToolCallHook, AfterToolCallHook,
};

use crate::builder::Oxi;

/// Builder for creating an agent with custom configuration.
pub struct AgentBuilder<'a> {
    oxi: &'a Oxi,
    config: AgentConfig,
    tools: ToolRegistry,
    workspace_dir: Option<PathBuf>,
    system_prompt: Option<String>,
    hooks: Option<oxi_agent::AgentHooks>,
}

impl<'a> AgentBuilder<'a> {
    pub fn new(oxi: &'a Oxi, config: AgentConfig) -> Self {
        Self {
            oxi,
            config,
            tools: ToolRegistry::new(),
            workspace_dir: None,
            system_prompt: None,
            hooks: None,
        }
    }
    
    /// Set the working directory for file tools.
    pub fn workspace(mut self, dir: impl Into<PathBuf>) -> Self {
        self.workspace_dir = Some(dir.into());
        self
    }
    
    /// Set a custom system prompt.
    pub fn system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt = Some(prompt.into());
        self
    }
    
    /// Register a tool.
    pub fn tool(mut self, tool: impl AgentTool + 'static) -> Self {
        self.tools.register(tool);
        self
    }
    
    /// Register multiple tools.
    pub fn tools(mut self, tools: impl IntoIterator<Item = impl AgentTool + 'static>) -> Self {
        for tool in tools {
            self.tools.register(tool);
        }
        self
    }
    
    /// Build the agent.
    pub fn build(self) -> anyhow::Result<Agent> {
        // Resolve model
        let model = self.oxi.resolve_model(&self.config.model_id)?;
        let provider = self.oxi.create_provider(&model.provider)?.clone();
        
        // Build AgentLoopConfig
        let loop_config = AgentLoopConfig {
            model_id: self.config.model_id.clone(),
            system_prompt: self.system_prompt.or(self.config.system_prompt.clone()),
            temperature: self.config.temperature.unwrap_or(1.0) as f32,
            max_tokens: self.config.max_tokens.unwrap_or(4096) as u32,
            max_iterations: self.config.max_iterations,
            tool_execution: oxi_agent::ToolExecutionMode::Parallel,
            compaction_strategy: self.config.compaction_strategy.clone(),
            context_window: self.config.context_window,
            compaction_instruction: self.config.compaction_instruction.clone(),
            session_id: None,
            transport: None,
            compact_on_start: false,
            max_retry_delay_ms: None,
            auto_retry_enabled: true,
            auto_retry_max_attempts: 3,
            auto_retry_base_delay_ms: 2000,
            api_key: self.config.api_key.clone(),
            workspace_dir: self.workspace_dir,
        };
        
        // Create AgentLoop
        let state = oxi_agent::SharedState::new();
        let tools = Arc::new(self.tools);
        let agent_loop = AgentLoop::new(Arc::new(provider), loop_config, tools, state);
        
        // Wrap in Agent (higher-level API)
        Ok(Agent::from_agent_loop(agent_loop, self.config.clone()))
    }
}