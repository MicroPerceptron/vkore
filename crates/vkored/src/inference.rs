//! Local OpenAI-compatible HTTP adapter. These are host API types, not a
//! guest wire protocol. The future Kore-owned channel plugs into `Backend`.
use std::{
    future::Future,
    net::{SocketAddr, TcpListener},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, State, rejection::JsonRejection},
    http::{StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::{Semaphore, oneshot};

pub const MAX_REQUEST_BYTES: usize = 1 << 20;
pub const MAX_OUTPUT_BYTES: usize = 4 << 20;
const MAX_CONNECTIONS: usize = 32;
const DEADLINE: Duration = Duration::from_secs(300);

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Message {
    pub role: String,
    pub content: String,
}

/// The first pass supports text chat, one completion and bounded sampling.
/// Unsupported options are rejected rather than silently changing semantics.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<Message>,
    #[serde(default)]
    pub stream: bool,
    pub max_tokens: Option<u32>,
    pub max_completion_tokens: Option<u32>,
    pub temperature: Option<f32>,
    pub top_p: Option<f32>,
    pub n: Option<u32>,
}

#[derive(Debug)]
pub struct Completion {
    pub text: String,
    pub finish_reason: FinishReason,
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FinishReason {
    Stop,
    Length,
}

#[derive(Clone, Debug, Serialize)]
pub struct Model {
    pub id: String,
}

/// A provider must return only currently usable models, honor every accepted
/// request option, and stop/reclaim work when its future is dropped. No replay
/// after timeout or disconnect. Implementations must not block the HTTP runtime.
pub trait Backend: Send + Sync + 'static {
    fn models(&self) -> impl Future<Output = Result<Vec<Model>, ApiError>> + Send;
    fn complete(
        &self,
        request: ChatRequest,
    ) -> impl Future<Output = Result<Completion, ApiError>> + Send;
}

/// Explicitly disconnected production adapter until libkore exports a channel.
pub struct Unavailable;
impl Backend for Unavailable {
    async fn models(&self) -> Result<Vec<Model>, ApiError> {
        Ok(Vec::new())
    }
    async fn complete(&self, _: ChatRequest) -> Result<Completion, ApiError> {
        Err(ApiError::unavailable())
    }
}

#[derive(Debug)]
pub struct ApiError {
    status: StatusCode,
    code: &'static str,
    message: String,
}
impl ApiError {
    pub fn unavailable() -> Self {
        Self::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "backend_unavailable",
            "No guest inference backend is connected. The Kore channel is not yet integrated.",
        )
    }
    pub fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
        }
    }
    fn invalid(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "invalid_request", message)
    }
}
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let kind = if self.status.is_server_error() {
            "server_error"
        } else {
            "invalid_request_error"
        };
        (self.status, Json(json!({"error": {"message": self.message, "type": kind, "param": null, "code": self.code}}))).into_response()
    }
}

pub fn router<B: Backend>(backend: Arc<B>) -> Router {
    Router::new()
        .route("/v1/models", get(models::<B>))
        .route("/v1/chat/completions", post(chat::<B>))
        .fallback(|| async {
            ApiError::new(StatusCode::NOT_FOUND, "not_found", "Unknown API endpoint")
        })
        .method_not_allowed_fallback(|| async {
            ApiError::new(
                StatusCode::METHOD_NOT_ALLOWED,
                "method_not_allowed",
                "Method not allowed",
            )
        })
        .layer(DefaultBodyLimit::max(MAX_REQUEST_BYTES))
        .layer(middleware::from_fn(local_request))
        .with_state(backend)
}

