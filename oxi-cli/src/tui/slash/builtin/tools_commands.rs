//! Tool/session-context commands: `/compact`, `/session`, `/settings`, `/mcp`,
//! `/tools`. Migrated off the legacy `handle_slash_command` match.

use super::super::registry::SlashCommand;
use crate::app::agent_session::AgentSession;
use crate::tui::app::{AppState, NotificationKind};
use crate::tui::completion::{CompletionItem, CompletionKind};
use crate::tui::overlay;
use crate::tui::slash::{SlashCtx, SlashOutcome};

/// `/compact [soft|snapcompact] [instructions]` — manually compact session context.
///
/// Subcommands:
///   `/compact`                — soft compaction (default)
///   `/compact snapcompact`    — snapcompact PNG-frame compaction
///   `/compact soft <text>`    — soft compaction with custom instructions
pub(crate) struct CompactCommand;

impl SlashCommand for CompactCommand {
    fn name(&self) -> &str {
        "compact"
    }
    fn description(&self) -> &str {
        "Manually compact the session context"
    }
    fn execute(&self, args: &str, ctx: &mut SlashCtx<'_>) -> SlashOutcome {
        let trimmed = args.trim();
        let (is_snapcompact, instructions) = if trimmed.is_empty() {
            (false, None)
        } else if trimmed.starts_with("snapcompact") || trimmed.starts_with("snap") {
            let rest = trimmed
                .strip_prefix("snapcompact")
                .or_else(|| trimmed.strip_prefix("snap"))
                .unwrap_or("")
                .trim();
            (
                true,
                if rest.is_empty() {
                    None
                } else {
                    Some(rest.to_string())
                },
            )
        } else {
            // "soft" keyword or anything else = soft with instructions
            let rest = trimmed.strip_prefix("soft").unwrap_or(trimmed).trim();
            (
                false,
                if rest.is_empty() {
                    None
                } else {
                    Some(rest.to_string())
                },
            )
        };

        let sh = ctx.session.clone_handle();
        let tx = ctx.ui_tx.clone();
        tokio::spawn(async move {
            // Save strategy, override for this compaction, restore after
            let saved_strategy = sh.agent_ref().compaction_strategy();
            if is_snapcompact {
                sh.agent_ref()
                    .set_compaction_strategy(oxi_sdk::CompactionStrategy::Snapcompact);
            }
            let result = sh.compact(instructions).await;
            // Restore original strategy (important: Snapcompact leak would
            // affect all subsequent auto-compactions)
            sh.agent_ref().set_compaction_strategy(saved_strategy);
            let _ = match &result {
                Ok(r) => tx.send(crate::tui::app::UiEvent::SystemMessage(format!(
                    "Compacted ({}) from {} tokens",
                    if is_snapcompact {
                        "snapcompact"
                    } else {
                        "soft"
                    },
                    r.tokens_before
                ))),
                Err(e) => tx.send(crate::tui::app::UiEvent::SystemMessage(format!(
                    "Compaction failed: {}",
                    e
                ))),
            };
        });
        SlashOutcome::Handled
    }
}

/// `/session` — show session info and stats.
pub(crate) struct SessionCommand;

impl SlashCommand for SessionCommand {
    fn name(&self) -> &str {
        "session"
    }
    fn description(&self) -> &str {
        "Show session info and stats"
    }
    fn execute(&self, _args: &str, ctx: &mut SlashCtx<'_>) -> SlashOutcome {
        let session = ctx.session;
        let stats = session.session_stats();
        let content = format!(
            "Session: {}\n\nMessages: {} ({} user, {} assistant)\nTools: {} calls, {} results\n\nModel: {}\nThinking: {:?}\n\nAuto-compact: {}\nAuto-retry: {}",
            stats.session_id,
            stats.total_messages,
            stats.user_messages,
            stats.assistant_messages,
            stats.tool_calls,
            stats.tool_results,
            session.model_id(),
            session.thinking_level(),
            session.auto_compaction_enabled(),
            session.auto_retry_enabled(),
        );
        ctx.state.overlay = None;
        ctx.state.overlay_state = Some(Box::new(overlay::text_viewer::TextViewerOverlay::new(
            " Session Info ",
            content,
        )));
        SlashOutcome::Handled
    }
}

/// `/settings` — edit settings (theme, language, tools, …).
pub(crate) struct SettingsCommand;

impl SlashCommand for SettingsCommand {
    fn name(&self) -> &str {
        "settings"
    }
    fn description(&self) -> &str {
        "Edit settings (theme, language, tools, ...)"
    }
    fn execute(&self, _args: &str, ctx: &mut SlashCtx<'_>) -> SlashOutcome {
        ctx.state.overlay_state = Some(overlay::settings_overlay(
            &ctx.session.clone_handle(),
            ctx.state,
        ));
        SlashOutcome::Handled
    }
}

