pub const BEGINING_OF_SEQUENCE: &str = "<bos>";
pub const END_OF_SEQUENCE: &str = "<eos>";
pub const C_NEWLINE: &str = "\n";
pub const C_LITERAL_NEWLINE: &str = "\\n";

pub const TURN_OPEN: &str = "<|turn>";
pub const TURN_CLOSE: &str = "<turn|>";

pub const CHANNEL_OPEN: &str = "<|channel>";
pub const CHANNEL_CLOSE: &str = "<channel|>";

pub const ROLE_SYSTEM: &str = "system";
pub const ROLE_USER: &str = "user";
pub const ROLE_MODEL: &str = "model";

pub const CHANNEL_THOUGHT: &str = "thought";
pub const CHANNEL_TEXT: &str = "text";

pub const THINK_OPEN: &str = "<think>";
pub const THINK_CLOSE: &str = "</think>";
pub const THOUGHT_OPEN: &str = "<thought>";
pub const THOUGHT_CLOSE: &str = "</thought>";
pub const LEGACY_THOUGHT_OPEN: &str = "<|thought>";

pub const TOOL_CALL_OPEN: &str = "<|tool_call>";
pub const TOOL_CALL_OPEN_ALT: &str = "<tool_call>";
pub const TOOL_CALL_CLOSE: &str = "<tool_call|>";
pub const TOOL_CALL_CLOSE_ALT: &str = "<|tool_call|>";

pub const TOOL_RESPONSE_OPEN: &str = "<|tool_response|>";
pub const TOOL_RESPONSE_CLOSE: &str = "<tool_response|>";

pub const STRING_MARKER_V1: &str = "<|\"|>";
pub const STRING_MARKER_V2: &str = "<|\">";
pub const STRING_MARKER_V3: &str = "<|'|>";
pub const STRING_MARKER_V4: &str = "<|'>";

// Pre-concatenated tags using concat! macro (requires literals)
pub const CHANNEL_THOUGHT_OPEN: &str = concat!("<|channel>", "thought");
pub const CHANNEL_THOUGHT_OPEN_NL: &str = concat!("<|channel>", "thought", "\n");
pub const CHANNEL_TEXT_OPEN: &str = concat!("<|channel>", "text");
pub const TURN_USER_OPEN_NL: &str = concat!("<|turn>", "user", "\n");
pub const TURN_MODEL_OPEN_NL: &str = concat!("<|turn>", "model", "\n");
pub const TURN_SYSTEM_OPEN_NL: &str = concat!("<|turn>", "system", "\n");
pub const TURN_CLOSE_NL: &str = concat!("<turn|>", "\n");
pub const THINK_OPEN_NL: &str = concat!("<think>", "\n");
pub const THINK_CLOSE_NL: &str = concat!("</think>", "\n");
pub const TURN_MODEL_OPEN_THOUGHT_NL: &str = concat!("<|turn>", "model", "\n", "<|channel>", "thought", "\n");
pub const THOUGHT_NL: &str = concat!("thought", "\n");
pub const THOUGHT_SP: &str = concat!("thought", " ");
