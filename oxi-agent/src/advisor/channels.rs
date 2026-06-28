//! Delivery-channel resolution + `<advisory>` rendering — pure functions
//! ported from omp `advise-tool.ts`.
//!
//! # Attribution
//!
//! Translated to Rust from omp (oh-my-pi), MIT licensed.

use crate::advisor::types::{
    ADVISOR_GUIDANCE, AdvisorDeliveryChannel, AdvisorNote, AdvisorSeverity, DeliveryOpts,
};

/// Whether advice at this severity should interrupt the running agent
/// (delivered via the steering channel) rather than ride the non-interrupting
/// aside queue. `concern` and `blocker` interrupt; a plain `nit` queues.
/// omp `isInterruptingSeverity`.
#[must_use]
pub fn is_interrupting_severity(severity: Option<AdvisorSeverity>) -> bool {
    matches!(
        severity,
        Some(AdvisorSeverity::Concern | AdvisorSeverity::Blocker)
    )
}

/// Half-open turn-count fence `[start, start + turns)` for the post-interrupt
/// cooldown. omp `isAdvisorInterruptImmuneTurnActive`.
#[must_use]
pub fn is_immune_turn_active(
    completed_turns: u64,
    immune_start: Option<u64>,
    immune_turns: u64,
) -> bool {
    let Some(start) = immune_start else {
        return false;
    };
    if immune_turns == 0 {
        return false;
    }
    completed_turns < start.saturating_add(immune_turns)
}

/// Decide how one advisor note reaches the primary agent. omp
/// `resolveAdvisorDeliveryChannel`.
///
/// - A non-interrupting `nit` always rides the aside queue.
/// - An interrupting `concern`/`blocker` is normally steered into the agent.
/// - After a deliberate user interrupt (`auto_resume_suppressed`) the advisor
///   must not auto-resume the stopped run; while the agent is idle or still
///   tearing the interrupted turn down (`aborting`) the note is preserved as a
///   visible card instead of restarting the run.
/// - During the post-interrupt immune-turn window, further `concern`/`blocker`
///   notes are downgraded to asides.
#[must_use]
pub fn resolve_delivery_channel(opts: DeliveryOpts) -> AdvisorDeliveryChannel {
    if !is_interrupting_severity(opts.severity) {
        return AdvisorDeliveryChannel::Aside;
    }
    if opts.auto_resume_suppressed && (opts.aborting || !opts.streaming) {
        return AdvisorDeliveryChannel::Preserve;
    }
    if opts.interrupt_immune_turn_active {
        return AdvisorDeliveryChannel::Aside;
    }
    AdvisorDeliveryChannel::Steer
}

/// Render a batch of advisor notes as the agent-facing message body: one
/// `<advisory>` element per note, `severity` as an attribute (omitted for a
/// plain nit), `guidance` always present. omp `formatAdvisorBatchContent`.
#[must_use]
pub fn format_advisory_batch(notes: &[AdvisorNote]) -> String {
    if notes.is_empty() {
        return String::new();
    }
    let mut parts: Vec<String> = Vec::with_capacity(notes.len());
    for n in notes {
        let severity_attr = match n.severity {
            Some(s) => format!(" severity=\"{}\"", s.as_str()),
            None => String::new(),
        };
        parts.push(format!(
            "<advisory{severity_attr} guidance=\"{g}\">\n{note}\n</advisory>",
            severity_attr = severity_attr,
            g = ADVISOR_GUIDANCE,
            note = escape_xml_text(&n.note)
        ));
    }
    parts.join("\n")
}

