from __future__ import annotations

import json
import re
import time
import uuid
from dataclasses import dataclass, field
from typing import Any, Callable

from .state import StoredResponse


JSON = dict[str, Any]


class HistoryError(ValueError):
    pass


def new_id(prefix: str) -> str:
    return f"{prefix}_{uuid.uuid4().hex}"


def compact_json(value: Any) -> str:
    return json.dumps(value, ensure_ascii=False, separators=(",", ":"))


def as_text(value: Any) -> str:
    """Best-effort text extraction for Responses and Chat content shapes."""
    if value is None:
        return ""
    if isinstance(value, str):
        return value
    if isinstance(value, (int, float, bool)):
        return str(value)
    if isinstance(value, list):
        parts: list[str] = []
        for item in value:
            if isinstance(item, dict):
                kind = item.get("type")
                if kind in {"input_text", "output_text", "text", "refusal"}:
                    parts.append(str(item.get("text") or item.get("refusal") or ""))
                elif kind in {"tool_result", "function_call_output"}:
                    parts.append(as_text(item.get("content", item.get("output", ""))))
                elif "content" in item:
                    parts.append(as_text(item["content"]))
                elif "text" in item:
                    parts.append(str(item["text"]))
                else:
                    parts.append(compact_json(item))
            else:
                parts.append(as_text(item))
        return "\n".join(part for part in parts if part)
    if isinstance(value, dict):
        if "text" in value:
            return str(value["text"])
        if "content" in value:
            return as_text(value["content"])
        if "output" in value:
            return as_text(value["output"])
        return compact_json(value)
    return str(value)


def extract_request(body: JSON) -> tuple[list[JSON], list[JSON]]:
    """Convert a Responses request body into incoming Chat messages and tool outputs.

    This intentionally preserves replayed Responses function_call items as
    assistant tool_calls because Chat Completions providers often need complete
    alternating history to accept following tool messages.
    """
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
            messages.append(
                {
                    "role": "assistant",
                    "content": "",
                    "tool_calls": pending_assistant_calls,
                }
            )
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
            tool_outputs.append(
                {
                    "role": "tool",
                    "tool_call_id": call_id,
                    "content": as_text(item.get("output", "")),
                }
            )
            continue

        if kind == "function_call":
            pending_assistant_calls.append(
                {
                    "id": str(item.get("call_id") or item.get("id") or new_id("call")),
                    "type": "function",
                    "function": {
                        "name": str(item.get("name") or "tool"),
                        "arguments": _arguments_text(item.get("arguments", "{}")),
                    },
                }
            )
            continue

        if kind in {"reasoning", "summary"}:
            # Reasoning items are not forwarded as visible chat content. Hidden
            # reasoning from upstream is stored only for replay continuity.
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
    return [
        str(output.get("tool_call_id"))
        for output in outputs
        if output.get("tool_call_id")
    ]


def merge_new_messages(base: list[JSON], incoming: list[JSON]) -> list[JSON]:
    return [dict(message) for message in base] + [dict(message) for message in incoming]


def normalize_upstream_roles(messages: list[JSON]) -> list[JSON]:
    """Return messages using only roles accepted by OpenCode Go.

    Multiple system/developer messages are collapsed to the first position. This
    mirrors the strict-provider hardening used by mature bridges and avoids
    upstreams that reject system messages in the middle of a conversation.
    """
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


def repair_history(
    messages: list[JSON], tool_outputs: list[JSON] | None = None
) -> list[JSON]:
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


_SAFE_TOOL_NAME = re.compile(r"[^a-zA-Z0-9_-]")
CHAT_TOOL_NAME_MAX_LEN = 64
CUSTOM_TOOL_INPUT_FIELD = "input"


def _safe_tool_name(name: str, used: set[str]) -> str:
    base = _SAFE_TOOL_NAME.sub("_", name.strip())[:CHAT_TOOL_NAME_MAX_LEN] or "tool"
    candidate = base
    suffix = 2
    while candidate in used:
        tail = f"_{suffix}"
        candidate = f"{base[: CHAT_TOOL_NAME_MAX_LEN - len(tail)]}{tail}"
        suffix += 1
    used.add(candidate)
    return candidate


def _tool_source(item: JSON) -> JSON | None:
    nested = item.get("function")
    return nested if isinstance(nested, dict) else item


