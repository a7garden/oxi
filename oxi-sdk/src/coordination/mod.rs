//! Inter-agent coordination primitives.
//!
//! Provides work distribution, shared state, and consensus:
//! - **WorkQueue**: priority-based atomic task queue with claim/complete lifecycle
//! - **SharedMemory**: versioned KV store with optimistic locking
//! - **Consensus**: simple majority/unanimity voting
//! - **CoordinatedGroup**: fan-out, vote, and map-reduce over AgentHandles

pub mod consensus;
pub mod group_ext;
pub mod shared_memory;
pub mod work_queue;

pub use consensus::{Consensus, VoteResult};
pub use group_ext::{CoordinatedGroup, CoordinatedGroupBuilder};
pub use shared_memory::{MemoryEntry, MemoryEvent, MemoryKey, SharedMemory};
pub use work_queue::{
    WorkEvent, WorkItem, WorkQueue, WorkQueueConfig, WorkQueueStats, WorkResult, WorkStatus,
};
