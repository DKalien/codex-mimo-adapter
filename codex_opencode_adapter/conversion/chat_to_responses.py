from __future__ import annotations

import time
from typing import Any, Callable

from ..state import StoredResponse
from .ids import canonicalize_json_string_if_parseable, new_id
from .responses_to_chat import repair_history
from .text import arguments_text, as_text, reasoning_text
from .tool_context import restore_tool_name

JSON = dict[str, Any]


def build_response(
    body: JSON,
    chat_response: JSON,
    *,
    model_alias: str,
    model_upstream: str,
    base_messages: list[JSON],
    reverse_names: dict[str, str],
    state_put: Callable[[StoredResponse], None],
    response_id: str | None = None,
    created_at: int | None = None,
) -> JSON:
    choice = (chat_response.get("choices") or [{}])[0]
    message = choice.get("message") or {}
    content = as_text(message.get("content"))
    reasoning = reasoning_text(message)
    thinking_blocks = message.get("thinking_blocks")
    response_id = response_id or new_id("resp")
    created_at = created_at or int(time.time())

    assistant: JSON = {"role": "assistant", "content": content}
    if reasoning:
        assistant["reasoning_content"] = reasoning
    if thinking_blocks:
        assistant["thinking_blocks"] = thinking_blocks

    output: list[JSON] = []
    if reasoning:
        output.append(reasoning_item(reasoning))
    if content:
        output.append(message_item(content))

    pending: list[str] = []
    replay_calls: list[JSON] = []
    for call in message.get("tool_calls") or []:
        function = call.get("function") or {}
        raw_name = str(function.get("name") or call.get("name") or "tool")
        call_id = str(call.get("id") or call.get("call_id") or new_id("call"))
        arguments = canonicalize_json_string_if_parseable(arguments_text(function.get("arguments", call.get("arguments", "{}"))))
        replay_calls.append({"id": call_id, "type": "function", "function": {"name": raw_name, "arguments": arguments}})
        pending.append(call_id)
        output.append(
            {
                "type": "function_call",
                "id": new_id("fc"),
                "call_id": call_id,
                "name": restore_tool_name(raw_name, reverse_names),
                "arguments": arguments,
                "status": "completed",
            }
        )
    if replay_calls:
        assistant["tool_calls"] = replay_calls

    state_put(
        StoredResponse(
            response_id=response_id,
            model_alias=model_alias,
            model_upstream=model_upstream,
            messages=repair_history(base_messages) + [assistant],
            pending_call_ids=pending,
            output=output,
            created_at=created_at,
            previous_response_id=str(body.get("previous_response_id") or ""),
        )
    )
    usage = chat_response.get("usage") or {}
    status, incomplete = completion_status(content, pending, choice.get("finish_reason"))
    return response_shell(
        body,
        response_id=response_id,
        created_at=created_at,
        model=model_alias,
        output=output,
        usage=usage,
        status=status,
        incomplete_details=incomplete,
    )


def reasoning_item(text: str, *, item_id: str | None = None) -> JSON:
    return {
        "type": "reasoning",
        "id": item_id or new_id("rs"),
        "summary": [{"type": "summary_text", "text": text}] if text else [],
    }


def message_item(content: str, *, item_id: str | None = None) -> JSON:
    return {
        "type": "message",
        "id": item_id or new_id("msg"),
        "status": "completed",
        "role": "assistant",
        "content": [{"type": "output_text", "text": content, "annotations": []}],
    }


def completion_status(content: str, pending: list[str], finish_reason: Any) -> tuple[str, JSON | None]:
    reason = str(finish_reason or "")
    if reason in {"length", "max_tokens"}:
        return "incomplete", {"reason": "max_output_tokens"}
    if reason in {"content_filter", "safety"}:
        return "incomplete", {"reason": "content_filter"}
    return "completed", None


def response_shell(
    body: JSON,
    *,
    response_id: str,
    created_at: int,
    model: str,
    output: list[JSON],
    usage: JSON,
    status: str,
    incomplete_details: JSON | None = None,
) -> JSON:
    input_tokens = usage.get("input_tokens", usage.get("prompt_tokens", 0))
    output_tokens = usage.get("output_tokens", usage.get("completion_tokens", 0))
    total_tokens = usage.get("total_tokens", input_tokens + output_tokens)
    input_details = usage.get("input_tokens_details") or usage.get("prompt_tokens_details") or {}
    output_details = usage.get("output_tokens_details") or usage.get("completion_tokens_details") or {}
    response_usage: JSON = {"input_tokens": input_tokens, "output_tokens": output_tokens, "total_tokens": total_tokens}
    if input_details:
        response_usage["input_tokens_details"] = input_details
    if output_details:
        response_usage["output_tokens_details"] = output_details
    return {
        "id": response_id,
        "object": "response",
        "created_at": created_at,
        "status": status,
        "error": None,
        "incomplete_details": incomplete_details,
        "instructions": body.get("instructions"),
        "model": model,
        "output": output,
        "parallel_tool_calls": bool(body.get("parallel_tool_calls", False)),
        "previous_response_id": body.get("previous_response_id"),
        "store": False,
        "usage": response_usage,
        "metadata": body.get("metadata") or {},
    }