async fn local_request(request: axum::extract::Request, next: Next) -> Response {
    // Loopback alone does not prevent a hostile web page or DNS rebinding.
    // No browser origins or non-local Host authorities are accepted.
    let local_host = request
        .headers()
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .and_then(|s| s.parse::<axum::http::uri::Authority>().ok())
        .is_some_and(|a| matches!(a.host(), "localhost" | "127.0.0.1" | "[::1]"));
    if !local_host || request.headers().contains_key(header::ORIGIN) {
        return ApiError::new(
            StatusCode::FORBIDDEN,
            "local_clients_only",
            "Use a local HTTP client with a loopback URL",
        )
        .into_response();
    }
    match tokio::time::timeout(DEADLINE, next.run(request)).await {
        Ok(response) => response,
        Err(_) => ApiError::new(
            StatusCode::GATEWAY_TIMEOUT,
            "request_timeout",
            "Inference request timed out",
        )
        .into_response(),
    }
}

async fn available_models<B: Backend>(backend: &B) -> Result<Vec<Model>, ApiError> {
    let models = backend.models().await?;
    if models.len() > 256 || models.iter().any(|m| m.id.is_empty() || m.id.len() > 256) {
        return Err(ApiError::new(
            StatusCode::BAD_GATEWAY,
            "invalid_backend_response",
            "Backend model list exceeds API bounds",
        ));
    }
    Ok(models)
}

async fn models<B: Backend>(State(backend): State<Arc<B>>) -> Result<Json<Value>, ApiError> {
    let models = available_models(backend.as_ref()).await?;
    Ok(Json(
        json!({"object": "list", "data": models.into_iter().map(|m| json!({"id": m.id, "object": "model", "created": 0, "owned_by": "kore"})).collect::<Vec<_>>()}),
    ))
}

async fn chat<B: Backend>(
    State(backend): State<Arc<B>>,
    request: Result<Json<ChatRequest>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let Json(request) =
        request.map_err(|e| ApiError::new(e.status(), "invalid_request", e.body_text()))?;
    validate(&request)?;
    // Empty availability is a disconnected/unready backend, not an invented model.
    let models = available_models(backend.as_ref()).await?;
    if models.is_empty() {
        return Err(ApiError::unavailable());
    }
    if !models.iter().any(|m| m.id == request.model) {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "model_not_found",
            "The requested model is not available",
        ));
    }
    let model = request.model.clone();
    let completion = backend.complete(request).await?;
    if completion.text.len() > MAX_OUTPUT_BYTES {
        return Err(ApiError::new(
            StatusCode::BAD_GATEWAY,
            "invalid_backend_response",
            "Backend output exceeds API bounds",
        ));
    }
    static NEXT_ID: AtomicU64 = AtomicU64::new(1);
    let created = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    Ok(Json(json!({
        "id": format!("chatcmpl-{}-{created}-{}", std::process::id(), NEXT_ID.fetch_add(1, Ordering::Relaxed)),
        "object": "chat.completion", "created": created, "model": model,
        "choices": [{"index": 0, "message": {"role": "assistant", "content": completion.text}, "finish_reason": completion.finish_reason}],
        "usage": {"prompt_tokens": completion.prompt_tokens, "completion_tokens": completion.completion_tokens,
            "total_tokens": u64::from(completion.prompt_tokens) + u64::from(completion.completion_tokens)}
    })))
}

fn validate(request: &ChatRequest) -> Result<(), ApiError> {
    if request.model.trim().is_empty() || request.model.len() > 256 {
        return Err(ApiError::invalid("model must contain 1 to 256 bytes"));
    }
    if request.messages.is_empty()
        || request.messages.len() > 256
        || request.messages.iter().any(|m| {
            !matches!(
                m.role.as_str(),
                "system" | "developer" | "user" | "assistant"
            )
        })
    {
        return Err(ApiError::invalid(
            "messages must contain 1 to 256 text messages with supported roles",
        ));
    }
    if request.stream {
        return Err(ApiError::invalid(
            "Streaming is not supported in this first pass; use stream: false",
        ));
    }
    if request.n.is_some_and(|n| n != 1) {
        return Err(ApiError::invalid("Only n: 1 is supported"));
    }
    if request.max_tokens.is_some() && request.max_completion_tokens.is_some() {
        return Err(ApiError::invalid("Specify only one token limit"));
    }
    if request
        .max_tokens
        .or(request.max_completion_tokens)
        .is_some_and(|n| n == 0 || n > 32768)
    {
        return Err(ApiError::invalid("Token limit must be between 1 and 32768"));
    }
    if request
        .temperature
        .is_some_and(|n| !n.is_finite() || !(0.0..=2.0).contains(&n))
        || request
            .top_p
            .is_some_and(|n| !n.is_finite() || !(0.0..=1.0).contains(&n))
    {
        return Err(ApiError::invalid("temperature must be 0..2 and top_p 0..1"));
    }
    Ok(())
}

