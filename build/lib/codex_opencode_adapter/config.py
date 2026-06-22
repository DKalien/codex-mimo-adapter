from __future__ import annotations

import os
from dataclasses import dataclass


@dataclass(frozen=True)
class Config:
    upstream_base: str
    upstream_key: str
    local_token: str
    host: str
    port: int
    timeout_seconds: float
    first_byte_timeout_seconds: float
    idle_timeout_seconds: float
    max_concurrency: int
    state_db: str
    state_ttl_seconds: int
    model_cache_ttl_seconds: int
    max_request_bytes: int

    @classmethod
    def from_env(cls) -> "Config":
        key = os.getenv("OPENCODE_GO_API_KEY", "").strip()
        if not key:
            raise RuntimeError("OPENCODE_GO_API_KEY is required")
        local_token = os.getenv("CODEX_OPENCODE_LOCAL_TOKEN", "").strip()
        if local_token and local_token == key:
            raise RuntimeError(
                "CODEX_OPENCODE_LOCAL_TOKEN must differ from OPENCODE_GO_API_KEY"
            )
        return cls(
            upstream_base=os.getenv(
                "OPENCODE_GO_BASE_URL", "https://opencode.ai/zen/go/v1"
            ).rstrip("/"),
            upstream_key=key,
            local_token=local_token,
            host=os.getenv("CODEX_OPENCODE_HOST", "127.0.0.1"),
            port=int(os.getenv("CODEX_OPENCODE_PORT", "4000")),
            timeout_seconds=float(os.getenv("CODEX_OPENCODE_TIMEOUT_SECONDS", "300")),
            first_byte_timeout_seconds=float(
                os.getenv("CODEX_OPENCODE_FIRST_BYTE_TIMEOUT_SECONDS", "45")
            ),
            idle_timeout_seconds=float(
                os.getenv("CODEX_OPENCODE_IDLE_TIMEOUT_SECONDS", "60")
            ),
            max_concurrency=int(os.getenv("CODEX_OPENCODE_MAX_CONCURRENCY", "2")),
            state_db=os.getenv(
                "CODEX_OPENCODE_STATE_DB", ".codex-opencode-state.sqlite3"
            ),
            state_ttl_seconds=int(os.getenv("CODEX_OPENCODE_STATE_TTL_SECONDS", "21600")),
            model_cache_ttl_seconds=int(
                os.getenv("CODEX_OPENCODE_MODEL_CACHE_TTL_SECONDS", "3600")
            ),
            max_request_bytes=int(
                os.getenv("CODEX_OPENCODE_MAX_REQUEST_BYTES", "10485760")
            ),
        )