/// `/mcp [dashboard|status]` — manage MCP servers / show status.
///
/// The legacy match had `/mcp`, `/mcp dashboard`, `/mcp status` as three
/// separate arms. Here they collapse into one command that parses its args.
pub(crate) struct McpCommand;

impl SlashCommand for McpCommand {
    fn name(&self) -> &str {
        "mcp"
    }
    fn description(&self) -> &str {
        "Manage MCP servers (Quick Add presets / add / edit / remove / reauth) or open status dashboard"
    }
    fn usage(&self) -> &str {
        "/mcp <dashboard|status|reauth <server>>"
    }
    fn execute(&self, args: &str, ctx: &mut SlashCtx<'_>) -> SlashOutcome {
        let session = ctx.session;
        // /mcp reauth <server> — force-refresh OAuth credentials (v2.2).
        let trimmed = args.trim();
        if trimmed == "reauth" || trimmed.starts_with("reauth ") {
            let server = trimmed.strip_prefix("reauth").unwrap_or("").trim();
            if server.is_empty() {
                ctx.state.add_notification(
                    "Usage: /mcp reauth <server>".into(),
                    NotificationKind::Warning,
                );
                return SlashOutcome::Handled;
            }
            match session.agent_ref().tools().mcp_manager() {
                Some(m) => {
                    let server_owned = server.to_string();
                    let mgr = m.clone();
                    let result = tokio::task::block_in_place(|| {
                        tokio::runtime::Handle::current().block_on(mgr.reauth_server(&server_owned))
                    });
                    match result {
                        Ok(()) => ctx.state.add_notification(
                            format!("Reauthenticated MCP server '{}'", server_owned),
                            NotificationKind::Info,
                        ),
                        Err(e) => ctx.state.add_notification(
                            format!("MCP reauth '{}' failed: {}", server_owned, e),
                            NotificationKind::Error,
                        ),
                    }
                }
                None => ctx.state.add_notification(
                    "MCP runtime manager unavailable.".into(),
                    NotificationKind::Warning,
                ),
            }
            return SlashOutcome::Handled;
        }
        match args.trim() {
            // Dashboard is a read-only runtime view; it requires the live
            // manager. Fall back to a helpful notice when missing.
            "dashboard" => match session.agent_ref().tools().mcp_manager() {
                Some(m) => {
                    ctx.state.overlay = None;
                    ctx.state.overlay_state = Some(Box::new(
                        overlay::mcp_dashboard::McpDashboardOverlay::new(m),
                    ));
                }
                None => {
                    ctx.state.add_notification(
                        "MCP runtime manager unavailable — use /mcp to configure servers.".into(),
                        NotificationKind::Warning,
                    );
                }
            },
            "status" => match session.agent_ref().tools().mcp_manager() {
                Some(m) => {
                    let status = m.status();
                    let status_text = tokio::task::block_in_place(|| {
                        tokio::runtime::Handle::current().block_on(status)
                    });
                    ctx.state
                        .add_notification(status_text, NotificationKind::Info);
                }
                None => {
                    ctx.state.add_notification(
                        "MCP runtime manager unavailable — use /mcp to configure servers.".into(),
                        NotificationKind::Warning,
                    );
                }
            },
            _ => {
                // Bare /mcp: always open the interactive management overlay,
                // even when no servers are configured. The overlay opens on
                // the Quick Add gallery when the list is empty. A missing
                // manager is rare (bootstrap wires it) but handled: the
                // overlay still reads/writes the config file.
                let manager = session.agent_ref().tools().mcp_manager();
                let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
                ctx.state.overlay = None;
                ctx.state.overlay_state = Some(Box::new(
                    overlay::mcp_config::McpConfigOverlay::new(manager, cwd),
                ));
            }
        }
        SlashOutcome::Handled
    }

    fn complete_arg(
        &self,
        prefix: &str,
        _session: &AgentSession,
        _state: &AppState,
    ) -> Vec<CompletionItem> {
        static_subcommands(prefix, "mcp", &["dashboard", "status", "reauth"])
    }
}

/// `/tools [name]` — list active tools or toggle a tool on/off.
pub(crate) struct ToolsCommand;

impl SlashCommand for ToolsCommand {
    fn name(&self) -> &str {
        "tools"
    }
    fn description(&self) -> &str {
        "List active tools or toggle tool on/off"
    }
    fn execute(&self, args: &str, ctx: &mut SlashCtx<'_>) -> SlashOutcome {
        let session = ctx.session;
        let registry = session.agent_ref().tools();
        let names = registry.names();
        if let Some(action) = parse_opt_arg(args) {
            handle_tool_command(action, &registry, ctx.state);
        } else {
            let mut out = "Available Tools:\n\n".to_string();
            for name in &names {
                if let Some(tool) = registry.get(name) {
                    out.push_str(&format!("  {} — {}\n", name, tool.label()));
                }
            }
            out.push_str("\n/tools <name>  Toggle tool on/off");
            ctx.state.overlay = None;
            ctx.state.overlay_state = Some(overlay::tools_overlay(out));
        }
        SlashOutcome::Handled
    }

