from codex_opencode_adapter.conversion.responses_to_chat import build_chat_payload
from codex_opencode_adapter.conversion.stream_chat_to_responses import StreamAssembler
from codex_opencode_adapter.conversion.tool_context import build_tool_context


def test_conversion_package_exports_tool_context():
    context = build_tool_context(
        [
            {
                "type": "namespace",
                "name": "mcp",
                "tools": [
                    {
                        "type": "function",
                        "name": "read.file",
                        "description": "Read file",
                        "parameters": {"type": "object", "properties": {}},
                    }
                ],
            },
            {"type": "custom", "name": "shell.exec"},
        ]
    )
    names = [tool["function"]["name"] for tool in context.chat_tools]
    assert names == ["mcp__read_file", "shell_exec"]
    assert context.restore_name("mcp__read_file") == "mcp__read.file"
    assert context.is_custom_tool_chat_name("shell_exec")


def test_request_transform_keeps_old_protocol_facade_contract():
    payload, messages, reverse = build_chat_payload(
        {
            "model": "opencode-go/deepseek-v4-pro",
            "instructions": "System.",
            "input": [{"type": "message", "role": "developer", "content": "Dev."}, "Hi"],
            "tools": [{"type": "function", "name": "mcp.read", "parameters": {"type": "object"}}],
            "tool_choice": {"type": "function", "name": "mcp.read"},
            "stream": True,
        },
        model_upstream="deepseek-v4-pro",
        previous=None,
        reasoning_parameter={},
    )
    assert messages[0] == {"role": "system", "content": "System.\n\nDev."}
    assert payload["stream_options"] == {"include_usage": True}
    assert payload["tool_choice"] == {"type": "function", "function": {"name": "mcp_read"}}
    assert reverse == {"mcp_read": "mcp.read"}


def test_stream_reasoning_lifecycle_is_emitted():
    events = []
    assembler = StreamAssembler(
        body={"model": "opencode-go/deepseek-v4-pro", "input": "x"},
        model_alias="opencode-go/deepseek-v4-pro",
        model_upstream="deepseek-v4-pro",
        base_messages=[{"role": "user", "content": "x"}],
        reverse_names={},
        state_put=lambda _: None,
        emit=lambda event, data: events.append((event, data)),
    )
    assembler.start()
    assembler.accept({"choices": [{"delta": {"reasoning_content": "think"}}]})
    assembler.accept({"choices": [{"delta": {"content": "answer"}, "finish_reason": "stop"}]})
    response = assembler.finalize()
    names = [name for name, _ in events]
    assert "response.reasoning_summary_text.delta" in names
    assert "response.reasoning_summary_text.done" in names
    assert "response.output_text.delta" in names
    assert response["output"][0]["type"] == "reasoning"
    assert response["output"][1]["type"] == "message"


# ---------------------------------------------------------------------------
# P0-2: streaming tool_call lifecycle tests
# ---------------------------------------------------------------------------

def _make_assembler(**kwargs):
    """Helper to create a StreamAssembler with sensible defaults for tests."""
    defaults = dict(
        body={"model": "test", "input": "x"},
        model_alias="test",
        model_upstream="test",
        base_messages=[{"role": "user", "content": "x"}],
        reverse_names={},
        state_put=lambda _: None,
        emit=lambda event, data: None,
    )
    defaults.update(kwargs)
    return StreamAssembler(**defaults)


def test_tool_call_arguments_before_id_and_name():
    """Case 1: arguments arrive before id/name; pending arguments are replayed on start."""
    events = []
    assembler = _make_assembler(emit=lambda event, data: events.append((event, data)))
    assembler.start()

    # arguments arrive first (no id, no name)
    assembler.accept({"choices": [{"delta": {"tool_calls": [
        {"index": 0, "function": {"arguments": '{"city":"Tokyo"}'}},
    ]}}]})

    # then id and name arrive
    assembler.accept({"choices": [{"delta": {"tool_calls": [
        {"index": 0, "id": "call_1", "type": "function", "function": {"name": "get_weather"}},
    ]}}]})

    # more arguments after start
    assembler.accept({"choices": [{"delta": {"tool_calls": [
        {"index": 0, "function": {"arguments": ',"unit":"celsius"}'}},
    ]}, "finish_reason": "tool_calls"}]})

    response = assembler.finalize()
    event_names = [name for name, _ in events]

    # start should have emitted output_item.added
    assert "response.output_item.added" in event_names
    # arguments delta should include the pending portion replayed
    arg_deltas = [d for n, d in events if n == "response.function_call_arguments.delta"]
    assert len(arg_deltas) >= 2  # pending replay + immediate
    # final arguments should be complete
    done_events = [d for n, d in events if n == "response.function_call_arguments.done"]
    assert len(done_events) == 1
    assert "Tokyo" in done_events[0]["arguments"]
    assert "celsius" in done_events[0]["arguments"]
    # tool_call in final output
    assert response["output"][0]["type"] == "function_call"
    assert response["output"][0]["call_id"] == "call_1"


