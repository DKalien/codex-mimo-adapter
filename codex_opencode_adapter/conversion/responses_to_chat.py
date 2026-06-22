from __future__ import annotations

from typing import Any

from ..state import StoredResponse
from .ids import new_id
from .text import arguments_text, as_text
from .tool_context import ToolContext, build_tool_context, convert_tool_choice

JSON = dict[str, Any]


class HistoryError(ValueError):
    pass


def extract_request(body: JSON) -> tuple[list[JSON], list[JSON]]:
    messages: list[JSON] = []
    tool_outputs: list[JSON] = []

    instructions = body.get("instructions")
    if instructions:
        messages.append({"role": "system", "content": as_text(instructions)})

    raw_input = body.get("input", [])
    if isinstance(raw_input, str):
        raw_input = [{"role": "user", "content": raw_input}]
    elif isinstance(raw_input, dict):
        raw_input = [raw_input]
    if not isinstance(raw_input, list):
        raise ValueError("input must be a string, object, or list")

    pending_assistant_calls: list[JSON] = []

    def flush_pending_calls() -> None:
        nonlocal pending_assistant_calls
        if pending_assistant_calls:
            messages.append({"role": "assistant", "content": "", "tool_calls": pending_assistant_calls})
            pending_assistant_calls = []

    for item in raw_input:
        if isinstance(item, str):
            flush_pending_calls()
            messages.append({"role": "user", "content": item})
            continue
        if not isinstance(item, dict):
            continue
        kind = str(item.get("type", ""))

        if kind == "function_call_output":
            call_id = str(item.get("call_id") or "")
            if not call_id:
                raise HistoryError("function_call_output requires call_id")
            flush_pending_calls()
            tool_outputs.append({"role": "tool", "tool_call_id": call_id, "content": as_text(item.get("output", ""))})
            continue

        if kind == "function_call":
            pending_assistant_calls.append(
                {
                    "id": str(item.get("call_id") or item.get("id") or new_id("call")),
                    "type": "function",
                    "function": {
                        "name": str(item.get("name") or "tool"),
                        "arguments": arguments_text(item.get("arguments", "{}")),
                    },
                }
            )
            continue

        if kind in {"reasoning", "summary"}:
            continue

        if kind in {"message", ""} or item.get("role"):
            flush_pending_calls()
            role = str(item.get("role") or "user")
            if role == "developer":
                role = "system"
            if role not in {"system", "user", "assistant", "tool"}:
                role = "user"
            messages.append({"role": role, "content": as_text(item.get("content", ""))})
            continue

        if kind in {"input_text", "output_text", "text"}:
            flush_pending_calls()
            messages.append({"role": "user", "content": as_text(item)})

    flush_pending_calls()
    return messages, tool_outputs


def function_output_call_ids(body: JSON) -> list[str]:
    _, outputs = extract_request(body)
    return [str(output.get("tool_call_id")) for output in outputs if output.get("tool_call_id")]


def merge_new_messages(base: list[JSON], incoming: list[JSON]) -> list[JSON]:
    return [dict(message) for message in base] + [dict(message) for message in incoming]


def normalize_upstream_roles(messages: list[JSON]) -> list[JSON]:
    system_chunks: list[str] = []
    rest: list[JSON] = []
    for message in messages:
        item = dict(message)
        if item.get("role") == "developer":
            item["role"] = "system"
        if item.get("role") == "system":
            text = as_text(item.get("content"))
            if text:
                system_chunks.append(text)
            continue
        if item.get("role") not in {"user", "assistant", "tool"}:
            item["role"] = "user"
        rest.append(item)
    if system_chunks:
        return [{"role": "system", "content": "\n\n".join(system_chunks)}] + rest
    return rest


def repair_history(messages: list[JSON], tool_outputs: list[JSON] | None = None) -> list[JSON]:
    repaired = [dict(message) for message in messages]
    if not tool_outputs:
        return repaired

    pending = {
        str(call.get("id"))
        for message in repaired
        if message.get("role") == "assistant"
        for call in (message.get("tool_calls") or [])
        if call.get("id")
    }
    seen: set[str] = set()
    for output in tool_outputs:
        call_id = str(output.get("tool_call_id") or "")
        if call_id not in pending:
            raise HistoryError(f"unknown tool call id: {call_id}")
        if call_id in seen:
            raise HistoryError(f"duplicate tool output: {call_id}")
        seen.add(call_id)
        repaired.append(dict(output))
    return repaired


def build_chat_payload(
    body: JSON,
    *,
    model_upstream: str,
    previous: StoredResponse | None,
    reasoning_parameter: JSON,
) -> tuple[JSON, list[JSON], dict[str, str]]:
    incoming, outputs = extract_request(body)
    if outputs:
        if previous is None:
            raise HistoryError("tool output has no matching stored response")
        messages = merge_new_messages(repair_history(previous.messages, outputs), incoming)
    elif previous is not None:
        messages = merge_new_messages(previous.messages, incoming)
    else:
        messages = incoming
    if not messages:
        messages = [{"role": "user", "content": ""}]
    messages = normalize_upstream_roles(messages)

    tool_context = build_tool_context(body.get("tools"))
    payload: JSON = {"model": model_upstream, "messages": messages, "stream": bool(body.get("stream"))}
    if tool_context.chat_tools:
        payload["tools"] = tool_context.chat_tools
        choice = convert_tool_choice(body.get("tool_choice"), tool_context)
        if choice is not None:
            payload["tool_choice"] = choice
        if body.get("parallel_tool_calls") is not None:
            payload["parallel_tool_calls"] = bool(body.get("parallel_tool_calls"))

    for source, target in (
        ("temperature", "temperature"),
        ("top_p", "top_p"),
        ("max_output_tokens", "max_tokens"),
        ("max_tokens", "max_tokens"),
        ("presence_penalty", "presence_penalty"),
        ("frequency_penalty", "frequency_penalty"),
        ("response_format", "response_format"),
        ("seed", "seed"),
        ("stop", "stop"),
    ):
        if body.get(source) is not None:
            payload[target] = body[source]
    if payload["stream"]:
        payload["stream_options"] = {"include_usage": True}
    payload.update(reasoning_parameter)
    return payload, messages, tool_context.reverse_names


def build_chat_payload_with_context(
    body: JSON,
    *,
    model_upstream: str,
    previous: StoredResponse | None,
    reasoning_parameter: JSON,
) -> tuple[JSON, list[JSON], ToolContext]:
    payload, messages, _ = build_chat_payload(
        body,
        model_upstream=model_upstream,
        previous=previous,
        reasoning_parameter=reasoning_parameter,
    )
    context = build_tool_context(body.get("tools"))
    return payload, messages, context
