from codex_opencode_adapter.models import ModelCapabilities, strip_model_prefix


def test_prefix_is_required():
    assert strip_model_prefix("opencode-go/deepseek-v4-pro") == "deepseek-v4-pro"
    try:
        strip_model_prefix("deepseek-v4-pro")
    except ValueError as error:
        assert "opencode-go/" in str(error)
    else:
        raise AssertionError("missing prefix was accepted")


def test_verified_reasoning_effort_is_mapped():
    registry = ModelCapabilities(lambda: {"data": []})
    decision = registry.reasoning_decision(
        "opencode-go/deepseek-v4-flash", {"effort": "high"}
    )
    assert decision.applied is True
    assert decision.parameter == {"reasoning_effort": "high"}


def test_undeclared_effort_is_not_guessed():
    registry = ModelCapabilities(
        lambda: {
            "data": [
                {
                    "id": "kimi-k2.6",
                    "capabilities": {"reasoning": True},
                    "variants": {},
                }
            ]
        }
    )
    decision = registry.reasoning_decision("opencode-go/kimi-k2.6", "high")
    assert decision.applied is False
    assert decision.reason == "effort_not_declared_by_model"
    assert decision.parameter == {}


def test_unknown_variant_protocol_is_not_guessed():
    registry = ModelCapabilities(
        lambda: {
            "data": [
                {
                    "id": "minimax-m3",
                    "capabilities": {"reasoning": True},
                    "variants": {"thinking": {"thinking": {"type": "adaptive"}}},
                }
            ]
        }
    )
    decision = registry.reasoning_decision("opencode-go/minimax-m3", "thinking")
    assert decision.applied is False
    assert decision.reason == "unsupported_variant_protocol"


def test_verified_profiles_survive_models_endpoint_failure():
    def fail():
        raise RuntimeError("offline")

    registry = ModelCapabilities(fail)
    decision = registry.reasoning_decision(
        "opencode-go/mimo-v2.5-pro", {"effort": "high"}
    )
    assert decision.applied is True
