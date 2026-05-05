/// Context builder utilities
//!
/// Builds a Context from agent state for provider calls.

use crate::state::AgentState;
use crate::tools::ToolRegistry;
use crate::types::ToolDefinition;
use oxi_ai::{Context, Tool};

/// Convert a ToolDefinition from the agent's tool registry into an oxi_ai Tool.
pub fn definition_to_oxi_tool(def: &ToolDefinition) -> Tool {
    let schema = serde_json::to_value(&def.input_schema)
        .unwrap_or_else(|_| serde_json::json!({"type": "object", "properties": {}}));
    
    Tool::new(&def.name, &def.description, schema)
}

/// Builds a Context from agent state for provider calls.
///
/// # Arguments
/// * `state` - The current agent state containing messages
/// * `system_prompt` - The system prompt to include (from config)
/// * `tools` - The tool registry to get tool definitions from
///
/// # Returns
/// A configured Context ready for provider calls.
pub fn build_context(
    state: &AgentState,
    system_prompt: Option<&str>,
    tools: &ToolRegistry,
) -> Context {
    let tool_defs = tools.definitions();
    let oxi_tools: Vec<Tool> = tool_defs
        .iter()
        .map(definition_to_oxi_tool)
        .collect();

    let mut context = Context::new();

    // Set system prompt if provided
    if let Some(prompt) = system_prompt {
        context.set_system_prompt(prompt.to_string());
    }

    // Add all messages from state
    for msg in &state.messages {
        context.add_message(msg.clone());
    }

    // Add tools if any
    if !oxi_tools.is_empty() {
        context.set_tools(oxi_tools);
    }

    context
}

/// Builds a minimal Context with just messages (no tools or system prompt).
///
/// Useful for operations that only need the message history.
pub fn build_messages_context(state: &AgentState) -> Context {
    let mut context = Context::new();
    
    for msg in &state.messages {
        context.add_message(msg.clone());
    }
    
    context
}

/// Builds a Context with system prompt and messages (no tools).
pub fn build_context_with_prompt(
    state: &AgentState,
    system_prompt: &str,
) -> Context {
    let mut context = Context::new();
    context.set_system_prompt(system_prompt.to_string());
    
    for msg in &state.messages {
        context.add_message(msg.clone());
    }
    
    context
}