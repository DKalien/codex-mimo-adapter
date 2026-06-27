mod common;

use common::mock_upstream::{
    start_mock_upstream, start_mock_upstream_error, start_mock_upstream_stream_error,
};
use common::{
    adapter_url, parse_sse_events, routed_model, start_adapter, start_multi_project_adapter,
    ProjectConfig,
};
use serde_json::{json, Value};

#[tokio::test]
async fn test_e2e_upstream_http_error() {
    let (upstream_addr, _mock, _received) = start_mock_upstream_error().await;
    let adapter = start_adapter(upstream_addr, None).await;

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

    assert_eq!(
        resp.status(),
        500,
        "upstream HTTP error status should be preserved"
    );
    let body: Value = resp.json().await.unwrap();
    assert!(
        body["error"]["type"].as_str().unwrap() == "upstream_error",
        "error type should be upstream_error"
    );
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("upstream internal error"),
        "error message should contain upstream message"
    );
}

#[tokio::test]
async fn test_e2e_upstream_stream_error() {
    let (upstream_addr, _mock, _received) = start_mock_upstream_stream_error().await;
    let adapter = start_adapter(upstream_addr, None).await;

    let resp = adapter
        .client
        .post(adapter_url(adapter.addr, "/v1/responses"))
        .json(&json!({
            "model": routed_model("opencode-go/deepseek-v4-flash"),
            "input": "Hello",
            "stream": true
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        200,
        "streaming errors come through the SSE stream, not HTTP status"
    );
    let body_text = resp.text().await.unwrap();
    let events = parse_sse_events(&body_text);

    // Should have a response.failed event.
    let failed = events.iter().find(|(name, _)| name == "response.failed");
    assert!(
        failed.is_some(),
        "should have response.failed event for stream error"
    );
    let failed_payload = &failed.unwrap().1["response"];
    assert_eq!(failed_payload["status"], "failed");
    assert_eq!(failed_payload["error"]["type"], "upstream_error");
    assert_eq!(failed_payload["error"]["code"], "upstream_error");
}

#[tokio::test]
async fn test_e2e_stream_rate_limit_error() {
    let (upstream_addr, _mock, _received) = start_mock_upstream().await;

    // Start adapter with max_concurrency = 0 so try_acquire_owned always fails.
    let configs = vec![ProjectConfig {
        project_id: "test-proj".to_string(),
        upstream_addr,
        upstream_key: "test-key".to_string(),
        raw_token: Some("raw-token".to_string()),
    }];
    let (addr, tokens) = start_multi_project_adapter(configs, 0).await;
    let token = tokens.get("test-proj").expect("token should exist");

    let client = reqwest::Client::new();
    let resp = client
        .post(adapter_url(addr, "/v1/responses"))
        .bearer_auth(token)
        .json(&json!({
            "model": "opencode_adapter/test-proj/opencode-go/test-model",
            "input": "Hello",
            "stream": true
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        200,
        "stream returns 200 even with rate limit"
    );
    let body_text = resp.text().await.unwrap();
    let events = parse_sse_events(&body_text);

    // Should have response.failed with rate_limit_error.
    let failed = events.iter().find(|(name, _)| name == "response.failed");
    assert!(
        failed.is_some(),
        "should have response.failed event for rate limit; events: {events:?}"
    );
    let failed_payload = &failed.unwrap().1["response"];
    assert_eq!(failed_payload["status"], "failed");
    assert_eq!(failed_payload["error"]["type"], "rate_limit_error");
    assert_eq!(failed_payload["error"]["code"], "rate_limit_error");
    assert_eq!(
        failed_payload["error"]["message"].as_str().unwrap(),
        "adapter concurrency limit reached"
    );
}
