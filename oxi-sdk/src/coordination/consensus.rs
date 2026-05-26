//! Consensus — simple voting for inter-agent decision making.

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Result of a vote.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoteResult {
    /// Whether a decision has been reached.
    pub decided: bool,
    /// The winning value (if decided).
    pub decision: Option<String>,
    /// Tally: value → count.
    pub tally: HashMap<String, usize>,
    /// Number of votes received.
    pub votes_received: usize,
    /// Total number of voters.
    pub total_voters: usize,
}

/// A vote session.
struct VoteSession {
    voters: Vec<String>,
    threshold: f32,
    votes: HashMap<String, String>,
}

/// Consensus manager — simple majority / unanimity voting.
///
/// For production use cases, replace with Raft or a distributed consensus protocol.
pub struct Consensus {
    sessions: RwLock<HashMap<String, VoteSession>>,
}

impl Consensus {
    /// Create a new consensus manager.
    pub fn new() -> Self {
        Self {
            sessions: RwLock::new(HashMap::new()),
        }
    }

    /// Start a new vote session.
    ///
    /// `threshold`: fraction of votes required to decide (0.5 = majority, 1.0 = unanimity).
    pub fn start(&self, vote_id: &str, voters: Vec<String>, threshold: f32) {
        self.sessions.write().insert(
            vote_id.to_string(),
            VoteSession {
                voters,
                threshold,
                votes: HashMap::new(),
            },
        );
    }

    /// Cast a vote. Returns the updated result.
    ///
    /// If the threshold is reached, `decided` becomes `true`.
    pub fn vote(&self, vote_id: &str, voter: &str, value: String) -> anyhow::Result<VoteResult> {
        let mut sessions = self.sessions.write();
        let session =
            sessions
                .get_mut(vote_id)
                .ok_or_else(|| crate::error::SdkError::VoteNotFound {
                    vote_id: vote_id.to_string(),
                })?;

        // Verify voter is eligible
        if !session.voters.iter().any(|v| v == voter) {
            return Err(anyhow::anyhow!("Voter '{}' not in voter list", voter));
        }

        session.votes.insert(voter.to_string(), value);
        Ok(self.compute_result(session))
    }

    /// Get the current status of a vote.
    pub fn status(&self, vote_id: &str) -> Option<VoteResult> {
        let sessions = self.sessions.read();
        sessions.get(vote_id).map(|s| self.compute_result(s))
    }

    fn compute_result(&self, session: &VoteSession) -> VoteResult {
        let mut tally: HashMap<String, usize> = HashMap::new();
        for value in session.votes.values() {
            *tally.entry(value.clone()).or_insert(0) += 1;
        }

        let votes_received = session.votes.len();
        let total_voters = session.voters.len();
        let required = (session.threshold * total_voters as f32).ceil() as usize;

        // Find the value with the most votes
        let (best_value, best_count) = tally
            .iter()
            .max_by_key(|(_, count)| *count)
            .map(|(v, c)| (v.clone(), *c))
            .unwrap_or_default();

        let decided = best_count >= required && votes_received > 0;
        let decision = if decided { Some(best_value) } else { None };

        VoteResult {
            decided,
            decision,
            tally,
            votes_received,
            total_voters,
        }
    }
}

impl Default for Consensus {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn majority_vote() {
        let c = Consensus::new();
        c.start("v1", vec!["a".into(), "b".into(), "c".into()], 0.5);

        c.vote("v1", "a", "yes".into()).unwrap();
        let r = c.vote("v1", "b", "yes".into()).unwrap();
        assert!(r.decided);
        assert_eq!(r.decision.unwrap(), "yes");
    }

    #[test]
    fn unanimity_required() {
        let c = Consensus::new();
        c.start("v2", vec!["a".into(), "b".into()], 1.0);

        c.vote("v2", "a", "yes".into()).unwrap();
        let r = c.status("v2").unwrap();
        assert!(!r.decided);

        c.vote("v2", "b", "yes".into()).unwrap();
        let r = c.status("v2").unwrap();
        assert!(r.decided);
    }

    #[test]
    fn split_vote_no_majority() {
        let c = Consensus::new();
        c.start("v3", vec!["a".into(), "b".into()], 0.6);

        c.vote("v3", "a", "yes".into()).unwrap();
        c.vote("v3", "b", "no".into()).unwrap();
        let r = c.status("v3").unwrap();
        assert!(!r.decided);
    }

    #[test]
    fn invalid_voter() {
        let c = Consensus::new();
        c.start("v4", vec!["a".into()], 0.5);
        let result = c.vote("v4", "intruder", "yes".into());
        assert!(result.is_err());
    }

    #[test]
    fn vote_not_found() {
        let c = Consensus::new();
        let result = c.vote("nonexistent", "a", "yes".into());
        assert!(result.is_err());
    }

    #[test]
    fn vote_result_tally() {
        let c = Consensus::new();
        c.start("v5", vec!["a".into(), "b".into(), "c".into()], 0.5);

        c.vote("v5", "a", "x".into()).unwrap();
        c.vote("v5", "b", "y".into()).unwrap();
        c.vote("v5", "c", "x".into()).unwrap();

        let r = c.status("v5").unwrap();
        assert!(r.decided);
        assert_eq!(r.decision.unwrap(), "x");
        assert_eq!(*r.tally.get("x").unwrap(), 2);
        assert_eq!(*r.tally.get("y").unwrap(), 1);
    }
}
