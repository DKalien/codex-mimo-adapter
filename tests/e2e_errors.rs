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
#[tokio::test]
async fn test_e2e_stream_early_history_error_event_order() {
    let (upstream_addr, _mock, _received) = start_mock_upstream().await;
    let adapter = start_adapter(upstream_addr, None).await;

    // Trigger: previous_response_id not found in store + tool call outputs exist
    let resp = adapter
        .client
        .post(adapter_url(adapter.addr, "/v1/responses"))
        .json(&json!({
            "model": routed_model("opencode-go/deepseek-v4-flash"),
            "input": [{"type":"function_call_output","call_id":"call_missing","output":"ok"}],
            "stream": true,
            "previous_response_id": "resp_never_stored"
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200, "stream returns 200 even on early failure");
    let body_text = resp.text().await.unwrap();

    // Parse SSE events manually to check ordering including [DONE].
    let mut event_names: Vec<String> = Vec::new();
    for block in body_text.split("\n\n") {
        let mut event = String::new();
        let mut data = String::new();
        for line in block.lines() {
            if let Some(rest) = line.strip_prefix("event:") {
                event = rest.trim().to_string();
            } else if let Some(rest) = line.strip_prefix("data:") {
                data = rest.trim().to_string();
            }
        }
        if !data.is_empty() {
            if data == "[DONE]" {
                event_names.push("[DONE]".to_string());
            } else if !event.is_empty() {
                event_names.push(event);
            }
        }
    }

    assert_eq!(
        event_names,
        vec!["response.created".to_string(), "response.in_progress".to_string(), "response.failed".to_string(), "[DONE]".to_string()],
        "early stream failure must emit created -> in_progress -> failed -> [DONE]"
    );

    // Verify response.failed carries expected error fields.
    for block in body_text.split("\n\n") {
        let mut event = String::new();
        let mut data = String::new();
        for line in block.lines() {
            if let Some(rest) = line.strip_prefix("event:") {
                event = rest.trim().to_string();
            } else if let Some(rest) = line.strip_prefix("data:") {
                data = rest.trim().to_string();
            }
        }
        if event == "response.failed" {
            let payload: serde_json::Value = serde_json::from_str(&data).unwrap();
            let r = &payload["response"];
            assert_eq!(r["status"], "failed");
            assert_eq!(r["error"]["type"], "invalid_tool_history");
            assert_eq!(r["error"]["code"], "invalid_tool_history");
            assert!(r["error"]["message"].as_str().unwrap().contains("resp_never_stored"));
            assert!(r["id"].as_str().unwrap().starts_with("resp_"));
            break;
        }
    }
}
