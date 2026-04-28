use serde_json::json;
use crate::tools::{Tool, FunctionDefinition};
use super::icons;
use std::path::Path;
use pdf_oxide::PdfDocument;

pub fn get_definition() -> Tool {
    Tool {
        tool_type: "function".to_string(),
        function: FunctionDefinition {
            name: "get_pdf_text".to_string(),
            description: "Extract the text layer from all pages of a PDF file using pure Rust.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "pdf_path": {
                        "type": "string",
                        "description": "The path to the PDF file"
                    },
                    "tool_call_id": {
                        "type": "string",
                        "description": "A unique identifier for this call"
                    }
                },
                "required": ["pdf_path", "tool_call_id"]
            }),
        },
    }
}

pub fn get_ui_description(arguments: &serde_json::Value) -> String {
    let path = arguments["pdf_path"].as_str().unwrap_or("");
    format!("{} Extracting text from PDF (Native): `{}`", icons::PATH, path)
}

pub async fn execute(pdf_path: &str, cwd: &str, tx: &tokio::sync::mpsc::UnboundedSender<crate::client::StreamEvent>) -> String {
    let pdf_path = pdf_path.trim_matches(|c| c == '\'' || c == '\"');
    let full_path = Path::new(cwd).join(pdf_path);
    
    if !full_path.exists() {
        return format!("ERROR: PDF file not found at {}", full_path.display());
    }

    match PdfDocument::open(&full_path) {
        Ok(mut doc) => {
            let mut full_text = String::new();
            let num_pages = match doc.page_count() {
                Ok(n) => n,
                Err(e) => return format!("ERROR: Failed to get page count: {}", e),
            };
            
            for i in 0..num_pages {
                let _ = tx.send(crate::client::StreamEvent::ToolProgress(format!("Extracting text from page {}/{}...", i + 1, num_pages)));
                match doc.extract_text(i) {
                    Ok(text) => {
                        full_text.push_str(&format!("--- Page {} ---\n", i + 1));
                        full_text.push_str(&text);
                        full_text.push('\n');
                    }
                    Err(e) => {
                        full_text.push_str(&format!("--- Page {} Error: {} ---\n", i + 1, e));
                    }
                }
            }
            
            if full_text.is_empty() {
                "Successfully opened PDF, but no text was extracted.".to_string()
            } else {
                full_text
            }
        }
        Err(e) => format!("ERROR: Failed to open PDF with pdf_oxide: {}", e),
    }
}
