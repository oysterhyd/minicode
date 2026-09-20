"""Tests for minicode.security.policy."""

from __future__ import annotations

import asyncio

from minicode.security.policy import (
    AutoAllowPolicy,
    DefaultPolicy,
    ModePolicy,
    PermissionMode,
    PolicyBehavior,
    PolicyDecision,
    parse_permission_mode,
)


def check(policy, tool_name, arguments=None):
    return asyncio.run(policy.check(tool_name, arguments or {}))


def test_default_policy_read_only_tools_allowed():
    policy = DefaultPolicy()
    for tool_name in ("read", "ls", "grep"):
        decision = check(policy, tool_name)
        assert isinstance(decision, PolicyDecision)
        assert decision.behavior is PolicyBehavior.ALLOW


def test_default_policy_mutating_tools_ask():
    policy = DefaultPolicy()
    for tool_name in ("edit", "write", "bash"):
        decision = check(policy, tool_name)
        assert decision.behavior is PolicyBehavior.ASK


def test_default_policy_unknown_tool_denied():
    policy = DefaultPolicy()
    decision = check(policy, "mystery_tool")
    assert decision.behavior is PolicyBehavior.DENY


def test_default_policy_rules_override_builtin():
    policy = DefaultPolicy(rules={"edit": PolicyBehavior.DENY})
    assert check(policy, "edit").behavior is PolicyBehavior.DENY
    # other built-ins keep their defaults
    assert check(policy, "read").behavior is PolicyBehavior.ALLOW
    assert check(policy, "bash").behavior is PolicyBehavior.ASK


def test_default_policy_rules_add_new_tool():
    policy = DefaultPolicy(rules={"web_fetch": PolicyBehavior.ASK})
    assert check(policy, "web_fetch").behavior is PolicyBehavior.ASK
    # unknown tools without a rule still hit the default
    assert check(policy, "other_tool").behavior is PolicyBehavior.DENY


def test_default_policy_custom_default():
    policy = DefaultPolicy(default=PolicyBehavior.ALLOW)
    assert check(policy, "anything").behavior is PolicyBehavior.ALLOW
    # built-ins are unchanged by a custom default
    assert check(policy, "edit").behavior is PolicyBehavior.ASK


def test_auto_allow_policy_allows_everything():
    policy = AutoAllowPolicy()
    for tool_name in ("read", "edit", "bash", "totally_unknown"):
        decision = check(policy, tool_name, {"path": "x"})
        assert decision.behavior is PolicyBehavior.ALLOW


# ---------------------------------------------------------------------------
# ModePolicy: the 3-mode permission state machine
# ---------------------------------------------------------------------------


def test_mode_policy_default_follows_builtin_rules():
    policy = ModePolicy()
    assert policy.mode is PermissionMode.DEFAULT
    assert check(policy, "read").behavior is PolicyBehavior.ALLOW
    assert check(policy, "edit").behavior is PolicyBehavior.ASK
    assert check(policy, "bash").behavior is PolicyBehavior.ASK
    assert check(policy, "unknown_tool").behavior is PolicyBehavior.DENY


def test_mode_policy_accept_edits_auto_allows_file_mutations_only():
    policy = ModePolicy(PermissionMode.ACCEPT_EDITS)
    assert check(policy, "edit").behavior is PolicyBehavior.ALLOW
    assert check(policy, "write").behavior is PolicyBehavior.ALLOW
    # Shell stays gated in accept_edits: edits are reviewable, commands are not.
    assert check(policy, "bash").behavior is PolicyBehavior.ASK


def test_mode_policy_bypass_allows_everything():
    policy = ModePolicy(PermissionMode.BYPASS)
    for tool_name in ("read", "edit", "write", "bash", "totally_unknown"):
        assert check(policy, tool_name, {"x": 1}).behavior is PolicyBehavior.ALLOW


def test_mode_policy_set_mode_transitions_and_returns_previous():
    policy = ModePolicy()
    previous = policy.set_mode(PermissionMode.ACCEPT_EDITS)
    assert previous is PermissionMode.DEFAULT
    assert policy.mode is PermissionMode.ACCEPT_EDITS
    previous = policy.set_mode(PermissionMode.BYPASS)
    assert previous is PermissionMode.ACCEPT_EDITS
    # The decision follows the current mode after each transition.
    assert check(policy, "bash").behavior is PolicyBehavior.ALLOW


def test_parse_permission_mode_tolerates_spellings():
    assert parse_permission_mode("default") is PermissionMode.DEFAULT
    assert parse_permission_mode("accept-edits") is PermissionMode.ACCEPT_EDITS
    assert parse_permission_mode("ACCEPT_EDITS") is PermissionMode.ACCEPT_EDITS
    assert parse_permission_mode("bypass") is PermissionMode.BYPASS
    assert parse_permission_mode("yolo") is None
    assert parse_permission_mode("") is None