def test_tool_call_multiple_interleaved():
    """Case 2: two tool_calls interleave; arguments must not cross-contaminate."""
    events = []
    assembler = _make_assembler(emit=lambda event, data: events.append((event, data)))
    assembler.start()

    # index=0 start
    assembler.accept({"choices": [{"delta": {"tool_calls": [
        {"index": 0, "id": "call_a", "type": "function", "function": {"name": "read_file"}},
    ]}}]})
    # index=1 start
    assembler.accept({"choices": [{"delta": {"tool_calls": [
        {"index": 1, "id": "call_b", "type": "function", "function": {"name": "write_file"}},
    ]}}]})
    # index=1 args
    assembler.accept({"choices": [{"delta": {"tool_calls": [
        {"index": 1, "function": {"arguments": '{"path":"b.txt"}'}},
    ]}}]})
    # index=0 args
    assembler.accept({"choices": [{"delta": {"tool_calls": [
        {"index": 0, "function": {"arguments": '{"path":"a.txt"}'}},
    ]}, "finish_reason": "tool_calls"}]})

    response = assembler.finalize()

    # two distinct function_call items
    tool_items = [o for o in response["output"] if o["type"] == "function_call"]
    assert len(tool_items) == 2
    assert tool_items[0]["call_id"] == "call_a"
    assert "a.txt" in tool_items[0]["arguments"]
    assert "b.txt" not in tool_items[0]["arguments"]
    assert tool_items[1]["call_id"] == "call_b"
    assert "b.txt" in tool_items[1]["arguments"]
    assert "a.txt" not in tool_items[1]["arguments"]


def test_tool_call_id_arrives_late():
    """Case 3: name and args arrive before id; start waits for id."""
    events = []
    assembler = _make_assembler(emit=lambda event, data: events.append((event, data)))
    assembler.start()

    # name + args arrive, but no id
    assembler.accept({"choices": [{"delta": {"tool_calls": [
        {"index": 0, "type": "function", "function": {"name": "search", "arguments": '{"q":"test"}'}},
    ]}}]})

    # no output_item.added yet (id missing)
    added_events = [n for n, _ in events if n == "response.output_item.added"]
    assert len(added_events) == 0

    # id arrives
    assembler.accept({"choices": [{"delta": {"tool_calls": [
        {"index": 0, "id": "call_late"},
    ]}, "finish_reason": "tool_calls"}]})

    response = assembler.finalize()

    # now it should have started
    added_events = [n for n, _ in events if n == "response.output_item.added"]
    assert len(added_events) >= 1
    tool_items = [o for o in response["output"] if o["type"] == "function_call"]
    assert len(tool_items) == 1
    assert tool_items[0]["call_id"] == "call_late"


def test_tool_call_missing_name_gets_fallback():
    """Case 4: id and args present but name missing; fallback name 'unknown_tool' used."""
    events = []
    assembler = _make_assembler(emit=lambda event, data: events.append((event, data)))
    assembler.start()

    assembler.accept({"choices": [{"delta": {"tool_calls": [
        {"index": 0, "id": "call_noname", "function": {"arguments": '{"x":1}'}},
    ]}, "finish_reason": "tool_calls"}]})

    response = assembler.finalize()

    # function_call should exist with fallback name "unknown_tool"
    tool_items = [o for o in response["output"] if o["type"] == "function_call"]
    assert len(tool_items) == 1
    assert tool_items[0]["name"] == "unknown_tool"
    assert tool_items[0]["call_id"] == "call_noname"
    assert "x" in tool_items[0]["arguments"]


def test_finalize_idempotent():
    """Case 5: calling finalize twice emits terminal event only once."""
    events = []
    assembler = _make_assembler(emit=lambda event, data: events.append((event, data)))
    assembler.start()
    assembler.accept({"choices": [{"delta": {"content": "hi"}, "finish_reason": "stop"}]})

    assembler.finalize()
    assembler.finalize()

    terminal_events = [n for n, _ in events
                       if n in ("response.completed", "response.incomplete", "response.failed")]
    assert len(terminal_events) == 1


def test_fail_and_finalize_mutually_exclusive():
    """Case 6: fail then finalize (and vice versa) emits terminal only once."""
    # fail first, then finalize
    events1 = []
    a1 = _make_assembler(emit=lambda event, data: events1.append((event, data)))
    a1.start()
    a1.fail("upstream_error", "disconnected")
    a1.finalize()
    terminal1 = [n for n, _ in events1
                 if n in ("response.completed", "response.incomplete", "response.failed")]
    assert len(terminal1) == 1
    assert terminal1[0] == "response.failed"

    # finalize first, then fail
    events2 = []
    a2 = _make_assembler(emit=lambda event, data: events2.append((event, data)))
    a2.start()
    a2.accept({"choices": [{"delta": {"content": "ok"}, "finish_reason": "stop"}]})
    a2.finalize()
    a2.fail("upstream_error", "disconnected")
    terminal2 = [n for n, _ in events2
                 if n in ("response.completed", "response.incomplete", "response.failed")]
    assert len(terminal2) == 1
    assert terminal2[0] != "response.failed"  # finalize wins, not fail
