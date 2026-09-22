"""Tests for minicode.tasks.background (BackgroundManager) — plain sync pytest
driving async with ``asyncio.run``; each scenario lives in one event loop so
watcher tasks survive for its whole lifetime."""

from __future__ import annotations

import asyncio
import os
import sys

from minicode.tasks.background import BackgroundJob, BackgroundManager, format_result

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


def _python_command(snippet: str) -> str:
    """Build a command running *snippet* with the current interpreter.

    Mirrors tests/test_tools_command.py: forward-slash interpreter path,
    quoted only when needed — valid under both PowerShell and bash.
    """
    exe = sys.executable.replace("\\", "/")
    if os.name == "nt":
        if " " in exe:
            return f'& "{exe}" -c "{snippet}"'
        return f'{exe} -c "{snippet}"'
    return f'"{exe}" -c "{snippet}"'


async def _wait_for_poll(manager: BackgroundManager, timeout_s: float = 15.0):
    """Poll until at least one job finished, or fail loudly."""
    loop = asyncio.get_running_loop()
    deadline = loop.time() + timeout_s
    while True:
        done = manager.poll_completed()
        if done:
            return done
        if loop.time() >= deadline:
            raise AssertionError("background job did not finish in time")
        await asyncio.sleep(0.05)


async def _wait_until_finished(
    manager: BackgroundManager, job_id: str, timeout_s: float = 15.0
) -> BackgroundJob:
    """Wait for a job's status to leave ``running`` without consuming the
    pending-notification set (uses ``get``, not ``poll_completed``)."""
    loop = asyncio.get_running_loop()
    deadline = loop.time() + timeout_s
    while True:
        job = manager.get(job_id)
        assert job is not None
        if job.status != "running":
            return job
        if loop.time() >= deadline:
            raise AssertionError(f"job {job_id} did not finish in time")
        await asyncio.sleep(0.05)


# ---------------------------------------------------------------------------
# Success / failure paths
# ---------------------------------------------------------------------------


def test_background_job_completes_with_output(tmp_path):
    received: list[BackgroundJob] = []

    async def on_complete(job: BackgroundJob) -> None:
        received.append(job)

    async def scenario() -> None:
        manager = BackgroundManager(on_complete=on_complete)
        job_id = await manager.start(_python_command("print('bg ok')"), tmp_path, 30.0)
        assert job_id.startswith("bg_")

        done = await _wait_for_poll(manager)
        assert [job.job_id for job in done] == [job_id]
        job = done[0]
        assert job.status == "completed"
        assert job.exit_code == 0
        assert "bg ok" in job.output
        assert job.started_at and job.finished_at

        # poll_completed is idempotent: each job is returned exactly once.
        assert manager.poll_completed() == []

        # The on_complete callback fired exactly once with the same job.
        assert len(received) == 1
        assert received[0].job_id == job_id

        # get() and jobs() expose the record; unknown ids return None.
        assert manager.get(job_id) is not None
        assert manager.get("bg_999") is None
        assert [j.job_id for j in manager.jobs()] == [job_id]

        await manager.cancel_all()  # nothing running; harmless teardown

    asyncio.run(scenario())


def test_background_job_nonzero_exit_is_failed(tmp_path):
    async def scenario() -> None:
        manager = BackgroundManager()
        job_id = await manager.start("exit 4", tmp_path, 30.0)
        (job,) = await _wait_for_poll(manager)
        assert job.status == "failed"
        assert job.exit_code == 4
        assert manager.poll_completed() == []

    asyncio.run(scenario())


def test_on_complete_exceptions_are_swallowed(tmp_path):
    async def bad_callback(job: BackgroundJob) -> None:
        raise RuntimeError("observer boom")

    async def scenario() -> None:
        manager = BackgroundManager(on_complete=bad_callback)
        job_id = await manager.start(_python_command("print('still ok')"), tmp_path, 30.0)
        (job,) = await _wait_for_poll(manager)
        assert job.status == "completed"
        assert "still ok" in job.output

    asyncio.run(scenario())


