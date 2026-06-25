use axum::extract::State as AxumState;
use axum::routing::{get, post};
use axum::{Json, Router};
use codex_opencode_adapter::config::Config;
use codex_opencode_adapter::server::{self, AppState};
use codex_opencode_adapter::state::StateStore;
use codex_opencode_adapter::upstream::OpenCodeGoClient;
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::{Mutex, Notify, Semaphore};

#[derive(Clone)]
struct BlockingUpstreamState {
    started: Arc<Mutex<usize>>,
    release: Arc<Notify>,
}

#[tokio::test]
async fn max_concurrency_two_allows_two_overlapping_nonstream_requests() {
    let harness = BlockingUpstreamHarness::start().await;
    let adapter_addr = start_adapter(harness.addr, 2).await;
    let client = reqwest::Client::new();

    let first = tokio::spawn(post_nonstream_request(client.clone(), adapter_addr, "first"));
    let second = tokio::spawn(post_nonstream_request(client.clone(), adapter_addr, "second"));

    harness.wait_until_started(2).await;
    harness.release.notify_waiters();

    let first = first.await.unwrap();
    let second = second.await.unwrap();

    assert_eq!(first.status(), 200);
    assert_eq!(second.status(), 200);
    assert_eq!(harness.started_count().await, 2);
}

#[tokio::test]
async fn max_concurrency_one_rejects_second_overlapping_nonstream_request() {
    let harness = BlockingUpstreamHarness::start().await;
    let adapter_addr = start_adapter(harness.addr, 1).await;
    let client = reqwest::Client::new();

    let first = tokio::spawn(post_nonstream_request(client.clone(), adapter_addr, "first"));
    harness.wait_until_started(1).await;

    let second = post_nonstream_request(client.clone(), adapter_addr, "second").await;
    assert_eq!(second.status(), 429);
    let body: Value = second.json().await.unwrap();
    assert_eq!(body["error"]["message"], "adapter concurrency limit reached");

    harness.release.notify_waiters();
    let first = first.await.unwrap();
    assert_eq!(first.status(), 200);
    assert_eq!(harness.started_count().await, 1);
}

async fn post_nonstream_request(
    client: reqwest::Client,
    adapter_addr: SocketAddr,
    marker: &str,
) -> reqwest::Response {
    client
        .post(format!("http://{adapter_addr}/v1/responses"))
        .json(&json!({
            "model": "opencode-go/deepseek-v4-flash",
            "input": marker,
            "stream": false,
        }))
        .send()
        .await
        .unwrap()
}

struct BlockingUpstreamHarness {
    addr: SocketAddr,
    started: Arc<Mutex<usize>>,
    release: Arc<Notify>,
}

impl BlockingUpstreamHarness {
    async fn start() -> Self {
        let started = Arc::new(Mutex::new(0usize));
        let release = Arc::new(Notify::new());
        let state = BlockingUpstreamState {
            started: Arc::clone(&started),
            release: Arc::clone(&release),
        };
        let app = Router::new()
            .route("/chat/completions", post(blocking_chat))
            .route("/models", get(mock_models))
            .with_state(state);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        Self {
            addr,
            started,
            release,
        }
    }

    async fn wait_until_started(&self, expected: usize) {
        for _ in 0..100 {
            if self.started_count().await >= expected {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        panic!("timed out waiting for {expected} upstream request(s) to start");
    }

    async fn started_count(&self) -> usize {
        *self.started.lock().await
    }
}

async fn blocking_chat(
    AxumState(state): AxumState<BlockingUpstreamState>,
    Json(payload): Json<Value>,
) -> Json<Value> {
    {
        let mut started = state.started.lock().await;
        *started += 1;
    }
    state.release.notified().await;

    let model = payload
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    Json(json!({
        "id": "chatcmpl-blocking",
        "object": "chat.completion",
        "model": model,
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "content": "ok"
            },
            "finish_reason": "stop"
        }],
        "usage": {
            "prompt_tokens": 1,
            "completion_tokens": 1,
            "total_tokens": 2
        }
    }))
}

async fn mock_models() -> Json<Value> {
    Json(json!({
        "object": "list",
        "data": [
            {"id": "deepseek-v4-flash", "object": "model", "owned_by": "opencode-go"}
        ]
    }))
}

async fn start_adapter(upstream_addr: SocketAddr, max_concurrency: usize) -> SocketAddr {
    let db_path = std::env::temp_dir().join(format!(
        "concurrency_limit_regression_{}.sqlite",
        uuid::Uuid::new_v4()
    ));
    let config = Config {
        host: "127.0.0.1".to_string(),
        port: 0,
        upstream_base: format!("http://{upstream_addr}"),
        upstream_key: "test-api-key".to_string(),
        local_token: None,
        state_db: db_path.to_string_lossy().to_string(),
        state_ttl_seconds: 21_600,
        timeout_seconds: 30,
        max_request_bytes: 8 * 1024 * 1024,
        max_concurrency,
    };
    let client = OpenCodeGoClient::new(
        &config.upstream_base,
        &config.upstream_key,
        config.timeout_seconds,
    )
    .unwrap();
    let state = StateStore::new(&config.state_db, config.state_ttl_seconds).unwrap();
    let app_state = AppState {
        config,
        client,
        state,
        capacity: Arc::new(Semaphore::new(max_concurrency)),
    };
    let app = server::router(app_state);

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    addr
}
