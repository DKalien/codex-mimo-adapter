use codex_opencode_adapter::conversion::responses_to_chat::build_chat_payload;
use codex_opencode_adapter::conversion::tool_context::ToolContext;
use serde_json::json;

#[test]
fn rust_tool_context_handles_namespace_and_custom() {
    let tools = json!([
        {
            "type": "namespace",
            "name": "mcp",
            "tools": [
                {
                    "type": "function",
                    "name": "read.file",
                    "description": "Read file",
                    "parameters": {"type":"object","properties":{}}
                }
            ]
        },
        {"type":"custom","name":"shell.exec"}
    ]);
    let context = ToolContext::build(Some(&tools));
    let names = context
        .chat_tools
        .iter()
        .map(|tool| tool["function"]["name"].as_str().unwrap().to_string())
        .collect::<Vec<_>>();
    assert_eq!(names, vec!["mcp__read_file", "shell_exec"]);
    assert_eq!(context.restore_name("mcp__read_file"), "mcp__read.file");
    assert!(context.is_custom_tool_chat_name("shell_exec"));
}

#[test]
fn rust_request_transform_maps_tools_and_tool_choice() {
    let body = json!({
        "model": "opencode-go/deepseek-v4-pro",
        "instructions": "System.",
        "input": [{"type":"message","role":"developer","content":"Dev."}, "Hi"],
        "tools": [{"type":"function","name":"mcp.read","parameters":{"type":"object"}}],
        "tool_choice": {"type":"function","name":"mcp.read"},
        "stream": true
    });
    let (payload, messages, reverse) = build_chat_payload(&body, "deepseek-v4-pro", None, json!({})).unwrap();
    assert_eq!(messages[0], json!({"role":"system","content":"System.\n\nDev."}));
    assert_eq!(payload["stream_options"], json!({"include_usage": true}));
    assert_eq!(payload["tool_choice"], json!({"type":"function","function":{"name":"mcp_read"}}));
    assert_eq!(reverse.get("mcp_read").unwrap(), "mcp.read");
}
