from codex_opencode_adapter.protocol import (
    StreamAssembler,
    build_chat_payload,
    build_response,
    convert_tools,
)
from codex_opencode_adapter.state import StoredResponse
from codex_opencode_adapter.state import StateStore


def test_request_and_tool_conversion():
    payload, messages, reverse = build_chat_payload(
        {
            "model": "opencode-go/deepseek-v4-pro",
            "instructions": "Be precise.",
            "input": "Inspect the file.",
            "tools": [
                {
                    "type": "function",
                    "name": "mcp__files.read",
                    "description": "Read a file",
                    "parameters": {
                        "type": "object",
                        "properties": {"path": {"type": "string"}},
                    },
                }
            ],
            "reasoning": {"effort": "high"},
        },
        model_upstream="deepseek-v4-pro",
        previous=None,
        reasoning_parameter={"reasoning_effort": "high"},
    )
    assert messages == [
        {"role": "system", "content": "Be precise."},
        {"role": "user", "content": "Inspect the file."},
    ]
    assert payload["reasoning_effort"] == "high"
    assert payload["tools"][0]["function"]["name"] == "mcp__files_read"
    assert reverse == {"mcp__files_read": "mcp__files.read"}


def test_developer_input_role_is_normalized_to_system():
    payload, messages, _ = build_chat_payload(
        {
            "model": "opencode-go/deepseek-v4-flash",
            "input": [
                {
                    "type": "message",
                    "role": "developer",
                    "content": "Stay concise.",
                },
                {"type": "message", "role": "user", "content": "Reply."},
            ],
        },
        model_upstream="deepseek-v4-flash",
        previous=None,
        reasoning_parameter={},
    )
    assert messages == [
        {"role": "system", "content": "Stay concise."},
        {"role": "user", "content": "Reply."},
    ]
    assert payload["messages"] == messages


def test_developer_role_in_stored_history_is_normalized_to_system():
    previous = StoredResponse(
        response_id="resp_old",
        model_alias="opencode-go/deepseek-v4-flash",
        model_upstream="deepseek-v4-flash",
        messages=[{"role": "developer", "content": "Old instructions."}],
        pending_call_ids=[],
        output=[],
        created_at=1,
        previous_response_id="",
    )
    payload, messages, _ = build_chat_payload(
        {
            "model": "opencode-go/deepseek-v4-flash",
            "previous_response_id": "resp_old",
            "input": "Continue.",
        },
        model_upstream="deepseek-v4-flash",
        previous=previous,
        reasoning_parameter={},
    )
    assert messages == [
        {"role": "system", "content": "Old instructions."},
        {"role": "user", "content": "Continue."},
    ]
    assert payload["messages"] == messages


def test_nonstream_tool_call_and_continuation():
    stored = []
    body = {
        "model": "opencode-go/deepseek-v4-pro",
        "input": "Read x.py",
        "tools": [
            {
                "type": "function",
                "name": "read",
                "parameters": {"type": "object", "properties": {}},
            }
        ],
    }
    result = build_response(
        body,
        {
            "choices": [
                {
                    "finish_reason": "tool_calls",
                    "message": {
                        "content": "",
                        "reasoning_content": "hidden",
                        "tool_calls": [
                            {
                                "id": "call_1",
                                "function": {
                                    "name": "read",
                                    "arguments": '{"path":"x.py"}',
                                },
                            }
                        ],
                    },
                }
            ],
            "usage": {"prompt_tokens": 4, "completion_tokens": 5, "total_tokens": 9},
        },
        model_alias=body["model"],
        model_upstream="deepseek-v4-pro",
        base_messages=[{"role": "user", "content": "Read x.py"}],
        reverse_names={"read": "read"},
        state_put=stored.append,
    )
    assert [item["type"] for item in result["output"]] == [
        "reasoning",
        "function_call",
    ]
    assert "hidden" not in str(result)
    previous = stored[0]

    payload, messages, _ = build_chat_payload(
        {
            "model": body["model"],
            "previous_response_id": result["id"],
            "input": [
                {
                    "type": "function_call_output",
                    "call_id": "call_1",
                    "output": "print('ok')",
                }
            ],
        },
        model_upstream="deepseek-v4-pro",
        previous=previous,
        reasoning_parameter={},
    )
    assert payload["messages"][-1] == {
        "role": "tool",
        "tool_call_id": "call_1",
        "content": "print('ok')",
    }
    assert any(message.get("reasoning_content") == "hidden" for message in messages)


