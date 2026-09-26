use reqwest::StatusCode;

pub(super) fn classify_body(url: &str, status: StatusCode, body: String) -> Result<String, String> {
    if status.is_success() {
        Ok(body)
    } else {
        Err(format!(
            "ERROR: HTTP {status} for {url}\nRESPONSE BODY:\n{body}"
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_success_status_preserves_body_as_an_error() {
        let result = classify_body(
            "https://example.invalid/status",
            StatusCode::SERVICE_UNAVAILABLE,
            "Service Unavailable tenant violet".to_string(),
        )
        .unwrap_err();

        assert!(result.contains("503 Service Unavailable"), "{result}");
        assert!(result.contains("tenant violet"), "{result}");
    }

    #[test]
    fn success_status_returns_body_unchanged() {
        let body = "ordinary response".to_string();
        assert_eq!(
            classify_body("https://example.invalid/ok", StatusCode::OK, body.clone()).unwrap(),
            body
        );
    }
}
