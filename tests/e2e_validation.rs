mod common;

use codex_opencode_adapter::config::Config;
use codex_opencode_adapter::project::sign_adapter_token;
use codex_opencode_adapter::server::{self, AppState, ProjectRuntime};
use codex_opencode_adapter::state::StateStore;
use codex_opencode_adapter::upstream::OpenCodeGoClient;
use common::mock_upstream::start_mock_upstream;
use common::{adapter_url, routed_model, start_adapter};
use serde_json::json;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use tokio::net::TcpListener;
use tokio::sync::Semaphore;

#[tokio::test]
async fn test_e2e_request_payload_shape() {
    let (upstream_addr, _mock, received) = start_mock_upstream().await;
    let adapter = start_adapter(upstream_addr, None).await;

    let _ = adapter
        .client
        .post(adapter_url(adapter.addr, "/v1/responses"))
        .json(&json!({
            "model": routed_model("opencode-go/deepseek-v4-flash"),
            "instructions": "You are a helpful assistant.",
            "input": [{"type": "message", "role": "user", "content": "Hi"}],
            "stream": false
        }))
        .send()
        .await
        .unwrap();

    // Give a moment for the mock to record the payload.
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let payloads = received.lock().await;
    assert_eq!(
        payloads.len(),
        1,
        "mock should have received exactly one payload"
    );

    let payload = &payloads[0];

    // Model prefix should be stripped.
    assert_eq!(
        payload["model"].as_str().unwrap(),
        "deepseek-v4-flash",
        "model prefix should be stripped"
    );

    // Should have messages array.
    let messages = payload["messages"].as_array().unwrap();
    assert!(!messages.is_empty(), "messages should not be empty");

    // First message should include the system instruction.
    let first_content = messages[0]["content"].as_str().unwrap();
    assert!(
        first_content.contains("You are a helpful assistant"),
        "system instruction should be in messages"
    );

    // stream should be false (adapter forces it).
    assert_eq!(
        payload["stream"].as_bool().unwrap(),
        false,
        "stream should be false for non-streaming"
    );
}

#[tokio::test]
async fn test_e2e_request_payload_streaming_shape() {
    let (upstream_addr, _mock, received) = start_mock_upstream().await;
    let adapter = start_adapter(upstream_addr, None).await;

    let _ = adapter
        .client
        .post(adapter_url(adapter.addr, "/v1/responses"))
        .json(&json!({
            "model": routed_model("opencode-go/deepseek-v4-flash"),
            "input": "Hi",
            "stream": true
        }))
        .send()
        .await
        .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let payloads = received.lock().await;
    assert_eq!(payloads.len(), 1);

    let payload = &payloads[0];

    // stream should be true.
    assert_eq!(payload["stream"].as_bool().unwrap(), true);

    // stream_options should include_usage.
    assert_eq!(
        payload["stream_options"]["include_usage"]
            .as_bool()
            .unwrap(),
        true,
        "stream_options.include_usage should be true"
    );
}