def test_reasoning_budget_exhaustion_is_incomplete():
    stored = []
    response = build_response(
        {"model": "opencode-go/deepseek-v4-flash", "input": "2+2"},
        {
            "choices": [
                {
                    "finish_reason": "length",
                    "message": {"content": "", "reasoning_content": "hidden"},
                }
            ],
            "usage": {"completion_tokens": 32},
        },
        model_alias="opencode-go/deepseek-v4-flash",
        model_upstream="deepseek-v4-flash",
        base_messages=[{"role": "user", "content": "2+2"}],
        reverse_names={},
        state_put=stored.append,
    )
    assert response["status"] == "incomplete"
    assert response["incomplete_details"]["reason"] == "max_output_tokens"


def test_stream_text_and_tool_arguments():
    events = []
    stored = []
    assembler = StreamAssembler(
        body={"model": "opencode-go/deepseek-v4-pro", "input": "read"},
        model_alias="opencode-go/deepseek-v4-pro",
        model_upstream="deepseek-v4-pro",
        base_messages=[{"role": "user", "content": "read"}],
        reverse_names={"safe_read": "mcp.read"},
        state_put=stored.append,
        emit=lambda event, data: events.append((event, data)),
    )
    assembler.start()
    assembler.accept(
        {
            "choices": [
                {
                    "delta": {
                        "reasoning_content": "hidden",
                        "tool_calls": [
                            {
                                "index": 0,
                                "id": "call_s",
                                "function": {
                                    "name": "safe_read",
                                    "arguments": '{"path":',
                                },
                            }
                        ],
                    }
                }
            ]
        }
    )
    assembler.accept(
        {
            "choices": [
                {
                    "delta": {
                        "tool_calls": [
                            {"index": 0, "function": {"arguments": '"x.py"}'}}
                        ]
                    },
                    "finish_reason": "tool_calls",
                }
            ]
        }
    )
    response = assembler.finalize()
    call = next(item for item in response["output"] if item["type"] == "function_call")
    assert call["name"] == "mcp.read"
    assert call["arguments"] == '{"path":"x.py"}'
    assert any(event == "response.completed" for event, _ in events)
    event_names = [event for event, _ in events]
    assert "response.output_item.added" in event_names
    assert "response.function_call_arguments.done" in event_names
    assert "response.output_item.done" in event_names
    assert stored[0].messages[-1]["reasoning_content"] == "hidden"


def test_tool_names_are_unique_after_sanitizing():
    converted, reverse = convert_tools(
        [
            {"name": "a.b", "parameters": {"type": "object"}},
            {"name": "a_b", "parameters": {"type": "object"}},
        ]
    )
    names = [item["function"]["name"] for item in converted]
    assert names == ["a_b", "a_b_2"]
    assert reverse["a_b"] == "a.b"


def test_stream_failure_emits_terminal_failed_event():
    events = []
    assembler = StreamAssembler(
        body={"model": "opencode-go/deepseek-v4-flash", "input": "x"},
        model_alias="opencode-go/deepseek-v4-flash",
        model_upstream="deepseek-v4-flash",
        base_messages=[{"role": "user", "content": "x"}],
        reverse_names={},
        state_put=lambda _: None,
        emit=lambda event, data: events.append((event, data)),
    )
    assembler.start()
    response = assembler.fail("upstream_error", "stream ended unexpectedly")
    assert response["status"] == "failed"
    assert response["error"]["type"] == "upstream_error"
    assert events[-1][0] == "response.failed"


def test_state_can_recover_response_by_call_id(tmp_path):
    state = StateStore(str(tmp_path / "state.db"))
    item = StoredResponse(
        response_id="resp_1",
        model_alias="opencode-go/deepseek-v4-pro",
        model_upstream="deepseek-v4-pro",
        messages=[],
        pending_call_ids=["call_1"],
    )
    state.put(item)
    recovered = state.find_by_call_ids(["call_1"])
    assert recovered is not None
    assert recovered.response_id == "resp_1"
