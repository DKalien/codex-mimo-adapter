from codex_opencode_adapter.config import Config
from codex_opencode_adapter.server import Adapter
from codex_opencode_adapter.state import StateStore


class FakeClient:
    def __init__(self):
        self.payloads = []

    def models(self):
        return {
            "data": [
                {
                    "id": "deepseek-v4-flash",
                    "capabilities": {"reasoning": True},
                    "variants": {
                        "low": {"reasoningEffort": "low"},
                        "high": {"reasoningEffort": "high"},
                    },
                }
            ]
        }

    def chat(self, payload):
        self.payloads.append(payload)
        return {
            "choices": [{"finish_reason": "stop", "message": {"content": "42"}}],
            "usage": {"prompt_tokens": 2, "completion_tokens": 1, "total_tokens": 3},
        }


class FailingStreamClient(FakeClient):
    def chat_stream(self, payload):
        yield {"choices": [{"delta": {"content": "partial"}}]}
        from codex_opencode_adapter.upstream import UpstreamError

        raise UpstreamError(502, "upstream disconnected")


def config(tmp_path):
    return Config(
        upstream_base="https://example.invalid/v1",
        upstream_key="upstream-secret",
        local_token="local-secret",
        host="127.0.0.1",
        port=0,
        timeout_seconds=5,
        first_byte_timeout_seconds=5,
        idle_timeout_seconds=5,
        max_concurrency=1,
        state_db=str(tmp_path / "state.db"),
        state_ttl_seconds=3600,
        model_cache_ttl_seconds=3600,
        max_request_bytes=10000,
    )


def test_adapter_applies_reasoning_and_returns_audit(tmp_path):
    client = FakeClient()
    cfg = config(tmp_path)
    adapter = Adapter(
        cfg, client=client, state=StateStore(cfg.state_db, cfg.state_ttl_seconds)
    )
    result = adapter.complete(
        {
            "model": "opencode-go/deepseek-v4-flash",
            "input": "17+25",
            "reasoning": {"effort": "high"},
        }
    )
    assert client.payloads[0]["reasoning_effort"] == "high"
    assert result["metadata"]["adapter"]["reasoning_applied"] is True
    assert result["output"][0]["content"][0]["text"] == "42"


def test_adapter_rejects_non_prefixed_model(tmp_path):
    cfg = config(tmp_path)
    adapter = Adapter(
        cfg, client=FakeClient(), state=StateStore(cfg.state_db, cfg.state_ttl_seconds)
    )
    try:
        adapter.complete({"model": "deepseek-v4-flash", "input": "hello"})
    except ValueError as error:
        assert "opencode-go/" in str(error)
    else:
        raise AssertionError("unprefixed model accepted")


def test_adapter_stream_emits_failed_terminal_on_upstream_disconnect(tmp_path):
    client = FailingStreamClient()
    cfg = config(tmp_path)
    adapter = Adapter(
        cfg, client=client, state=StateStore(cfg.state_db, cfg.state_ttl_seconds)
    )
    events = []
    result = adapter.stream(
        {
            "model": "opencode-go/deepseek-v4-flash",
            "input": "hello",
            "stream": True,
        },
        lambda event, data: events.append((event, data)),
    )
    assert result["status"] == "failed"
    assert events[-1][0] == "response.failed"
    assert not any(event == "response.completed" for event, _ in events)


def test_config_rejects_reusing_upstream_key_as_local_token(monkeypatch):
    monkeypatch.setenv("OPENCODE_GO_API_KEY", "same-secret")
    monkeypatch.setenv("CODEX_OPENCODE_LOCAL_TOKEN", "same-secret")
    try:
        Config.from_env()
    except RuntimeError as error:
        assert "must differ" in str(error)
    else:
        raise AssertionError("reused upstream key was accepted")


def test_default_port_avoids_legacy_bridge(monkeypatch):
    monkeypatch.setenv("OPENCODE_GO_API_KEY", "upstream-secret")
    monkeypatch.delenv("CODEX_OPENCODE_LOCAL_TOKEN", raising=False)
    monkeypatch.delenv("CODEX_OPENCODE_PORT", raising=False)
    assert Config.from_env().port == 4010
