use super::{MAX_WEB_APPROVAL_PREVIEW_BYTES, approval_preview};
use crate::wfe::contracts::ProjectionTruncationKind;

pub(super) struct ToolCallProjection {
    pub(super) content: String,
    pub(super) filtered: bool,
    pub(super) truncation: Option<ProjectionTruncationKind>,
}

pub(super) fn approval_safe_tool_call_content(content: &str) -> ToolCallProjection {
    let body = content.strip_prefix("call:").unwrap_or(content);
    let Some(arguments_start) = body.find('{') else {
        return incomplete_projection("tool", ProjectionTruncationKind::InvalidSource);
    };
    let tool_name = body[..arguments_start].trim();
    let tool_name = if tool_name.is_empty() {
        "tool"
    } else {
        tool_name
    };
    let arguments_source = &body[arguments_start..];
    if arguments_source.len() > MAX_WEB_APPROVAL_PREVIEW_BYTES {
        return incomplete_projection(tool_name, ProjectionTruncationKind::SizeLimit);
    }
    let Ok(arguments) = serde_json::from_str::<serde_json::Value>(arguments_source) else {
        return incomplete_projection(tool_name, ProjectionTruncationKind::InvalidSource);
    };

    if tool_name == "python" {
        let Some(arguments) = arguments.as_object() else {
            return incomplete_projection(tool_name, ProjectionTruncationKind::InvalidSource);
        };
        let Some(code) = arguments.get("code").and_then(serde_json::Value::as_str) else {
            return incomplete_projection(tool_name, ProjectionTruncationKind::InvalidSource);
        };
        let filtered = arguments.keys().any(|name| name != "code");
        let preview = serde_json::to_string_pretty(&serde_json::json!({ "code": code }))
            .expect("serializing a string-only JSON object cannot fail");
        return ToolCallProjection {
            content: format!("call:{tool_name}{preview}"),
            filtered,
            truncation: None,
        };
    }

    ToolCallProjection {
        content: format!(
            "call:{tool_name}{}",
            approval_preview(tool_name, &arguments)
        ),
        filtered: false,
        truncation: None,
    }
}

fn incomplete_projection(
    tool_name: &str,
    truncation: ProjectionTruncationKind,
) -> ToolCallProjection {
    ToolCallProjection {
        content: format!("call:{tool_name}{{}}"),
        filtered: false,
        truncation: Some(truncation),
    }
}