def convert_tools(tools: Any) -> tuple[list[JSON], dict[str, str]]:
    """Convert Responses tools to Chat Completions tools.

    Supports normal function tools plus a conservative lowering of Responses
    namespace/custom/tool_search tools to Chat function tools.
    """
    if not isinstance(tools, list):
        return [], {}
    converted: list[JSON] = []
    reverse: dict[str, str] = {}
    used: set[str] = set()

    def add_function(source: JSON, *, original_name: str | None = None) -> None:
        original = str(original_name or source.get("name") or "").strip()
        if not original:
            return
        safe = _safe_tool_name(original, used)
        reverse[safe] = original
        parameters = source.get("parameters")
        if not isinstance(parameters, dict):
            parameters = {"type": "object", "properties": {}}
        converted.append(
            {
                "type": "function",
                "function": {
                    "name": safe,
                    "description": str(source.get("description") or ""),
                    "parameters": parameters,
                },
            }
        )

    def add_custom(source: JSON) -> None:
        original = str(source.get("name") or "").strip()
        if not original:
            return
        description = str(source.get("description") or "")
        preserved = compact_json(source)
        add_function(
            {
                "name": original,
                "description": (
                    f"{description}\n\nOriginal Responses custom tool definition:\n{preserved}"
                    if description
                    else f"Original Responses custom tool definition:\n{preserved}"
                ),
                "parameters": {
                    "type": "object",
                    "properties": {
                        CUSTOM_TOOL_INPUT_FIELD: {
                            "type": "string",
                            "description": "Raw string input for the original custom tool.",
                        }
                    },
                    "required": [CUSTOM_TOOL_INPUT_FIELD],
                },
            }
        )

    def add_namespace(source: JSON) -> None:
        namespace = str(source.get("name") or "").strip()
        children = source.get("tools") or source.get("children") or []
        if not namespace or not isinstance(children, list):
            return
        for child in children:
            if not isinstance(child, dict):
                continue
            child_source = _tool_source(child)
            if not isinstance(child_source, dict):
                continue
            child_name = str(child_source.get("name") or "").strip()
            if child_name:
                add_function(child_source, original_name=f"{namespace}__{child_name}")

    for item in tools:
        if isinstance(item, str):
            add_custom({"type": "custom", "name": item})
            continue
        if not isinstance(item, dict):
            continue
        kind = str(item.get("type") or "function")
        if kind == "namespace":
            add_namespace(item)
        elif kind in {"custom", "tool_search"}:
            add_custom(item)
        else:
            source = _tool_source(item)
            if isinstance(source, dict):
                add_function(source)
    return converted, reverse


def restore_tool_name(name: str, reverse: dict[str, str]) -> str:
    return reverse.get(name, name)


def _convert_tool_choice(tool_choice: Any, reverse: dict[str, str]) -> Any:
    if tool_choice is None:
        return None
    if isinstance(tool_choice, str):
        if tool_choice == "required":
            return "required"
        return tool_choice
    if not isinstance(tool_choice, dict):
        return tool_choice
    kind = tool_choice.get("type")
    if kind == "function":
        name = str(tool_choice.get("name") or "")
        chat_name = next((safe for safe, original in reverse.items() if original == name), name)
        return {"type": "function", "function": {"name": chat_name}}
    if kind == "tool":
        name = str(tool_choice.get("name") or "")
        chat_name = next((safe for safe, original in reverse.items() if original == name), name)
        return {"type": "function", "function": {"name": chat_name}}
    if kind in {"auto", "none", "required"}:
        return kind
    return tool_choice


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
        messages = repair_history(previous.messages, outputs)
        messages = merge_new_messages(messages, incoming)
    elif previous is not None:
        messages = merge_new_messages(previous.messages, incoming)
    else:
        messages = incoming
    if not messages:
        messages = [{"role": "user", "content": ""}]
    messages = normalize_upstream_roles(messages)

    tools, reverse = convert_tools(body.get("tools"))
    payload: JSON = {
        "model": model_upstream,
        "messages": messages,
        "stream": bool(body.get("stream")),
    }
    if tools:
        payload["tools"] = tools
        choice = _convert_tool_choice(body.get("tool_choice"), reverse)
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
    return payload, messages, reverse


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
    reasoning = _reasoning_text(message)
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
        output.append({"type": "reasoning", "id": new_id("rs"), "summary": []})
    if content:
        output.append(_message_item(content))

    pending: list[str] = []
    replay_calls: list[JSON] = []
    for call in message.get("tool_calls") or []:
        function = call.get("function") or {}
        raw_name = str(function.get("name") or call.get("name") or "tool")
        call_id = str(call.get("id") or call.get("call_id") or new_id("call"))
        arguments = canonicalize_json_string_if_parseable(
            _arguments_text(function.get("arguments", call.get("arguments", "{}")))
        )
        replay_calls.append(
            {
                "id": call_id,
                "type": "function",
                "function": {"name": raw_name, "arguments": arguments},
            }
        )
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

    messages = repair_history(base_messages) + [assistant]
    state_put(
        StoredResponse(
            response_id=response_id,
            model_alias=model_alias,
            model_upstream=model_upstream,
            messages=messages,
            pending_call_ids=pending,
            output=output,
            created_at=created_at,
            previous_response_id=str(body.get("previous_response_id") or ""),
        )
    )
    usage = chat_response.get("usage") or {}
    status, incomplete = _completion_status(content, pending, choice.get("finish_reason"))
    return _response_shell(
        body,
        response_id=response_id,
        created_at=created_at,
        model=model_alias,
        output=output,
        usage=usage,
        status=status,
        incomplete_details=incomplete,
    )


