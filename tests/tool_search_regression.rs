use codex_opencode_adapter::conversion::responses_to_chat::build_chat_payload;
use serde_json::json;

#[test]
fn tool_search_uses_query_limit_schema() {
    let body = json!({
        "model": "opencode-go/deepseek-v4-pro",
        "input": "Find a tool",
        "tools": [{"type":"tool_search"}],
        "stream": true
    });
    let (payload, _messages, _reverse, tool_ctx) = build_chat_payload(&body, "deepseek-v4-pro", None, json!({})).unwrap();
    let tool = &payload["tools"][0]["function"];
    assert_eq!(tool["name"], "tool_search");
    assert_eq!(tool["parameters"]["properties"]["query"]["type"], "string");
    assert_eq!(tool["parameters"]["properties"]["limit"]["type"], "integer");
    assert_eq!(tool["parameters"]["required"], json!(["query"]));
    assert!(tool["parameters"]["properties"].get("input").is_none());
    assert!(tool_ctx.is_tool_search_chat_name("tool_search"));
    assert!(!tool_ctx.is_custom_tool_chat_name("tool_search"));
}
