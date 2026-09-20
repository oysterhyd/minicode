"""Tests for the model catalog (context windows, effort levels, fallbacks)."""

from __future__ import annotations

from minicode.core.catalog import (
    DEFAULT_MODEL,
    EFFORT_LEVELS,
    MODEL_CATALOG,
    lookup_model,
    models_for_provider,
    parse_effort,
)


def test_catalog_known_models_have_one_million_windows():
    deepseek = MODEL_CATALOG[DEFAULT_MODEL]
    glm = MODEL_CATALOG["z.ai/glm-5.3-flash"]
    assert deepseek.provider == "commandcode"
    assert glm.provider == "commandcode"
    assert deepseek.context_window == 1_000_000
    assert glm.context_window == 1_000_000
    assert deepseek.supports_effort and glm.supports_effort


def test_claude_entry_has_no_effort_and_200k_window():
    claude = MODEL_CATALOG["claude-sonnet-4-5"]
    assert claude.provider == "anthropic"
    assert claude.context_window == 200_000
    assert claude.supports_effort is False


def test_lookup_model_unknown_gets_conservative_fallback():
    info = lookup_model("totally/unknown-model")
    assert info.name == "totally/unknown-model"
    assert info.context_window == 200_000


def test_models_for_provider_filters_by_provider():
    commandcode_models = {info.name for info in models_for_provider("commandcode")}
    assert "z.ai/glm-5.3-flash" in commandcode_models
    assert "claude-sonnet-4-5" not in commandcode_models


def test_parse_effort_accepts_levels_and_rejects_garbage():
    assert parse_effort("LOW") == "low"
    assert parse_effort(" off ") == "off"
    assert parse_effort("medium") == "medium"
    assert parse_effort("maximum") is None
    assert parse_effort("") is None
    assert set(EFFORT_LEVELS) == {"off", "low", "medium", "high"}
