import json

from codex_opencode_adapter.upstream import OpenCodeGoClient


class FakeResponse:
    def __init__(self, lines):
        self.lines = [line.encode("utf-8") for line in lines]

    def __enter__(self):
        return self

    def __exit__(self, *_):
        return False

    def __iter__(self):
        return iter(self.lines)


def test_upstream_uses_bearer_key_without_putting_it_in_url():
    client = OpenCodeGoClient("https://example.test/v1", "secret-key")
    headers = client._headers("application/json")
    assert headers["Authorization"] == "Bearer secret-key"
    assert "secret-key" not in client.base_url


def test_stream_parser_handles_sse_chunks(monkeypatch):
    chunks = [
        {"choices": [{"delta": {"content": "hel"}}]},
        {"choices": [{"delta": {"content": "lo"}, "finish_reason": "stop"}]},
    ]
    lines = [
        f"data: {json.dumps(chunks[0])}\n",
        "\n",
        f"data: {json.dumps(chunks[1])}\n",
        "\n",
        "data: [DONE]\n",
        "\n",
    ]
    monkeypatch.setattr(
        "urllib.request.urlopen", lambda *args, **kwargs: FakeResponse(lines)
    )
    client = OpenCodeGoClient("https://example.test/v1", "secret-key")
    assert list(client.chat_stream({"model": "x", "messages": []})) == chunks

