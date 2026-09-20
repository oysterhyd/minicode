"""Shell command execution tool with timeouts and tree-safe killing.

The platform-shell helpers (``shell_command`` / ``spawn_shell`` /
``kill_process_tree``) live at module level because they are also reused by
``minicode.tasks.background`` and ``minicode.goals.checker``.
"""

from __future__ import annotations

import asyncio
import contextlib
import os
import shutil
import signal
import subprocess
from pathlib import Path
from typing import Any

from pydantic import BaseModel

from minicode.core.models import ToolOutcome
from minicode.tools.base import BaseTool, ToolContext, resolve_or_fail, truncate_output


def shell_command(command: str) -> list[str]:
    """Wrap *command* for the platform shell, passed as a fixed argv."""
    if os.name == "nt":
        shell = "pwsh" if shutil.which("pwsh") else "powershell"
        return [shell, "-NoProfile", "-NonInteractive", "-Command", command]
    return ["bash", "-c", command]


async def spawn_shell(command: str, cwd: str | Path) -> asyncio.subprocess.Process:
    """Spawn *command* (wrapped for the platform shell) in *cwd*, with stdout
    and stderr merged into one pipe. Off Windows the child becomes a session
    leader so :func:`kill_process_tree` can take down its whole tree.

    Raises OSError/ValueError when the process cannot be started.
    """
    kwargs: dict[str, Any] = {
        "cwd": str(cwd),
        "stdout": asyncio.subprocess.PIPE,
        "stderr": asyncio.subprocess.STDOUT,
    }
    if os.name != "nt":
        kwargs["start_new_session"] = True
    return await asyncio.create_subprocess_exec(*shell_command(command), **kwargs)


async def kill_process_tree(proc: asyncio.subprocess.Process) -> None:
    """Best-effort kill of *proc* and everything it spawned."""
    if os.name == "nt":
        try:
            subprocess.run(
                ["taskkill", "/PID", str(proc.pid), "/T", "/F"],
                capture_output=True,
                timeout=15,
            )
        except (OSError, subprocess.SubprocessError):
            with contextlib.suppress(ProcessLookupError):
                proc.kill()
    else:
        try:
            # start_new_session makes the child a session leader, so its
            # process group id equals its pid.
            os.killpg(proc.pid, signal.SIGKILL)
        except OSError:
            with contextlib.suppress(ProcessLookupError):
                proc.kill()
    with contextlib.suppress(Exception):
        await proc.wait()


class BashArgs(BaseModel):
    command: str
    timeout_s: float | None = None
    cwd: str = "."
    background: bool = False


class BashTool(BaseTool):
    name = "bash"
    description = (
        "Run a shell command inside the workspace (PowerShell on Windows, "
        "bash elsewhere) and return its combined output and exit code. "
        "Set background=true to start the command as a background job "
        "instead: it returns a job_id immediately and the result is "
        "delivered by the system once the job completes. Requires approval."
    )
    requires_approval = True
    args_model = BashArgs

    async def execute(self, args: RunCommandArgs, ctx: ToolContext) -> ToolOutcome:
        cwd, failure = resolve_or_fail(ctx, args.cwd)
        if failure is not None:
            return failure
        if not cwd.is_dir():
            return ToolOutcome.failure(f"path is not a directory: {args.cwd}")

        timeout_s = (
            ctx.limits.default_command_timeout_s
            if args.timeout_s is None
            else args.timeout_s
        )
        timeout_s = max(1.0, min(timeout_s, ctx.limits.max_command_timeout_s))

        if args.background:
            manager = ctx.background_manager
            if manager is None:
                return ToolOutcome.failure(
                    "background execution requested but no background manager is configured"
                )
            job_id = await manager.start(args.command, cwd, timeout_s)
            return ToolOutcome(
                success=True,
                output=(
                    f"已在后台启动命令。\njob_id: {job_id}\n命令: {args.command}\n"
                    "结果将在后台完成后由系统通知。"
                ),
            )

        try:
            proc = await spawn_shell(args.command, cwd)
        except (OSError, ValueError) as exc:
            return ToolOutcome.failure(f"failed to start command: {exc}")

        try:
            stdout, _ = await asyncio.wait_for(proc.communicate(), timeout=timeout_s)
        except asyncio.TimeoutError:
            await kill_process_tree(proc)
            return ToolOutcome.failure(f"command timed out after {timeout_s}s and was killed")

        output = stdout.decode("utf-8", errors="replace").replace("\r\n", "\n")
        output = truncate_output(output, ctx.limits.max_command_output_chars)
        if proc.returncode == 0:
            return ToolOutcome(success=True, output=output, exit_code=0)
        return ToolOutcome(
            success=False,
            output=output,
            error=f"command exited with code {proc.returncode}",
            exit_code=proc.returncode,
        )
