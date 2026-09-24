"""Background command jobs: fire-and-forget shell runs with result collection.

:class:`BackgroundManager` reuses the shared shell helpers of
:mod:`minicode.tools.command` (shell wrapping, merged stdout/stderr,
tree-safe kill) and runs each command as a detached asyncio task inside the
caller's event loop. Jobs are tracked in memory; finished jobs are delivered
exactly once via :meth:`BackgroundManager.poll_completed` and optionally
pushed to an ``on_complete`` callback.

Protocol note: :class:`~minicode.tools.base.BackgroundManagerLike` declares
``start`` with a plain (synchronous) signature as a duck-typing reference
only. The real manager must be awaited (``await manager.start(...)``), which
is what :meth:`BackgroundManager.start` implements — runtime checks are
structural, so call sites simply await it.
"""

from __future__ import annotations

import asyncio
import uuid
from pathlib import Path
from typing import Awaitable, Callable, Literal

from pydantic import BaseModel, Field

from minicode.core.clock import utc_now
from minicode.tools.base import tail_output
from minicode.tools.command import capture_bounded, decode_shell_output, kill_process_tree, spawn_shell

#: Called when a job reaches a terminal state (completed / failed).
CompleteCallback = Callable[["BackgroundJob"], Awaitable[None]]

JobStatus = Literal["running", "completed", "failed", "lost"]


class BackgroundJob(BaseModel):
    """State of one background command tracked by :class:`BackgroundManager`."""

    job_id: str
    command: str
    cwd: str
    status: JobStatus = "running"
    exit_code: int | None = None
    output: str = ""
    full_output: str | None = None
    started_at: str = Field(default_factory=utc_now)
    finished_at: str | None = None


def format_result(job: BackgroundJob) -> str:
    """Render *job* as the Chinese result text shown to the model."""
    status_labels = {
        "running": "运行中",
        "completed": "已完成",
        "failed": "失败",
        "lost": "结果丢失",
    }
    exit_code = "未知（进程被终止）" if job.exit_code is None else str(job.exit_code)
    body = job.output if job.output else "（无输出）"
    return "\n".join(
        [
            f"job_id: {job.job_id}",
            f"命令: {job.command}",
            f"工作目录: {job.cwd}",
            f"状态: {status_labels.get(job.status, job.status)}",
            f"退出码: {exit_code}",
            f"输出:\n{body}",
        ]
    )


