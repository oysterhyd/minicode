"""Tests for minicode.tools.command (run_command)."""

from __future__ import annotations

import asyncio
import os
import sys

from minicode.tools.base import ToolContext, ToolLimits
from minicode.tools.command import BashTool


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
