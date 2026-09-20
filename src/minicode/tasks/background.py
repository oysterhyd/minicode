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
from pathlib import Path
from typing import Awaitable, Callable, Literal

from pydantic import BaseModel, Field

from minicode.core.clock import utc_now
from minicode.tools.base import truncate_output
from minicode.tools.command import kill_process_tree, spawn_shell

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
        max_output_chars: int = 10_000,
    ) -> None:
        self._on_complete = on_complete
        self._max_output_chars = max_output_chars
        self._jobs: dict[str, BackgroundJob] = {}
        self._tasks: dict[str, asyncio.Task[None]] = {}
        self._procs: dict[str, asyncio.subprocess.Process] = {}
        # Job ids that reached a terminal state but were not collected yet.
        self._pending: set[str] = set()
        self._counter = 0

    # -- starting -----------------------------------------------------------

    async def start(self, command: str, cwd: Path, timeout_s: float) -> str:
        """Start *command* in *cwd* and return its job id immediately.

        The command is wrapped for the platform shell exactly like the
        foreground ``run_command`` tool; the process is spawned detached and
        watched by a background task that enforces *timeout_s*.
        """
        self._counter += 1
        job = BackgroundJob(
            job_id=f"bg_{self._counter}", command=command, cwd=str(cwd)
        )
        self._jobs[job.job_id] = job

        try:
            proc = await spawn_shell(command, cwd)
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
        timeout_s: float,
    ) -> None:
        """Wait for the process, then record the terminal state and notify.

        Cancellation propagates untouched (see :meth:`cancel_all`): a
        cancelled watcher must not mark the job finished nor fire the
        callback.
        """
        try:
            stdout, _ = await asyncio.wait_for(proc.communicate(), timeout=timeout_s)
        except asyncio.TimeoutError:
            await kill_process_tree(proc)
            job.status = "failed"
            job.exit_code = None  # killed, no natural exit code
            job.output = f"command timed out after {timeout_s}s and was killed"
        else:
            output = stdout.decode("utf-8", errors="replace").replace("\r\n", "\n")
            job.exit_code = proc.returncode
            job.status = "completed" if proc.returncode == 0 else "failed"
            job.output = truncate_output(output, self._max_output_chars)
        finally:
            self._procs.pop(job.job_id, None)

        job.finished_at = utc_now()
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
        still-running jobs as ``lost`` (they will never report a result).

        Jobs that managed to finish before the cancellation keep their
        terminal state and stay collectable through :meth:`poll_completed`.
        """
        tasks = list(self._tasks.values())
        for task in tasks:
            task.cancel()
        self._tasks.clear()

        procs = list(self._procs.values())
        self._procs.clear()
        for proc in procs:
            await kill_process_tree(proc)
        if tasks:
            await asyncio.gather(*tasks, return_exceptions=True)

        now = utc_now()
        for job in self._jobs.values():
            if job.status == "running":
                job.status = "lost"
                job.finished_at = now
