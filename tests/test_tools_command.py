"""Tests for minicode.tools.command (run_command)."""

from __future__ import annotations

import asyncio
import locale
import os
import sys

import pytest

from minicode.tools.base import ToolContext, ToolLimits
from minicode.tools.command import (
    BashTool,
    _translate_posix_redirects,
    decode_shell_output,
    shell_command,
)
from minicode.storage import ArtifactStore, SqliteStore
from minicode.tools.artifacts import ReadArtifactTool


def _python_command(snippet: str) -> str:
    """Build a command running *snippet* with the current interpreter.

    Works on both PowerShell and bash: the interpreter path uses forward
    slashes (accepted by both) and is only quoted when it must be.
    """
    exe = sys.executable.replace("\\", "/")
    if os.name == "nt":
        if " " in exe:
            return f'& "{exe}" -c "{snippet}"'
        return f'{exe} -c "{snippet}"'
    return f'"{exe}" -c "{snippet}"'


def _print_cwd_command() -> str:
    return "(Get-Location).Path" if os.name == "nt" else "pwd"


def run(raw_args, tmp_path, **limit_overrides):
    limits = ToolLimits(**limit_overrides) if limit_overrides else ToolLimits()
    ctx = ToolContext(workspace=tmp_path, limits=limits)
    return asyncio.run(BashTool().run(raw_args, ctx))


def test_run_command_success(tmp_path):
    outcome = run({"command": "echo minicode-ok"}, tmp_path)
    assert outcome.success is True
    assert outcome.exit_code == 0
    assert outcome.error is None
    assert "minicode-ok" in outcome.output


def test_run_command_nonzero_exit(tmp_path):
    # NOTE: PowerShell's -Command collapses a failing native child (e.g.
    # python sys.exit(3)) to exit code 1, but `exit 3` propagates exactly on
    # pwsh, powershell and bash alike, so it gives a deterministic code 3.
    outcome = run({"command": "exit 3"}, tmp_path)
    assert outcome.success is False
    assert outcome.exit_code == 3
    assert outcome.error is not None and "code 3" in outcome.error


def test_run_command_python_child_runs(tmp_path):
    outcome = run({"command": _python_command("print('child-ok')")}, tmp_path)
    assert outcome.success is True
    assert "child-ok" in outcome.output


def test_run_command_timeout_kills_process(tmp_path):
    outcome = run(
        {"command": _python_command("import time; time.sleep(5)"), "timeout_s": 1},
        tmp_path,
    )
    assert outcome.success is False
    assert outcome.error is not None and "timed out" in outcome.error
    assert "1.0s" in (outcome.error or "")


def test_run_command_output_truncated(tmp_path):
    outcome = run(
        {"command": _python_command("print('x' * 50000)")},
        tmp_path,
        max_command_output_chars=2000,
    )
    assert outcome.success is True
    assert len(outcome.output) <= 2200
    assert "truncated" in outcome.output
    assert outcome.full_output is not None
    assert len(outcome.full_output) > 50_000


def test_large_command_output_spills_to_pageable_artifact(tmp_path):
    store = SqliteStore(tmp_path / "logs.sqlite3")
    session_id = store.create_session(workspace=str(tmp_path), provider="fake", model="fake")
    artifacts = ArtifactStore(store)
    ctx = ToolContext(
        workspace=tmp_path, artifact_store=artifacts, session_id=session_id,
        limits=ToolLimits(max_command_output_chars=1000),
    )
    try:
        outcome = asyncio.run(BashTool().run(
            {"command": _python_command("print('x' * 2100000)")}, ctx
        ))
        assert outcome.success
        assert "[artifact:" in outcome.output
        artifact_id = store.list_artifacts(session_id)[0]["artifact_id"]
        page = asyncio.run(ReadArtifactTool().run({
            "artifact_id": artifact_id, "offset": 2_000_000, "limit": 1000,
        }, ctx))
        assert page.success
        assert "x" * 100 in page.output
        assert "next_offset=" in page.output
    finally:
        store.close()


def test_command_output_quota_returns_recoverable_failure(tmp_path):
    outcome = run(
        {"command": _python_command("print('x' * 50000)")},
        tmp_path, max_command_capture_bytes=1000,
    )
    assert not outcome.success
    assert "capture quota" in (outcome.error or "")
    assert outcome.output


