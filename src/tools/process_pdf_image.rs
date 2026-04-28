use serde_json::json;
use crate::tools::{Tool, FunctionDefinition};
use super::icons;
use std::path::Path;
use pdf_oxide::PdfDocument;
use pdf_oxide::rendering::{render_page, RenderOptions};
use base64::{Engine as _, engine::general_purpose};
use std::io::Cursor;

pub fn get_definition() -> Tool {
    Tool {
        tool_type: "function".to_string(),
        function: FunctionDefinition {
            name: "process_pdf_image".to_string(),
            description: "Render a specific PDF page to an image and analyze it using vision capabilities. Resizes the resulting image so the long edge is at most 'max_size' (default 1024) before processing. Pure Rust implementation.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "prompt": {
                        "type": "string",
                        "description": "What to ask about the PDF page image"
                    },
                    "pdf_path": {
                        "type": "string",
                        "description": "Path to the PDF file"
                    },
                    "page_num": {
                        "type": "integer",
                        "description": "The page number to render (1-indexed)"
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
                "required": ["prompt", "pdf_path", "page_num", "tool_call_id"]
            }),
        },
    }
}

pub fn get_ui_description(arguments: &serde_json::Value) -> String {
    let prompt = arguments["prompt"].as_str().unwrap_or("");
    let path = arguments["pdf_path"].as_str().unwrap_or("");
    let page = arguments["page_num"].as_u64().unwrap_or(1);
    format!("{} Analyzing PDF (Native) `{}` page {}: {}", icons::IMAGE, path, page, prompt)
}

pub async fn execute(
    prompt: &str, 
    pdf_path: &str, 
    page_num: usize, 
    max_size: Option<u32>, 
    cwd: &str, 
    client: &reqwest::Client, 
    config: &crate::config::Config,
    tx: &tokio::sync::mpsc::UnboundedSender<crate::client::StreamEvent>
) -> String {
    let pdf_path = pdf_path.trim_matches(|c| c == '\'' || c == '\"');
    let full_path = Path::new(cwd).join(pdf_path);
    if !full_path.exists() {
        return format!("ERROR: PDF file not found at {}", full_path.display());
    }

    let mut doc = match PdfDocument::open(&full_path) {
        Ok(d) => d,
        Err(e) => return format!("ERROR: Failed to open PDF: {}", e),
    };

    let total_pages = match doc.page_count() {
        Ok(n) => n,
        Err(e) => return format!("ERROR: Failed to get page count: {}", e),
    };

    if page_num == 0 || page_num > total_pages {
        return format!("ERROR: Page {} does not exist. PDF has {} pages.", page_num, total_pages);
    }

    // Render at a decent DPI
    let opts = RenderOptions::with_dpi(300);
    let rendered = match render_page(&mut doc, page_num - 1, &opts) {
        Ok(r) => r,
        Err(e) => return format!("ERROR: Failed to render PDF page: {}", e),
    };

    // pdf_oxide returns encoded data (PNG by default), decode it to DynamicImage
    let img = match image::load_from_memory(&rendered.data) {
        Ok(i) => i,
        Err(e) => return format!("ERROR: Failed to decode rendered PDF image: {}", e),
    };

    let (orig_w, orig_h) = (img.width(), img.height());
    let limit = max_size.unwrap_or(1024);
    let resized_img = if orig_w > limit || orig_h > limit {
        img.resize(limit, limit, image::imageops::FilterType::CatmullRom)
    } else {
        img
    };

    let _ = tx.send(crate::client::StreamEvent::DebugLog(format!("[VISION] Resized PDF page from {}x{} to {}x{}", orig_w, orig_h, resized_img.width(), resized_img.height())));
    let _ = tx.send(crate::client::StreamEvent::ToolProgress("Processing PDF vision request...".to_string()));

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

