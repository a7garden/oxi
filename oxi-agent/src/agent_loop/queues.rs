//! Queue management for agent loop

use oxi_ai::Message;

impl super::AgentLoop {
    /// Drain all messages from the steering queue.
    pub fn drain_steering_queue(&self) -> Vec<Message> {
        let mut queue = self.steering_queue.write();
        queue.drain(..).collect()
    }

    /// Drain all messages from the follow-up queue.
    pub fn drain_follow_up_queue(&self) -> Vec<Message> {
        let mut queue = self.follow_up_queue.write();
        queue.drain(..).collect()
    }

    /// Clear the steering queue.
    pub fn clear_steering_queue(&self) {
        self.steering_queue.write().clear();
    }

    /// Clear the follow-up queue.
    pub fn clear_follow_up_queue(&self) {
        self.follow_up_queue.write().clear();
    }

    /// Clear all queues (steering and follow-up).
    pub fn clear_all_queues(&self) {
        self.clear_steering_queue();
        self.clear_follow_up_queue();
    }
}