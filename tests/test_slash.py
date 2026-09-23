"""Tests for the shared slash-command registry and prefix filtering."""

from __future__ import annotations

import re

from minicode.slash import (
    SLASH_COMMANDS,
    SUBMENU_COMMANDS,
    SlashCommand,
    filter_commands,
)


def test_registry_has_expected_core_commands():
    names = {cmd.name for cmd in SLASH_COMMANDS}
    assert {"/help", "/model", "/effort", "/permissions", "/clear", "/new"} <= names


def test_every_command_has_usage_and_summary():
    for cmd in SLASH_COMMANDS:
        assert cmd.name.startswith("/")
        assert cmd.usage.startswith(cmd.name)
        assert cmd.summary


def test_filter_empty_prefix_returns_everything():
    assert len(filter_commands("/")) == len(SLASH_COMMANDS)
    assert len(filter_commands("")) == len(SLASH_COMMANDS)


def test_filter_by_prefix():
    assert [cmd.name for cmd in filter_commands("/h")] == ["/help"]
    assert [cmd.name for cmd in filter_commands("/m")] == ["/model"]
    assert {cmd.name for cmd in filter_commands("/s")} == {"/sessions", "/skill"}
    assert filter_commands("/zzz") == []


def test_filter_is_case_insensitive():
    assert [cmd.name for cmd in filter_commands("/H")] == ["/help"]


def test_slash_command_is_frozen_dataclass():
    cmd = SlashCommand("/x", "/x", "summary")
    try:
        cmd.name = "/y"  # type: ignore[misc]
    except AttributeError:
        pass
    else:  # pragma: no cover
        raise AssertionError("SlashCommand must be frozen")


# ---------------------------------------------------------------------------
# Submenu enrolment invariant
#
# Regression test for the cascading-menu gap: /model and /effort drilled into a
# second level while /permissions (same kind of enumerated argument) executed
# blindly. Any command whose usage enumerates its values must be enrolled in
# SUBMENU_COMMANDS, so the cascade cannot be forgotten again.
# ---------------------------------------------------------------------------


_ENUMERATED_USAGE = re.compile(r"\[[a-z_]+(?:\|[a-z_]+)+\]")


def test_enumerated_argument_commands_have_a_submenu():
    missing = [
        cmd.name
        for cmd in SLASH_COMMANDS
        if _ENUMERATED_USAGE.search(cmd.usage) and cmd.name not in SUBMENU_COMMANDS
    ]
    assert missing == [], f"命令有枚举取值但未接入二级菜单: {missing}"


def test_submenu_commands_are_registered_and_single_level():
    names = {cmd.name for cmd in SLASH_COMMANDS}
    assert set(SUBMENU_COMMANDS) <= names
    for name, title in SUBMENU_COMMANDS.items():
        assert title, name
        # One level only: the cascade must not point at a command that is
        # itself a submenu entry.
        assert not name.endswith("/")
