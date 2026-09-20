"""Best-effort discovery of the local ZCode gateway configuration.

minicode can reuse the CommandCode (ZCode) gateway credentials the user has
already configured on this machine. This module performs a *courtesy
discovery* only:

- first the environment variables ``COMMANDCODE_API_KEY`` (required) and
  ``COMMANDCODE_BASE_URL`` (optional, defaults to :data:`DEFAULT_BASE_URL`);
- otherwise the machine-local ZCode CLI config file
  ``~/.zcode/v2/provider_config.json``.

Credentials can equivalently be provided purely through environment
variables — the config file is never required. The discovery is strictly
read-only and defensive: it never raises, never logs or prints the key,
and returns ``None`` whenever anything looks off (missing file, unexpected
structure, missing/empty fields), letting the caller decide how to
surface the problem.
"""

from __future__ import annotations

import json
import os
from pathlib import Path
from typing import Any

#: Default OpenAI-compatible endpoint of the ZCode (CommandCode) gateway.
DEFAULT_BASE_URL = "https://api.commandcode.ai/provider/v1"

#: Default chat model served by the gateway.
DEFAULT_MODEL = "deepseek/deepseek-v4.1-flash"

#: Relative location of the ZCode CLI config inside the user's home directory.
_CONFIG_PARTS = (".zcode", "v2", "provider_config.json")

#: ``providerName`` identifying the CommandCode rule inside the config file.
_COMMAND_CODE_RULE_NAME = "Command Code"

#: ``config.api.type`` value identifying OpenAI chat-completions rules.
_OPENAI_TYPE = "openai-chat-completions"


def _clean(value: Any) -> str | None:
    """Return *value* as a non-empty stripped string, else ``None``."""
    if not isinstance(value, str):
        return None
    value = value.strip()
    return value or None


def _extract_rule(rule: dict[str, Any]) -> tuple[str, str] | None:
    """Pull ``(base_url, api_key)`` out of one ``providerRules`` entry.

    Returns ``None`` when the rule's shape is unexpected or a needed
    field is missing/empty. Never raises and never leaks the key.
    """
    config = rule.get("config")
    if not isinstance(config, dict):
        return None
    api = config.get("api")
    access = config.get("access")
    if not isinstance(api, dict) or not isinstance(access, dict):
        return None
    base_url = _clean(api.get("baseUrl"))
    api_key = _clean(access.get("apiKey"))
    if base_url is None or api_key is None:
        return None
    return (base_url.rstrip("/"), api_key)


def _discover_from_config_file() -> tuple[str, str] | None:
    """Read ``~/.zcode/v2/provider_config.json`` and extract the gateway.

    Preference order among the ``providerRules`` entries:

    1. the rule with ``providerName == "Command Code"``;
    2. otherwise the first rule whose ``config.api.type`` is
       ``"openai-chat-completions"``.

    Any problem (missing file, unparsable JSON, unexpected structure,
    missing/empty ``baseUrl``/``apiKey``) yields ``None``.
    """
    path = Path.home().joinpath(*_CONFIG_PARTS)
    raw = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(raw, dict):
        return None
    config = raw.get("config")
    if not isinstance(config, dict):
        return None
    rules_container = config.get("providerConfigRules")
    if not isinstance(rules_container, dict):
        return None
    rules = rules_container.get("providerRules")
    if not isinstance(rules, list):
        return None

    command_rule: dict[str, Any] | None = None
    openai_rule: dict[str, Any] | None = None
    for rule in rules:
        if not isinstance(rule, dict):
            continue
        if rule.get("providerName") == _COMMAND_CODE_RULE_NAME:
            command_rule = rule
            break
        if openai_rule is None:
            rule_config = rule.get("config")
            api = rule_config.get("api") if isinstance(rule_config, dict) else None
            if isinstance(api, dict) and api.get("type") == _OPENAI_TYPE:
                openai_rule = rule

    chosen = command_rule if command_rule is not None else openai_rule
    if chosen is None:
        return None
    return _extract_rule(chosen)


def discover_commandcode() -> tuple[str, str] | None:
    """Return ``(base_url, api_key)`` for the CommandCode gateway, or ``None``.

    Resolution order:

    1. Environment: ``COMMANDCODE_API_KEY`` (with ``COMMANDCODE_BASE_URL``
       defaulting to :data:`DEFAULT_BASE_URL`) — preferred when present;
    2. the local ZCode config file ``~/.zcode/v2/provider_config.json``
       (see :func:`_discover_from_config_file`).

    This is a best-effort, read-only discovery of machine-local ZCode
    configuration; credentials may equivalently be supplied entirely via
    environment variables. Never raises, never prints the key.
    """
    env_key = _clean(os.environ.get("COMMANDCODE_API_KEY"))
    if env_key is not None:
        env_base = _clean(os.environ.get("COMMANDCODE_BASE_URL")) or DEFAULT_BASE_URL
        return (env_base.rstrip("/"), env_key)

    try:
        return _discover_from_config_file()
    except Exception:
        # Missing file, permission error, unparsable JSON, wrong structure...
        # Discovering credentials is best-effort: never raise, never log.
        return None
