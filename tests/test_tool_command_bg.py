"""Tests for run_command's ``background`` parameter — the tool must hand work
to the configured background manager instead of running the command itself."""

from __future__ import annotations

import asyncio
from pathlib import Path

from minicode.tools.base import ToolContext
from minicode.tools.command import BashTool


class RecordingManager:
    """Fake BackgroundManagerLike that records start() calls."""

    def __init__(self) -> None:
        self.calls: list[tuple[str, Path, float]] = []

    async def start(self, command: str, cwd: Path, timeout_s: float) -> str:
        self.calls.append((command, cwd, timeout_s))
        return f"bg_{len(self.calls)}"


def run(raw_args, tmp_path, manager=None):
    ctx = ToolContext(workspace=tmp_path, background_manager=manager)
    return asyncio.run(BashTool().run(raw_args, ctx))


# ---------------------------------------------------------------------------
# background=True with a manager
# ---------------------------------------------------------------------------


def test_background_start_returns_job_id(tmp_path):
    manager = RecordingManager()
    outcome = run(
        {"command": "echo hi", "background": True}, tmp_path, manager=manager
    )

    assert outcome.success is True
    assert "bg_1" in outcome.output
    assert "echo hi" in outcome.output
    assert "job_id" in outcome.output
    # The command was handed to the manager, never run in the foreground.
    assert manager.calls == [("echo hi", tmp_path.resolve(), 60.0)]


def test_background_passes_resolved_cwd_and_timeout(tmp_path):
    sub = tmp_path / "sub"
    sub.mkdir()
    manager = RecordingManager()
    outcome = run(
        {"command": "echo hi", "background": True, "cwd": "sub", "timeout_s": 5},
        tmp_path,
        manager=manager,
    )

    assert outcome.success is True
    assert manager.calls[0][1] == sub.resolve()
    assert manager.calls[0][2] == 5.0


def test_background_start_failure_job_ids_increment(tmp_path):
    manager = RecordingManager()
    first = run({"command": "a", "background": True}, tmp_path, manager=manager)
    second = run({"command": "b", "background": True}, tmp_path, manager=manager)
    assert "bg_1" in first.output and "bg_2" in second.output


# ---------------------------------------------------------------------------
# background=True without a manager
# ---------------------------------------------------------------------------


def test_background_without_manager_is_a_failure(tmp_path):
    outcome = run({"command": "echo hi", "background": True}, tmp_path)

    assert outcome.success is False
    assert "no background manager" in (outcome.error or "")
    assert "job_id" not in outcome.output


def test_background_cwd_still_validated_without_manager(tmp_path):
    outcome = run({"command": "echo hi", "background": True, "cwd": ".."}, tmp_path)

    assert outcome.success is False
    assert "outside workspace" in (outcome.error or "")


# ---------------------------------------------------------------------------
# background=False: foreground behaviour unchanged
# ---------------------------------------------------------------------------


def test_foreground_run_ignores_manager(tmp_path):
    manager = RecordingManager()
    outcome = run({"command": "echo plain"}, tmp_path, manager=manager)

    assert outcome.success is True
    assert "plain" in outcome.output
    assert manager.calls == []  # manager never consulted


def test_default_is_foreground(tmp_path):
    outcome = run({"command": "echo default"}, tmp_path)
    assert outcome.success is True
    assert "default" in outcome.output
