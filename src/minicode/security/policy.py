"""Permission policy: config-driven gate consulted before every tool run."""

from __future__ import annotations

import enum
from typing import Any, Protocol

from pydantic import BaseModel


class PolicyBehavior(str, enum.Enum):
    ALLOW = "allow"
    ASK = "ask"
    DENY = "deny"


class PolicyDecision(BaseModel):
    behavior: PolicyBehavior
    reason: str | None = None


class PermissionPolicy(Protocol):
    """Anything usable as the runtime's permission gate."""

    async def check(self, tool_name: str, arguments: dict[str, Any]) -> PolicyDecision:
        """Decide whether *tool_name* may run with *arguments*."""
        ...


_BUILTIN_RULES: dict[str, PolicyBehavior] = {
    "read_file": PolicyBehavior.ALLOW,
    "list_files": PolicyBehavior.ALLOW,
    "search_text": PolicyBehavior.ALLOW,
    "apply_patch": PolicyBehavior.ASK,
    "run_command": PolicyBehavior.ASK,
}


class DefaultPolicy:
    """Config-driven permission gate.

    Built-in defaults (overridable via ``rules``): read_file / list_files /
    search_text -> ALLOW; apply_patch / run_command -> ASK. Anything
    unconfigured uses ``default`` (DENY by default — unknown means no).
    """

    def __init__(
        self,
        rules: dict[str, PolicyBehavior] | None = None,
        default: PolicyBehavior = PolicyBehavior.DENY,
    ) -> None:
        self._overrides: dict[str, PolicyBehavior] = dict(rules or {})
        self._default = default

    async def check(self, tool_name: str, arguments: dict[str, Any]) -> PolicyDecision:
        if tool_name in self._overrides:
            return PolicyDecision(
                behavior=self._overrides[tool_name],
                reason=f"configured rule for {tool_name}",
            )
        if tool_name in _BUILTIN_RULES:
            return PolicyDecision(
                behavior=_BUILTIN_RULES[tool_name],
                reason=f"built-in default for {tool_name}",
            )
        return PolicyDecision(
            behavior=self._default,
            reason=f"no rule for {tool_name!r}; applying policy default",
        )


class AutoAllowPolicy:
    """Allow everything — for tests and ``--yes`` demos."""

    async def check(self, tool_name: str, arguments: dict[str, Any]) -> PolicyDecision:
        return PolicyDecision(
            behavior=PolicyBehavior.ALLOW,
            reason="auto-allow: all tools permitted",
        )
