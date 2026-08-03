//! Queue management for agent loop.
//!
//! Provides capacity-limited push, drain, and clear operations for the
//! steering and follow-up message queues used by the agent loop.

/// Maximum number of messages in the steering queue before back-pressure is applied.
const STEERING_QUEUE_CAPACITY: usize = 256;

/// Maximum number of messages in the follow-up queue before back-pressure is applied.
const FOLLOW_UP_QUEUE_CAPACITY: usize = 64;

/// Clear the steering queue.
pub(crate) fn clear_steering_queue(loop_ref: &super::AgentLoop) {
    loop_ref.steering_queue.write().clear();
}

/// Clear the follow-up queue.
pub(crate) fn clear_follow_up_queue(loop_ref: &super::AgentLoop) {
    loop_ref.follow_up_queue.write().clear();
}

/// Clear both steering and follow-up queues.
pub(crate) fn clear_all_queues(loop_ref: &super::AgentLoop) {
    clear_steering_queue(loop_ref);
    clear_follow_up_queue(loop_ref);
}

/// Drain and return all messages from the steering queue.
pub(crate) fn drain_steering_queue(loop_ref: &super::AgentLoop) -> Vec<oxicode_ai::Message> {
    let mut queue = loop_ref.steering_queue.write();
    queue.drain(..).collect()
}

/// Drain and return all messages from the follow-up queue.
pub(crate) fn drain_follow_up_queue(loop_ref: &super::AgentLoop) -> Vec<oxicode_ai::Message> {
    let mut queue = loop_ref.follow_up_queue.write();
    queue.drain(..).collect()
}

/// Try to push a message onto the steering queue.
///
/// Returns `true` if the message was accepted, `false` if the queue is at
/// capacity (in which case the message is dropped and a warning is logged).
pub(crate) fn try_push_steering(loop_ref: &super::AgentLoop, message: oxicode_ai::Message) -> bool {
    let mut queue = loop_ref.steering_queue.write();
    if queue.len() >= STEERING_QUEUE_CAPACITY {
        tracing::warn!(
            capacity = STEERING_QUEUE_CAPACITY,
            "Steering queue at capacity — dropping message"
        );
        return false;
    }
    queue.push(message);
    true
}

/// Try to push a message onto the follow-up queue.
///
/// Returns `true` if the message was accepted, `false` if the queue is at
/// capacity (in which case the message is dropped and a warning is logged).
pub(crate) fn try_push_follow_up(
    loop_ref: &super::AgentLoop,
    message: oxicode_ai::Message,
) -> bool {
    let mut queue = loop_ref.follow_up_queue.write();
    if queue.len() >= FOLLOW_UP_QUEUE_CAPACITY {
        tracing::warn!(
            capacity = FOLLOW_UP_QUEUE_CAPACITY,
            "Follow-up queue at capacity — dropping message"
        );
        return false;
    }
    queue.push(message);
    true
}

/// Returns the number of messages currently in the steering queue.
#[allow(dead_code)]
pub(crate) fn steering_queue_len(loop_ref: &super::AgentLoop) -> usize {
    loop_ref.steering_queue.read().len()
}

/// Returns the number of messages currently in the follow-up queue.
#[allow(dead_code)]
pub(crate) fn follow_up_queue_len(loop_ref: &super::AgentLoop) -> usize {
    loop_ref.follow_up_queue.read().len()
}
