from __future__ import annotations

import json
import socket
import urllib.error
import urllib.request
from typing import Any, Iterator


JSON = dict[str, Any]


class UpstreamError(RuntimeError):
    def __init__(self, status: int, message: str):
        super().__init__(message)
        self.status = status
        self.message = message


class OpenCodeGoClient:
    def __init__(self, base_url: str, api_key: str, timeout_seconds: float = 300):
        self.base_url = base_url.rstrip("/")
        self.api_key = api_key
        self.timeout_seconds = timeout_seconds

    def models(self) -> JSON:
        return self._request_json("GET", "/models")

    def chat(self, payload: JSON) -> JSON:
        request = dict(payload)
        request["stream"] = False
        return self._request_json("POST", "/chat/completions", request)

    def chat_stream(self, payload: JSON) -> Iterator[JSON]:
        request = dict(payload)
        request["stream"] = True
        raw = json.dumps(request, ensure_ascii=False).encode("utf-8")
        req = urllib.request.Request(
            f"{self.base_url}/chat/completions",
            data=raw,
            headers=self._headers("text/event-stream"),
            method="POST",
        )
        try:
            with urllib.request.urlopen(req, timeout=self.timeout_seconds) as response:
                data_lines: list[str] = []
                for raw_line in response:
                    line = raw_line.decode("utf-8", errors="replace").rstrip("\r\n")
                    if not line:
                        if data_lines:
                            data = "\n".join(data_lines)
                            data_lines = []
                            if data == "[DONE]":
                                return
                            parsed = json.loads(data)
                            if isinstance(parsed, dict):
                                yield parsed
                        continue
                    if line.startswith("data:"):
                        data_lines.append(line[5:].lstrip())
                if data_lines:
                    data = "\n".join(data_lines)
                    if data != "[DONE]":
                        parsed = json.loads(data)
                        if isinstance(parsed, dict):
                            yield parsed
        except urllib.error.HTTPError as error:
            raise self._http_error(error) from None
        except (urllib.error.URLError, socket.timeout, TimeoutError) as error:
            raise UpstreamError(502, f"OpenCode Go network error: {error}") from None

    def _request_json(
        self, method: str, path: str, payload: JSON | None = None
    ) -> JSON:
        data = (
            json.dumps(payload, ensure_ascii=False).encode("utf-8")
            if payload is not None
            else None
        )
        req = urllib.request.Request(
            f"{self.base_url}{path}",
            data=data,
            headers=self._headers("application/json"),
            method=method,
        )
        try:
            with urllib.request.urlopen(req, timeout=self.timeout_seconds) as response:
                parsed = json.loads(response.read().decode("utf-8"))
                if not isinstance(parsed, dict):
                    raise UpstreamError(502, "OpenCode Go returned a non-object response")
                return parsed
        except urllib.error.HTTPError as error:
            raise self._http_error(error) from None
        except (urllib.error.URLError, socket.timeout, TimeoutError) as error:
            raise UpstreamError(502, f"OpenCode Go network error: {error}") from None

    def _headers(self, accept: str) -> dict[str, str]:
        return {
            "Authorization": f"Bearer {self.api_key}",
            "Content-Type": "application/json",
            "Accept": accept,
            "User-Agent": "codex-opencode-adapter/0.1",
        }

    @staticmethod
    def _http_error(error: urllib.error.HTTPError) -> UpstreamError:
        body = error.read().decode("utf-8", errors="replace")
        try:
            parsed = json.loads(body)
            message = str((parsed.get("error") or {}).get("message") or body)
        except Exception:
            message = body
        return UpstreamError(error.code, message[:2000])

