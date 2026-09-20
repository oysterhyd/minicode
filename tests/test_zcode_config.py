"""Tests for :mod:`minicode.providers.zcode_config` (credential discovery).

Every test redirects ``Path.home`` into a temporary directory and clears the
``COMMANDCODE_*`` environment variables first, so the real
``~/.zcode/v2/provider_config.json`` is never read and no real credential
can leak into (or out of) the tests.
"""

from __future__ import annotations

import json
from pathlib import Path
from typing import Any

import pytest

from minicode.providers.zcode_config import (
    DEFAULT_BASE_URL,
    DEFAULT_MODEL,
    discover_commandcode,
)


# ---------------------------------------------------------------------------
# Isolation + config file scaffolding
# ---------------------------------------------------------------------------


@pytest.fixture(autouse=True)
def _isolated_home(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(Path, "home", lambda: tmp_path)
    monkeypatch.delenv("COMMANDCODE_API_KEY", raising=False)
    monkeypatch.delenv("COMMANDCODE_BASE_URL", raising=False)


def write_config(tmp_path: Path, rules: list[dict[str, Any]]) -> Path:
    """Write a fake ``~/.zcode/v2/provider_config.json`` and return its path."""
    config_dir = tmp_path / ".zcode" / "v2"
    config_dir.mkdir(parents=True, exist_ok=True)
    path = config_dir / "provider_config.json"
    doc = {"config": {"providerConfigRules": {"providerRules": rules}}}
    path.write_text(json.dumps(doc), encoding="utf-8")
    return path


def make_rule(
    *,
    name: str | None = None,
    api_type: str = "openai-chat-completions",
    base_url: str | None = "https://api.commandcode.ai/provider/v1",
    api_key: Any = "cc-key",
    include_api: bool = True,
    include_access: bool = True,
) -> dict[str, Any]:
    """Build one ``providerRules`` entry (fields omitted when ``None``)."""
    rule: dict[str, Any] = {}
    if name is not None:
        rule["providerName"] = name
    config: dict[str, Any] = {}
    if include_api:
        api: dict[str, Any] = {"type": api_type}
        if base_url is not None:
            api["baseUrl"] = base_url
        config["api"] = api
    if include_access:
        access: dict[str, Any] = {}
        if api_key is not None:
            access["apiKey"] = api_key
        config["access"] = access
    rule["config"] = config
    return rule


# ---------------------------------------------------------------------------
# Constants
# ---------------------------------------------------------------------------


def test_default_constants() -> None:
    assert DEFAULT_BASE_URL == "https://api.commandcode.ai/provider/v1"
    assert DEFAULT_MODEL == "deepseek/deepseek-v4.1-flash"


# ---------------------------------------------------------------------------
# Environment variable resolution
# ---------------------------------------------------------------------------


def test_env_key_and_base_url(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    # A decoy config file: env vars must take precedence regardless.
    write_config(tmp_path, [make_rule(name="Command Code", api_key="file-key")])
    monkeypatch.setenv("COMMANDCODE_API_KEY", "env-key")
    monkeypatch.setenv("COMMANDCODE_BASE_URL", "https://env.example/v1")

    assert discover_commandcode() == ("https://env.example/v1", "env-key")


def test_env_key_without_base_url_falls_back_to_default_base_url(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("COMMANDCODE_API_KEY", "env-key")

    assert discover_commandcode() == (DEFAULT_BASE_URL, "env-key")


def test_env_base_url_without_key_falls_through_to_file(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    write_config(tmp_path, [make_rule(name="Command Code", base_url="https://file.example/v1", api_key="file-key")])
    monkeypatch.setenv("COMMANDCODE_BASE_URL", "https://env.example/v1")

    assert discover_commandcode() == ("https://file.example/v1", "file-key")


def test_env_key_whitespace_only_treated_as_unset(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    write_config(tmp_path, [make_rule(name="Command Code", api_key="file-key")])
    monkeypatch.setenv("COMMANDCODE_API_KEY", "   ")

    assert discover_commandcode() == ("https://api.commandcode.ai/provider/v1", "file-key")


# ---------------------------------------------------------------------------
# Config file resolution
# ---------------------------------------------------------------------------


def test_config_prefers_command_code_rule_over_earlier_openai_rule(tmp_path: Path) -> None:
    write_config(
        tmp_path,
        [
            make_rule(name="Generic Gateway", base_url="https://fallback.example/v1", api_key="fallback-key"),
            make_rule(name="Command Code", base_url="https://cc.example/v1", api_key="cc-key"),
        ],
    )

    assert discover_commandcode() == ("https://cc.example/v1", "cc-key")


def test_config_falls_back_to_first_openai_rule(tmp_path: Path) -> None:
    write_config(
        tmp_path,
        [
            make_rule(name="Anthropic", api_type="anthropic", base_url="https://a.example", api_key="a-key"),
            make_rule(base_url="https://first-openai.example/v1", api_key="first-key"),
            make_rule(base_url="https://second-openai.example/v1", api_key="second-key"),
        ],
    )

    assert discover_commandcode() == ("https://first-openai.example/v1", "first-key")


def test_config_normalizes_trailing_slash_and_surrounding_whitespace(tmp_path: Path) -> None:
    write_config(
        tmp_path,
        [make_rule(name="Command Code", base_url=" https://cc.example/v1/ ", api_key="  cc-key  ")],
    )

    assert discover_commandcode() == ("https://cc.example/v1", "cc-key")


# ---------------------------------------------------------------------------
# Failure modes: always None, never raise
# ---------------------------------------------------------------------------


def test_missing_file_returns_none(tmp_path: Path) -> None:
    assert discover_commandcode() is None


def test_unparsable_json_returns_none(tmp_path: Path) -> None:
    path = write_config(tmp_path, [])
    path.write_text("{definitely not json", encoding="utf-8")

    assert discover_commandcode() is None


def test_wrong_structure_returns_none(tmp_path: Path) -> None:
    # Top-level not an object.
    path = write_config(tmp_path, [])
    path.write_text("[1, 2, 3]", encoding="utf-8")
    assert discover_commandcode() is None

    # config missing / not an object.
    path.write_text(json.dumps({"config": "nope"}), encoding="utf-8")
    assert discover_commandcode() is None

    # providerConfigRules missing.
    path.write_text(json.dumps({"config": {}}), encoding="utf-8")
    assert discover_commandcode() is None

    # providerRules not a list.
    path.write_text(
        json.dumps({"config": {"providerConfigRules": {"providerRules": {}}}}),
        encoding="utf-8",
    )
    assert discover_commandcode() is None

    # No usable rule at all.
    path.write_text(
        json.dumps({"config": {"providerConfigRules": {"providerRules": []}}}),
        encoding="utf-8",
    )
    assert discover_commandcode() is None


def test_rule_field_problems_return_none(tmp_path: Path) -> None:
    # Command Code rule chosen, but its apiKey is empty -> None (no fallback).
    write_config(
        tmp_path,
        [
            make_rule(base_url="https://fallback.example/v1", api_key="fallback-key"),
            make_rule(name="Command Code", api_key=""),
        ],
    )
    assert discover_commandcode() is None

    # Missing baseUrl.
    write_config(tmp_path, [make_rule(name="Command Code", base_url=None, api_key="k")])
    assert discover_commandcode() is None

    # Missing access section entirely.
    write_config(tmp_path, [make_rule(name="Command Code", include_access=False)])
    assert discover_commandcode() is None

    # Missing api section entirely.
    write_config(tmp_path, [make_rule(name="Command Code", include_api=False)])
    assert discover_commandcode() is None

    # Non-string fields.
    write_config(tmp_path, [make_rule(name="Command Code", base_url=123, api_key="k")])
    assert discover_commandcode() is None

    write_config(tmp_path, [make_rule(name="Command Code", api_key={"nested": True})])
    assert discover_commandcode() is None


def test_config_path_being_a_directory_returns_none(tmp_path: Path) -> None:
    # The config "file" is actually a directory: reading raises IsADirectoryError,
    # which must be swallowed into None.
    (tmp_path / ".zcode" / "v2" / "provider_config.json").mkdir(parents=True)

    assert discover_commandcode() is None
