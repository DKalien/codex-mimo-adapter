use crate::state::{now_ts, StoredResponse};
use serde_json::{json, Value};
use uuid::Uuid;

use super::responses_to_chat::repair_history;
use super::text::{arguments_text, as_text, canonicalize_json_string_if_parseable, reasoning_text};
use super::tool_context::ToolContext;

pub fn build_response<F>(
    body: &Value,
    chat_response: &Value,
    model_alias: &str,
    model_upstream: &str,
    base_messages: &[Value],
    context: &ToolContext,
    mut state_put: F,
) -> anyhow::Result<Value>
where
    F: FnMut(StoredResponse) -> anyhow::Result<()>,
{
    let choice = chat_response
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|a| a.first())
        .cloned()
        .unwrap_or_else(|| json!({}));
    let message = choice.get("message").cloned().unwrap_or_else(|| json!({}));
    let content = message.get("content").map(as_text).unwrap_or_default();
    let reasoning = reasoning_text(&message).unwrap_or_default();
    let response_id = format!("resp_{}", Uuid::new_v4().simple());
    let created_at = now_ts();

    let mut assistant = json!({"role":"assistant","content":content});
    if !reasoning.is_empty() {
        assistant["reasoning_content"] = Value::String(reasoning.clone());
    }
    if let Some(blocks) = message.get("thinking_blocks") {
        assistant["thinking_blocks"] = blocks.clone();
    }

    let mut output = Vec::new();
    if !reasoning.is_empty() {
        output.push(reasoning_item(&reasoning, None));
    }
    if !content.is_empty() {
        output.push(message_item(&content, None));
    }

    let mut pending = Vec::new();
    let mut replay_calls = Vec::new();
    if let Some(calls) = message.get("tool_calls").and_then(Value::as_array) {
        for call in calls {
            let function = call.get("function").unwrap_or(&Value::Null);
            let raw_name = function.get("name").or_else(|| call.get("name")).and_then(Value::as_str).unwrap_or("tool");
            let call_id = call.get("id").or_else(|| call.get("call_id")).and_then(Value::as_str).map(ToString::to_string).unwrap_or_else(|| format!("call_{}", Uuid::new_v4().simple()));
            let arguments = canonicalize_json_string_if_parseable(&arguments_text(function.get("arguments").or_else(|| call.get("arguments"))));
            replay_calls.push(json!({"id":call_id,"type":"function","function":{"name":raw_name,"arguments":arguments}}));
            pending.push(call_id.clone());

            let restored_name = context.restore_name(raw_name);
            let spec = context.lookup_spec(raw_name);
            let item_type = match spec.as_ref().map(|s| &s.kind) {
                Some(super::tool_context::ToolKind::Custom) => "custom_tool_call",
                Some(super::tool_context::ToolKind::ToolSearch) => "tool_search_call",
                _ => "function_call",
            };
            let mut item = json!({
                "type": item_type,
                "id": format!("fc_{}", Uuid::new_v4().simple()),
                "call_id": call_id,
                "name": restored_name,
                "arguments": arguments,
                "status": "completed"
            });
            // Attach namespace if present
            if let Some(ns) = spec.and_then(|s| s.namespace.as_deref()).filter(|n| !n.is_empty()) {
                item["namespace"] = Value::String(ns.to_string());
            }
            // Attach reasoning content to tool items
            if !reasoning.is_empty() {
                item["reasoning_content"] = Value::String(reasoning.clone());
            }
            output.push(item);
        }
    }
    if !replay_calls.is_empty() {
        assistant["tool_calls"] = Value::Array(replay_calls);
    }

    let mut stored_messages = repair_history(base_messages, None)?;
    stored_messages.push(assistant);
    state_put(StoredResponse {
        response_id: response_id.clone(),
        model_alias: model_alias.to_string(),
        model_upstream: model_upstream.to_string(),
        messages: stored_messages,
        pending_call_ids: pending.clone(),
        output: output.clone(),
        created_at,
        previous_response_id: body.get("previous_response_id").and_then(Value::as_str).unwrap_or("").to_string(),
    })?;

    let usage = chat_response.get("usage").cloned().unwrap_or_else(|| json!({}));
    let (status, incomplete) = completion_status(&content, &pending, choice.get("finish_reason"));
    Ok(response_shell(body, &response_id, created_at, model_alias, output, &usage, status, incomplete))
}

pub fn reasoning_item(text: &str, item_id: Option<String>) -> Value {
    json!({
        "type":"reasoning",
        "id": item_id.unwrap_or_else(|| format!("rs_{}", Uuid::new_v4().simple())),
        "summary": if text.is_empty() { json!([]) } else { json!([{"type":"summary_text","text":text}]) }
    })
}

pub fn message_item(content: &str, item_id: Option<String>) -> Value {
    json!({
        "type":"message",
        "id": item_id.unwrap_or_else(|| format!("msg_{}", Uuid::new_v4().simple())),
        "status":"completed",
        "role":"assistant",
        "content":[{"type":"output_text","text":content,"annotations":[]}]
    })
}

pub fn completion_status(_content: &str, _pending: &[String], finish_reason: Option<&Value>) -> (&'static str, Option<Value>) {
    match finish_reason.and_then(Value::as_str).unwrap_or("") {
        "length" | "max_tokens" => ("incomplete", Some(json!({"reason":"max_output_tokens"}))),
        "content_filter" | "safety" => ("incomplete", Some(json!({"reason":"content_filter"}))),
        _ => ("completed", None),
    }
}

pub fn response_shell(
    body: &Value,
    response_id: &str,
    created_at: i64,
    model: &str,
    output: Vec<Value>,
    usage: &Value,
    status: &str,
    incomplete_details: Option<Value>,
) -> Value {
    let input_tokens = usage.get("input_tokens").or_else(|| usage.get("prompt_tokens")).and_then(Value::as_i64).unwrap_or(0);
    let output_tokens = usage.get("output_tokens").or_else(|| usage.get("completion_tokens")).and_then(Value::as_i64).unwrap_or(0);
    let total_tokens = usage.get("total_tokens").and_then(Value::as_i64).unwrap_or(input_tokens + output_tokens);
    let mut response_usage = json!({"input_tokens":input_tokens,"output_tokens":output_tokens,"total_tokens":total_tokens});
    if let Some(details) = usage.get("input_tokens_details").or_else(|| usage.get("prompt_tokens_details")) {
        response_usage["input_tokens_details"] = details.clone();
    }
    if let Some(details) = usage.get("output_tokens_details").or_else(|| usage.get("completion_tokens_details")) {
        response_usage["output_tokens_details"] = details.clone();
    }
    json!({
        "id": response_id,
        "object":"response",
        "created_at": created_at,
        "status": status,
        "error": null,
        "incomplete_details": incomplete_details,
        "instructions": body.get("instructions").cloned().unwrap_or(Value::Null),
        "model": model,
        "output": output,
        "parallel_tool_calls": body.get("parallel_tool_calls").and_then(Value::as_bool).unwrap_or(false),
        "previous_response_id": body.get("previous_response_id").cloned().unwrap_or(Value::Null),
        "store": false,
        "usage": response_usage,
        "metadata": body.get("metadata").cloned().unwrap_or_else(|| json!({}))
    })
}
