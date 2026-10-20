#[cfg(feature = "http")]
pub mod client;
pub mod sse;
pub mod stream;
pub mod types;

#[cfg(feature = "http")]
pub use client::{
    ChatOptions, complete, complete_with_usage, stream_chat, stream_chat_with_body,
    stream_chat_with_encoded_body,
};
pub use stream::StreamParser;
pub use types::{
    AssistantToolCall, Completion, FunctionCall, Message, PromptTokenDetails, Role, StreamEvent,
    ToolDefinition, Usage,
};

pub mod retry;

pub mod request;
pub use request::{RESERVED_EXTRA_BODY_FIELDS, build_request, validate_extra_body};