def test_run_command_cwd(tmp_path):
    sub = tmp_path / "sub"
    sub.mkdir()
    outcome = run({"command": _print_cwd_command(), "cwd": "sub"}, tmp_path)
    assert outcome.success is True
    got = outcome.output.strip().replace("\\", "/").lower()
    want = str(sub.resolve()).replace("\\", "/").lower()
    assert want in got


def test_run_command_cwd_outside_workspace(tmp_path):
    outcome = run({"command": "echo hi", "cwd": ".."}, tmp_path)
    assert outcome.success is False
    assert "outside workspace" in (outcome.error or "")


def test_run_command_timeout_clamped_to_limit(tmp_path):
    # timeout_s above the cap is clamped; a fast command still succeeds and
    # the clamping logic itself is exercised via a below-minimum value too.
    fast = run({"command": "echo ok", "timeout_s": 9999.0}, tmp_path)
    assert fast.success is True
    tiny = run({"command": "echo ok", "timeout_s": 0.001}, tmp_path)
    assert tiny.success is True  # clamped up to 1s minimum, echo finishes fast


# ---------------------------------------------------------------------------
# Output decoding: encoding, POSIX idioms and ANSI noise
#
# Regression tests for session 39e5f16a, where PowerShell's code-page output
# was decoded as UTF-8: every Chinese character of `Get-Content README.md`
# reached the model as mojibake, and `2>/dev/null` was taken as a redirect to
# a file named D:\dev\null.
# ---------------------------------------------------------------------------


def test_decode_shell_output_uses_the_system_code_page():
    """Bytes in the shell's own code page survive decoding on any platform."""
    text = "演示会**实际修改**工作区内的文件"
    raw = text.encode(locale.getpreferredencoding(False))
    assert decode_shell_output(raw) == text


def test_decode_shell_output_prefers_utf8_then_falls_back():
    assert decode_shell_output("中文 ok".encode("utf-8")) == "中文 ok"
    # Undecodable bytes degrade to replacement characters, never to an error.
    assert "\ufffd" in decode_shell_output(b"\xff\xfe\x00bad")


def test_decode_shell_output_strips_sgr_colour():
    raw = b"\x1b[32;1mLines\x1b[0m \x1b[31mFile\x1b[0m\n"
    assert decode_shell_output(raw) == "Lines File\n"


def test_decode_shell_output_normalises_crlf():
    assert decode_shell_output(b"a\r\nb\r\n") == "a\nb\n"


def test_posix_null_redirect_is_translated_for_powershell():
    assert _translate_posix_redirects("git status 2>/dev/null") == "git status 2>$null"
    assert _translate_posix_redirects("ls >/dev/null") == "ls >$null"
    # Only the redirect target is rewritten; other text is left alone.
    assert _translate_posix_redirects("echo /dev/nullx") == "echo /dev/nullx"


@pytest.mark.skipif(os.name != "nt", reason="PowerShell-specific command wrapping")
def test_windows_shell_command_requests_utf8_and_translates_dev_null():
    argv = shell_command("Get-ChildItem 2>/dev/null")
    assert "$null" in argv[-1]
    # Both output encodings are pinned before the user's command runs.
    assert "[Console]::OutputEncoding=[System.Text.Encoding]::UTF8" in argv[-1]
    assert "$OutputEncoding=[System.Text.Encoding]::UTF8" in argv[-1]
    assert argv[-1].endswith("Get-ChildItem 2>$null")


def test_posix_shell_command_keeps_command_verbatim():
    if os.name == "nt":  # the POSIX branch is only reachable on POSIX
        return
    assert shell_command("git status 2>/dev/null")[-1] == "git status 2>/dev/null"


@pytest.mark.skipif(os.name != "nt", reason="PowerShell console code page")
def test_run_command_non_ascii_output_is_readable(tmp_path):
    (tmp_path / "cn.md").write_text("演示会：中文内容\n", encoding="utf-8")
    outcome = run({"command": "Get-Content cn.md"}, tmp_path)
    assert outcome.success is True
    assert "演示会" in outcome.output


@pytest.mark.skipif(os.name != "nt", reason="POSIX redirect under PowerShell")
def test_run_command_posix_null_redirect_succeeds(tmp_path):
    outcome = run({"command": "Get-ChildItem . 2>/dev/null"}, tmp_path)
    assert outcome.success is True
    assert outcome.exit_code == 0
