//! Authenticated POST routes for the independent rooted file capability.

use super::ServerState;
use crate::wfe::file_contracts::{
    FilesArchiveRequest, FilesDownloadRequest, FilesErrorResponse, FilesListRequest,
    FilesReadRequest, WFE_FILES_READ_CHUNK_BYTES, WFE_FILES_REQUEST_BYTES,
};
use crate::wfe::files::{FileOperationGuard, FilesError};
use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::{Request, State};
use axum::http::header::{
    CACHE_CONTROL, CONTENT_DISPOSITION, CONTENT_LENGTH, CONTENT_TYPE, RETRY_AFTER,
};
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use futures_util::{Stream, StreamExt};
use serde::de::DeserializeOwned;
use std::convert::Infallible;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::time::timeout;

const FILE_REQUEST_BODY_TIMEOUT: Duration = Duration::from_secs(5);

static EXCLUDED_PROTECTED: HeaderName =
    HeaderName::from_static("x-lethetic-files-excluded-protected");
static EXCLUDED_UNSUPPORTED: HeaderName =
    HeaderName::from_static("x-lethetic-files-excluded-unsupported");
static EXCLUDED_UNREADABLE: HeaderName =
    HeaderName::from_static("x-lethetic-files-excluded-unreadable");

pub(super) fn register(router: Router<Arc<ServerState>>) -> Router<Arc<ServerState>> {
    router
        .route("/api/files/list", post(list))
        .route("/api/files/read", post(read))
        .route("/api/files/download", post(download))
        .route("/api/files/archive", post(archive))
}

async fn list(State(state): State<Arc<ServerState>>, request: Request) -> Response {
    let request = match authenticated_json::<FilesListRequest>(&state, request).await {
        Ok(request) => request,
        Err(response) => return response,
    };
    let Some(files) = state.files.as_ref() else {
        return super::plain_response(StatusCode::NOT_FOUND, "not found");
    };
    match files.list(request, state.cancellation.clone()).await {
        Ok(operation) => json_operation_response(operation),
        Err(error) => error_response(error),
    }
}

async fn read(State(state): State<Arc<ServerState>>, request: Request) -> Response {
    let request = match authenticated_json::<FilesReadRequest>(&state, request).await {
        Ok(request) => request,
        Err(response) => return response,
    };
    let Some(files) = state.files.as_ref() else {
        return super::plain_response(StatusCode::NOT_FOUND, "not found");
    };
    match files.read(request, state.cancellation.clone()).await {
        Ok(operation) => json_operation_response(operation),
        Err(error) => error_response(error),
    }
}

async fn download(State(state): State<Arc<ServerState>>, request: Request) -> Response {
    let request = match authenticated_json::<FilesDownloadRequest>(&state, request).await {
        Ok(request) => request,
        Err(response) => return response,
    };
    let Some(files) = state.files.as_ref() else {
        return super::plain_response(StatusCode::NOT_FOUND, "not found");
    };
    match files.download(request, state.cancellation.clone()).await {
        Ok(operation) => {
            let (download, guard) = operation.into_parts();
            binary_response(
                download.bytes,
                guard,
                download.content_type,
                &download.filename,
                None,
            )
        }
        Err(error) => error_response(error),
    }
}

async fn archive(State(state): State<Arc<ServerState>>, request: Request) -> Response {
    let request = match authenticated_json::<FilesArchiveRequest>(&state, request).await {
        Ok(request) => request,
        Err(response) => return response,
    };
    let Some(files) = state.files.as_ref() else {
        return super::plain_response(StatusCode::NOT_FOUND, "not found");
    };
    match files.archive(request, state.cancellation.clone()).await {
        Ok(operation) => {
            let (archive, guard) = operation.into_parts();
            let exclusions = archive.exclusions.clone();
            binary_response(
                archive.bytes,
                guard,
                "application/zip",
                &archive.filename,
                Some(exclusions),
            )
        }
        Err(error) => error_response(error),
    }
}

