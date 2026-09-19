"""Tests for minicode.security.policy."""

from __future__ import annotations

import asyncio

from minicode.security.policy import (
    AutoAllowPolicy,
    DefaultPolicy,
    PolicyBehavior,
    PolicyDecision,
)


def check(policy, tool_name, arguments=None):
    return asyncio.run(policy.check(tool_name, arguments or {}))


def test_default_policy_read_only_tools_allowed():
    policy = DefaultPolicy()
    for tool_name in ("read_file", "list_files", "search_text"):
        decision = check(policy, tool_name)
        assert isinstance(decision, PolicyDecision)
        assert decision.behavior is PolicyBehavior.ALLOW


def test_default_policy_mutating_tools_ask():
    policy = DefaultPolicy()
    for tool_name in ("apply_patch", "run_command"):
        decision = check(policy, tool_name)
        assert decision.behavior is PolicyBehavior.ASK


def test_default_policy_unknown_tool_denied():
    policy = DefaultPolicy()
    decision = check(policy, "mystery_tool")
    assert decision.behavior is PolicyBehavior.DENY


def test_default_policy_rules_override_builtin():
    policy = DefaultPolicy(rules={"apply_patch": PolicyBehavior.DENY})
    assert check(policy, "apply_patch").behavior is PolicyBehavior.DENY
    # other built-ins keep their defaults
    assert check(policy, "read_file").behavior is PolicyBehavior.ALLOW
    assert check(policy, "run_command").behavior is PolicyBehavior.ASK


def test_default_policy_rules_add_new_tool():
    policy = DefaultPolicy(rules={"web_fetch": PolicyBehavior.ASK})
    assert check(policy, "web_fetch").behavior is PolicyBehavior.ASK
    # unknown tools without a rule still hit the default
    assert check(policy, "other_tool").behavior is PolicyBehavior.DENY


def test_default_policy_custom_default():
    policy = DefaultPolicy(default=PolicyBehavior.ALLOW)
    assert check(policy, "anything").behavior is PolicyBehavior.ALLOW
    # built-ins are unchanged by a custom default
    assert check(policy, "apply_patch").behavior is PolicyBehavior.ASK


def test_auto_allow_policy_allows_everything():
    policy = AutoAllowPolicy()
    for tool_name in ("read_file", "apply_patch", "run_command", "totally_unknown"):
        decision = check(policy, tool_name, {"path": "x"})
        assert decision.behavior is PolicyBehavior.ALLOW