#[tokio::test]
async fn test_e2e_auth_required() {
    let (upstream_addr, _mock, _received) = start_mock_upstream().await;
    let adapter = start_adapter(upstream_addr, Some("my-secret-token".to_string())).await;

    // Without auth → 401 (use bare client, no auth headers).
    let unauth_client = reqwest::Client::new();
    let resp = unauth_client
        .post(adapter_url(adapter.addr, "/v1/responses"))
        .json(&json!({
            "model": routed_model("opencode-go/deepseek-v4-flash"),
            "input": "Hello",
            "stream": false
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401, "should return 401 without auth");

    // With signed token → 200 (adapter.client has pre-configured Bearer auth).
    let resp = adapter
        .client
        .post(adapter_url(adapter.addr, "/v1/responses"))
        .json(&json!({
            "model": routed_model("opencode-go/deepseek-v4-flash"),
            "input": "Hello",
            "stream": false
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "should return 200 with correct auth");
}

#[tokio::test]
async fn test_e2e_missing_model_prefix() {
    let (upstream_addr, _mock, _received) = start_mock_upstream().await;
    let adapter = start_adapter(upstream_addr, None).await;

    // Routed model without opencode-go/ real-model prefix should be rejected.
    let resp = adapter
        .client
        .post(adapter_url(adapter.addr, "/v1/responses"))
        .json(&json!({
            "model": "opencode_adapter/test_project/deepseek-v4-flash",
            "input": "Hello",
            "stream": false
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400, "model without prefix should return 400");
}

#[tokio::test]
async fn test_e2e_legacy_model_format_rejected() {
    let (upstream_addr, _mock, _received) = start_mock_upstream().await;
    let adapter = start_adapter(upstream_addr, None).await;

    let resp = adapter
        .client
        .post(adapter_url(adapter.addr, "/v1/responses"))
        .json(&json!({
            "model": "opencode-go/deepseek-v4-flash",
            "input": "Hello",
            "stream": false
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 400, "legacy model format should return 400");
}

#[tokio::test]
async fn dual_project_http_isolation() {
    use axum::routing::{get, post};
    use axum::Router;
    use std::sync::Mutex;

    let auth_a: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let auth_b: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));

    // --- Mock upstream A: returns model-a, records auth header ---
    let mock_a_state = MockState {
        model_id: "model-a".to_string(),
        auth_recorder: auth_a.clone(),
    };
    let app_a = Router::new()
        .route("/models", get(mock_models_handler))
        .route("/chat/completions", post(mock_chat_handler))
        .with_state(mock_a_state);
    let listener_a = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr_a = listener_a.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener_a, app_a).await.unwrap();
    });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // --- Mock upstream B: returns model-b, records auth header ---
    let mock_b_state = MockState {
        model_id: "model-b".to_string(),
        auth_recorder: auth_b.clone(),
    };
    let app_b = Router::new()
        .route("/models", get(mock_models_handler))
        .route("/chat/completions", post(mock_chat_handler))
        .with_state(mock_b_state);
    let listener_b = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr_b = listener_b.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener_b, app_b).await.unwrap();
    });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let temp_dir = std::env::temp_dir();
    let db_a = temp_dir.join(format!("dual_http_a_{}.sqlite", uuid::Uuid::new_v4()));
    let db_b = temp_dir.join(format!("dual_http_b_{}.sqlite", uuid::Uuid::new_v4()));

    let pid_a = "test_a".to_string();
    let pid_b = "test_b".to_string();
    let raw_a = "local-token-a".to_string();
    let raw_b = "local-token-b".to_string();

    let config_a = Config {
        host: "127.0.0.1".to_string(),
        port: 0,
        upstream_base: format!("http://{addr_a}"),
        upstream_key: "key-a".to_string(),
        local_token: Some(raw_a.clone()),
        state_db: db_a.to_string_lossy().to_string(),
        state_ttl_seconds: 21600,
        timeout_seconds: 30,
        max_request_bytes: 8 * 1024 * 1024,
        max_concurrency: 10,
    };
    let config_b = Config {
        host: "127.0.0.1".to_string(),
        port: 0,
        upstream_base: format!("http://{addr_b}"),
        upstream_key: "key-b".to_string(),
        local_token: Some(raw_b.clone()),
        state_db: db_b.to_string_lossy().to_string(),
        state_ttl_seconds: 21600,
        timeout_seconds: 30,
        max_request_bytes: 8 * 1024 * 1024,
        max_concurrency: 10,
    };

    let client_a = OpenCodeGoClient::new(
        &config_a.upstream_base,
        &config_a.upstream_key,
        config_a.timeout_seconds,
    )
    .unwrap();
    let client_b = OpenCodeGoClient::new(
        &config_b.upstream_base,
        &config_b.upstream_key,
        config_b.timeout_seconds,
    )
    .unwrap();
    let state_store_a = StateStore::new(&config_a.state_db, config_a.state_ttl_seconds).unwrap();
    let state_store_b = StateStore::new(&config_b.state_db, config_b.state_ttl_seconds).unwrap();

    let mut projects = HashMap::new();
    projects.insert(
        pid_a.clone(),
        ProjectRuntime {
            config: config_a,
            client: client_a,
            state: state_store_a.clone(),
        },
    );
    projects.insert(
        pid_b.clone(),
        ProjectRuntime {
            config: config_b,
            client: client_b,
            state: state_store_b.clone(),
        },
    );
    let app_state = AppState {
        projects: Arc::new(RwLock::new(projects)),
        capacity: Arc::new(Semaphore::new(10)),
        config_overrides: Default::default(),
    };
    let app = server::router(app_state);

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let client = reqwest::Client::new();

    // --- 1. Adapter token -> /v1/models -> sees the aggregated routed model list ---
    let signed_a = sign_adapter_token(&raw_a);
    let resp_a = client
        .get(format!("http://{addr}/v1/models"))
        .bearer_auth(&signed_a)
        .send()
        .await
        .unwrap();
    assert_eq!(resp_a.status(), 200, "adapter token should succeed");
    let body_a: serde_json::Value = resp_a.json().await.unwrap();
    let models_a = body_a["data"].as_array().expect("A should see model list");
    assert!(!models_a.is_empty(), "A should have at least one model");
    assert!(
        models_a
            .iter()
            .any(|m| m["id"] == "opencode_adapter/test_a/opencode-go/model-a"),
        "models should include test_a model; got {:?}",
        models_a
    );
    assert!(
        models_a
            .iter()
            .any(|m| m["id"] == "opencode_adapter/test_b/opencode-go/model-b"),
        "adapter-level token should see all routed models; got {:?}",
        models_a
    );
    let model_a = models_a[0]["id"].as_str().expect("A model id").to_string();
    let resp_a_infer = client
        .post(format!("http://{addr}/v1/responses"))
        .bearer_auth(&signed_a)
        .json(&serde_json::json!({
            "model": model_a,
            "input": "Hello",
            "stream": false
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp_a_infer.status(),
        200,
        "token A should be able to use the routed model returned by /v1/models"
    );

    // --- 2. Another valid adapter token sees the same aggregated routed model list ---
    let signed_b = sign_adapter_token(&raw_b);
    let resp_b = client
        .get(format!("http://{addr}/v1/models"))
        .bearer_auth(&signed_b)
        .send()
        .await
        .unwrap();
    assert_eq!(resp_b.status(), 200, "adapter token should succeed");
    let body_b: serde_json::Value = resp_b.json().await.unwrap();
    let models_b = body_b["data"].as_array().expect("B should see model list");
    assert!(!models_b.is_empty(), "B should have at least one model");
    assert!(
        models_b
            .iter()
            .any(|m| m["id"] == "opencode_adapter/test_b/opencode-go/model-b"),
        "models should include test_b model; got {:?}",
        models_b
    );
    assert!(
        models_b
            .iter()
            .any(|m| m["id"] == "opencode_adapter/test_a/opencode-go/model-a"),
        "adapter-level token should see all routed models; got {:?}",
        models_b
    );

    // --- 3. Auth header tracking: each upstream received its own key ---
    {
        let recorded_a = auth_a.lock().unwrap();
        assert!(
            !recorded_a.is_empty(),
            "upstream A should have received at least one request"
        );
        assert!(
            recorded_a.iter().any(|h| h.contains("key-a")),
            "upstream A Authorization should contain key-a; got {:?}",
            *recorded_a,
        );
    }
    {
        let recorded_b = auth_b.lock().unwrap();
        assert!(
            !recorded_b.is_empty(),
            "upstream B should have received at least one request"
        );
        assert!(
            recorded_b.iter().any(|h| h.contains("key-b")),
            "upstream B Authorization should contain key-b; got {:?}",
            *recorded_b,
        );
    }

    // --- 4. State isolation: put into A, verify B cannot read it ---
    use codex_opencode_adapter::state::now_ts;
    let stored = codex_opencode_adapter::state::StoredResponse {
        response_id: "isolated-response-001".to_string(),
        model_alias: "opencode-go/test".to_string(),
        model_upstream: "test".to_string(),
        messages: vec![],
        pending_call_ids: vec![],
        output: vec![],
        created_at: now_ts(),
        previous_response_id: String::new(),
    };
    state_store_a.put(&stored).unwrap();
    let from_b = state_store_b.get("isolated-response-001").unwrap();
    assert!(from_b.is_none(), "state B should NOT contain A data");

    // --- 5. Cross-project /v1/responses: adapter-level token routes by model project key ---
    let resp_cross = client
        .post(format!("http://{addr}/v1/responses"))
        .bearer_auth(&signed_a)
        .json(&serde_json::json!({
            "model": "opencode_adapter/test_b/opencode-go/model-b",
            "input": "Hello",
            "stream": false
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp_cross.status(),
        200,
        "adapter-level token should be accepted; model route selects project B"
    );
    // Cleanup DB files.
    let _ = std::fs::remove_file(&db_a);
    let _ = std::fs::remove_file(&db_b);
}

#[derive(Clone)]
struct MockState {
    model_id: String,
    auth_recorder: Arc<std::sync::Mutex<Vec<String>>>,
}

async fn mock_models_handler(
    axum::extract::State(state): axum::extract::State<MockState>,
    headers: axum::http::HeaderMap,
) -> axum::response::Json<serde_json::Value> {
    if let Some(auth) = headers.get("authorization").and_then(|v| v.to_str().ok()) {
        state.auth_recorder.lock().unwrap().push(auth.to_string());
    }
    axum::response::Json(serde_json::json!({"data": [{"id": state.model_id}]}))
}

async fn mock_chat_handler(
    axum::extract::State(state): axum::extract::State<MockState>,
    headers: axum::http::HeaderMap,
    axum::Json(payload): axum::Json<serde_json::Value>,
) -> axum::response::Json<serde_json::Value> {
    if let Some(auth) = headers.get("authorization").and_then(|v| v.to_str().ok()) {
        state.auth_recorder.lock().unwrap().push(auth.to_string());
    }
    let model = payload
        .get("model")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown");
    axum::response::Json(serde_json::json!({
        "id": "chatcmpl-dual-project",
        "object": "chat.completion",
        "model": model,
        "choices": [{
            "index": 0,
            "message": {"role": "assistant", "content": "ok"},
            "finish_reason": "stop"
        }],
        "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
    }))
}
