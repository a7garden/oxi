//! In-memory port implementations.
//!
//! Use these for:
//! - Tests that need fast fakes
//! - Single-process products that don't need persistence
//! - Ephemeral sessions (one-shot, short-lived)
//!
//! For durable storage, use [`crate::ports::fs`].

pub mod cron;
pub mod event;
pub mod memory;
pub mod resources;
pub mod todo_state;
pub mod url_router;

pub use cron::InMemoryCronScheduler;
pub use event::InProcessEventBus;
pub use memory::InMemoryMemoryStore;
pub use resources::CountingResourceMonitor;
pub use todo_state::InMemoryTodoState;