async fn authenticated_json<T: DeserializeOwned>(
    state: &ServerState,
    request: Request,
) -> Result<T, Response> {
    let (parts, body) = request.into_parts();
    if !super::exact_origin(&parts.headers, state) {
        return Err(super::plain_response(
            StatusCode::FORBIDDEN,
            "request origin rejected",
        ));
    }
    if !state.process_cookie.verify_headers(&parts.headers)
        || !super::exact_header_matches(
            &parts.headers,
            super::WFE_WEBSOCKET_PROTOCOL_HEADER.clone(),
            |candidate| state.websocket_protocol.verify(candidate),
        )
    {
        return Err(super::plain_response(
            StatusCode::UNAUTHORIZED,
            "authentication rejected",
        ));
    }
    if !super::exact_header_matches(&parts.headers, CONTENT_TYPE, |value| {
        value.eq_ignore_ascii_case("application/json")
    }) {
        return Err(super::plain_response(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "invalid request",
        ));
    }
    if parts.uri.query().is_some() {
        return Err(super::plain_response(
            StatusCode::BAD_REQUEST,
            "invalid request",
        ));
    }
    let bytes = match timeout(FILE_REQUEST_BODY_TIMEOUT, read_bounded_body(body)).await {
        Err(_) => {
            return Err(super::plain_response(
                StatusCode::REQUEST_TIMEOUT,
                "invalid request",
            ));
        }
        Ok(Err(BodyReadError::TooLarge)) => {
            return Err(super::plain_response(
                StatusCode::PAYLOAD_TOO_LARGE,
                "invalid request",
            ));
        }
        Ok(Err(BodyReadError::Invalid)) => {
            return Err(super::plain_response(
                StatusCode::BAD_REQUEST,
                "invalid request",
            ));
        }
        Ok(Ok(bytes)) => bytes,
    };
    serde_json::from_slice(&bytes)
        .map_err(|_| super::plain_response(StatusCode::BAD_REQUEST, "invalid request"))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BodyReadError {
    TooLarge,
    Invalid,
}

async fn read_bounded_body(body: Body) -> Result<Vec<u8>, BodyReadError> {
    let mut stream = body.into_data_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| BodyReadError::Invalid)?;
        let new_length = bytes
            .len()
            .checked_add(chunk.len())
            .ok_or(BodyReadError::TooLarge)?;
        if new_length > WFE_FILES_REQUEST_BYTES {
            return Err(BodyReadError::TooLarge);
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

fn json_operation_response<T: serde::Serialize>(
    operation: crate::wfe::files::FileOperation<T>,
) -> Response {
    let (value, guard) = operation.into_parts();
    let bytes = match serde_json::to_vec(&value) {
        Ok(bytes) => bytes,
        Err(_) => return error_response(FilesError::Unavailable),
    };
    let mut response = Response::new(guarded_body(bytes, guard));
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    response
}

fn binary_response(
    bytes: Vec<u8>,
    guard: FileOperationGuard,
    content_type: &'static str,
    filename: &str,
    exclusions: Option<crate::wfe::file_contracts::FileExclusionCounts>,
) -> Response {
    let length = bytes.len();
    let mut response = Response::new(guarded_body(bytes, guard));
    let headers = response.headers_mut();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static(content_type));
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    if let Ok(value) = HeaderValue::from_str(&length.to_string()) {
        headers.insert(CONTENT_LENGTH, value);
    }
    let disposition = format!("attachment; filename=\"{filename}\"");
    let Ok(disposition) = HeaderValue::from_str(&disposition) else {
        return error_response(FilesError::Unavailable);
    };
    headers.insert(CONTENT_DISPOSITION, disposition);
    if let Some(exclusions) = exclusions {
        insert_count_header(headers, EXCLUDED_PROTECTED.clone(), exclusions.protected);
        insert_count_header(
            headers,
            EXCLUDED_UNSUPPORTED.clone(),
            exclusions.unsupported,
        );
        insert_count_header(headers, EXCLUDED_UNREADABLE.clone(), exclusions.unreadable);
    }
    response
}

fn insert_count_header(headers: &mut HeaderMap, name: HeaderName, count: u32) {
    if let Ok(value) = HeaderValue::from_str(&count.to_string()) {
        headers.insert(name, value);
    }
}

fn guarded_body(bytes: Vec<u8>, guard: FileOperationGuard) -> Body {
    Body::from_stream(GuardedBytes {
        bytes: Bytes::from(bytes),
        offset: 0,
        _guard: Some(guard),
    })
}

struct GuardedBytes {
    bytes: Bytes,
    offset: usize,
    _guard: Option<FileOperationGuard>,
}

impl Stream for GuardedBytes {
    type Item = Result<Bytes, Infallible>;

    fn poll_next(mut self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if self.offset < self.bytes.len() {
            let end = self
                .offset
                .saturating_add(WFE_FILES_READ_CHUNK_BYTES)
                .min(self.bytes.len());
            let chunk = self.bytes.slice(self.offset..end);
            self.offset = end;
            return Poll::Ready(Some(Ok(chunk)));
        }
        self._guard.take();
        Poll::Ready(None)
    }
}

fn error_response(error: FilesError) -> Response {
    let status = match error {
        FilesError::BadRequest => StatusCode::BAD_REQUEST,
        FilesError::NotFound => StatusCode::NOT_FOUND,
        FilesError::Protected | FilesError::SensitiveContent => StatusCode::FORBIDDEN,
        FilesError::Unsupported | FilesError::PreviewUnavailable => {
            StatusCode::UNPROCESSABLE_ENTITY
        }
        FilesError::TooLarge => StatusCode::PAYLOAD_TOO_LARGE,
        FilesError::Changed => StatusCode::CONFLICT,
        FilesError::Busy | FilesError::Cancelled => StatusCode::SERVICE_UNAVAILABLE,
        FilesError::Deadline => StatusCode::REQUEST_TIMEOUT,
        FilesError::Unavailable => StatusCode::INTERNAL_SERVER_ERROR,
    };
    let payload = FilesErrorResponse {
        error: error.api_error(),
    };
    let bytes = serde_json::to_vec(&payload)
        .unwrap_or_else(|_| b"{\"error\":{\"code\":\"unavailable\",\"message\":\"The file service could not complete the operation\",\"retryable\":true}}".to_vec());
    let mut response = (status, [(CONTENT_TYPE, "application/json")], bytes).into_response();
    if error == FilesError::Busy {
        response
            .headers_mut()
            .insert(RETRY_AFTER, HeaderValue::from_static("1"));
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_errors_do_not_include_paths_or_raw_io() {
        let response = FilesErrorResponse {
            error: FilesError::Protected.api_error(),
        };
        let encoded = serde_json::to_string(&response).unwrap();
        assert_eq!(
            encoded,
            r#"{"error":{"code":"protected","message":"The selected item is protected","retryable":false}}"#
        );
    }

    #[tokio::test]
    async fn bounded_body_and_exact_json_shape_reject_overflow_and_duplicates() {
        assert_eq!(
            read_bounded_body(Body::from(vec![0_u8; WFE_FILES_REQUEST_BYTES + 1])).await,
            Err(BodyReadError::TooLarge)
        );
        assert!(serde_json::from_slice::<FilesListRequest>(br#"{"path":"","extra":1}"#).is_err());
        assert!(serde_json::from_slice::<FilesListRequest>(br#"{"path":"","path":"x"}"#).is_err());
        assert!(serde_json::from_slice::<FilesListRequest>(br#"[]"#).is_err());
    }

    #[tokio::test]
    async fn response_body_uses_fixed_bounded_chunks() {
        let mut body = GuardedBytes {
            bytes: Bytes::from(vec![0_u8; WFE_FILES_READ_CHUNK_BYTES * 2 + 1]),
            offset: 0,
            _guard: None,
        };
        assert_eq!(
            body.next().await.unwrap().unwrap().len(),
            WFE_FILES_READ_CHUNK_BYTES
        );
        assert_eq!(
            body.next().await.unwrap().unwrap().len(),
            WFE_FILES_READ_CHUNK_BYTES
        );
        assert_eq!(body.next().await.unwrap().unwrap().len(), 1);
        assert!(body.next().await.is_none());
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn every_file_request_requires_origin_cookie_proof_and_content_type() {
        use crate::wfe::files::{DisclosurePolicy, RootedFiles};
        use crate::wfe::security::{
            ControllerAuthenticationMode, SecurityFileOptions, WfeTarget,
            prepare_security_with_authentication,
        };
        use axum::http::header::{COOKIE, ORIGIN};

        for authentication_mode in [
            ControllerAuthenticationMode::TokenRequired,
            ControllerAuthenticationMode::Disabled,
        ] {
            let temporary = tempfile::tempdir().unwrap();
            let workspace = temporary.path().join("workspace");
            std::fs::create_dir(&workspace).unwrap();
            std::fs::write(workspace.join("ordinary.txt"), "ordinary").unwrap();
            let target = WfeTarget::parse("https://127.0.0.1:19443").unwrap();
            let security = prepare_security_with_authentication(
                &target,
                SecurityFileOptions::default(),
                authentication_mode,
                Some(&temporary.path().join("security")),
            )
            .unwrap();
            let app = crate::app::App::new(&crate::config::Config::default());
            let (frontend, _runtime) = crate::wfe::runtime::WfeRuntime::new(&app, vec![]).unwrap();
            let files = RootedFiles::open(&workspace, DisclosurePolicy::new()).unwrap();
            let state = Arc::new(
                super::super::ServerState::new_with_options(
                    security,
                    frontend,
                    tokio_util::sync::CancellationToken::new(),
                    super::super::WfeServerOptions { files: Some(files) },
                )
                .unwrap(),
            );

            for (name, secret) in [
                ("copied-cookie.txt", state.process_cookie.expose()),
                ("copied-proof.txt", state.websocket_protocol.expose()),
            ] {
                std::fs::write(workspace.join(name), secret).unwrap();
                let body = format!(r#"{{"path":"{name}"}}"#);
                let request = authenticated_request(&state, body.as_bytes());
                assert_eq!(
                    read(State(Arc::clone(&state)), request).await.status(),
                    StatusCode::FORBIDDEN
                );
            }

            let valid = authenticated_request(&state, br#"{"path":""}"#);
            let response = list(State(Arc::clone(&state)), valid).await;
            assert_eq!(response.status(), StatusCode::OK);
            drop(response);

            let mut missing_origin = authenticated_request(&state, br#"{"path":""}"#);
            missing_origin.headers_mut().remove(ORIGIN);
            assert_eq!(
                list(State(Arc::clone(&state)), missing_origin)
                    .await
                    .status(),
                StatusCode::FORBIDDEN
            );

            let mut duplicate_origin = authenticated_request(&state, br#"{"path":""}"#);
            duplicate_origin.headers_mut().append(
                ORIGIN,
                HeaderValue::from_str(&state.security.target().canonical_origin()).unwrap(),
            );
            assert_eq!(
                list(State(Arc::clone(&state)), duplicate_origin)
                    .await
                    .status(),
                StatusCode::FORBIDDEN
            );

            let mut missing_cookie = authenticated_request(&state, br#"{"path":""}"#);
            missing_cookie.headers_mut().remove(COOKIE);
            assert_eq!(
                list(State(Arc::clone(&state)), missing_cookie)
                    .await
                    .status(),
                StatusCode::UNAUTHORIZED
            );

            let mut duplicate_proof = authenticated_request(&state, br#"{"path":""}"#);
            duplicate_proof.headers_mut().append(
                super::super::WFE_WEBSOCKET_PROTOCOL_HEADER.clone(),
                HeaderValue::from_str(state.websocket_protocol.expose()).unwrap(),
            );
            assert_eq!(
                list(State(Arc::clone(&state)), duplicate_proof)
                    .await
                    .status(),
                StatusCode::UNAUTHORIZED
            );

            let mut missing_content_type = authenticated_request(&state, br#"{"path":""}"#);
            missing_content_type.headers_mut().remove(CONTENT_TYPE);
            assert_eq!(
                list(State(Arc::clone(&state)), missing_content_type)
                    .await
                    .status(),
                StatusCode::UNSUPPORTED_MEDIA_TYPE
            );

            let mut query_path = authenticated_request(&state, br#"{"path":""}"#);
            *query_path.uri_mut() = "/api/files/list?path=ordinary.txt".parse().unwrap();
            assert_eq!(
                list(State(Arc::clone(&state)), query_path).await.status(),
                StatusCode::BAD_REQUEST
            );

            let unknown_field = authenticated_request(&state, br#"{"path":"","extra":1}"#);
            assert_eq!(
                list(State(Arc::clone(&state)), unknown_field)
                    .await
                    .status(),
                StatusCode::BAD_REQUEST
            );
            let duplicate_field =
                authenticated_request(&state, br#"{"path":"","path":"ordinary.txt"}"#);
            assert_eq!(
                list(State(Arc::clone(&state)), duplicate_field)
                    .await
                    .status(),
                StatusCode::BAD_REQUEST
            );
            let oversized = authenticated_request(
                &state,
                &vec![b' '; WFE_FILES_REQUEST_BYTES.saturating_add(1)],
            );
            assert_eq!(
                list(State(Arc::clone(&state)), oversized).await.status(),
                StatusCode::PAYLOAD_TOO_LARGE
            );
        }
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn explicit_controller_token_copies_are_refused_by_file_routes() {
        use crate::wfe::files::{DisclosurePolicy, RootedFiles};
        use crate::wfe::security::{
            ControllerAuthenticationMode, SecurityFileOptions, WfeTarget,
            prepare_security_with_authentication,
        };
        use base64::Engine as _;
        use std::os::unix::fs::PermissionsExt as _;

        let temporary = tempfile::tempdir().unwrap();
        let workspace = temporary.path().join("workspace");
        let generated_root = temporary.path().join("generated-security");
        std::fs::create_dir(&workspace).unwrap();
        let target = WfeTarget::parse("https://127.0.0.1:19444").unwrap();
        let generated = prepare_security_with_authentication(
            &target,
            SecurityFileOptions::default(),
            ControllerAuthenticationMode::TokenRequired,
            Some(&generated_root),
        )
        .unwrap();
        drop(generated);
        let identity_root = std::fs::read_dir(&generated_root)
            .unwrap()
            .map(|entry| entry.unwrap())
            .find(|entry| entry.file_type().unwrap().is_dir())
            .expect("generated target identity directory")
            .path();

        let token = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(*b"synthetic-explicit-token-32bytes!");
        let token_path = temporary.path().join("controller.token");
        std::fs::write(&token_path, &token).unwrap();
        std::fs::set_permissions(&token_path, std::fs::Permissions::from_mode(0o600)).unwrap();
        std::fs::write(workspace.join("copied-token.txt"), &token).unwrap();
        let security = prepare_security_with_authentication(
            &target,
            SecurityFileOptions::new(
                Some(identity_root.join("certificate.der")),
                Some(identity_root.join("private-key.der")),
                Some(token_path),
            ),
            ControllerAuthenticationMode::TokenRequired,
            None,
        )
        .unwrap();
        assert!(security.bootstrap_token_for_host().is_none());

        let app = crate::app::App::new(&crate::config::Config::default());
        let (frontend, _runtime) = crate::wfe::runtime::WfeRuntime::new(&app, vec![]).unwrap();
        let files = RootedFiles::open(&workspace, DisclosurePolicy::new()).unwrap();
        let state = Arc::new(
            super::super::ServerState::new_with_options(
                security,
                frontend,
                tokio_util::sync::CancellationToken::new(),
                super::super::WfeServerOptions { files: Some(files) },
            )
            .unwrap(),
        );
        let request = authenticated_request(&state, br#"{"path":"copied-token.txt"}"#);
        assert_eq!(
            read(State(state), request).await.status(),
            StatusCode::FORBIDDEN
        );
    }

    #[cfg(target_os = "linux")]
    fn authenticated_request(state: &ServerState, body: &[u8]) -> Request {
        use axum::http::header::{COOKIE, ORIGIN};
        Request::builder()
            .method("POST")
            .uri("/api/files/list")
            .header(ORIGIN, state.security.target().canonical_origin())
            .header(
                COOKIE,
                format!(
                    "{}={}",
                    super::super::WFE_COOKIE_NAME,
                    state.process_cookie.expose()
                ),
            )
            .header(
                super::super::WFE_WEBSOCKET_PROTOCOL_HEADER.clone(),
                state.websocket_protocol.expose(),
            )
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_vec()))
            .unwrap()
    }
}
