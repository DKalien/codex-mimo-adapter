from __future__ import annotations

import json
import uuid
from typing import Any


def new_id(prefix: str) -> str:
    return f"{prefix}_{uuid.uuid4().hex}"


def compact_json(value: Any) -> str:
    return json.dumps(value, ensure_ascii=False, separators=(",", ":"))


def canonicalize_json_string_if_parseable(value: str) -> str:
    try:
        parsed = json.loads(value)
    except Exception:
        return value
    return compact_json(parsed)
