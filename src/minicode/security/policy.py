"""Permission policy: config-driven gate consulted before every tool run."""

from __future__ import annotations

import enum
from typing import Any, Protocol

from pydantic import BaseModel


class PolicyBehavior(str, enum.Enum):
    ALLOW = "allow"
    ASK = "ask"
    DENY = "deny"


class PermissionMode(str, enum.Enum):
    """Interactive permission modes (aligned with ZCode / Claude Code).

    ``DEFAULT`` asks per write; ``ACCEPT_EDITS`` auto-allows file edits but
    still asks before running commands; ``BYPASS`` allows everything. The
    three states form the runtime-switchable permission state machine.
    """

    DEFAULT = "default"
    ACCEPT_EDITS = "accept_edits"
    BYPASS = "bypass"


#: Chinese label for each permission mode (status bar / command output).
PERMISSION_MODE_LABELS: dict[PermissionMode, str] = {
    PermissionMode.DEFAULT: "默认（写操作逐个审批）",
    PermissionMode.ACCEPT_EDITS: "自动接受编辑",
    PermissionMode.BYPASS: "全部允许（跳过审批）",
}


def parse_permission_mode(text: str) -> PermissionMode | None:
    """Parse *text* into a :class:`PermissionMode`, or ``None`` when invalid.

    Tolerates ``accept-edits`` / ``ACCEPT_EDITS`` style spellings so the
    ``/permissions`` command is forgiving about separators and case.
    """
    normalized = text.strip().lower().replace("-", "_")
    try:
        return PermissionMode(normalized)
    except ValueError:
        return None


def permission_mode_lines() -> list[str]:
    """Indented ``<mode>  <label>`` lines for the ``/permissions`` listing."""
    return [
        f"  {mode.value:<14}{PERMISSION_MODE_LABELS[mode]}" for mode in PermissionMode
    ]


class PolicyDecision(BaseModel):
    behavior: PolicyBehavior
    reason: str | None = None


class PermissionPolicy(Protocol):
    """Anything usable as the runtime's permission gate."""

    async def check(self, tool_name: str, arguments: dict[str, Any]) -> PolicyDecision:
        """Decide whether *tool_name* may run with *arguments*."""
        ...


#: Tools that only read workspace state: always safe to auto-allow.
_READ_TOOLS = frozenset({"read", "ls", "grep"})
#: Tools that mutate files; ``bash`` is the escape hatch and never auto-allowed.
_EDIT_TOOLS = frozenset({"edit", "write"})


def _rule_decision(tool_name: str) -> PolicyDecision:
    """Built-in per-tool behavior: reads ALLOW, writes/shell ASK, rest DENY."""
    if tool_name in _READ_TOOLS:
        return PolicyDecision(behavior=PolicyBehavior.ALLOW, reason=f"read-only tool: {tool_name}")
    if tool_name in _EDIT_TOOLS:
        return PolicyDecision(
            behavior=PolicyBehavior.ASK, reason=f"file mutation tool: {tool_name}"
        )
    if tool_name == "bash":
        return PolicyDecision(behavior=PolicyBehavior.ASK, reason="shell command")
    return PolicyDecision(
        behavior=PolicyBehavior.DENY,
        reason=f"no rule for {tool_name!r}; applying policy default",
    )


class DefaultPolicy:
    """Config-driven permission gate.

    Built-in defaults (overridable via ``rules``): reads -> ALLOW,
    edit / write / bash -> ASK. Anything unconfigured uses ``default``
    (DENY by default — unknown means no).
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
        decision = _rule_decision(tool_name)
        if decision.behavior is PolicyBehavior.DENY:
            return PolicyDecision(behavior=self._default, reason=decision.reason)
        return decision


class ModePolicy:
    """Permission gate with a runtime-switchable :class:`PermissionMode`.

    ``DEFAULT`` follows the built-in per-tool rules; ``ACCEPT_EDITS``
    additionally auto-allows the file mutation tools (shell still asks);
    ``BYPASS`` allows everything. ``set_mode`` is the single state
    transition, so a UI can switch modes live and read :attr:`mode` for
    the status bar.
    """

    def __init__(self, mode: PermissionMode = PermissionMode.DEFAULT) -> None:
        self._mode = mode

    @property
    def mode(self) -> PermissionMode:
        """The currently active mode (status bar / prompt display)."""
        return self._mode

    def set_mode(self, mode: PermissionMode) -> PermissionMode:
        """Switch to *mode* and return the previous one (state transition)."""
        previous = self._mode
        self._mode = mode
        return previous

    async def check(self, tool_name: str, arguments: dict[str, Any]) -> PolicyDecision:
        if self._mode is PermissionMode.BYPASS:
            return PolicyDecision(
                behavior=PolicyBehavior.ALLOW,
                reason="permission mode bypass: everything allowed",
            )
        decision = _rule_decision(tool_name)
        if (
            self._mode is PermissionMode.ACCEPT_EDITS
            and tool_name in _EDIT_TOOLS
            and decision.behavior is PolicyBehavior.ASK
        ):
            return PolicyDecision(
                behavior=PolicyBehavior.ALLOW,
                reason="permission mode accept_edits: file edits auto-allowed",
            )
        return decision


class AutoAllowPolicy:
    """Allow everything — for tests and ``--yes`` demos."""

    async def check(self, tool_name: str, arguments: dict[str, Any]) -> PolicyDecision:
        return PolicyDecision(
            behavior=PolicyBehavior.ALLOW,
            reason="auto-allow: all tools permitted",
        )