    fn complete_arg(
        &self,
        prefix: &str,
        session: &AgentSession,
        _state: &AppState,
    ) -> Vec<CompletionItem> {
        // Complete registered tool names (the toggle target).
        let last = prefix.rsplit(' ').next().unwrap_or("");
        session
            .agent_ref()
            .tools()
            .names()
            .into_iter()
            .filter(|n| n.starts_with(last))
            .map(|n| CompletionItem {
                text: n.clone(),
                label: n,
                description: None,
                kind: CompletionKind::SlashArgument {
                    command: "tools".to_string(),
                },
            })
            .collect()
    }
}

/// Map a non-empty trimmed arg to `Some(&str)`, else `None`. Mirrors the legacy
/// `arg: Option<&str>` shape so ported bodies change as little as possible.
fn parse_opt_arg(args: &str) -> Option<&str> {
    let t = args.trim();
    if t.is_empty() { None } else { Some(t) }
}

/// Build completion items for a fixed set of subcommand tokens.
fn static_subcommands(prefix: &str, command: &str, tokens: &[&str]) -> Vec<CompletionItem> {
    let last = prefix.rsplit(' ').next().unwrap_or("");
    tokens
        .iter()
        .filter(|t| t.starts_with(last))
        .map(|t| CompletionItem {
            text: t.to_string(),
            label: t.to_string(),
            description: None,
            kind: CompletionKind::SlashArgument {
                command: command.to_string(),
            },
        })
        .collect()
}

// ── Tool toggle ─────────────────────────────────────────────────────────

/// Built-in tool definitions (name) for toggle validation.
const BUILTIN_TOOL_NAMES: &[&str] = &[
    "read",
    "write",
    "edit",
    "bash",
    "grep",
    "find",
    "ls",
    "web_search",
    "get_search_results",
    "github",
    "github_search",
    "subagent",
];

/// Handle `/tools <name>` — toggle a tool on/off.
pub(super) fn handle_tool_command(
    action: &str,
    registry: &std::sync::Arc<oxi_agent::ToolRegistry>,
    state: &mut AppState,
) {
    let tool_name = action.trim().to_lowercase();

    let is_known =
        BUILTIN_TOOL_NAMES.contains(&tool_name.as_str()) || registry.get(&tool_name).is_some();

    if !is_known {
        state.add_notification(
            format!("Unknown tool: {}", tool_name),
            NotificationKind::Warning,
        );
        return;
    }

    if registry.get(&tool_name).is_some() {
        if let Some(tool) = registry.get(&tool_name)
            && tool.essential()
        {
            state.add_notification(
                format!("Cannot disable essential tool: {}", tool_name),
                NotificationKind::Warning,
            );
            return;
        }
        registry.unregister(&tool_name);
        if tool_name == "web_search" {
            registry.unregister("get_search_results");
        }
        state.add_notification(
            format!("Tool disabled: {}", tool_name),
            NotificationKind::Info,
        );
    } else {
        let re_registered = try_re_register_tool(&tool_name, registry);
        if re_registered {
            state.add_notification(
                format!("Tool enabled: {}", tool_name),
                NotificationKind::Success,
            );
        } else {
            state.add_notification(
                format!("Cannot re-enable {}", tool_name),
                NotificationKind::Warning,
            );
        }
    }
}

/// Try to re-register a previously disabled built-in tool.
fn try_re_register_tool(name: &str, registry: &std::sync::Arc<oxi_agent::ToolRegistry>) -> bool {
    use std::sync::Arc;

    match name {
        "read" | "write" | "edit" | "bash" | "grep" | "find" | "ls" => return false,
        "web_search" => {
            let cache = Arc::new(oxi_agent::SearchCache::new());
            registry.register(oxi_agent::WebSearchTool::new(cache.clone()));
            registry.register(oxi_agent::GetSearchResultsTool::new(cache));
        }
        "get_search_results" => {
            if registry.get("web_search").is_some() {
                return false;
            }
            let cache = Arc::new(oxi_agent::SearchCache::new());
            registry.register(oxi_agent::GetSearchResultsTool::new(cache));
        }
        "github" | "github_search" => {
            let cache = Arc::new(oxi_agent::SearchCache::new());
            registry.register(oxi_agent::GitHubTool::new(cache));
        }
        "subagent" => registry.register(oxi_agent::SubagentTool::with_cwd(
            std::path::PathBuf::from("."),
        )),
        _ => return false,
    }
    true
}
