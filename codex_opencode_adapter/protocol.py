from __future__ import annotations

from .conversion.chat_to_responses import (
    build_response,
    completion_status as _completion_status,
    message_item as _message_item,
    reasoning_item as _reasoning_item,
    response_shell as _response_shell,
)
from .conversion.ids import (
    canonicalize_json_string_if_parseable,
    compact_json,
    new_id,
)
from .conversion.responses_to_chat import (
    HistoryError,
    build_chat_payload,
    extract_request,
    function_output_call_ids,
    merge_new_messages,
    normalize_upstream_roles,
    repair_history,
)
from .conversion.stream_chat_to_responses import StreamAssembler
from .conversion.text import arguments_text as _arguments_text
from .conversion.text import as_text, reasoning_text as _reasoning_text
from .conversion.tool_context import (
    ToolContext,
    build_tool_context,
    convert_tool_choice as _convert_tool_choice,
    convert_tools,
    restore_tool_name,
)

__all__ = [
    "HistoryError",
    "StreamAssembler",
    "ToolContext",
    "as_text",
    "build_chat_payload",
    "build_response",
    "build_tool_context",
    "canonicalize_json_string_if_parseable",
    "compact_json",
    "convert_tools",
    "extract_request",
    "function_output_call_ids",
    "merge_new_messages",
    "new_id",
    "normalize_upstream_roles",
    "repair_history",
    "restore_tool_name",
]
