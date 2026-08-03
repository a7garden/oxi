//! Unified audit sink — single destination for all security events.
//!
//! All security decisions flow through `AuditSink` into the Merkle-chain
//! `AuditTrail` for tamper-evidence.

use std::path::PathBuf;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::observability::audit_trail::{AuditAction, AuditTrail};

// ─── Audit Event ────────────────────────────────────────────────────────────

/// Unified security audit event.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind")]
#[allow(missing_docs)]
pub enum AuditEvent {
    ToolAccess {
        #[serde(with = "chrono::serde::ts_milliseconds")]
        timestamp: DateTime<Utc>,
        agent: String,
        tool: String,
        allowed: bool,
        layer: Option<String>,
        reason: Option<String>,
    },
    PathAccess {
        #[serde(with = "chrono::serde::ts_milliseconds")]
        timestamp: DateTime<Utc>,
        agent: String,
        path: String,
        mode: String,
        allowed: bool,
        layer: Option<String>,
        reason: Option<String>,
    },
    ExecAccess {
        #[serde(with = "chrono::serde::ts_milliseconds")]
        timestamp: DateTime<Utc>,
        agent: String,
        binary: String,
        allowed: bool,
        layer: Option<String>,
        reason: Option<String>,
    },
    RbacDecision {
        #[serde(with = "chrono::serde::ts_milliseconds")]
        timestamp: DateTime<Utc>,
        subject: String,
        action: String,
        resource: String,
        allowed: bool,
        reason: Option<String>,
    },
    SandboxViolation {
        #[serde(with = "chrono::serde::ts_milliseconds")]
        timestamp: DateTime<Utc>,
        agent: String,
        path: String,
        workspace: String,
    },
    Approval {
        #[serde(with = "chrono::serde::ts_milliseconds")]
        timestamp: DateTime<Utc>,
        approval_id: String,
        subject: String,
        action: String,
        status: String,
    },
}

impl AuditEvent {
    /// Agent/subject responsible.
    pub fn actor(&self) -> &str {
        match self {
            AuditEvent::ToolAccess { agent, .. } => agent,
            AuditEvent::PathAccess { agent, .. } => agent,
            AuditEvent::ExecAccess { agent, .. } => agent,
            AuditEvent::RbacDecision { subject, .. } => subject,
            AuditEvent::SandboxViolation { agent, .. } => agent,
            AuditEvent::Approval { subject, .. } => subject,
        }
    }

    /// Convert to AuditAction for the Merkle chain.
    pub fn to_audit_action(&self) -> AuditAction {
        match self {
            AuditEvent::ToolAccess { tool, allowed, .. } => AuditAction::Other {
                detail: format!("tool_access:{tool}:allowed={allowed}"),
            },
            AuditEvent::PathAccess {
                path,
                mode,
                allowed,
                ..
            } => AuditAction::Other {
                detail: format!("path_access:{path}:{mode}:allowed={allowed}"),
            },
            AuditEvent::ExecAccess {
                binary, allowed, ..
            } => AuditAction::Other {
                detail: format!("exec_access:{binary}:allowed={allowed}"),
            },
            AuditEvent::RbacDecision {
                subject,
                action,
                allowed,
                ..
            } => AuditAction::Other {
                detail: format!("rbac:{subject}:{action}:allowed={allowed}"),
            },
            AuditEvent::SandboxViolation {
                agent,
                path,
                workspace,
                ..
            } => AuditAction::Other {
                detail: format!("sandbox_violation:{agent}:{path}:ws={workspace}"),
            },
            AuditEvent::Approval {
                approval_id,
                status,
                ..
            } => AuditAction::Other {
                detail: format!("approval:{approval_id}:{status}"),
            },
        }
    }
}

// ─── Audit Sink Trait ───────────────────────────────────────────────────────

/// Destination for all security audit events.
pub trait AuditSink: Send + Sync {
    /// Record a security audit event.
    fn record(&self, event: AuditEvent);
}

// ─── Trail Audit Sink ───────────────────────────────────────────────────────

/// Production sink: Merkle chain + async JSONL file writer.
pub struct TrailAuditSink {
    trail: Arc<AuditTrail>,
    file_tx: tokio::sync::mpsc::Sender<String>,
}

impl TrailAuditSink {
    /// Create a new sink. Spawns a background task writing JSONL to `audit_path`.
    pub fn new(trail: Arc<AuditTrail>, audit_path: PathBuf) -> Self {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<String>(1000);

        tokio::spawn(async move {
            if let Ok(mut file) = tokio::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&audit_path)
                .await
            {
                use tokio::io::AsyncWriteExt;
                while let Some(line) = rx.recv().await {
                    let _ = file.write_all(line.as_bytes()).await;
                    let _ = file.write_all(b"\n").await;
                }
            }
        });

        Self { trail, file_tx: tx }
    }
}

impl AuditSink for TrailAuditSink {
    fn record(&self, event: AuditEvent) {
        let actor = event.actor().to_string();
        let action = event.to_audit_action();
        self.trail.append(actor, action, "access_gate".into());

        if let Ok(line) = serde_json::to_string(&event) {
            match self.file_tx.try_send(line) {
                Ok(()) => {}
                Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                    tracing::warn!("Audit sink channel full — event still in Merkle chain");
                }
                Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                    tracing::warn!("Audit sink channel closed");
                }
            }
        }
    }
}

/// Minimal sink that logs denied tool access via tracing.
pub struct TracingAuditSink;

impl AuditSink for TracingAuditSink {
    fn record(&self, event: AuditEvent) {
        if let AuditEvent::ToolAccess {
            agent,
            tool,
            allowed: false,
            layer,
            ..
        } = &event
        {
            tracing::warn!(
                agent = %agent,
                tool = %tool,
                layer = ?layer,
                "Access denied"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// No-op sink for tests.
    #[allow(dead_code)] // test helper, not all test binaries use it
    pub struct NoOpAuditSink;

    #[cfg(test)]
    impl AuditSink for NoOpAuditSink {
        fn record(&self, _event: AuditEvent) {}
    }

    #[test]
    fn test_event_actor() {
        let event = AuditEvent::ToolAccess {
            timestamp: Utc::now(),
            agent: "test-agent".into(),
            tool: "exec".into(),
            allowed: true,
            layer: None,
            reason: None,
        };
        assert_eq!(event.actor(), "test-agent");
    }

    #[test]
    fn test_event_serialization() {
        let event = AuditEvent::ExecAccess {
            timestamp: Utc::now(),
            agent: "test".into(),
            binary: "git".into(),
            allowed: true,
            layer: None,
            reason: None,
        };
        let json = serde_json::to_string(&event).unwrap();
        let de: AuditEvent = serde_json::from_str(&json).unwrap();
        assert!(matches!(de, AuditEvent::ExecAccess { .. }));
    }
}
