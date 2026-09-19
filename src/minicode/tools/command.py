"""Shell command execution tool with timeouts and tree-safe killing."""

from __future__ import annotations

import asyncio
import contextlib
import os
import shutil
import signal
import subprocess
from typing import Any

from pydantic import BaseModel

from minicode.core.models import ToolOutcome
from minicode.core.paths import PathOutsideWorkspaceError, resolve_in_workspace
from minicode.tools.base import BaseTool, ToolContext, truncate_output


class RunCommandArgs(BaseModel):
    command: str
    timeout_s: float | None = None
    cwd: str = "."


class RunCommandTool(BaseTool):
    name = "run_command"
    description = (
        "Run a shell command inside the workspace (PowerShell on Windows, "
        "bash elsewhere) and return its combined output and exit code. "
        "Requires approval."
    )
    requires_approval = True
    args_model = RunCommandArgs

    async def execute(self, args: RunCommandArgs, ctx: ToolContext) -> ToolOutcome:
        try:
            cwd = resolve_in_workspace(ctx.workspace, args.cwd)
        except PathOutsideWorkspaceError as exc:
            return ToolOutcome.failure(f"path outside workspace: {exc}")
        except OSError as exc:
            return ToolOutcome.failure(f"invalid path: {exc}")
        if not cwd.is_dir():
            return ToolOutcome.failure(f"path is not a directory: {args.cwd}")

        timeout_s = (
            ctx.limits.default_command_timeout_s
            if args.timeout_s is None
            else args.timeout_s
        )
        timeout_s = max(1.0, min(timeout_s, ctx.limits.max_command_timeout_s))

        shell_cmd = self._shell_command(args.command)
        kwargs: dict[str, Any] = {
            "cwd": str(cwd),
            "stdout": asyncio.subprocess.PIPE,
            "stderr": asyncio.subprocess.STDOUT,
        }
        if os.name != "nt":
            kwargs["start_new_session"] = True
        try:
            proc = await asyncio.create_subprocess_exec(*shell_cmd, **kwargs)
        except (OSError, ValueError) as exc:
            return ToolOutcome.failure(f"failed to start command: {exc}")

        try:
            stdout, _ = await asyncio.wait_for(proc.communicate(), timeout=timeout_s)
        except asyncio.TimeoutError:
            await self._kill_process_tree(proc)
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

    @staticmethod
    def _shell_command(command: str) -> list[str]:
        """Wrap *command* for the platform shell, passed as a fixed argv."""
        if os.name == "nt":
            shell = "pwsh" if shutil.which("pwsh") else "powershell"
            return [shell, "-NoProfile", "-NonInteractive", "-Command", command]
        return ["bash", "-c", command]

    @staticmethod
    async def _kill_process_tree(proc: asyncio.subprocess.Process) -> None:
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