/// Owns the listener and all its connections. Drop stops it and cancels work;
/// the UI and CLI share this lifecycle, with no independent inference policy.
pub struct Server {
    pub address: SocketAddr,
    stop: Option<oneshot::Sender<()>>,
    worker: Option<thread::JoinHandle<()>>,
}
impl Server {
    pub fn start<B: Backend>(address: SocketAddr, backend: Arc<B>) -> Result<Self, String> {
        if !address.ip().is_loopback() {
            return Err("the inference API must bind to a loopback address".into());
        }
        let listener = TcpListener::bind(address).map_err(|e| e.to_string())?;
        listener.set_nonblocking(true).map_err(|e| e.to_string())?;
        let address = listener.local_addr().map_err(|e| e.to_string())?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| e.to_string())?;
        let (stop, stopped) = oneshot::channel();
        let worker = thread::Builder::new().name("vkore-inference-api".into()).spawn(move || {
            runtime.block_on(async move {
                let Ok(listener) = tokio::net::TcpListener::from_std(listener) else { return; };
                let app = router(backend);
                let slots = Arc::new(Semaphore::new(MAX_CONNECTIONS));
                let mut tasks = tokio::task::JoinSet::new();
                tokio::pin!(stopped);
                loop {
                    tokio::select! {
                        _ = &mut stopped => break,
                        _ = tasks.join_next(), if !tasks.is_empty() => {},
                        accepted = listener.accept() => {
                            let Ok((socket, _)) = accepted else { break; };
                            let Ok(permit) = slots.clone().try_acquire_owned() else { drop(socket); continue; };
                            let service = hyper_util::service::TowerToHyperService::new(app.clone());
                            tasks.spawn(async move {
                                let _permit = permit;
                                let mut http = hyper::server::conn::http1::Builder::new();
                                http.keep_alive(false).max_buf_size(16 * 1024)
                                    .timer(hyper_util::rt::TokioTimer::new())
                                    .header_read_timeout(Duration::from_secs(10));
                                let connection = http.serve_connection(hyper_util::rt::TokioIo::new(socket), service);
                                let _ = tokio::time::timeout(DEADLINE + Duration::from_secs(5), connection).await;
                            });
                        }
                    }
                }
                tasks.abort_all();
                while tasks.join_next().await.is_some() {}
            });
        }).map_err(|e| e.to_string())?;
        Ok(Self {
            address,
            stop: Some(stop),
            worker: Some(worker),
        })
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::{Body, to_bytes},
        http::Request,
    };
    use tower::ServiceExt;

    async fn request<B: Backend>(
        backend: B,
        path: &str,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let mut req = Request::builder()
            .uri(path)
            .header("host", "127.0.0.1:8080");
        let body = match body {
            Some(body) => {
                req = req
                    .method("POST")
                    .header("content-type", "application/json");
                Body::from(body.to_string())
            }
            None => Body::empty(),
        };
        let response = router(Arc::new(backend))
            .oneshot(req.body(body).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let body = to_bytes(response.into_body(), MAX_OUTPUT_BYTES)
            .await
            .unwrap();
        (status, serde_json::from_slice(&body).unwrap())
    }
    fn prompt() -> Value {
        json!({"model": "test", "messages": [{"role": "user", "content": "hello"}]})
    }

    #[tokio::test]
    async fn disconnected_backend_is_honest_and_errors_are_openai_shaped() {
        let (status, body) = request(Unavailable, "/v1/models", None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["data"], json!([]));
        let (status, body) = request(Unavailable, "/v1/chat/completions", Some(prompt())).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body["error"]["code"], "backend_unavailable");
        let mut invalid = prompt();
        invalid["tools"] = json!([]);
        assert_eq!(
            request(Unavailable, "/v1/chat/completions", Some(invalid))
                .await
                .0,
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }
    struct Fixture;
    impl Backend for Fixture {
        async fn models(&self) -> Result<Vec<Model>, ApiError> {
            Ok(vec![Model { id: "test".into() }])
        }
        async fn complete(&self, request: ChatRequest) -> Result<Completion, ApiError> {
            assert_eq!(request.messages[0].content, "hello");
            Ok(Completion {
                text: "world".into(),
                finish_reason: FinishReason::Stop,
                prompt_tokens: 1,
                completion_tokens: 1,
            })
        }
    }
    #[tokio::test]
    async fn completion_envelope_and_option_refusals() {
        let (status, body) = request(Fixture, "/v1/chat/completions", Some(prompt())).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["choices"][0]["message"]["content"], "world");
        assert_eq!(body["usage"]["total_tokens"], 2);
        for (key, value) in [
            ("stream", json!(true)),
            ("n", json!(2)),
            ("max_tokens", json!(0)),
            ("temperature", json!(3)),
        ] {
            let mut input = prompt();
            input[key] = value;
            assert_eq!(
                request(Fixture, "/v1/chat/completions", Some(input))
                    .await
                    .0,
                StatusCode::BAD_REQUEST
            );
        }
    }
    #[tokio::test]
    async fn request_bounds_and_browser_origin_are_enforced() {
        for (host, origin) in [
            ("attacker.example", None),
            ("127.0.0.1:8080", Some("https://attacker.example")),
        ] {
            let mut req = Request::builder().uri("/v1/models").header("host", host);
            if let Some(origin) = origin {
                req = req.header("origin", origin);
            }
            let response = router(Arc::new(Unavailable))
                .oneshot(req.body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::FORBIDDEN);
        }
        let req = Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header("host", "localhost:8080")
            .header("content-type", "application/json")
            .body(Body::from("x".repeat(MAX_REQUEST_BYTES + 1)))
            .unwrap();
        let response = router(Arc::new(Unavailable)).oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
        let body = to_bytes(response.into_body(), MAX_OUTPUT_BYTES)
            .await
            .unwrap();
        let body: Value = serde_json::from_slice(&body).unwrap();
        assert!(body["error"]["message"].is_string());
        assert_eq!(
            request(Unavailable, "/v1/missing", None).await.0,
            StatusCode::NOT_FOUND
        );
    }

    #[test]
    fn listener_is_local_and_stops_on_drop() {
        use std::io::{ErrorKind, Read};
        assert!(Server::start("0.0.0.0:0".parse().unwrap(), Arc::new(Unavailable)).is_err());
        let backend = Arc::new(Unavailable);
        let server = Server::start("127.0.0.1:0".parse().unwrap(), backend.clone()).unwrap();
        let address = server.address;
        let response = ureq::get(&format!("http://{address}/v1/models"))
            .call()
            .unwrap();
        assert_eq!(response.status(), 200);
        let mut connection = std::net::TcpStream::connect(address).unwrap();
        connection
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        drop(server);
        // Drop joins the worker and releases its router before returning.
        assert_eq!(Arc::strong_count(&backend), 1);
        // Check an existing connection rather than a port another test may reuse.
        match connection.read(&mut [0]) {
            Ok(0) => {}
            Err(error) if error.kind() == ErrorKind::ConnectionReset => {}
            other => panic!("the stopped server retained its connection: {other:?}"),
        }
    }
}
