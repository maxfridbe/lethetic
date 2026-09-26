use super::icons;
use crate::tools::{FunctionDefinition, Tool, ToolExecution};
use pdf_oxide::PdfDocument;
use serde_json::json;
use std::path::Path;

pub fn get_definition() -> Tool {
    Tool {
        tool_type: "function".to_string(),
        function: FunctionDefinition {
            name: "get_pdf_text".to_string(),
            description: "Extract the text layer from all pages of a PDF file using pure Rust."
                .to_string(),
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
    format!(
        "{} Extracting text from PDF (Native): `{}`",
        icons::PATH,
        path
    )
}

pub async fn execute(
    pdf_path: &str,
    cwd: &str,
    tx: &tokio::sync::mpsc::UnboundedSender<crate::client::StreamEvent>,
) -> String {
    execute_classified(
        pdf_path,
        cwd,
        tx,
        tokio_util::sync::CancellationToken::new(),
    )
    .await
    .output
}

pub(super) async fn execute_classified(
    pdf_path: &str,
    cwd: &str,
    tx: &tokio::sync::mpsc::UnboundedSender<crate::client::StreamEvent>,
    cancellation_token: tokio_util::sync::CancellationToken,
) -> ToolExecution {
    let result = extract_pdf_text(pdf_path, cwd, tx, &cancellation_token);
    match result {
        Ok(output) => ToolExecution::success(output, cwd),
        Err(error) => ToolExecution::error(error, cwd),
    }
}

fn extract_pdf_text(
    pdf_path: &str,
    cwd: &str,
    tx: &tokio::sync::mpsc::UnboundedSender<crate::client::StreamEvent>,
    cancellation_token: &tokio_util::sync::CancellationToken,
) -> Result<String, String> {
    if cancellation_token.is_cancelled() {
        return Err("[Operation Cancelled by User]".to_string());
    }

    let pdf_path = pdf_path.trim_matches(|c| c == '\'' || c == '"');
    let full_path = Path::new(cwd).join(pdf_path);
    if !full_path.exists() {
        return Err(format!(
            "ERROR: PDF file not found at {}",
            full_path.display()
        ));
    }

    let doc = PdfDocument::open(&full_path)
        .map_err(|error| format!("ERROR: Failed to open PDF with pdf_oxide: {error}"))?;
    let num_pages = doc
        .page_count()
        .map_err(|error| format!("ERROR: Failed to get page count: {error}"))?;
    let mut full_text = String::new();
    let mut had_page_error = false;

    for i in 0..num_pages {
        if cancellation_token.is_cancelled() {
            return Err(cancelled_with_partial_output(full_text));
        }
        let _ = tx.send(crate::client::StreamEvent::ToolProgress(format!(
            "Extracting text from page {}/{}...",
            i + 1,
            num_pages
        )));
        match doc.extract_text(i) {
            Ok(text) => {
                full_text.push_str(&format!("--- Page {} ---\n", i + 1));
                full_text.push_str(&text);
                full_text.push('\n');
            }
            Err(error) => {
                had_page_error = true;
                full_text.push_str(&format!("--- Page {} Error: {} ---\n", i + 1, error));
            }
        }
        if cancellation_token.is_cancelled() {
            return Err(cancelled_with_partial_output(full_text));
        }
    }

    finish_extraction(full_text, had_page_error)
}

fn cancelled_with_partial_output(partial: String) -> String {
    if partial.is_empty() {
        "[Operation Cancelled by User]".to_string()
    } else {
        format!("[Operation Cancelled by User]\n\n{partial}")
    }
}

fn finish_extraction(full_text: String, had_page_error: bool) -> Result<String, String> {
    if had_page_error {
        Err(full_text)
    } else if full_text.is_empty() {
        Ok("Successfully opened PDF, but no text was extracted.".to_string())
    } else {
        Ok(full_text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_extraction_diagnostic_is_an_error() {
        let diagnostic =
            "--- Page 1 Error: PDF is encrypted and requires a password ---\n".to_string();
        assert_eq!(finish_extraction(diagnostic.clone(), true), Err(diagnostic));
    }

    #[tokio::test]
    async fn pre_cancelled_extraction_is_a_typed_error() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let token = tokio_util::sync::CancellationToken::new();
        token.cancel();

        let result = execute_classified("never-opened.pdf", ".", &tx, token).await;

        assert!(result.is_error);
        assert_eq!(result.output, "[Operation Cancelled by User]");
    }
}
