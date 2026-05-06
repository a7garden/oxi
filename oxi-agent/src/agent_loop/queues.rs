/// Queue management for agent loop


pub(crate) fn clear_steering_queue(loop_ref: &super::AgentLoop) {
    loop_ref.steering_queue.write().clear();
}

pub(crate) fn clear_follow_up_queue(loop_ref: &super::AgentLoop) {
    loop_ref.follow_up_queue.write().clear();
}

pub(crate) fn clear_all_queues(loop_ref: &super::AgentLoop) {
    clear_steering_queue(loop_ref);
    clear_follow_up_queue(loop_ref);
}

pub(crate) fn drain_steering_queue(loop_ref: &super::AgentLoop) -> Vec<oxi_ai::Message> {
    let mut queue = loop_ref.steering_queue.write();
    queue.drain(..).collect()
}

pub(crate) fn drain_follow_up_queue(loop_ref: &super::AgentLoop) -> Vec<oxi_ai::Message> {
    let mut queue = loop_ref.follow_up_queue.write();
    queue.drain(..).collect()
}