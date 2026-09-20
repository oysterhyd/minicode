"""Tests for the shared slash-command registry and prefix filtering."""

from __future__ import annotations

from minicode.slash import SLASH_COMMANDS, SlashCommand, filter_commands


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
    assert {cmd.name for cmd in filter_commands("/s")} == {"/sessions"}
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
