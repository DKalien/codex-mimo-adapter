from __future__ import annotations

import threading
import time
from dataclasses import dataclass
from typing import Any, Callable


JSON = dict[str, Any]
Fetcher = Callable[[], JSON]

VERIFIED_MODEL_METADATA: dict[str, JSON] = {
    "deepseek-v4-flash": {
        "id": "deepseek-v4-flash",
        "capabilities": {"reasoning": True, "toolcall": True},
        "variants": {
            level: {"reasoningEffort": level}
            for level in ("low", "medium", "high", "max")
        },
    },
    "deepseek-v4-pro": {
        "id": "deepseek-v4-pro",
        "capabilities": {"reasoning": True, "toolcall": True},
        "variants": {
            level: {"reasoningEffort": level}
            for level in ("low", "medium", "high", "max")
        },
    },
    "mimo-v2.5": {
        "id": "mimo-v2.5",
        "capabilities": {"reasoning": True, "toolcall": True},
        "variants": {
            level: {"reasoningEffort": level}
            for level in ("low", "medium", "high")
        },
    },
    "mimo-v2.5-pro": {
        "id": "mimo-v2.5-pro",
        "capabilities": {"reasoning": True, "toolcall": True},
        "variants": {
            level: {"reasoningEffort": level}
            for level in ("low", "medium", "high")
        },
    },
}


@dataclass(frozen=True)
class ReasoningDecision:
    requested: str | None
    applied: bool
    parameter: JSON
    reason: str


class ModelCapabilities:
    """Caches OpenCode Go /models metadata and makes conservative decisions."""

    def __init__(self, fetcher: Fetcher, ttl_seconds: int = 3600):
        self.fetcher = fetcher
        self.ttl_seconds = ttl_seconds
        self._loaded_at = 0.0
        self._models: dict[str, JSON] = {}
        self._lock = threading.Lock()

    def all(self) -> list[JSON]:
        self._refresh_if_needed()
        return list(self._models.values())

    def get(self, model: str) -> JSON:
        self._refresh_if_needed()
        return self._models.get(strip_model_prefix(model), {})

    def reasoning_decision(
        self, model: str, requested_effort: str | None
    ) -> ReasoningDecision:
        requested = normalize_effort(requested_effort)
        if not requested:
            return ReasoningDecision(None, False, {}, "not_requested")

        metadata = self.get(model)
        capabilities = metadata.get("capabilities") or {}
        if not capabilities.get("reasoning"):
            return ReasoningDecision(requested, False, {}, "model_does_not_support_reasoning")

        variants = metadata.get("variants") or {}
        variant = variants.get(requested)
        if not isinstance(variant, dict):
            return ReasoningDecision(requested, False, {}, "effort_not_declared_by_model")

        if "reasoningEffort" in variant:
            value = str(variant["reasoningEffort"])
            return ReasoningDecision(
                requested, True, {"reasoning_effort": value}, "metadata_reasoning_effort"
            )

        # Other provider protocols are deliberately not guessed in v1.
        return ReasoningDecision(requested, False, {}, "unsupported_variant_protocol")

    def _refresh_if_needed(self) -> None:
        if self._models and time.time() - self._loaded_at < self.ttl_seconds:
            return
        with self._lock:
            if self._models and time.time() - self._loaded_at < self.ttl_seconds:
                return
            try:
                raw = self.fetcher()
            except Exception:
                raw = {"data": []}
            rows = raw.get("data", raw if isinstance(raw, list) else [])
            if not isinstance(rows, list):
                rows = []
            discovered = {
                str(row.get("id")): row
                for row in rows
                if isinstance(row, dict) and row.get("id")
            }
            # The Go /models endpoint may expose less metadata than models.dev.
            # Verified profiles fill only missing fields and never overwrite
            # fresher upstream declarations.
            merged = {key: dict(value) for key, value in VERIFIED_MODEL_METADATA.items()}
            for key, value in discovered.items():
                base = merged.get(key, {})
                merged[key] = {
                    **base,
                    **value,
                    "capabilities": {
                        **(base.get("capabilities") or {}),
                        **(value.get("capabilities") or {}),
                    },
                    "variants": value.get("variants") or base.get("variants") or {},
                }
            self._models = merged
            self._loaded_at = time.time()


def strip_model_prefix(model: str) -> str:
    value = str(model or "").strip()
    if not value.startswith("opencode-go/"):
        raise ValueError("model must use the opencode-go/ prefix")
    model_id = value.split("/", 1)[1]
    if not model_id:
        raise ValueError("model id is empty")
    return model_id


def normalize_effort(value: Any) -> str | None:
    if isinstance(value, dict):
        value = value.get("effort")
    if value is None:
        return None
    normalized = str(value).strip().lower()
    aliases = {"off": "none", "xhigh": "max"}
    return aliases.get(normalized, normalized) if normalized else None
