//! AgentBuilder — Fluent API for creating agents

use std::path::PathBuf;
use std::sync::Arc;

use oxi_ai::Provider;
use oxi_agent::{Agent, AgentConfig, ToolRegistry, AgentTool};

use crate::builder::Oxi;

/// Builder for creating an agent with custom configuration.
#[allow(dead_code)]
pub struct AgentBuilder<'a> {
    oxi: &'a Oxi,
    config: AgentConfig,
    tools: ToolRegistry,
    workspace_dir: Option<PathBuf>,
    system_prompt: Option<String>,
}

impl<'a> AgentBuilder<'a> {
    pub fn new(oxi: &'a Oxi, config: AgentConfig) -> Self {
        Self {
            oxi,
            config,
            tools: ToolRegistry::new(),
            workspace_dir: None,
            system_prompt: None,
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
        // Resolve model to get the provider
        let model = self.oxi.resolve_model(&self.config.model_id)?;
        let provider = self.oxi.create_provider(&model.provider)?;
        
        // Convert Box<dyn Provider> to Arc<dyn Provider>
        let provider: Arc<dyn Provider> = provider.into();
        
        // Merge workspace_dir into config
        let mut config = self.config.clone();
        config.workspace_dir = self.workspace_dir.or(config.workspace_dir);
        if let Some(ref prompt) = self.system_prompt {
            config.system_prompt = Some(prompt.clone());
        }
        
        let agent = Agent::new(provider, config);
        
        // Register tools from the builder's registry
        for name in self.tools.names() {
            if let Some(tool) = self.tools.get(&name) {
                agent.tools().register_arc(tool);
            }
        }
        
        Ok(agent)
    }
}