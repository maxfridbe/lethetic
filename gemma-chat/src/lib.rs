pub mod types;
pub mod sse;
pub mod stream;
pub mod client;

pub use types::{Message, Role, ToolDefinition, StreamEvent, AssistantToolCall, FunctionCall};
pub use stream::StreamParser;
pub use client::{build_request, stream_chat, complete};
