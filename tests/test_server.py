import http.client
import json
import threading
from http.server import ThreadingHTTPServer

from codex_opencode_adapter.config import Config
from codex_opencode_adapter.server import Handler


class FakeAdapter:
    def __init__(self):
        self.config = Config(
            upstream_base="https://example.invalid/v1",
            upstream_key="upstream",
            local_token="local",
            host="127.0.0.1",
            port=0,
            timeout_seconds=5,
            first_byte_timeout_seconds=5,
            idle_timeout_seconds=5,
            max_concurrency=1,
            state_db="unused.db",
            state_ttl_seconds=60,
            model_cache_ttl_seconds=60,
            max_request_bytes=10000,
        )

    def complete(self, body):
        return {
            "id": "resp_mock",
            "object": "response",
            "status": "completed",
            "model": body["model"],
            "output": [],
        }

    def stream(self, body, emit):
        response = self.complete(body)
        emit("response.completed", {"type": "response.completed", "response": response})
        return response


def request(server, method, path, body=None, token=None):
    connection = http.client.HTTPConnection("127.0.0.1", server.server_port, timeout=5)
    headers = {}
    if token:
        headers["Authorization"] = f"Bearer {token}"
    encoded = None
    if body is not None:
        encoded = json.dumps(body).encode()
        headers["Content-Type"] = "application/json"
        headers["Content-Length"] = str(len(encoded))
    connection.request(method, path, body=encoded, headers=headers)
    response = connection.getresponse()
    data = response.read()
    connection.close()
    return response.status, response.getheader("Content-Type"), data


def test_http_auth_json_and_sse():
    Handler.adapter = FakeAdapter()
    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        status, _, _ = request(
            server,
            "POST",
            "/v1/responses",
            {"model": "opencode-go/test", "input": "x"},
        )
        assert status == 401

        status, content_type, data = request(
            server,
            "POST",
            "/v1/responses",
            {"model": "opencode-go/test", "input": "x"},
            token="local",
        )
        assert status == 200
        assert content_type == "application/json"
        assert json.loads(data)["id"] == "resp_mock"

        status, content_type, data = request(
            server,
            "POST",
            "/v1/responses",
            {"model": "opencode-go/test", "input": "x", "stream": True},
            token="local",
        )
        assert status == 200
        assert content_type == "text/event-stream"
        assert b"event: response.completed" in data
        assert data.endswith(b"data: [DONE]\n\n")
    finally:
        server.shutdown()
        server.server_close()

