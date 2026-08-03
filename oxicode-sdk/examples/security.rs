//! Security capability enforcement demo.
//!
//! Run: `cargo run -p oxicode-sdk --example security`
//!
//! No API key required — this example demonstrates the security module only.

use std::sync::Arc;

use oxicode_sdk::StringPattern;
use oxicode_sdk::prelude::*;

fn main() {
    let audit = Arc::new(AuditLog::new(64));
    let authorizer = Arc::new(Authorizer::new(Arc::clone(&audit)));

    // Define roles
    authorizer.define_role("coder", CapabilitySet::coding("/workspace"));
    authorizer.define_role("reader", CapabilitySet::read_only("/workspace"));

    // Grant direct capabilities
    authorizer.grant(
        CapabilitySubject::Agent("admin".into()),
        CapabilitySet::all(),
    );

    // Bind roles to agents
    authorizer.bind_role("dev-agent", "coder");
    authorizer.bind_role("research-agent", "reader");

    // ── Check coder permissions ──
    let coder = CapabilitySubject::Agent("dev-agent".into());
    println!(
        "Coder can read workspace: {}",
        authorizer.check(
            &coder,
            &Capability::FileRead {
                path_pattern: "/workspace/src/main.rs".into(),
            }
        )
    );
    println!(
        "Coder can write workspace: {}",
        authorizer.check(
            &coder,
            &Capability::FileWrite {
                path_pattern: "/workspace/src/main.rs".into(),
            }
        )
    );
    println!(
        "Coder can write /etc: {}",
        authorizer.check(
            &coder,
            &Capability::FileWrite {
                path_pattern: "/etc/passwd".into(),
            }
        )
    );

    // ── Check reader permissions ──
    let reader = CapabilitySubject::Agent("research-agent".into());
    println!(
        "Reader can read: {}",
        authorizer.check(
            &reader,
            &Capability::FileRead {
                path_pattern: "/workspace/file".into(),
            }
        )
    );
    println!(
        "Reader can write: {}",
        authorizer.check(
            &reader,
            &Capability::FileWrite {
                path_pattern: "/workspace/file".into(),
            }
        )
    );

    // ── Check admin ──
    let admin = CapabilitySubject::Agent("admin".into());
    println!(
        "Admin can run bash: {}",
        authorizer.check(
            &admin,
            &Capability::Bash {
                allowed_commands: vec![StringPattern::Wildcard],
                timeout_secs: None,
            }
        )
    );

    // ── Audit log ──
    println!("\nAudit entries: {}", audit.entries().len());
    for entry in audit.entries().iter().take(3) {
        println!("  {:?}", entry);
    }
}
