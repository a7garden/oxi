//! Chat widgets: per-message item, tool-call card, and animated spinner.
//!
//! Higher-level chat UI (the scrollable `ChatView`) composes these
//! primitives along with `Text`, `List`, `Border`, and `Scrollbar`.

pub mod message_item;
pub mod spinner;
pub mod tool_call;

pub use message_item::MessageItem;
pub use spinner::Spinner;
pub use tool_call::ToolCall;
