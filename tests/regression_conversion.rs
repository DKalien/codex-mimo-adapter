use std::sync::{Arc, Mutex};

use codex_mimo_adapter::conversion::chat_to_responses::{build_response, response_shell};
use codex_mimo_adapter::conversion::responses_to_chat::{
    build_chat_payload, function_output_call_ids,
};
use codex_mimo_adapter::conversion::stream_chat_to_responses::StreamAssembler;
use codex_mimo_adapter::conversion::tool_context::ToolContext;
use serde_json::{json, Value};

#[test]
fn function_output_call_ids_reads_responses_call_id() {
    let body = json!({
        "model": "mimo/test-model",
        "input": [
            {"type": "function_call_output", "call_id": "call_123", "output": "ok"},
            {"type": "custom_tool_call_output", "call_id": "call_456", "output": "done"}
        ]
    });

    let ids = function_output_call_ids(&body).expect("extract call ids");
    assert_eq!(ids, vec!["call_123".to_string(), "call_456".to_string()]);
}

#[test]
fn build_chat_payload_converts_custom_and_tool_search_calls() {
    let body = json!({
        "model": "mimo/test-model",
        "tools": [
            {"type": "custom", "name": "shell", "description": "run shell"},
            {"type": "tool_search"}
        ],
        "input": [
            {"type": "custom_tool_call", "call_id": "call_custom", "name": "shell", "input": "ls -la"},
            {"type": "tool_search_call", "call_id": "call_search", "arguments": {"query": "gmail"}}
        ]
    });

    let (_payload, messages, _reverse, _context) =
        build_chat_payload(&body, "test-model", None, json!({})).expect("build payload");

    let tool_calls = messages
        .iter()
        .find(|message| message.get("role").and_then(Value::as_str) == Some("assistant"))
        .and_then(|message| message.get("tool_calls"))
        .and_then(Value::as_array)
        .expect("assistant tool calls");

    assert_eq!(tool_calls[0]["id"], "call_custom");
    assert_eq!(tool_calls[0]["function"]["name"], "shell");
    assert_eq!(
        tool_calls[0]["function"]["arguments"],
        r#"{"input":"ls -la"}"#
    );
    assert_eq!(tool_calls[1]["id"], "call_search");
    assert_eq!(tool_calls[1]["function"]["name"], "tool_search");
    assert_eq!(
        tool_calls[1]["function"]["arguments"],
        r#"{"query":"gmail"}"#
    );
}

#[test]
fn stream_truncated_with_output_can_finalize_as_incomplete() {
    let stored = Arc::new(Mutex::new(Vec::new()));
    let stored_for_put = stored.clone();
    let emitted = Arc::new(Mutex::new(Vec::<(String, Value)>::new()));
    let emitted_for_put = emitted.clone();

    let mut assembler = StreamAssembler::new(
        json!({"model": "mimo/test-model", "stream": true}),
        "mimo/test-model".to_string(),
        "test-model".to_string(),
        vec![],
        ToolContext::build(None),
        Box::new(move |item| {
            stored_for_put.lock().expect("stored lock").push(item);
            Ok(())
        }),
        Box::new(move |event, payload| {
            emitted_for_put
                .lock()
                .expect("emitted lock")
                .push((event.to_string(), payload));
            Ok(())
        }),
    );

    assembler.start().expect("start stream");
    assembler
        .accept(&json!({
            "choices": [{"delta": {"content": "hello"}}]
        }))
        .expect("accept content");
    assert!(assembler.has_substantive_output());
    assert!(!assembler.has_finish_reason());

    assembler.mark_truncated_as_length();
    let response = assembler.finalize().expect("finalize response");

    assert_eq!(response["status"], "incomplete");
    assert_eq!(
        response["incomplete_details"]["reason"],
        "max_output_tokens"
    );
    assert_eq!(
        emitted.lock().expect("emitted lock").last().unwrap().0,
        "response.incomplete"
    );
    assert_eq!(stored.lock().expect("stored lock").len(), 1);
}

#[test]
fn nonstream_usage_details_are_normalized_without_losing_extra_fields() {
    let response = build_response(
        &json!({"model": "mimo/test-model"}),
        &json!({
            "choices": [{"message": {"content": "ok"}, "finish_reason": "stop"}],
            "usage": {
                "prompt_tokens": 11,
                "completion_tokens": 7,
                "total_tokens": 18,
                "prompt_tokens_details": {
                    "cached_tokens": "invalid",
                    "audio_tokens": 3
                },
                "completion_tokens_details": {
                    "reasoning_tokens": null,
                    "accepted_prediction_tokens": 2
                }
            }
        }),
        "mimo/test-model",
        "test-model",
        &[],
        &ToolContext::build(None),
        |_| Ok(()),
    )
    .expect("build nonstream response");

    assert_eq!(
        response["usage"]["input_tokens_details"]["cached_tokens"],
        0
    );
    assert_eq!(response["usage"]["input_tokens_details"]["audio_tokens"], 3);
    assert_eq!(
        response["usage"]["output_tokens_details"]["reasoning_tokens"],
        0
    );
    assert_eq!(
        response["usage"]["output_tokens_details"]["accepted_prediction_tokens"],
        2
    );
}

#[test]
fn response_shell_defaults_both_usage_detail_objects() {
    let response = response_shell(
        &json!({}),
        "resp_test",
        1,
        "mimo/test-model",
        vec![],
        &json!({}),
        "completed",
        None,
    );

    assert_eq!(
        response["usage"]["input_tokens_details"],
        json!({"cached_tokens": 0})
    );
    assert_eq!(
        response["usage"]["output_tokens_details"],
        json!({"reasoning_tokens": 0})
    );
}

#[test]
fn stream_completed_usage_has_required_detail_fields() {
    let emitted = Arc::new(Mutex::new(Vec::<(String, Value)>::new()));
    let emitted_for_put = emitted.clone();
    let mut assembler = StreamAssembler::new(
        json!({"model": "mimo/test-model", "stream": true}),
        "mimo/test-model".to_string(),
        "test-model".to_string(),
        vec![],
        ToolContext::build(None),
        Box::new(|_| Ok(())),
        Box::new(move |event, payload| {
            emitted_for_put
                .lock()
                .expect("emitted lock")
                .push((event.to_string(), payload));
            Ok(())
        }),
    );

    assembler.start().expect("start stream");
    assembler
        .accept(&json!({
            "choices": [{"delta": {"content": "ok"}, "finish_reason": "stop"}],
            "usage": {
                "prompt_tokens": 4,
                "completion_tokens": 2,
                "prompt_tokens_details": {"cache_creation_tokens": 1},
                "completion_tokens_details": "malformed"
            }
        }))
        .expect("accept terminal chunk");
    let response = assembler.finalize().expect("finalize stream");

    assert_eq!(
        response["usage"]["input_tokens_details"]["cached_tokens"],
        0
    );
    assert_eq!(
        response["usage"]["input_tokens_details"]["cache_creation_tokens"],
        1
    );
    assert_eq!(
        response["usage"]["output_tokens_details"]["reasoning_tokens"],
        0
    );
    let completed = emitted
        .lock()
        .expect("emitted lock")
        .iter()
        .find(|(event, _)| event == "response.completed")
        .cloned()
        .expect("response.completed event");
    assert_eq!(
        completed.1["response"]["usage"], response["usage"],
        "terminal event and final response should share normalized usage"
    );
}