/// Escape the five XML-significant characters for safe embedding in element
/// text. omp `escapeXmlText`.
#[must_use]
pub(crate) fn escape_xml_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn nit_is_non_interrupting() {
        assert!(!is_interrupting_severity(None));
        assert!(!is_interrupting_severity(Some(AdvisorSeverity::Nit)));
        assert!(is_interrupting_severity(Some(AdvisorSeverity::Concern)));
        assert!(is_interrupting_severity(Some(AdvisorSeverity::Blocker)));
    }

    #[test]
    fn immune_fence_is_half_open() {
        // no start -> inactive
        assert!(!is_immune_turn_active(5, None, 3));
        // zero turns -> inactive
        assert!(!is_immune_turn_active(0, Some(0), 0));
        // [start, start+turns): start=10, turns=2 -> active at 10,11; inactive at 12
        assert!(is_immune_turn_active(10, Some(10), 2));
        assert!(is_immune_turn_active(11, Some(10), 2));
        assert!(!is_immune_turn_active(12, Some(10), 2));
    }

    #[test]
    fn channel_nit_always_aside() {
        for &streaming in &[false, true] {
            for &supp in &[false, true] {
                assert_eq!(
                    resolve_delivery_channel(DeliveryOpts {
                        severity: Some(AdvisorSeverity::Nit),
                        auto_resume_suppressed: supp,
                        streaming,
                        aborting: false,
                        interrupt_immune_turn_active: false,
                    }),
                    AdvisorDeliveryChannel::Aside
                );
            }
        }
    }

    #[test]
    fn channel_concern_live_steers() {
        assert_eq!(
            resolve_delivery_channel(DeliveryOpts {
                severity: Some(AdvisorSeverity::Concern),
                streaming: true,
                ..Default::default()
            }),
            AdvisorDeliveryChannel::Steer
        );
    }

    #[test]
    fn channel_post_interrupt_idle_preserves() {
        // user interrupted (auto_resume_suppressed), agent idle -> preserve
        assert_eq!(
            resolve_delivery_channel(DeliveryOpts {
                severity: Some(AdvisorSeverity::Blocker),
                auto_resume_suppressed: true,
                streaming: false, // idle
                aborting: false,
                ..Default::default()
            }),
            AdvisorDeliveryChannel::Preserve
        );
        // but once streaming again (user resumed), steer does not auto-resume
        assert_eq!(
            resolve_delivery_channel(DeliveryOpts {
                severity: Some(AdvisorSeverity::Blocker),
                auto_resume_suppressed: true,
                streaming: true,
                aborting: false,
                ..Default::default()
            }),
            AdvisorDeliveryChannel::Steer
        );
        // aborting + suppressed -> preserve even though we can't tell streaming yet
        assert_eq!(
            resolve_delivery_channel(DeliveryOpts {
                severity: Some(AdvisorSeverity::Concern),
                auto_resume_suppressed: true,
                streaming: true,
                aborting: true,
                ..Default::default()
            }),
            AdvisorDeliveryChannel::Preserve
        );
    }

    #[test]
    fn channel_immune_downgrades_to_aside() {
        assert_eq!(
            resolve_delivery_channel(DeliveryOpts {
                severity: Some(AdvisorSeverity::Blocker),
                streaming: true,
                interrupt_immune_turn_active: true,
                ..Default::default()
            }),
            AdvisorDeliveryChannel::Aside
        );
    }

    #[test]
    fn batch_renders_advisory_elements() {
        let notes = vec![
            AdvisorNote {
                note: "Stop.".into(),
                severity: Some(AdvisorSeverity::Blocker),
            },
            AdvisorNote {
                note: "rename x".into(),
                severity: None, // plain nit -> no severity attr
            },
        ];
        let out = format_advisory_batch(&notes);
        assert!(
            out.contains("<advisory severity=\"blocker\" guidance=\"weigh, don't blindly obey\">")
        );
        assert!(out.contains("Stop."));
        // second note has no severity attribute
        assert!(out.contains("<advisory guidance=\"weigh, don't blindly obey\">\nrename x"));
    }

    #[test]
    fn batch_escapes_xml_significant_chars() {
        let notes = vec![AdvisorNote {
            note: "a < b & c > d \"e\"".into(),
            severity: Some(AdvisorSeverity::Nit),
        }];
        let out = format_advisory_batch(&notes);
        assert!(out.contains("&lt;"));
        assert!(out.contains("&amp;"));
        assert!(out.contains("&gt;"));
        assert!(out.contains("&quot;"));
        assert!(!out.contains(" < ") || out.matches("&lt;").count() >= 1);
    }

    #[test]
    fn empty_batch_is_empty() {
        assert_eq!(format_advisory_batch(&[]), "");
    }
}
