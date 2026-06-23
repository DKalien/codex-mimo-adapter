from __future__ import annotations

import logging
import time
from dataclasses import dataclass, field
from typing import Any, Callable

from ..state import StoredResponse
from .chat_to_responses import completion_status, message_item, reasoning_item, response_shell
from .ids import canonicalize_json_string_if_parseable, new_id
from .responses_to_chat import repair_history
from .text import arguments_text, as_text, reasoning_text
from .tool_context import restore_tool_name

JSON = dict[str, Any]


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
    reasoning_output_index: int | None = None
    message_item_id: str = field(default_factory=lambda: new_id("msg"))
    reasoning_item_id: str = field(default_factory=lambda: new_id("rs"))
    text_done: bool = False
    reasoning_done: bool = False
    terminal_emitted: bool = False

    def start(self) -> None:
        self._emit(
            "response.created",
            {
                "type": "response.created",
                "response": response_shell(
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
                "response": response_shell(
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
            reasoning_delta = reasoning_text(delta)
            if reasoning_delta:
                self._push_reasoning_delta(reasoning_delta)
            if delta.get("thinking_blocks"):
                self.thinking_blocks.append(delta["thinking_blocks"])
            if delta.get("content") is not None:
                text = as_text(delta["content"])
                if text:
                    self._push_text_delta(text)
            for call in delta.get("tool_calls") or []:
                if self.reasoning_parts and not self.reasoning_done:
                    self._finish_reasoning_item()
                self._accept_tool_delta(call)
            if choice.get("finish_reason"):
                self.finish_reason = str(choice["finish_reason"])

    def finalize(self) -> JSON:
        if self.terminal_emitted:
            return {}
        content = "".join(self.content_parts)
        reasoning = "".join(self.reasoning_parts)
        output: list[JSON] = []
        assistant: JSON = {"role": "assistant", "content": content}
        if reasoning:
            assistant["reasoning_content"] = reasoning
            self._finish_reasoning_item()
            output.append(reasoning_item(reasoning, item_id=self.reasoning_item_id))
        if self.thinking_blocks:
            assistant["thinking_blocks"] = self.thinking_blocks

        if content:
            self._finish_text_item()
            output.append(message_item(content, item_id=self.message_item_id))

        replay_calls: list[JSON] = []
        pending: list[str] = []
        for index in sorted(self.tool_calls):
            call = self.tool_calls[index]
            if not call["name"]:
                call["name"] = "unknown_tool"
                logging.warning("streaming tool call missing name at index %d, using fallback", index)
            if not call["id"]:
                call["id"] = f"call_{index}"
            self._ensure_tool_started(call)
            emitted = int(call.get("emitted_chars", 0))
            if emitted < len(call["arguments"]):
                self._emit_tool_arguments(call, call["arguments"][emitted:])
            call_id = call["id"]
            raw_name = call["name"]
            arguments = canonicalize_json_string_if_parseable(call["arguments"] or "{}")
            replay_calls.append({"id": call_id, "type": "function", "function": {"name": raw_name, "arguments": arguments}})
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
            self._emit("response.output_item.done", {"type": "response.output_item.done", "output_index": call["output_index"], "item": item})
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
        status, incomplete = completion_status(content, pending, self.finish_reason)
        response = response_shell(
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
        if self.terminal_emitted:
            return {}
        response = response_shell(
            self.body,
            response_id=self.response_id,
            created_at=self.created_at,
            model=self.model_alias,
            output=[],
            usage=self.usage,
            status="failed",
        )
        response["error"] = {"type": str(error_type or "upstream_error"), "message": str(message or "Upstream stream failed")[:1000]}
        self._emit("response.failed", {"type": "response.failed", "response": response})
        self.terminal_emitted = True
        return response

    def _push_reasoning_delta(self, delta: str) -> None:
        if self.reasoning_output_index is None:
            self.reasoning_output_index = self._allocate_output_index()
            self._emit(
                "response.output_item.added",
                {
                    "type": "response.output_item.added",
                    "output_index": self.reasoning_output_index,
                    "item": {"id": self.reasoning_item_id, "type": "reasoning", "status": "in_progress", "summary": []},
                },
            )
            self._emit(
                "response.reasoning_summary_part.added",
                {
                    "type": "response.reasoning_summary_part.added",
                    "item_id": self.reasoning_item_id,
                    "output_index": self.reasoning_output_index,
                    "summary_index": 0,
                    "part": {"type": "summary_text", "text": ""},
                },
            )
        self.reasoning_parts.append(delta)
        self._emit(
            "response.reasoning_summary_text.delta",
            {
                "type": "response.reasoning_summary_text.delta",
                "item_id": self.reasoning_item_id,
                "output_index": self.reasoning_output_index,
                "summary_index": 0,
                "delta": delta,
            },
        )

    def _finish_reasoning_item(self) -> None:
        if self.reasoning_output_index is None or self.reasoning_done:
            return
        text = "".join(self.reasoning_parts)
        item = reasoning_item(text, item_id=self.reasoning_item_id)
        self._emit(
            "response.reasoning_summary_text.done",
            {
                "type": "response.reasoning_summary_text.done",
                "item_id": self.reasoning_item_id,
                "output_index": self.reasoning_output_index,
                "summary_index": 0,
                "text": text,
            },
        )
        self._emit(
            "response.reasoning_summary_part.done",
            {
                "type": "response.reasoning_summary_part.done",
                "item_id": self.reasoning_item_id,
                "output_index": self.reasoning_output_index,
                "summary_index": 0,
                "part": {"type": "summary_text", "text": text},
            },
        )
        self._emit("response.output_item.done", {"type": "response.output_item.done", "output_index": self.reasoning_output_index, "item": item})
        self.reasoning_done = True

    def _push_text_delta(self, text: str) -> None:
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

    def _accept_tool_delta(self, delta: JSON) -> None:
        raw_index = delta.get("index")
        if raw_index is None:
            return  # missing index → ignore (aligned with Rust)
        try:
            index = int(raw_index)
        except (TypeError, ValueError):
            return
        state = self.tool_calls.setdefault(
            index,
            {"id": "", "item_id": new_id("fc"), "name": "", "arguments": "", "output_index": None, "added": False, "emitted_chars": 0},
        )
        function = delta.get("function") or {}
        if delta.get("id"):
            if state["added"]:
                if not state["id"]:
                    state["id"] = str(delta["id"])
                elif state["id"] != str(delta["id"]):
                    pass  # ignore id change after start (aligned with Rust)
            else:
                state["id"] = str(delta["id"])
        name_delta = function.get("name") if function.get("name") is not None else delta.get("name")
        if name_delta:
            if state["added"]:
                if state["name"] != str(name_delta):
                    pass  # ignore name change after start (aligned with Rust)
            else:
                # Name is overwritten, not appended. This assumes providers send the
                # full function name in a single delta (standard OpenAI behavior).
                state["name"] = str(name_delta)
        arguments = function.get("arguments", delta.get("arguments"))
        if arguments is not None:
            part = arguments_text(arguments)
            state["arguments"] += part
            if state["added"]:
                self._emit_tool_arguments(state, part)
        self._ensure_tool_started(state)

    def _ensure_text_started(self) -> None:
        if self.text_output_index is not None:
            return
        if self.reasoning_output_index is not None and not self.reasoning_done:
            self._finish_reasoning_item()
        self.text_output_index = self._allocate_output_index()
        self._emit(
            "response.output_item.added",
            {
                "type": "response.output_item.added",
                "output_index": self.text_output_index,
                "item": {"type": "message", "id": self.message_item_id, "status": "in_progress", "role": "assistant", "content": []},
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
        item = message_item(content, item_id=self.message_item_id)
        self._emit("response.output_text.done", {"type": "response.output_text.done", "output_index": self.text_output_index, "content_index": 0, "item_id": self.message_item_id, "text": content})
        self._emit(
            "response.content_part.done",
            {
                "type": "response.content_part.done",
                "output_index": self.text_output_index,
                "content_index": 0,
                "item_id": self.message_item_id,
                "part": {"type": "output_text", "text": content, "annotations": []},
            },
        )
        self._emit("response.output_item.done", {"type": "response.output_item.done", "output_index": self.text_output_index, "item": item})
        self.text_done = True

    def _ensure_tool_started(self, state: JSON) -> None:
        if state["added"] or not state["id"] or not state["name"]:
            return
        if self.text_output_index is not None and not self.text_done:
            self._finish_text_item()
        if self.reasoning_output_index is not None and not self.reasoning_done:
            self._finish_reasoning_item()
        state["output_index"] = self._allocate_output_index()
        state["added"] = True
        self._emit(
            "response.output_item.added",
            {
                "type": "response.output_item.added",
                "output_index": state["output_index"],
                "item": {"type": "function_call", "id": state["item_id"], "call_id": state["id"], "name": restore_tool_name(state["name"], self.reverse_names), "arguments": "", "status": "in_progress"},
            },
        )
        if state["arguments"]:
            self._emit_tool_arguments(state, state["arguments"])

    def _emit_tool_arguments(self, state: JSON, part: str) -> None:
        if not part:
            return
        self._emit(
            "response.function_call_arguments.delta",
            {"type": "response.function_call_arguments.delta", "output_index": state["output_index"], "item_id": state["item_id"], "call_id": state["id"], "delta": part},
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
