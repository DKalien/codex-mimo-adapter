from __future__ import annotations

import json
import logging
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from typing import Any

from .config import Config
from .models import ModelCapabilities, strip_model_prefix
from .protocol import (
    HistoryError,
    StreamAssembler,
    build_chat_payload,
    build_response,
    function_output_call_ids,
)
from .state import StateStore
from .upstream import OpenCodeGoClient, UpstreamError


JSON = dict[str, Any]
LOG = logging.getLogger("codex_opencode_adapter")


class Adapter:
    def __init__(
        self,
        config: Config,
        *,
        client: OpenCodeGoClient | None = None,
        state: StateStore | None = None,
    ):
        self.config = config
        self.client = client or OpenCodeGoClient(
            config.upstream_base, config.upstream_key, config.timeout_seconds
        )
        self.state = state or StateStore(config.state_db, config.state_ttl_seconds)
        self.models = ModelCapabilities(
            self.client.models, config.model_cache_ttl_seconds
        )
        self.capacity = threading.BoundedSemaphore(config.max_concurrency)

    def prepare(self, body: JSON) -> tuple[JSON, list[JSON], dict[str, str], str, JSON]:
        model_alias = str(body.get("model") or "")
        model_upstream = strip_model_prefix(model_alias)
        previous_id = str(body.get("previous_response_id") or "")
        previous = self.state.get(previous_id) if previous_id else None
        if previous is None:
            call_ids = function_output_call_ids(body)
            if call_ids:
                previous = self.state.find_by_call_ids(call_ids)

        effort = body.get("reasoning")
        if effort is None:
            effort = body.get("reasoning_effort")
        decision = self.models.reasoning_decision(model_alias, effort)
        payload, messages, reverse = build_chat_payload(
            body,
            model_upstream=model_upstream,
            previous=previous,
            reasoning_parameter=decision.parameter,
        )
        audit = {
            "model": model_upstream,
            "reasoning_requested": decision.requested,
            "reasoning_applied": decision.applied,
            "reasoning_reason": decision.reason,
        }
        LOG.info("request_prepared %s", json.dumps(audit, separators=(",", ":")))
        return payload, messages, reverse, model_upstream, audit

    def complete(self, body: JSON) -> JSON:
        payload, messages, reverse, upstream_model, audit = self.prepare(body)
        if not self.capacity.acquire(blocking=False):
            raise UpstreamError(429, "adapter concurrency limit reached")
        try:
            response = self.client.chat(payload)
        finally:
            self.capacity.release()
        result = build_response(
            body,
            response,
            model_alias=str(body["model"]),
            model_upstream=upstream_model,
            base_messages=messages,
            reverse_names=reverse,
            state_put=self.state.put,
        )
        result["metadata"] = {**result.get("metadata", {}), "adapter": audit}
        return result

    def stream(self, body: JSON, emit) -> JSON:
        payload, messages, reverse, upstream_model, audit = self.prepare(body)
        assembler = StreamAssembler(
            body=body,
            model_alias=str(body["model"]),
            model_upstream=upstream_model,
            base_messages=messages,
            reverse_names=reverse,
            state_put=self.state.put,
            emit=emit,
        )
        assembler.start()
        if not self.capacity.acquire(blocking=False):
            raise UpstreamError(429, "adapter concurrency limit reached")
        try:
            for chunk in self.client.chat_stream(payload):
                assembler.accept(chunk)
        finally:
            self.capacity.release()
        response = assembler.finalize()
        response["metadata"] = {**response.get("metadata", {}), "adapter": audit}
        return response


class Handler(BaseHTTPRequestHandler):
    adapter: Adapter
    server_version = "codex-opencode-adapter/0.1"

    def do_GET(self) -> None:  # noqa: N802
        if self.path.rstrip("/") == "/health":
            self._json(200, {"status": "ok"})
            return
        if self.path.rstrip("/") in {"/v1/models", "/models"}:
            if not self._authorized():
                return
            rows = [
                {
                    "id": f"opencode-go/{row['id']}",
                    "object": "model",
                    "owned_by": "opencode-go",
                }
                for row in self.adapter.models.all()
                if row.get("id")
            ]
            self._json(200, {"object": "list", "data": rows})
            return
        self._error(404, "not_found", f"Unknown path: {self.path}")

    def do_POST(self) -> None:  # noqa: N802
        if self.path.rstrip("/") not in {"/v1/responses", "/responses"}:
            self._error(404, "not_found", f"Unknown path: {self.path}")
            return
        if not self._authorized():
            return
        try:
            length = int(self.headers.get("Content-Length", "0"))
            if length <= 0 or length > self.adapter.config.max_request_bytes:
                self._error(413, "request_too_large", "Invalid request size")
                return
            body = json.loads(self.rfile.read(length).decode("utf-8"))
            if not isinstance(body, dict):
                raise ValueError("request body must be an object")
            if body.get("stream"):
                self._start_sse()
                self.adapter.stream(body, self._sse)
                self._write(b"data: [DONE]\n\n")
            else:
                self._json(200, self.adapter.complete(body))
        except (ValueError, HistoryError) as error:
            self._error(400, "invalid_request_error", str(error))
        except UpstreamError as error:
            self._error(error.status, "upstream_error", error.message)
        except (BrokenPipeError, ConnectionResetError):
            self.close_connection = True
        except Exception:
            LOG.exception("request_failed")
            self._error(500, "internal_error", "Adapter internal error")

    def log_message(self, fmt: str, *args: Any) -> None:
        LOG.info("%s - %s", self.address_string(), fmt % args)

    def _authorized(self) -> bool:
        token = self.adapter.config.local_token
        if not token:
            return True
        header = self.headers.get("Authorization", "")
        if header == f"Bearer {token}":
            return True
        self._error(401, "unauthorized", "Unauthorized")
        return False

    def _start_sse(self) -> None:
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Cache-Control", "no-cache")
        self.send_header("Connection", "close")
        self.end_headers()

    def _sse(self, event: str, data: JSON) -> None:
        payload = json.dumps(data, ensure_ascii=False, separators=(",", ":"))
        self._write(f"event: {event}\ndata: {payload}\n\n".encode("utf-8"))

    def _write(self, data: bytes) -> None:
        self.wfile.write(data)
        self.wfile.flush()

    def _json(self, status: int, payload: JSON) -> None:
        data = json.dumps(payload, ensure_ascii=False).encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self._write(data)

    def _error(self, status: int, kind: str, message: str) -> None:
        self._json(status, {"error": {"type": kind, "message": message}})


def create_server(config: Config) -> ThreadingHTTPServer:
    Handler.adapter = Adapter(config)
    return ThreadingHTTPServer((config.host, config.port), Handler)


def main() -> None:
    logging.basicConfig(level=logging.INFO, format="%(asctime)s %(levelname)s %(message)s")
    config = Config.from_env()
    server = create_server(config)
    LOG.info("listening on http://%s:%s", config.host, config.port)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        server.server_close()
