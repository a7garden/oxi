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
    ///
    /// When set, subsequent calls to `.coding_tools()` or `.readonly_tools()`
    /// will automatically use this directory as the tool root.
    pub fn workspace(mut self, dir: impl Into<PathBuf>) -> Self {
        self.workspace_dir = Some(dir.into());
        self
    }

    /// Set a custom system prompt.
    pub fn system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt = Some(prompt.into());
        self
    }

    /// Register the standard coding tools (read, write, edit, ls).
    /// If `.workspace()` was called, uses that directory as cwd for tools.
    /// Otherwise uses current directory.
    #[allow(unused_mut)]
    pub fn coding_tools(mut self) -> Self {
        let cwd = self.workspace_dir.clone()
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
        let tools = crate::tool_factory::coding_tools(&cwd);
        for name in tools.names() {
            if let Some(tool) = tools.get(&name) {
                self.tools.register_arc(tool);
            }
        }
        self
    }

    /// Register read-only tools (read, ls).
    /// If `.workspace()` was called, uses that directory as cwd for tools.
    /// Otherwise uses current directory.
    #[allow(unused_mut)]
    pub fn readonly_tools(mut self) -> Self {
        let cwd = self.workspace_dir.clone()
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
        let tools = crate::tool_factory::readonly_tools(&cwd);
        for name in tools.names() {
            if let Some(tool) = tools.get(&name) {
                self.tools.register_arc(tool);
            }
        }
        self
    }

    /// Register a tool.
    #[allow(unused_mut)]
    pub fn tool(mut self, tool: impl AgentTool + 'static) -> Self {
        self.tools.register(tool);
        self
    }

    /// Register multiple tools.
    #[allow(unused_mut)]
    pub fn tools(mut self, tools: impl IntoIterator<Item = impl AgentTool + 'static>) -> Self {
        for tool in tools {
            self.tools.register(tool);
        }
        self
    }

    /// Build the agent.
    pub fn build(self) -> anyhow::Result<Agent> {
        // Resolve model from Oxi's instance registry
        let model = self.oxi.resolve_model(&self.config.model_id)?;

        // Create provider via Oxi's ProviderRegistry (custom, then built-ins)
        let provider: Arc<dyn Provider> = self.oxi.create_provider(&model.provider)?;

        // Merge workspace_dir into config
        let mut config = self.config.clone();
        config.workspace_dir = self.workspace_dir.or(config.workspace_dir);
        if let Some(ref prompt) = self.system_prompt {
            config.system_prompt = Some(prompt.clone());
        }

        // Create the agent, passing the builder's tool registry directly
        let agent = Agent::new(provider, config, Arc::new(self.tools));

        Ok(agent)
    }
}