class BackgroundManager:
    """In-memory tracker for background shell commands.

    Every job runs in its own asyncio task (created by :meth:`start`); the
    watcher merges stdout+stderr, enforces the timeout by killing the whole
    process tree, and records the terminal state on the job. Observer errors
    from ``on_complete`` are swallowed: job bookkeeping must never break
    because a listener misbehaved.
    """

    def __init__(
        self,
        on_complete: CompleteCallback | None = None,
        max_output_chars: int = 50 * 1024,
    ) -> None:
        self._on_complete = on_complete
        self._max_output_chars = max_output_chars
        self._jobs: dict[str, BackgroundJob] = {}
        self._tasks: dict[str, asyncio.Task[None]] = {}
        self._procs: dict[str, asyncio.subprocess.Process] = {}
        # Job ids that reached a terminal state but were not collected yet.
        self._pending: set[str] = set()

    # -- starting -----------------------------------------------------------

    async def start(self, command: str, cwd: Path, timeout_s: float | None) -> str:
        """Start *command* in *cwd* and return its job id immediately.

        The command is wrapped for the platform shell exactly like the
        foreground ``bash`` tool; the process is spawned detached and
        watched by a background task that enforces *timeout_s*.
        """
        job = BackgroundJob(
            job_id=f"bg_{uuid.uuid4().hex}", command=command, cwd=str(cwd)
        )
        self._jobs[job.job_id] = job

        try:
            proc = await spawn_shell(command, cwd)
        except asyncio.CancelledError:
            await self._mark_lost(job)
            raise
        except (OSError, ValueError) as exc:
            job.status = "failed"
            job.output = f"failed to start command: {exc}"
            job.finished_at = utc_now()
            self._pending.add(job.job_id)
            await self._notify(job)
            return job.job_id

        self._procs[job.job_id] = proc
        task = asyncio.create_task(
            self._watch(job, proc, timeout_s), name=f"bg-watch-{job.job_id}"
        )
        self._tasks[job.job_id] = task
        task.add_done_callback(
            lambda _task, job_id=job.job_id: self._tasks.pop(job_id, None)
        )
        return job.job_id

    async def _watch(
        self,
        job: BackgroundJob,
        proc: asyncio.subprocess.Process,
        timeout_s: float | None,
    ) -> None:
        """Wait for the process, then record the terminal state and notify.

        Cancellation kills the process tree and records a collectable lost
        result before propagating to the caller.
        """
        try:
            stdout, timed_out, over_limit, pipe_lingered = await capture_bounded(proc, timeout_s)
        except asyncio.CancelledError:
            await kill_process_tree(proc)
            await self._mark_lost(job)
            raise
        except Exception as exc:  # noqa: BLE001 - watcher must reach a terminal state
            job.status = "failed"
            job.exit_code = None
            job.output = f"background command capture failed: {exc}"
        else:
            output = decode_shell_output(stdout)
            job.exit_code = None if timed_out or over_limit or pipe_lingered else proc.returncode
            job.status = "completed" if proc.returncode == 0 and not timed_out and not over_limit and not pipe_lingered else "failed"
            job.output = tail_output(output, self._max_output_chars - 200)
            if job.output != output:
                job.full_output = output
                job.output = "...[output truncated; 完整输出见归档]\n" + job.output
            if timed_out:
                job.output += f"\ncommand timed out after {timeout_s}s and was killed"
            if over_limit:
                job.output += "\ncommand output exceeded capture quota and was killed"
            if pipe_lingered:
                job.output += "\nshell exited but an inherited output pipe stayed open; output capture was stopped"
        finally:
            self._procs.pop(job.job_id, None)

        job.finished_at = utc_now()
        self._pending.add(job.job_id)
        await self._notify(job)

    async def _mark_lost(self, job: BackgroundJob) -> None:
        if job.status != "running":
            return
        job.status = "lost"
        job.finished_at = utc_now()
        job.output = "后台任务随回合结束或取消而终止，未取得完整结果。"
        self._pending.add(job.job_id)
        await self._notify(job)

    async def _notify(self, job: BackgroundJob) -> None:
        if self._on_complete is None:
            return
        try:
            await self._on_complete(job)
        except Exception:  # noqa: BLE001 - observer bugs must not propagate
            pass

    # -- inspection ----------------------------------------------------------

    def get(self, job_id: str) -> BackgroundJob | None:
        """Return the live job record for *job_id* (None when unknown)."""
        return self._jobs.get(job_id)

    def jobs(self) -> list[BackgroundJob]:
        """Snapshot of every tracked job (deep copies, safe to mutate)."""
        return [job.model_copy(deep=True) for job in self._jobs.values()]

    def poll_completed(self) -> list[BackgroundJob]:
        """Return finished jobs not collected before; each job exactly once.

        A job enters the pending-notification set when it reaches a terminal
        state and leaves it here, so repeated polls stay idempotent. The job
        records themselves remain available through :meth:`get` / :meth:`jobs`.
        """
        collected = [
            job for job in self._jobs.values() if job.job_id in self._pending
        ]
        for job in collected:
            self._pending.discard(job.job_id)
        return collected

    # -- teardown ------------------------------------------------------------

    async def cancel_all(self) -> None:
        """Cancel every watcher task, kill every live process tree and mark
        still-running jobs as ``lost`` with a collectable terminal result.

        Jobs that managed to finish before the cancellation keep their
        terminal state and stay collectable through :meth:`poll_completed`.
        """
        tasks = list(self._tasks.values())
        procs = list(self._procs.values())
        for task in tasks:
            task.cancel()
        self._tasks.clear()

        if tasks:
            await asyncio.gather(*tasks, return_exceptions=True)
        # Also covers watchers cancelled before their coroutine first ran.
        for proc in procs:
            if proc.returncode is None:
                await kill_process_tree(proc)
        self._procs.clear()
        for job in self._jobs.values():
            await self._mark_lost(job)
