use serde_json::json;
use crate::tools::{Tool, FunctionDefinition};
use super::icons;
use std::path::Path;
use image::GenericImageView;
use base64::{Engine as _, engine::general_purpose};
use std::io::Cursor;

pub fn get_definition() -> Tool {
    Tool {
        tool_type: "function".to_string(),
        function: FunctionDefinition {
            name: "process_image".to_string(),
            description: "Analyze an image using vision capabilities. Resizes the image so the long edge is at most 'max_size' (default 1024) before processing. Pure Rust implementation.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "prompt": {
                        "type": "string",
                        "description": "What to ask about the image"
                    },
                    "image_path": {
                        "type": "string",
                        "description": "Path to the image file"
                    },
                    "max_size": {
                        "type": "integer",
                        "description": "Maximum size for the long edge of the image (default 1024)",
                        "default": 1024
                    },
                    "tool_call_id": {
                        "type": "string",
                        "description": "A unique identifier for this call"
                    }
                },
                "required": ["prompt", "image_path", "tool_call_id"]
            }),
        },
    }
}

pub fn get_ui_description(arguments: &serde_json::Value) -> String {
    let prompt = arguments["prompt"].as_str().unwrap_or("");
    let path = arguments["image_path"].as_str().unwrap_or("");
    format!("{} Analyzing image `{}`: {}", icons::IMAGE, path, prompt)
}

pub async fn execute(
    prompt: &str, 
    image_path: &str, 
    max_size: Option<u32>, 
    cwd: &str, 
    client: &reqwest::Client, 
    config: &crate::config::Config,
    tx: &tokio::sync::mpsc::UnboundedSender<crate::client::StreamEvent>
) -> String {
    let image_path = image_path.trim_matches(|c| c == '\'' || c == '\"');
    let full_path = Path::new(cwd).join(image_path);
    if !full_path.exists() {
        return format!("ERROR: Image file not found at {}", full_path.display());
    }

    let img = match image::open(&full_path) {
        Ok(i) => i,
        Err(e) => return format!("ERROR: Failed to open image: {}", e),
    };

    let (width, height) = img.dimensions();
    let limit = max_size.unwrap_or(1024);
    
    let resized_img = if width > limit || height > limit {
        img.resize(limit, limit, image::imageops::FilterType::CatmullRom)
    } else {
        img
    };

    let _ = tx.send(crate::client::StreamEvent::DebugLog(format!("[VISION] Resized image from {}x{} to {}x{}", width, height, resized_img.width(), resized_img.height())));
    let _ = tx.send(crate::client::StreamEvent::ToolProgress("Processing vision request...".to_string()));

    let mut buf = Vec::new();
    if let Err(e) = resized_img.write_to(&mut Cursor::new(&mut buf), image::ImageFormat::Png) {
        return format!("ERROR: Failed to encode image: {}", e);
    }

    let b64 = general_purpose::STANDARD.encode(buf);
    
    match crate::client::get_single_response(client, config, prompt.to_string(), Some(vec![b64]), Some(tx)).await {
        Ok(res) => res,
        Err(e) => format!("ERROR: Vision request failed: {}", e),
    }
}