@dataclass
class StreamAssembler:
    body: JSON
    model_alias: str
    model_upstream: str
    base_messages: list[JSON]
    reverse_names: dict[str, str]
    state_put: Callable[[StoredResponse], None]
    emit: Callable[[str, JSON], None]
    response_id: str = field(default_factory=lambda: new_id("resp"))
    created_at: int = field(default_factory=lambda: int(time.time()))
    content_parts: list[str] = field(default_factory=list)
    reasoning_parts: list[str] = field(default_factory=list)
    thinking_blocks: list[Any] = field(default_factory=list)
    tool_calls: dict[int, JSON] = field(default_factory=dict)
    usage: JSON = field(default_factory=dict)
    finish_reason: str | None = None
    sequence: int = 0
    started: bool = False
    next_output_index: int = 0
    text_output_index: int | None = None
    message_item_id: str = field(default_factory=lambda: new_id("msg"))
    text_done: bool = False
    terminal_emitted: bool = False

    def start(self) -> None:
        self._emit(
            "response.created",
            {
                "type": "response.created",
                "response": _response_shell(
                    self.body,
                    response_id=self.response_id,
                    created_at=self.created_at,
                    model=self.model_alias,
                    output=[],
                    usage={},
                    status="in_progress",
                ),
            },
        )
        self._emit(
            "response.in_progress",
            {
                "type": "response.in_progress",
                "response": _response_shell(
                    self.body,
                    response_id=self.response_id,
                    created_at=self.created_at,
                    model=self.model_alias,
                    output=[],
                    usage={},
                    status="in_progress",
                ),
            },
        )
        self.started = True

    def accept(self, chunk: JSON) -> None:
        if chunk.get("usage"):
            self.usage = chunk["usage"]
        for choice in chunk.get("choices") or []:
            delta = choice.get("delta") or {}
            reasoning_delta = _reasoning_text(delta)
            if reasoning_delta:
                self.reasoning_parts.append(reasoning_delta)
            if delta.get("thinking_blocks"):
                self.thinking_blocks.append(delta["thinking_blocks"])
            if delta.get("content") is not None:
                text = as_text(delta["content"])
                if text:
                    self._ensure_text_started()
                    self.content_parts.append(text)
                    self._emit(
                        "response.output_text.delta",
                        {
                            "type": "response.output_text.delta",
                            "output_index": self.text_output_index,
                            "content_index": 0,
                            "item_id": self.message_item_id,
                            "delta": text,
                        },
                    )
            for call in delta.get("tool_calls") or []:
                self._accept_tool_delta(call)
            if choice.get("finish_reason"):
                self.finish_reason = str(choice["finish_reason"])

    def finalize(self) -> JSON:
        content = "".join(self.content_parts)
        reasoning = "".join(self.reasoning_parts)
        output: list[JSON] = []
        assistant: JSON = {"role": "assistant", "content": content}
        if reasoning:
            assistant["reasoning_content"] = reasoning
            output.append({"type": "reasoning", "id": new_id("rs"), "summary": []})
        if self.thinking_blocks:
            assistant["thinking_blocks"] = self.thinking_blocks

        if content:
            self._finish_text_item()
            output.append(_message_item(content, item_id=self.message_item_id))

        replay_calls: list[JSON] = []
        pending: list[str] = []
        for index in sorted(self.tool_calls):
            call = self.tool_calls[index]
            if not call["name"]:
                call["name"] = "tool"
            self._ensure_tool_started(call)
            emitted = int(call.get("emitted_chars", 0))
            if emitted < len(call["arguments"]):
                self._emit_tool_arguments(call, call["arguments"][emitted:])
            call_id = call["id"]
            raw_name = call["name"] or "tool"
            arguments = canonicalize_json_string_if_parseable(call["arguments"] or "{}")
            replay_calls.append(
                {
                    "id": call_id,
                    "type": "function",
                    "function": {"name": raw_name, "arguments": arguments},
                }
            )
            pending.append(call_id)
            item = {
                "type": "function_call",
                "id": call["item_id"],
                "call_id": call_id,
                "name": restore_tool_name(raw_name, self.reverse_names),
                "arguments": arguments,
                "status": "completed",
            }
            output.append(item)
            self._emit(
                "response.function_call_arguments.done",
                {
                    "type": "response.function_call_arguments.done",
                    "output_index": call["output_index"],
                    "item_id": call["item_id"],
                    "call_id": call_id,
                    "name": restore_tool_name(raw_name, self.reverse_names),
                    "arguments": arguments,
                },
            )
            self._emit(
                "response.output_item.done",
                {
                    "type": "response.output_item.done",
                    "output_index": call["output_index"],
                    "item": item,
                },
            )
        if replay_calls:
            assistant["tool_calls"] = replay_calls

        self.state_put(
            StoredResponse(
                response_id=self.response_id,
                model_alias=self.model_alias,
                model_upstream=self.model_upstream,
                messages=repair_history(self.base_messages) + [assistant],
                pending_call_ids=pending,
                output=output,
                created_at=self.created_at,
                previous_response_id=str(self.body.get("previous_response_id") or ""),
            )
        )
        status, incomplete = _completion_status(content, pending, self.finish_reason)
        response = _response_shell(
            self.body,
            response_id=self.response_id,
            created_at=self.created_at,
            model=self.model_alias,
            output=output,
            usage=self.usage,
            status=status,
            incomplete_details=incomplete,
        )
        event = "response.completed" if status == "completed" else "response.incomplete"
        self._emit(event, {"type": event, "response": response})
        self.terminal_emitted = True
        return response

    def fail(self, error_type: str, message: str) -> JSON:
        """Emit a valid terminal Responses event after an SSE stream has started."""
        if self.terminal_emitted:
            return {}
        response = _response_shell(
            self.body,
            response_id=self.response_id,
            created_at=self.created_at,
            model=self.model_alias,
            output=[],
            usage=self.usage,
            status="failed",
        )
        response["error"] = {
            "type": str(error_type or "upstream_error"),
            "message": str(message or "Upstream stream failed")[:1000],
        }
        self._emit("response.failed", {"type": "response.failed", "response": response})
        self.terminal_emitted = True
        return response

    def _accept_tool_delta(self, delta: JSON) -> None:
        try:
            index = int(delta.get("index", 0))
        except (TypeError, ValueError):
            index = 0
        state = self.tool_calls.setdefault(
            index,
            {
                "id": str(delta.get("id") or new_id("call")),
                "item_id": new_id("fc"),
                "name": "",
                "arguments": "",
                "output_index": None,
                "added": False,
                "emitted_chars": 0,
            },
        )
        function = delta.get("function") or {}
        if delta.get("id"):
            state["id"] = str(delta["id"])
        name_delta = function.get("name") if function.get("name") is not None else delta.get("name")
        if name_delta:
            state["name"] += str(name_delta)
            self._ensure_tool_started(state)
        arguments = function.get("arguments", delta.get("arguments"))
        if arguments is not None:
            part = _arguments_text(arguments)
            state["arguments"] += part
            if state["added"]:
                self._emit_tool_arguments(state, part)

    def _ensure_text_started(self) -> None:
        if self.text_output_index is not None:
            return
        self.text_output_index = self._allocate_output_index()
        item = {
            "type": "message",
            "id": self.message_item_id,
            "status": "in_progress",
            "role": "assistant",
            "content": [],
        }
        self._emit(
            "response.output_item.added",
            {
                "type": "response.output_item.added",
                "output_index": self.text_output_index,
                "item": item,
            },
        )
        self._emit(
            "response.content_part.added",
            {
                "type": "response.content_part.added",
                "output_index": self.text_output_index,
                "content_index": 0,
                "item_id": self.message_item_id,
                "part": {"type": "output_text", "text": "", "annotations": []},
            },
        )

    def _finish_text_item(self) -> None:
        if self.text_output_index is None or self.text_done:
            return
        content = "".join(self.content_parts)
        self._emit(
            "response.output_text.done",
            {
                "type": "response.output_text.done",
                "output_index": self.text_output_index,
                "content_index": 0,
                "item_id": self.message_item_id,
                "text": content,
            },
        )
        part = {"type": "output_text", "text": content, "annotations": []}
        self._emit(
            "response.content_part.done",
            {
                "type": "response.content_part.done",
                "output_index": self.text_output_index,
                "content_index": 0,
                "item_id": self.message_item_id,
                "part": part,
            },
        )
        self._emit(
            "response.output_item.done",
            {
                "type": "response.output_item.done",
                "output_index": self.text_output_index,
                "item": _message_item(content, item_id=self.message_item_id),
            },
        )
        self.text_done = True

    def _ensure_tool_started(self, state: JSON) -> None:
        if state["added"] or not state["name"]:
            return
        if self.text_output_index is not None and not self.text_done:
            self._finish_text_item()
        state["output_index"] = self._allocate_output_index()
        state["added"] = True
        self._emit(
            "response.output_item.added",
            {
                "type": "response.output_item.added",
                "output_index": state["output_index"],
                "item": {
                    "type": "function_call",
                    "id": state["item_id"],
                    "call_id": state["id"],
                    "name": restore_tool_name(state["name"], self.reverse_names),
                    "arguments": "",
                    "status": "in_progress",
                },
            },
        )
        if state["arguments"]:
            self._emit_tool_arguments(state, state["arguments"])

    def _emit_tool_arguments(self, state: JSON, part: str) -> None:
        if not part:
            return
        self._emit(
            "response.function_call_arguments.delta",
            {
                "type": "response.function_call_arguments.delta",
                "output_index": state["output_index"],
                "item_id": state["item_id"],
                "call_id": state["id"],
                "delta": part,
            },
        )
        state["emitted_chars"] = int(state.get("emitted_chars", 0)) + len(part)

    def _allocate_output_index(self) -> int:
        value = self.next_output_index
        self.next_output_index += 1
        return value

    def _emit(self, event: str, payload: JSON) -> None:
        self.sequence += 1
        data = dict(payload)
        data.setdefault("response_id", self.response_id)
        data.setdefault("sequence_number", self.sequence)
        self.emit(event, data)


def _arguments_text(value: Any) -> str:
    return value if isinstance(value, str) else compact_json(value)


def canonicalize_json_string_if_parseable(value: str) -> str:
    try:
        parsed = json.loads(value)
    except Exception:
        return value
    return compact_json(parsed)


def _reasoning_text(value: JSON) -> str:
    for key in ("reasoning_content", "reasoning", "thinking"):
        if value.get(key):
            return as_text(value[key])
    return ""


def _message_item(content: str, *, item_id: str | None = None) -> JSON:
    return {
        "type": "message",
        "id": item_id or new_id("msg"),
        "status": "completed",
        "role": "assistant",
        "content": [{"type": "output_text", "text": content, "annotations": []}],
    }


def _completion_status(
    content: str, pending: list[str], finish_reason: Any
) -> tuple[str, JSON | None]:
    reason = str(finish_reason or "")
    if reason in {"length", "max_tokens"}:
        return "incomplete", {"reason": "max_output_tokens"}
    if reason in {"content_filter", "safety"}:
        return "incomplete", {"reason": "content_filter"}
    return "completed", None


def _response_shell(
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
    response_usage: JSON = {
        "input_tokens": input_tokens,
        "output_tokens": output_tokens,
        "total_tokens": total_tokens,
    }
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