# ---------------------------------------------------------------------------
# Timeout path
# ---------------------------------------------------------------------------


def test_background_job_timeout_kills_process(tmp_path):
    async def scenario() -> None:
        manager = BackgroundManager()
        job_id = await manager.start(
            _python_command("import time; time.sleep(5)"), tmp_path, 1.0
        )
        (job,) = await _wait_for_poll(manager, timeout_s=10.0)
        assert job.status == "failed"
        assert job.exit_code is None  # killed, no natural exit code
        assert "timed out" in job.output

    asyncio.run(scenario())


# ---------------------------------------------------------------------------
# cancel_all
# ---------------------------------------------------------------------------


def test_cancel_all_marks_running_jobs_lost(tmp_path):
    received: list[BackgroundJob] = []

    async def on_complete(job: BackgroundJob) -> None:
        received.append(job)

    async def scenario() -> None:
        manager = BackgroundManager(on_complete=on_complete)
        job_id = await manager.start(
            _python_command("import time; time.sleep(30)"), tmp_path, 60.0
        )
        await asyncio.sleep(0.2)  # let the process spawn

        await manager.cancel_all()

        job = manager.get(job_id)
        assert job is not None
        assert job.status == "lost"
        assert job.finished_at is not None
        assert manager.poll_completed() == [job]
        assert manager.poll_completed() == []
        assert received == [job]

    asyncio.run(scenario())


def test_cancel_all_keeps_already_finished_jobs_collectable(tmp_path):
    async def scenario() -> None:
        manager = BackgroundManager()
        job_id = await manager.start(_python_command("print('done early')"), tmp_path, 30.0)
        job = await _wait_until_finished(manager, job_id)
        assert job.status == "completed"
        # Deliberately NOT polled before cancel_all.
        await manager.cancel_all()
        (again,) = manager.poll_completed()
        assert again.job_id == job_id

    asyncio.run(scenario())


# ---------------------------------------------------------------------------
# Snapshot behaviour and output truncation
# ---------------------------------------------------------------------------


def test_jobs_returns_snapshot_copies(tmp_path):
    async def scenario() -> None:
        manager = BackgroundManager()
        job_id = await manager.start(_python_command("print('snap')"), tmp_path, 30.0)
        await _wait_until_finished(manager, job_id)

        snapshot = manager.jobs()
        assert len(snapshot) == 1
        snapshot[0].status = "failed"  # mutate the copy
        assert manager.get(job_id) is not None
        assert manager.get(job_id).status == "completed"  # internal state untouched

    asyncio.run(scenario())


def test_output_truncated_to_max_output_chars(tmp_path):
    async def scenario() -> None:
        manager = BackgroundManager(max_output_chars=500)
        job_id = await manager.start(_python_command("print('y' * 5000)"), tmp_path, 30.0)
        (job,) = await _wait_for_poll(manager)
        assert len(job.output) <= 600
        assert "truncated" in job.output

    asyncio.run(scenario())


# ---------------------------------------------------------------------------
# format_result
# ---------------------------------------------------------------------------


def test_format_result_renders_job_for_the_model():
    job = BackgroundJob(
        job_id="bg_3",
        command="echo hello",
        cwd="/tmp",
        status="completed",
        exit_code=0,
        output="hello\n",
        started_at="2026-01-01T00:00:00+00:00",
        finished_at="2026-01-01T00:00:01+00:00",
    )
    text = format_result(job)
    assert "bg_3" in text
    assert "echo hello" in text
    assert "已完成" in text
    assert "hello" in text

    killed = job.model_copy(
        update={"status": "failed", "exit_code": None, "output": ""}
    )
    text = format_result(killed)
    assert "失败" in text
    assert "未知" in text  # unknown exit code for a killed process
    assert "（无输出）" in text
