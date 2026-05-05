/// Compaction initialization utilities

use crate::{CompactionManager, CompactionStrategy};
use crate::model_id::resolve_model_from_id;
use oxi_ai::{LlmCompactor, Provider};
use std::sync::Arc;

/// Creates a CompactionManager with LLM compactor initialized for the given model.
///
/// # Arguments
/// * `strategy` - The compaction strategy to use
/// * `context_window` - Maximum context window size in tokens
/// * `model_id` - Model ID in "provider/model" format
/// * `provider` - The provider to use for the LLM compactor
///
/// # Returns
/// A configured CompactionManager. If the strategy is Disabled, returns a manager
/// without an LLM compactor.
pub fn create_compaction_manager(
    strategy: CompactionStrategy,
    context_window: usize,
    model_id: &str,
    provider: Arc<dyn Provider>,
) -> CompactionManager {
    let mut manager = CompactionManager::new(strategy.clone(), context_window);
    
    if strategy != CompactionStrategy::Disabled {
        if let Some(model) = resolve_model_from_id(model_id) {
            let compactor = Arc::new(LlmCompactor::new(model, provider));
            manager.set_compactor(compactor);
        }
    }
    
    manager
}