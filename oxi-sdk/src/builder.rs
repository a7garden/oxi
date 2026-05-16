//! OxiBuilder and Oxi — SDK entry point

use anyhow::Result;
use std::sync::Arc;

use oxi_ai::{Provider, Model};
use oxi_agent::{Agent, AgentLoop, AgentLoopConfig, ToolRegistry};

use crate::agent_builder::AgentBuilder;

/// Oxi AI engine instance.
///
/// Created via [`OxiBuilder`]. Provides access to providers, models,
/// and agent creation.
pub struct Oxi {
    tools: Arc<ToolRegistry>,
}

impl Oxi {
    /// Create an agent builder with the given config.
    pub fn agent(&self, config: oxi_agent::AgentConfig) -> AgentBuilder<'_> {
        AgentBuilder::new(self, config)
    }

    /// Get the shared tool registry.
    pub fn tools(&self) -> Arc<ToolRegistry> {
        Arc::clone(&self.tools)
    }
    
    /// Resolve a model ID to a Model.
    pub fn resolve_model(&self, model_id: &str) -> Result<Model> {
        let parts: Vec<&str> = model_id.splitn(2, '/').collect();
        let (provider, model) = if parts.len() == 2 {
            (parts[0], parts[1])
        } else {
            ("anthropic", parts[0])
        };
        crate::oxi_lookup_model(provider, model)
            .ok_or_else(|| anyhow::anyhow!("Model '{}' not found", model_id))
    }
    
    /// Create a provider instance for a given provider name.
    pub fn create_provider(&self, name: &str) -> Result<Box<dyn Provider>> {
        crate::get_provider(name)
            .ok_or_else(|| anyhow::anyhow!("Provider '{}' not found", name))
    }
}

/// Builder for creating an Oxi instance.
pub struct OxiBuilder {
    tools: ToolRegistry,
}

impl OxiBuilder {
    /// Create a new empty builder.
    pub fn new() -> Self {
        Self {
            tools: ToolRegistry::new(),
        }
    }
    
    /// Register all built-in providers and models (populated by model_registry globals).
    pub fn with_builtins(mut self) -> Self {
        // Models are already globally registered in oxi-ai
        // This method exists for API completeness
        self
    }
    
    /// Register a tool.
    pub fn tool(mut self, tool: impl oxi_agent::AgentTool + 'static) -> Self {
        self.tools.register(tool);
        self
    }
    
    /// Build the Oxi instance.
    pub fn build(self) -> Oxi {
        Oxi {
            tools: Arc::new(self.tools),
        }
    }
}

impl Default for OxiBuilder {
    fn default() -> Self { Self::new() }
}